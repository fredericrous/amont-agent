//! Can `doctor` tell a working guard from a dead one?
//!
//! The failure this command exists to catch is silent by construction: a hook
//! whose command cannot be found exits 127, and 127 is a NON-blocking status,
//! so Claude Code carries on and nobody is told. A dead guard and a quiet week
//! produce identical evidence. Every test here sets up one of those states and
//! insists `doctor` distinguishes it.

use std::path::PathBuf;
use std::process::Command;

struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let dir =
            std::env::temp_dir().join(format!("amont-agent-doctor-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Home(dir)
    }
    fn settings(&self) -> PathBuf {
        self.0.join("settings.json")
    }
    /// A transcripts tree, so the liveness check has sessions to reason about.
    fn with_a_session(&self) -> &Home {
        let d = self.0.join("projects").join("-tmp-x");
        std::fs::create_dir_all(&d).expect("projects dir");
        std::fs::write(d.join("s.jsonl"), "{}\n").expect("a transcript");
        self
    }
    fn run(&self, args: &[&str]) -> (i32, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .args(args)
            .env("CLAUDE_CONFIG_DIR", &self.0)
            .env_remove("AMONT_AGENT_OFF")
            .output()
            .expect("the binary runs");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr),
        )
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn an_uninstalled_guard_is_reported_and_exits_nonzero() {
    let h = Home::new("absent");
    let (code, out) = h.run(&["doctor"]);
    assert_ne!(code, 0, "a guard that is not installed is not healthy");
    assert!(out.contains("not installed"), "{out}");
}

/// The 127 case, and the reason this command exists. A command that does not
/// resolve produces a hook that silently does nothing.
#[test]
fn a_missing_binary_is_reported_as_broken() {
    let h = Home::new("missing-bin");
    // Built from the scratch dir rather than written as `/nonexistent/...`,
    // because that literal is NOT absolute on Windows — `doctor` then correctly
    // reported "not an absolute path" and this test, which had assumed POSIX
    // path semantics, failed on the Windows runner while the code was right.
    let missing = h.0.join("nowhere").join("amont-agent");
    assert!(missing.is_absolute(), "the fixture must be absolute");
    std::fs::write(
        h.settings(),
        serde_json::to_string_pretty(&serde_json::json!({
            "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [
                {"type": "command", "command": missing, "args": ["hook"]}
            ]}]}
        }))
        .unwrap(),
    )
    .unwrap();
    let (code, out) = h.run(&["doctor"]);
    assert_ne!(code, 0);
    assert!(out.contains("does not exist"), "{out}");
    assert!(
        out.contains("127"),
        "the reason it is invisible is named: {out}"
    );
}

/// A `PATH`-resolved command is the same failure waiting to happen.
#[test]
fn a_relative_command_is_reported_as_fragile() {
    let h = Home::new("relative");
    std::fs::write(
        h.settings(),
        serde_json::to_string_pretty(&serde_json::json!({
            "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [
                {"type": "command", "command": "amont-agent", "args": ["hook"]}
            ]}]}
        }))
        .unwrap(),
    )
    .unwrap();
    let (code, out) = h.run(&["doctor"]);
    assert_ne!(code, 0);
    assert!(out.contains("absolute"), "{out}");
}

/// A fresh install must be healthy, and must say what it is acting on rather
/// than merely that it exists.
#[test]
fn a_fresh_install_is_healthy() {
    let h = Home::new("fresh");
    h.with_a_session();
    let (code, out) = h.run(&["install", "--write"]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = h.run(&["doctor"]);
    assert_eq!(code, 0, "a fresh install should be healthy:\n{out}");
    assert!(out.contains("pipe-to-tail"), "names what is armed: {out}");
}

/// Installing writes BOTH entries. Without the SessionStart one no heartbeat is
/// ever written, and liveness silently becomes unanswerable.
#[test]
fn installing_covers_the_session_start_event_too() {
    let h = Home::new("both");
    h.run(&["install", "--write"]);
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(h.settings()).unwrap()).unwrap();
    assert!(doc["hooks"]["PreToolUse"].is_array(), "{doc}");
    assert!(doc["hooks"]["SessionStart"].is_array(), "{doc}");
    let (_, out) = h.run(&["doctor"]);
    assert!(!out.contains("1 of 2 events"), "{out}");
}

/// The bug this test was written for: with no SessionStart entry there is no
/// heartbeat, and the first version reported "the guard has never run" — on a
/// machine where it had refused a command minutes earlier. Absence of a
/// heartbeat that could never have been written is not evidence of death.
#[test]
fn a_missing_heartbeat_is_not_evidence_the_guard_is_dead() {
    let h = Home::new("no-beat");
    h.with_a_session();
    std::fs::write(
        h.settings(),
        serde_json::to_string_pretty(&serde_json::json!({
            "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [
                {"type": "command", "command": env!("CARGO_BIN_EXE_amont-agent"), "args": ["hook"]}
            ]}]}
        }))
        .unwrap(),
    )
    .unwrap();
    let (code, out) = h.run(&["doctor"]);
    assert!(
        out.contains("cannot be judged") || out.contains("no heartbeat"),
        "{out}"
    );
    assert!(
        !out.contains("never run"),
        "accused a guard that was never given a way to prove itself:\n{out}"
    );
    assert_eq!(
        code, 0,
        "a missing SessionStart entry is advisory, not fatal"
    );
}

/// Hooks from different settings files MERGE rather than override, so two
/// installs mean every command is judged twice and every reason printed twice.
#[test]
fn two_installs_are_reported_as_a_double_fire() {
    let h = Home::new("double");
    h.with_a_session();
    h.run(&["install", "--write"]);
    // A project-scoped copy alongside the user one.
    let proj = h.0.join("proj").join(".claude");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::copy(h.settings(), proj.join("settings.json")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
        .arg("doctor")
        .current_dir(h.0.join("proj"))
        .env("CLAUDE_CONFIG_DIR", &h.0)
        .output()
        .expect("runs");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("settings files"), "{text}");
    assert!(text.contains("twice"), "{text}");
}

/// The kill switch must be visible. A guard that is installed, runnable and
/// completely inert is the most misleading state of all.
#[test]
fn the_kill_switch_is_reported() {
    let h = Home::new("off");
    h.with_a_session();
    h.run(&["install", "--write"]);
    let out = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
        .arg("doctor")
        .env("CLAUDE_CONFIG_DIR", &h.0)
        .env("AMONT_AGENT_OFF", "1")
        .output()
        .expect("runs");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("AMONT_AGENT_OFF"), "{text}");
    assert!(text.contains("observe"), "{text}");
}

/// A long-running session must not be mistaken for a dead guard.
///
/// `SessionStart` writes the heartbeat once, at the beginning, so in a
/// session open for more than the six-hour grace the heartbeat ages while
/// the transcript keeps being written. `doctor` announced "the guard has not
/// run in 14h" with the journal in the same directory, last written seconds
/// earlier by a real `pipe-to-tail` denial.
///
/// A long session is the NORMAL case for this tool, so the check was
/// accusing it of the one thing it was demonstrably not doing — and telling
/// somebody to go read debug logs for a hook that was working.
fn install_both(h: &Home) {
    std::fs::write(
        h.settings(),
        serde_json::to_string_pretty(&serde_json::json!({
            "hooks": {
                "PreToolUse": [{"matcher": "Bash", "hooks": [
                    {"type": "command", "command": env!("CARGO_BIN_EXE_amont-agent"), "args": ["hook"]}
                ]}],
                "SessionStart": [{"hooks": [
                    {"type": "command", "command": env!("CARGO_BIN_EXE_amont-agent"), "args": ["hook"]}
                ]}]
            }
        }))
        .unwrap(),
    )
    .unwrap();
}

/// Backdate a file, since the whole question is about relative ages.
fn backdate(path: &std::path::Path, stamp: &str) {
    let out = Command::new("touch")
        .arg("-t")
        .arg(stamp)
        .arg(path)
        .output()
        .expect("touch runs");
    assert!(out.status.success(), "touch -t {stamp}: {out:?}");
}

#[test]
fn a_recent_firing_outweighs_an_old_heartbeat() {
    let h = Home::new("long-session");
    h.with_a_session();
    install_both(&h);

    let dir = h.0.join("amont-agent");
    std::fs::create_dir_all(&dir).expect("state dir");
    std::fs::write(dir.join("heartbeat"), "x\n").expect("heartbeat");
    // Well past the six-hour grace, and before the session was written.
    backdate(&dir.join("heartbeat"), "202001010000");
    // The journal, written now: a rule fired, so the hook is plainly alive.
    std::fs::write(
        dir.join("journal.log"),
        "F 1 pipe-to-tail deny denied - - - x\n",
    )
    .expect("journal");

    let (code, out) = h.run(&["doctor"]);
    assert!(
        !out.contains("has not run"),
        "the journal proves it ran; accusing it anyway is the bug:\n{out}"
    );
    assert!(out.contains("last ran"), "{out}");
    assert_eq!(code, 0, "a live guard must not exit non-zero:\n{out}");
}

/// And the journal only ever CONFIRMS. With no firings recorded, an old
/// heartbeat beside a much newer session still means the hook stopped —
/// which is the accusation the check exists to make, and which the fix above
/// must not have softened into uselessness.
#[test]
fn an_old_heartbeat_with_no_firings_still_accuses() {
    let h = Home::new("really-dead");
    h.with_a_session();
    install_both(&h);

    let dir = h.0.join("amont-agent");
    std::fs::create_dir_all(&dir).expect("state dir");
    std::fs::write(dir.join("heartbeat"), "x\n").expect("heartbeat");
    backdate(&dir.join("heartbeat"), "202001010000");
    // No journal at all: nothing has fired since.

    let (_code, out) = h.run(&["doctor"]);
    assert!(
        out.contains("has not run"),
        "a genuinely dead guard must still be called out:\n{out}"
    );
}

/// Asking whether the guard is healthy must not change what it has measured.
///
/// `doctor` proves the guard works by feeding the real binary a command it
/// must refuse, and that firing used to be journalled like any other. The
/// journal is the measurement: `status` counts it, and the per-1000 evidence
/// that gates `graduate` comes from the same data — so a rule looked more
/// necessary the more often somebody checked on it.
///
/// It also made the liveness check unfalsifiable, since the journal was
/// always seconds old by the time it was read.
#[test]
fn doctor_does_not_journal_its_own_probe() {
    let h = Home::new("no-pollution");
    h.with_a_session();
    install_both(&h);

    let journal = h.0.join("amont-agent").join("journal.log");
    let before = std::fs::read_to_string(&journal).unwrap_or_default();

    let (_code, out) = h.run(&["doctor"]);
    // The probe really did run — otherwise this proves nothing.
    assert!(
        out.contains("valid decision document"),
        "the probe must actually fire for this test to mean anything:\n{out}"
    );

    let after = std::fs::read_to_string(&journal).unwrap_or_default();
    assert_eq!(
        before,
        after,
        "doctor wrote {} bytes to the journal it is only supposed to read",
        after.len().saturating_sub(before.len())
    );
}
