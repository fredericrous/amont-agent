//! Mining, compliance and novelty, against transcripts written by hand.
//!
//! These three verbs read a file format somebody else defines and derive
//! numbers from it, which is the shape of code that passes its unit tests and
//! reports nonsense on the real thing. So the fixtures here are whole
//! transcripts — assistant turns carrying `tool_use` blocks, user turns
//! carrying `tool_result` blocks with `is_error` where it belongs — and the
//! binary is driven exactly as a person would drive it.
//!
//! `timestamp` is not decoration. An entry without one is skipped before its
//! tool-call blocks are looked at, so a fixture missing it parses to zero
//! calls and every assertion about "no shape was proposed" passes for the
//! wrong reason. The sigpipe test learned that first.
//!
//! The git config files are pinned to empty fixture files for the same reason
//! `stance_scope` pins them: `--compliance` prints the stance in force, and
//! the stance in force is whatever the machine running the tests has in its
//! global config.

use std::path::PathBuf;
use std::process::Command;

struct Transcript {
    dir: PathBuf,
    lines: Vec<String>,
    n: usize,
}

impl Transcript {
    fn new(name: &str) -> Transcript {
        let dir =
            std::env::temp_dir().join(format!("amont-agent-mine-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("projects/p")).expect("scratch dir");
        std::fs::write(dir.join("global"), "").expect("global config");
        std::fs::write(dir.join("system"), "").expect("system config");
        Transcript {
            dir,
            lines: Vec::new(),
            n: 0,
        }
    }

    /// One Bash call by `model`, and the result it got.
    fn call(&mut self, model: &str, command: &str, failed: bool) -> &mut Self {
        self.n += 1;
        let id = format!("t{}", self.n);
        let day = 3 + self.n / 40;
        let stamp = format!("2026-08-{day:02}T10:00:00.000Z");
        let command = escape(command);
        self.lines.push(format!(
            r#"{{"sessionId":"s","cwd":"/tmp","timestamp":"{stamp}","type":"assistant","message":{{"model":"{model}","content":[{{"type":"tool_use","id":"{id}","name":"Bash","input":{{"command":"{command}"}}}}]}}}}"#
        ));
        self.lines.push(format!(
            r#"{{"sessionId":"s","cwd":"/tmp","timestamp":"{stamp}","type":"user","message":{{"content":[{{"tool_use_id":"{id}","type":"tool_result","content":"out","is_error":{failed}}}]}}}}"#
        ));
        self
    }

    /// A tool call that is not Bash. It carries no command and can match no
    /// rule; it is there to take up room in the window.
    fn other(&mut self) -> &mut Self {
        self.n += 1;
        let id = format!("t{}", self.n);
        let day = 3 + self.n / 40;
        self.lines.push(format!(
            r#"{{"sessionId":"s","cwd":"/tmp","timestamp":"2026-08-{day:02}T10:00:00.000Z","type":"assistant","message":{{"model":"m","content":[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"/tmp/x"}}}}]}}}}"#
        ));
        self
    }

    fn write(&self) -> &PathBuf {
        let mut out = self.lines.join("\n");
        out.push('\n');
        std::fs::write(self.dir.join("projects/p/session.jsonl"), out).expect("transcript");
        &self.dir
    }

    fn run(&self, args: &[&str]) -> String {
        let dir = self.write();
        let out = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .args(args)
            .arg("--transcripts")
            .arg(dir.join("projects"))
            .env("GIT_CONFIG_GLOBAL", dir.join("global"))
            .env("GIT_CONFIG_SYSTEM", dir.join("system"))
            .env_remove("AMONT_AGENT_OFF")
            .output()
            .expect("the binary runs");
        assert!(
            out.status.success(),
            "{args:?} exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A shape the model keeps getting wrong, with nothing in the rules about it,
/// is what `mine` exists to surface.
#[test]
fn a_repeated_mistake_is_proposed_with_its_samples() {
    let mut t = Transcript::new("propose");
    for i in 0..6 {
        // The mistake, then the fix: one episode each time.
        t.call("m", &format!("tofu apply -target module.a{i}"), true);
        t.call("m", &format!("tofu apply -target module.b{i}"), false);
        // Something else, so the next episode is not adjacent to this one.
        t.other().other().other().other();
    }
    let out = t.run(&["mine"]);
    assert!(
        out.contains("tofu apply"),
        "the shape should be proposed:\n{out}"
    );
    assert!(
        out.contains("-target"),
        "the flag is part of the shape:\n{out}"
    );
    assert!(
        out.contains("tofu apply -target module.a0"),
        "a sample of a call that went wrong:\n{out}"
    );
}

/// A polling loop is one decision repeated. Counting each turn of it as a
/// correction of the last put `echo waiting` at the top of the real report.
#[test]
fn a_polling_loop_is_not_a_run_of_mistakes() {
    let mut t = Transcript::new("poll");
    for i in 0..20 {
        t.call("m", &format!("sleep 5 && curl -sS http://h/{i}"), false);
    }
    let out = t.run(&["mine"]);
    assert!(
        !out.contains("curl"),
        "a loop is not twenty corrections:\n{out}"
    );
    assert!(out.contains("no shape clears the thresholds"), "{out}");
}

/// A shape an existing rule already names is listed, not proposed. Proposing
/// it would send a reader off to write a rule that is already written.
#[test]
fn a_shape_a_rule_already_names_is_listed_apart() {
    let mut t = Transcript::new("covered");
    for i in 0..6 {
        t.call(
            "m",
            &format!("git push origin feat/a{i} 2>&1 | tail -5"),
            true,
        );
        t.call("m", &format!("git push origin feat/b{i}"), false);
        t.other().other().other().other();
    }
    let out = t.run(&["mine"]);
    let (proposed, covered) = out
        .split_once("already covered by a rule")
        .expect("a covered section");
    assert!(
        covered.contains("pipe-to-tail"),
        "named by the rule that covers it:\n{out}"
    );
    assert!(
        !proposed.contains("tail -5"),
        "it must not also be proposed:\n{out}"
    );
}

/// The whole point of `--format cases`: what mining finds goes straight into
/// the file the reviewer edits and `corpus check` reads.
#[test]
fn format_cases_emits_unreviewed_lines_in_the_corpus_format() {
    let mut t = Transcript::new("cases");
    for i in 0..6 {
        t.call("m", &format!("tofu apply -target module.a{i}"), true);
        t.call("m", &format!("tofu apply -target module.b{i}"), false);
        t.other().other().other().other();
    }
    let out = t.run(&["mine", "--format", "cases"]);
    assert!(out.starts_with("# amont-agent-cases-v1\n"), "{out}");
    let cases: Vec<&str> = out.lines().skip(1).filter(|l| !l.is_empty()).collect();
    assert!(!cases.is_empty(), "no cases emitted:\n{out}");
    for line in &cases {
        let (verdict, command) = line.split_once('\t').expect("a tab-separated case");
        assert_eq!(verdict, "?", "every case starts unreviewed");
        assert!(!command.contains('\n'), "one line per case");
    }
}

/// Compliance is per model, because the answer differs by model and a pooled
/// number hides which one is listening.
#[test]
fn compliance_separates_the_model_that_listened_from_the_one_that_did_not() {
    let mut t = Transcript::new("compliance");
    for i in 0..4 {
        // Told, then did it differently: complied.
        t.call("listener", &format!("rg --include=*.rs alpha{i}"), false);
        t.call("listener", &format!("rg --include='*.rs' alpha{i}"), false);
        // Told, then did the same thing again: ignored.
        t.call("deaf", &format!("rg --include=*.ts beta{i}"), false);
        t.call("deaf", &format!("rg --include=*.ts beta{i}b"), false);
    }
    let out = t.run(&["backtest", "--compliance", "--rule", "glob-in-flag-value"]);
    let rows: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("listener") || l.contains("deaf"))
        .collect();
    assert_eq!(rows.len(), 2, "one row per model:\n{out}");
    let listener = rows.iter().find(|r| r.contains("listener")).unwrap();
    let deaf = rows.iter().find(|r| r.contains("deaf")).unwrap();
    assert!(listener.ends_with("100%"), "{listener}");
    assert!(deaf.ends_with("0%"), "{deaf}");
}

/// A `deny` rule has no "next command" to look at — the command it names did
/// not run — so it is left out rather than reported as ignored.
#[test]
fn compliance_leaves_out_what_was_refused() {
    let mut t = Transcript::new("deny");
    for i in 0..4 {
        t.call("m", &format!("git push origin feat/x{i} | tail -5"), false);
    }
    let out = t.run(&["backtest", "--compliance", "--rule", "pipe-to-tail"]);
    assert!(
        out.contains("no rule fired over these transcripts"),
        "a denied rule has no compliance to report:\n{out}"
    );
}

/// Novelty ranking exists to stop a reviewer labelling twenty spellings of
/// one command. With one odd match among many alike, it comes first.
///
/// The odd one out has to be odd against the REVIEWED CORPUS too, not just
/// against the other matches — a shape the corpus already covers is exactly
/// what novelty is supposed to skip, and the first attempt at this test
/// picked a `kubectl … custom-columns` that `glob-in-flag-value.cases`
/// already holds.
#[test]
fn novelty_puts_the_unlike_match_first() {
    let mut t = Transcript::new("novelty");
    for i in 0..8 {
        t.call("m", &format!("rg --include=*.rs alpha{i}"), false);
    }
    t.call("m", "helm template chart --set=image.tag=v1.*", false);
    let plain = t.run(&["explain", "glob-in-flag-value", "--sample", "1"]);
    let novel = t.run(&[
        "explain",
        "glob-in-flag-value",
        "--sample",
        "1",
        "--rank",
        "novelty",
    ]);
    assert!(
        plain.contains("--include"),
        "the first match, as met:\n{plain}"
    );
    assert!(
        novel.contains("--set="),
        "the match least like the rest comes first:\n{novel}"
    );
}

/// `--rank` takes one value, and a typo is a usage error rather than a
/// silently ignored flag.
#[test]
fn an_unknown_rank_is_refused() {
    let dir = Transcript::new("rank").write().clone();
    let out = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
        .args(["explain", "pipe-to-tail", "--rank", "wisdom"])
        .arg("--transcripts")
        .arg(dir.join("projects"))
        .output()
        .expect("the binary runs");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--rank takes `novelty`"));
}
