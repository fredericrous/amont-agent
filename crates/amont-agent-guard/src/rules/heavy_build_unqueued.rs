//! `heavy-build-unqueued` — a heavy build or test run outside the host slot
//! queue.
//!
//! ```sh
//! cargo test                                   # beside every other session's
//! amont slot run --on-timeout=run -- cargo test  # falls back to unqueued
//! ```
//!
//! Every heavy job on the machine queues on amont's host slots (decisions
//! ADR-0049). A build or test suite started bare runs beside whatever the
//! other sessions are running, and the host thrashes until gates time out.
//!
//! Heavy is [`runner_words`] from `test-output-trimmed`, plus `cargo build`,
//! `cargo check`, `xcodebuild` and `swift build`. A clause wrapped as
//! `amont slot run -- <cmd>`, `amont run <check>` or `amont rehearse` is
//! queued and is not judged; `amont slot run --on-timeout=run -- <cmd>` is the
//! exception, because it launches the command unqueued when no slot frees.
//!
//! ## `confirm` is where the fact lives
//!
//! The rule speaks only when queueing is possible: the hook's environment has
//! no `AMONT_HOST_SLOT` (the command already holds a slot otherwise), and
//! `amont --version` on `PATH` is 1.62.0 or newer (the first with `slot run`).
//! A version that was read is remembered for the session; the probe has a
//! short deadline. Amont's slot files are never read: `amont slots` is the query.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::rules::test_output_trimmed::{exempt_words, runner_words};
use crate::rules::{Confirmed, Context, Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple, Word};

pub const RULE: Rule = Rule {
    id: "heavy-build-unqueued",
    prevents: "A build or test suite run beside every other session's, unqueued, thrashing the host until gates time out",
    irreversible: false,
    default_stance: Stance::Advise,
    max_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-10-29",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: Some(confirm),
};

/// The first `amont` with `slot run`.
const FIRST_VERSION_WITH_SLOTS: (u64, u64, u64) = (1, 62, 0);

/// How long `amont --version` may take.
const PROBE_DEADLINE: Duration = Duration::from_millis(1500);

/// Set by `amont slot run` in the environment of the command it launches.
const HELD_SLOT_VARIABLE: &str = "AMONT_HOST_SLOT";

/// The most words of the command the advice repeats.
const ECHOED_WORDS: usize = 8;

struct Heavy<'a> {
    program: &'a str,
    args: &'a [Word],
}

/// Build commands beyond the runners `test-output-trimmed` knows.
fn builds(program: &str, words: &[Word]) -> bool {
    let operands: Vec<&str> = words
        .iter()
        .filter(|w| !w.quoted)
        .map(|w| w.text.as_str())
        .collect();
    let first = operands
        .iter()
        .copied()
        .find(|a| !a.starts_with('-') && !a.starts_with('+'));
    match program {
        "cargo" => matches!(first, Some("build" | "check")),
        "swift" => first == Some("build"),
        "xcodebuild" => !operands
            .iter()
            .any(|a| matches!(*a, "-version" | "-list" | "-showsdks" | "-help")),
        _ => false,
    }
}

fn is_heavy(program: &str, words: &[Word]) -> bool {
    !exempt_words(words) && (runner_words(program, words) || builds(program, words))
}

/// The heavy command a clause starts unqueued, if any.
fn unqueued(cmd: &Simple) -> Option<Heavy<'_>> {
    let program = cmd.program()?;
    if program == "amont" {
        return slot_run_fallback(cmd);
    }
    is_heavy(program, cmd.args()).then(|| Heavy {
        program,
        args: cmd.args(),
    })
}

/// `amont slot run --on-timeout=run -- <heavy>`: queued, but it runs the
/// command unqueued when no slot frees in time.
fn slot_run_fallback(cmd: &Simple) -> Option<Heavy<'_>> {
    let args = cmd.args();
    let separator = args.iter().position(|w| !w.quoted && w.text == "--")?;
    let own: Vec<&str> = args[..separator]
        .iter()
        .filter(|w| !w.quoted)
        .map(|w| w.text.as_str())
        .collect();
    let mut operands = own.iter().copied().filter(|a| !a.starts_with('-'));
    if operands.next() != Some("slot") || operands.next() != Some("run") {
        return None;
    }
    let runs_on_timeout = own.iter().enumerate().any(|(i, a)| {
        *a == "--on-timeout=run" || (*a == "--on-timeout" && own.get(i + 1) == Some(&"run"))
    });
    if !runs_on_timeout {
        return None;
    }
    let (program, rest) = args[separator + 1..].split_first()?;
    if program.quoted || !is_heavy(&program.text, rest) {
        return None;
    }
    Some(Heavy {
        program: &program.text,
        args: rest,
    })
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.judgeable() {
        let Some(heavy) = unqueued(cmd) else {
            continue;
        };
        let mut shown = vec![heavy.program];
        shown.extend(heavy.args.iter().take(ECHOED_WORDS).map(|w| w.raw.as_str()));
        let shown = shown.join(" ");
        return Some(Finding {
            reason: format!(
                "`{shown}` starts a heavy build or test run outside the host slot queue, \
                 so it runs beside every other session's and the machine thrashes until \
                 gates time out."
            ),
            remedy: format!(
                "Run it as `amont slot run -- {shown}`: it waits at most 90 s \
                 (`--max-wait`) for a slot; exit 75 means no slot freed and the command \
                 was not launched. `amont slots` shows who holds them."
            ),
            span: cmd.at..cmd.end,
        });
    }
    None
}

fn confirm(context: &Context, _finding: &Finding) -> Confirmed {
    if std::env::var_os(HELD_SLOT_VARIABLE).is_some() {
        return Confirmed::No("the hook runs inside a host slot already");
    }
    if !amont_has_slots(context.session) {
        return Confirmed::No("amont on PATH is missing, unanswering or older than 1.62.0");
    }
    Confirmed::Yes
}

/// Whether `amont` on `PATH` has `slot run`, remembered on the session's
/// record so the next hook process does not ask. Only a version that was read
/// is remembered: a missing, failing or slow amont (a loaded host is when the
/// probe times out) is asked again next time, never recorded as too old.
fn amont_has_slots(session: &str) -> bool {
    const CAPABLE: &str = "heavy-build-unqueued amont-has-slots";
    const INCAPABLE: &str = "heavy-build-unqueued amont-lacks-slots";
    if crate::lsp::was_told(session, CAPABLE) {
        return true;
    }
    if crate::lsp::was_told(session, INCAPABLE) {
        return false;
    }
    let Some(version) = probe_version() else {
        return false;
    };
    let capable = version >= FIRST_VERSION_WITH_SLOTS;
    let _ = crate::lsp::told(session, if capable { CAPABLE } else { INCAPABLE }); // unrecorded: the next hook process asks again
    capable
}

/// `amont --version`, killed at [`PROBE_DEADLINE`]; `None` when amont is
/// missing, fails, hangs or prints no version.
fn probe_version() -> Option<(u64, u64, u64)> {
    let mut child = Command::new("amont")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + PROBE_DEADLINE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill(); // past the deadline the answer is already unknown
                let _ = child.wait(); // a reap that fails leaves nothing more to report
                return None;
            }
        }
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut text).ok()?;
    parse_version(&text)
}

/// The first `major.minor.patch` in `--version` output such as `amont 1.62.0`.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    text.split_whitespace().find_map(|token| {
        let mut parts = token.trim_start_matches('v').split('.').map(|part| {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u64>().ok()
        });
        Some((parts.next()??, parts.next()??, parts.next()??))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn a_bare_heavy_command_fires() {
        for c in [
            "cargo test",
            "cd x && pnpm test",
            "xcodebuild -scheme App build",
            "cargo build --release",
            "cargo check",
            "swift build",
            "amont slot run --on-timeout=run -- cargo test",
            "amont slot run --on-timeout run -- cargo test",
        ] {
            assert!(fires(c), "{c}");
        }
    }

    #[test]
    fn a_queued_or_light_command_does_not() {
        for c in [
            "amont slot run -- cargo test",
            "amont slot run --weight 2 -- cargo test",
            "amont slot run --on-timeout=fail -- cargo test",
            "amont run cargo-test",
            "amont rehearse --wait",
            "cargo fmt",
            "cargo test --help",
            "cargo test --no-run",
            "swift --version",
            "xcodebuild -version",
            "git status",
        ] {
            assert!(!fires(c), "{c}");
        }
    }

    #[test]
    fn the_advice_names_the_queue_and_the_exit_code() {
        let finding = examine(&lex("cd x && cargo test -p foo")).unwrap();
        assert!(finding
            .remedy
            .contains("amont slot run -- cargo test -p foo"));
        assert!(finding.remedy.contains("90 s"));
        assert!(finding.remedy.contains("75"));
        assert!(finding.remedy.contains("amont slots"));
    }

    #[test]
    fn a_version_is_read_from_the_output() {
        assert_eq!(parse_version("amont 1.62.0\n"), Some((1, 62, 0)));
        assert_eq!(parse_version("amont v1.61.1-rc1"), Some((1, 61, 1)));
        assert!(parse_version("amont 2.0.0") >= Some(FIRST_VERSION_WITH_SLOTS));
        assert_eq!(parse_version("no version here"), None);
    }
}
