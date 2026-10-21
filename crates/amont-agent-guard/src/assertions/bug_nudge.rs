//! `bug-nudge`: a check run failed and the `diagnosing-bugs` skill was not
//! loaded, so say so once per session.
//!
//! The pull toward another command after a red run is strong, and a loop built
//! for the failure is cheaper than guessing. This puts the skill in front of
//! the model at the first red check, then stays quiet for the session: the
//! marker file is created last, so a call that could not be judged leaves no
//! trace and is judged again next time.
//!
//! Advise only. Every uncertainty (no transcript, an unreadable one, a marker
//! that cannot be written) is silence, and is reported as an unknown verdict.

use crate::assertions::{Assertion, Claim, Verdict};
use crate::rules::test_output_trimmed::is_check_run;
use crate::rules::{Context, Evidence, Stance, Trend};
use crate::shell::Parsed;
use crate::skill_window::{self, Window};

const SKILL: &str = "diagnosing-bugs";

pub const ASSERTION: Assertion = Assertion {
    id: "bug-nudge",
    prevents:
        "Guessing at another command after a red run when the diagnosing-bugs skill was not loaded",
    irreversible: false,
    default_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-10-21",
        trend: Trend::Rare,
    },
    examine,
    verify,
};

fn examine(parsed: &Parsed) -> Option<Claim> {
    let cmd = parsed.judgeable().find(|c| is_check_run(c))?;
    Some(Claim {
        what: "a failed check run".to_string(),
        span: cmd.at..cmd.end,
    })
}

/// The output names a failure.
fn names_failure(output: Option<&str>) -> bool {
    output.is_some_and(|o| {
        ["FAILED", "panicked", "error[E"]
            .iter()
            .any(|m| o.contains(m))
    })
}

/// Whether the run behind `claim` was red. The connector after the runner's
/// clause decides what the evidence is worth: a failed call's exit status is
/// the runner's own only when nothing follows it or `&&` does; a call that
/// exited 0 says nothing of a bare runner, so its text is read only when the
/// runner feeds a pipe or precedes `;`.
fn is_red(ctx: &Context, claim: &Claim) -> bool {
    use crate::shell::Connector;
    let next = ctx
        .parsed
        .clauses()
        .iter()
        .find(|c| c.at == claim.span.start)
        .map(|c| c.next);
    if ctx.failed {
        let exit_is_the_runners = matches!(next, Some(None | Some(Connector::AndAnd)));
        exit_is_the_runners || names_failure(ctx.output)
    } else {
        let text_is_the_runners = matches!(next, Some(Some(Connector::Pipe | Connector::Semi)));
        text_is_the_runners && names_failure(ctx.output)
    }
}

/// Whether the skill was loaded in this turn or the one before; `Err` names
/// why that cannot be told.
fn skill_loaded(ctx: &Context) -> Result<bool, &'static str> {
    if ctx.transcript.is_none() && ctx.agent_transcript.is_none() {
        return Err("no transcript to read");
    }
    for path in [ctx.transcript, ctx.agent_transcript].into_iter().flatten() {
        let Ok(file) = std::fs::File::open(path) else {
            return Err("transcript unreadable");
        };
        match skill_window::skill_window(file, SKILL) {
            Ok(Window::Current | Window::Previous) => return Ok(true),
            Err(_) => return Err("transcript unreadable"),
            Ok(Window::Outside) => {}
        }
    }
    Ok(false)
}

/// Create the session's marker. `Ok(true)` only for the call that created
/// it; `Ok(false)` when it already exists.
fn claim_marker(session: &str) -> Result<bool, &'static str> {
    if !crate::session_state::usable_id(session) {
        return Err("session id unusable for a marker");
    }
    let Some(dir) = crate::journal::dir() else {
        return Err("no state directory for a marker");
    };
    let sessions = dir.join("sessions");
    if std::fs::create_dir_all(&sessions).is_err() {
        return Err("marker directory not creatable");
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(sessions.join(format!("{session}.bug-nudge")))
    {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(_) => Err("marker not creatable"),
    }
}

fn verify(ctx: &Context, claim: &Claim) -> Verdict {
    if !is_red(ctx, claim) {
        return Verdict::Held;
    }
    match skill_loaded(ctx) {
        Ok(true) => return Verdict::Held,
        Ok(false) => {}
        Err(why) => return Verdict::Unknown(why),
    }
    match claim_marker(ctx.session) {
        Ok(true) => {}
        Ok(false) => return Verdict::Held,
        Err(why) => return Verdict::Unknown(why),
    }
    Verdict::Broken {
        reason: "Call the diagnosing-bugs skill, then build a red loop before running more \
                 commands: a check run failed and the skill was not loaded."
            .to_string(),
        remedy: "If the skill does not apply, ask the person; they can run `git config \
                 --global amont.agent.bug-nudge.stance observe`."
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;
    use std::path::Path;

    /// `CLAUDE_CONFIG_DIR` is process-wide and the test threads are not, so
    /// the tests that set it take turns.
    fn with_home<T>(name: &str, f: impl FnOnce(&Path) -> T) -> T {
        let _turn = crate::assertions::test_support::CONFIG_DIR_TURN
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir =
            std::env::temp_dir().join(format!("amont-agent-nudge-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
        let out = f(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    fn transcript(dir: &Path, lines: &[String]) -> std::path::PathBuf {
        let p = dir.join("t.jsonl");
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        p
    }

    fn human() -> String {
        serde_json::json!({"type":"user","message":{"role":"user","content":"go"},"origin":{"kind":"human"}}).to_string()
    }

    fn skill() -> String {
        serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"t1","name":"Skill","input":{"skill":SKILL}}]}})
        .to_string()
    }

    fn run(
        command: &str,
        failed: bool,
        output: Option<&str>,
        transcript: Option<&Path>,
        session: &str,
    ) -> Verdict {
        let parsed = lex(command);
        let claim = examine(&parsed).expect("a check run is claimed");
        let ctx = Context {
            platform: None,
            cwd: Path::new("/session"),
            parsed: &parsed,
            background: false,
            timeout_ms: None,
            tool_use_id: "",
            transcript,
            agent_transcript: None,
            command,
            output,
            session,
            failed,
        };
        verify(&ctx, &claim)
    }

    fn said(v: &Verdict) -> bool {
        matches!(v, Verdict::Broken { .. })
    }

    #[test]
    fn a_red_cargo_test_fires_once_in_a_session() {
        with_home("fires", |dir| {
            let t = transcript(dir, &[human()]);
            let v = run("cargo test", true, None, Some(&t), "s1");
            match &v {
                Verdict::Broken { reason, remedy } => {
                    let head: String = format!("amont-agent/bug-nudge: {reason}")
                        .chars()
                        .take(80)
                        .collect();
                    assert!(head.contains("Call the diagnosing-bugs skill"), "{head}");
                    assert!(reason.contains("build a red loop before running more commands"));
                    assert!(remedy.contains("amont.agent.bug-nudge.stance observe"));
                }
                _ => panic!("a red run is said"),
            }
        });
    }

    #[test]
    fn a_second_call_in_the_same_session_is_silent() {
        with_home("twice", |dir| {
            let t = transcript(dir, &[human()]);
            assert!(said(&run("cargo test", true, None, Some(&t), "s1")));
            assert!(!said(&run("cargo test", true, None, Some(&t), "s1")));
        });
    }

    #[test]
    fn a_green_run_is_silent() {
        with_home("green", |dir| {
            let t = transcript(dir, &[human()]);
            let v = run(
                "cargo test",
                false,
                Some("test result: ok. 3 passed"),
                Some(&t),
                "s1",
            );
            assert!(!said(&v));
        });
    }

    #[test]
    fn a_grep_printing_error_is_silent() {
        with_home("grep", |dir| {
            let t = transcript(dir, &[human()]);
            let parsed = lex("grep error src/lib.rs");
            assert!(examine(&parsed).is_none());
            // Even if a runner is named, an output that never names a failure
            // on a successful call stays silent.
            let v = run(
                "cargo test",
                false,
                Some("error handling is covered"),
                Some(&t),
                "s1",
            );
            assert!(!said(&v));
        });
    }

    #[test]
    fn a_loaded_skill_is_silent() {
        with_home("loaded", |dir| {
            let t = transcript(dir, &[human(), skill()]);
            assert!(!said(&run("cargo test", true, None, Some(&t), "s1")));
        });
    }

    #[test]
    fn a_missing_transcript_is_unknown() {
        with_home("missing", |_| {
            let v = run("cargo test", true, None, None, "s1");
            assert!(matches!(v, Verdict::Unknown(_)));
        });
    }

    #[test]
    fn an_unreadable_transcript_is_unknown() {
        with_home("unreadable", |dir| {
            let gone = dir.join("gone.jsonl");
            let v = run("cargo test", true, None, Some(&gone), "s1");
            assert!(matches!(v, Verdict::Unknown(_)));
        });
    }

    #[test]
    fn a_failed_runner_followed_by_semicolon_needs_failure_text() {
        with_home("semi", |dir| {
            let t = transcript(dir, &[human()]);
            let v = run(
                "cargo test; ls missing",
                true,
                Some("ls: missing"),
                Some(&t),
                "s1",
            );
            assert!(!said(&v));
            let v = run("cargo test || true", true, Some("quiet"), Some(&t), "s1");
            assert!(!said(&v));
            let v = run(
                "cargo test; ls missing",
                true,
                Some("test x ... FAILED"),
                Some(&t),
                "s1",
            );
            assert!(said(&v));
        });
    }

    #[test]
    fn a_bare_runner_that_exited_0_is_not_read_for_text() {
        with_home("bare", |dir| {
            let t = transcript(dir, &[human()]);
            for cmd in [
                "cargo test -- --nocapture",
                "cargo test -- --nocapture && echo done",
            ] {
                let v = run(cmd, false, Some("test x ... FAILED"), Some(&t), "s1");
                assert!(!said(&v), "{cmd}");
            }
        });
    }

    #[test]
    fn a_runner_that_is_not_last_with_no_text_match_is_silent() {
        with_home("pipeline", |dir| {
            let t = transcript(dir, &[human()]);
            let v = run(
                "cargo test | tail -5",
                true,
                Some("all quiet"),
                Some(&t),
                "s1",
            );
            assert!(!said(&v));
            let v = run(
                "cargo test | tail -5",
                true,
                Some("test x ... FAILED"),
                Some(&t),
                "s1",
            );
            assert!(said(&v));
        });
    }

    #[test]
    fn a_heredoc_body_naming_a_runner_is_not_claimed() {
        let parsed = lex("python3 - <<'EOF'\nprint('cargo test -q')\nEOF");
        assert!(examine(&parsed).is_none());
    }

    #[test]
    fn a_runner_after_a_heredoc_is_still_claimed() {
        let parsed = lex("python3 - <<'EOF'\nprint('cargo test -q')\nEOF\ncargo test");
        assert!(examine(&parsed).is_some());
    }
}
