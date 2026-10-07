//! `amont-agent rules --json`: the interface the homebrew tap reads.
//!
//! `scripts/bump-tap.py` writes the formula's list of rules that refuse by
//! default from this output, and the formula's `brew test` checks the list
//! against it, so its keys are pinned here. Every run gets its own git
//! config files, so nothing on the machine running the tests reaches in.

use std::path::PathBuf;
use std::process::{Command, Output};

struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let dir = std::env::temp_dir().join(format!(
            "amont-agent-rules-json-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("global"), "").unwrap();
        Home(dir)
    }

    fn rules(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .arg("rules")
            .args(args)
            .current_dir(&self.0)
            .env("CLAUDE_CONFIG_DIR", self.0.join("claude"))
            .env("GIT_CONFIG_GLOBAL", self.0.join("global"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("AMONT_AGENT_OFF")
            .output()
            .expect("the binary runs")
    }

    fn json(&self) -> Vec<serde_json::Value> {
        let out = self.rules(&["--json"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.ends_with("]\n"), "one array and a final newline");
        serde_json::from_str(&text).expect("rules --json is JSON")
    }

    fn set(&self, key: &str, value: &str) {
        let out = Command::new("git")
            .args(["config", "--file"])
            .arg(self.0.join("global"))
            .args([key, value])
            .output()
            .unwrap();
        assert!(out.status.success());
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn every_rule_and_assertion_with_exactly_the_pinned_keys() {
    let h = Home::new("keys");
    let entries = h.json();
    let text = String::from_utf8(h.rules(&[]).stdout).unwrap();
    let ids_in_text: Vec<&str> = text
        .lines()
        .map(|l| l.split_whitespace().next().unwrap())
        .collect();
    let ids_in_json: Vec<&str> = entries.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids_in_json, ids_in_text,
        "same entries, same order as the text"
    );

    let keys = [
        "default_stance",
        "id",
        "kind",
        "max_stance",
        "measured",
        "per_1000",
        "stance",
    ];
    for e in &entries {
        let mut got: Vec<&str> = e.as_object().unwrap().keys().map(String::as_str).collect();
        got.sort_unstable();
        assert_eq!(got, keys, "{e}");
        assert!(
            matches!(e["kind"].as_str(), Some("rule" | "assertion")),
            "{e}"
        );
        assert!(e["per_1000"].is_number(), "{e}");
    }
    assert!(entries.iter().any(|e| e["kind"] == "assertion"));
}

#[test]
fn the_rules_that_refuse_by_default() {
    let entries = Home::new("deny").json();
    let mut deny: Vec<&str> = entries
        .iter()
        .filter(|e| e["kind"] == "rule" && e["default_stance"] == "deny")
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    deny.sort_unstable();
    assert_eq!(
        deny,
        ["pipe-to-tail", "plan-phases-open", "plan-review-panel"]
    );
}

#[test]
fn the_stance_in_force_is_this_machines_and_the_default_is_what_ships() {
    let h = Home::new("stance");
    // A rule that does not ship at deny, taken from the output rather than
    // named here, because shipped stances move.
    let entries = h.json();
    let rule = entries
        .iter()
        .find(|e| e["kind"] == "rule" && e["default_stance"] != "deny" && e["max_stance"] == "deny")
        .expect("a rule that ships below deny and can be raised");
    let id = rule["id"].as_str().unwrap().to_string();
    let ships = rule["default_stance"].as_str().unwrap().to_string();
    assert_eq!(
        rule["stance"],
        ships.as_str(),
        "an empty config changes nothing"
    );

    h.set(&format!("amont.agent.{id}.stance"), "deny");
    let raised = h.json();
    let rule = raised.iter().find(|e| e["id"] == id.as_str()).unwrap();
    assert_eq!(rule["stance"], "deny");
    assert_eq!(rule["default_stance"], ships.as_str());
}

#[test]
fn rules_takes_only_json() {
    let h = Home::new("args");
    for args in [
        &["--bogus"][..],
        &["--force"][..],
        &["foo"][..],
        &["--json", "--json"][..],
    ] {
        let out = h.rules(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let err = String::from_utf8(out.stderr).unwrap();
        assert_eq!(err.lines().count(), 1, "{args:?}: {err}");
        assert!(
            err.ends_with("`rules` takes only --json\n"),
            "{args:?}: {err}"
        );
    }
    for help in [
        &["-h"][..],
        &["--help"][..],
        &["--bogus", "--help"][..],
        &["--json", "-h"][..],
    ] {
        let out = h.rules(help);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            "usage: amont-agent rules [--json]\n"
        );
    }
}
