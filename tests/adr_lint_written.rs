//! `adr-lint-written`, end to end through the real hook binary, with fake
//! `amont` and `adr-lint` programs (decisions ADR-0044, behaviour 4).
//!
//! Each fake appends a line to a file when it runs, so a test can say how
//! often, and from where, it was asked.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const FINDING: &str = r##"[{"path":"src/a.rs","line":7,"col":3,"rule":"tests.first-properties","message":"an ignored test needs a reason","detail":"d","text":"#[ignore]"}]"##;
/// The row as amont lists it before `amont trust`: `unusable`, no command.
const GATE_OPEN: &str = r#"{"format":"amont-list-v1","checks":[{"id":"pre-commit-adr-lint-check","status":"unusable","command":null}]}"#;
const GATE_CLOSED: &str = r#"{"format":"amont-list-v1","checks":[{"id":"pre-commit-cargo-test","command":"cargo test"}]}"#;

struct World {
    root: PathBuf,
    repo: PathBuf,
}

impl World {
    fn new(name: &str) -> World {
        let root =
            std::env::temp_dir().join(format!("amont-agent-adrlint-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo/src")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let repo = root.join("repo");
        let init = Command::new("git")
            .args(["init", "-q", "--template="])
            .arg(&repo)
            .output()
            .unwrap();
        assert!(init.status.success());
        std::fs::write(repo.join("src/a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(repo.join("README.md"), "x\n").unwrap();
        World { root, repo }
    }

    fn bin(&self, name: &str) -> PathBuf {
        self.root.join("bin").join(name)
    }

    fn log(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.log"))
    }

    /// An `amont` that records where it ran and prints `document`.
    #[cfg(unix)] // a #!/bin/sh fake
    fn fake_amont(&self, document: &str) {
        let script = format!(
            "#!/bin/sh\npwd >> '{}'\nprintf '%s' '{document}'\n",
            self.log("amont").display()
        );
        common::write_fake(&self.bin("amont"), &script);
    }

    /// An `adr-lint` that records its arguments, then runs `body`.
    #[cfg(unix)] // a #!/bin/sh fake
    fn fake_adr_lint(&self, body: &str) {
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\n{body}\n",
            self.log("adr-lint").display()
        );
        common::write_fake(&self.bin("adr-lint"), &script);
    }

    fn runs(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.log(name))
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// A finished Write of `file`, reported from `cwd`.
    #[cfg(unix)] // PATH is joined with ':'
    fn written(&self, session: &str, file: &Path, cwd: &Path) -> (String, std::time::Duration) {
        let payload = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "cwd": cwd,
            "session_id": session,
            "permission_mode": "default",
            "tool_input": {"file_path": file, "content": "x"},
            "tool_response": {"type": "update"},
        })
        .to_string();
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let started = std::time::Instant::now();
        let mut child = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .arg("hook")
            .env("CLAUDE_CONFIG_DIR", self.root.join("config"))
            .env("PATH", path)
            .env("AMONT_AGENT_ADR_LINT", self.bin("adr-lint"))
            .env("GIT_CONFIG_GLOBAL", self.root.join("no-global-gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("AMONT_AGENT_OFF")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0));
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            started.elapsed(),
        )
    }

    #[cfg(unix)] // built on `written`
    fn edit(&self, session: &str, relative: &str) -> (String, std::time::Duration) {
        self.written(session, &self.repo.join(relative), &self.repo)
    }

    fn journal(&self, session: &str) -> Vec<String> {
        std::fs::read_to_string(self.root.join("config/amont-agent/journal.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains(session))
            .map(str::to_string)
            .collect()
    }
}

fn reason(stdout: &str) -> String {
    serde_json::from_str::<serde_json::Value>(stdout)
        .ok()
        .and_then(|v| {
            Some(
                v.get("hookSpecificOutput")?
                    .get("additionalContext")?
                    .as_str()?
                    .to_string(),
            )
        })
        .unwrap_or_default()
}

#[cfg(unix)] // fake programs are #!/bin/sh scripts
#[test]
fn a_finding_on_a_vendored_repository_comes_back_as_advice_and_is_journalled() {
    let w = World::new("finding");
    w.fake_amont(GATE_OPEN);
    w.fake_adr_lint(&format!("printf '%s' '{FINDING}'\nexit 1"));
    let (stdout, _) = w.edit("adr-a", "src/a.rs");
    let said = reason(&stdout);
    assert!(
        said.contains("src/a.rs:7:3: tests.first-properties: an ignored test needs a reason"),
        "{stdout}"
    );
    assert!(
        said.contains("aval rule tests.first-properties"),
        "{stdout}"
    );
    assert!(
        said.contains("adr-lint: allow <rule-id> -- <reason>"),
        "{stdout}"
    );
    assert_eq!(w.runs("adr-lint"), ["check --json -- src/a.rs"]);
    let rows = w.journal("adr-a");
    assert!(
        rows.iter().any(|r| r.contains("adr-lint-written")
            && r.contains(" advised ")
            && r.contains("tests.first-properties src/a.rs #[ignore]")),
        "{rows:?}"
    );
}

#[cfg(unix)] // fake programs are #!/bin/sh scripts
#[test]
fn a_repository_without_the_check_row_is_left_alone() {
    let w = World::new("closed");
    w.fake_amont(GATE_CLOSED);
    w.fake_adr_lint(&format!("printf '%s' '{FINDING}'\nexit 1"));
    let (stdout, _) = w.edit("adr-b", "src/a.rs");
    assert_eq!(stdout, "");
    assert!(w.runs("adr-lint").is_empty());
}

#[cfg(unix)] // fake programs are #!/bin/sh scripts
#[test]
fn a_file_that_is_not_source_runs_neither_program() {
    let w = World::new("markdown");
    w.fake_amont(GATE_OPEN);
    w.fake_adr_lint(&format!("printf '%s' '{FINDING}'\nexit 1"));
    let (stdout, _) = w.edit("adr-c", "README.md");
    assert_eq!(stdout, "");
    assert!(w.runs("amont").is_empty());
    assert!(w.runs("adr-lint").is_empty());
}

#[cfg(unix)] // fake programs are #!/bin/sh scripts
#[test]
fn a_slow_adr_lint_is_killed_and_journalled_and_the_hook_returns() {
    let w = World::new("slow");
    w.fake_amont(GATE_OPEN);
    // A clean run first: it warms the gate and times the hook's own start,
    // which a loaded machine stretches and the deadline must not be blamed for.
    w.fake_adr_lint("exit 0");
    let (_, baseline) = w.edit("adr-d", "src/a.rs");
    w.fake_adr_lint("sleep 3");
    let (stdout, took) = w.edit("adr-d", "src/a.rs");
    assert_eq!(stdout, "");
    // Killed at 1 s: well short of the 3 s the fake would take.
    assert!(
        took < baseline + std::time::Duration::from_secs(2),
        "{took:?} against a clean run of {baseline:?}"
    );
    let rows = w.journal("adr-d");
    assert!(rows.iter().any(|r| r.contains("adr-lint-slow")), "{rows:?}");
}

#[cfg(unix)] // fake programs are #!/bin/sh scripts
#[test]
fn a_failure_is_journalled_once_per_session_and_never_shown() {
    let w = World::new("exit3");
    w.fake_amont(GATE_OPEN);
    w.fake_adr_lint("exit 3");
    let (first, _) = w.edit("adr-e", "src/a.rs");
    let (second, _) = w.edit("adr-e", "src/a.rs");
    assert_eq!((first.as_str(), second.as_str()), ("", ""));
    let rows = w.journal("adr-e");
    let failures = rows
        .iter()
        .filter(|r| r.contains("adr-lint-exit-3"))
        .count();
    assert_eq!(failures, 1, "{rows:?}");
}

#[cfg(unix)] // fake programs are #!/bin/sh scripts
#[test]
fn the_gate_is_asked_once_per_session_and_repository() {
    let w = World::new("cache");
    w.fake_amont(GATE_OPEN);
    w.fake_adr_lint("exit 0");
    w.edit("adr-f", "src/a.rs");
    w.edit("adr-f", "src/a.rs");
    assert_eq!(w.runs("amont").len(), 1);
    assert_eq!(w.runs("adr-lint").len(), 2);
}

#[cfg(unix)] // fake programs are #!/bin/sh scripts
#[test]
fn the_gate_is_asked_in_the_files_repository_not_the_payload_cwd() {
    let w = World::new("root");
    w.fake_amont(GATE_OPEN);
    w.fake_adr_lint("exit 0");
    w.written("adr-g", &w.repo.join("src/a.rs"), &w.root.join("elsewhere"));
    assert_eq!(w.runs("amont"), [w.repo.display().to_string()]);
    assert_eq!(w.runs("adr-lint"), ["check --json -- src/a.rs"]);
}

#[cfg(unix)] // fake programs are #!/bin/sh scripts
#[test]
fn a_failing_amont_closes_the_gate_and_is_journalled() {
    let w = World::new("badamont");
    w.fake_amont("not json");
    w.fake_adr_lint(&format!("printf '%s' '{FINDING}'\nexit 1"));
    let (stdout, _) = w.edit("adr-h", "src/a.rs");
    assert_eq!(stdout, "");
    assert!(w.runs("adr-lint").is_empty());
    let rows = w.journal("adr-h");
    assert!(
        rows.iter().any(|r| r.contains("amont-list-unexpected")),
        "{rows:?}"
    );
}

#[cfg(unix)] // fake programs are #!/bin/sh scripts
#[test]
fn observing_journals_the_findings_and_says_nothing() {
    let w = World::new("observe");
    w.fake_amont(GATE_OPEN);
    w.fake_adr_lint(&format!("printf '%s' '{FINDING}'\nexit 1"));
    // The harness points GIT_CONFIG_GLOBAL at this file; a person sets a
    // stance by writing one.
    std::fs::write(
        w.root.join("no-global-gitconfig"),
        "[amont \"agent\"]\n\tstance = observe\n",
    )
    .unwrap();
    let (stdout, _) = w.edit("adr-i", "src/a.rs");
    assert_eq!(stdout, "");
    let rows = w.journal("adr-i");
    assert!(
        rows.iter().any(|r| r.contains("adr-lint-written")
            && r.contains(" watched ")
            && r.contains("tests.first-properties")),
        "{rows:?}"
    );
}
