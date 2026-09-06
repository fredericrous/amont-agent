//! Assertions: what a command CLAIMED, checked after it ran.
//!
//! Every rule in `rules/` reads a command before it runs and can refuse it.
//! That catches a mistake visible in the command string. It cannot catch the
//! other half, which is a command that ran, exited 0, and did not do the thing:
//!
//!   - `git push` printing "Everything up-to-date" because the branch was never
//!     the one you thought, or reaching a remote that took the ref and dropped
//!     it;
//!   - `gh run watch --exit-status` returning 0 for a run that concluded
//!     `failure`;
//!   - `git tag v1.4.0` naming the commit BEFORE the one you just made, so CI
//!     builds the new version number out of stale code.
//!
//! None of those is a failure anybody can see. The exit code says success, the
//! output looks like success, and the model reports success — which is exactly
//! the admission test this crate applies to any new guard: *does the failure it
//! prevents go unnoticed?*
//!
//! ## Only successful calls
//!
//! Claude Code sends a failed tool call to `PostToolUseFailure`, a separate
//! event this crate ignores. So everything reaching here claimed to work, and
//! an assertion's whole job is to ask whether the claim holds.
//!
//! ## The same two halves as a rule, on the other side of execution
//!
//! [`Assertion::examine`] is pure — it reads the command string and nothing
//! else, which is what lets the backtester replay it against a transcript.
//! [`Assertion::verify`] is the one place the world may be consulted, mirroring
//! `Rule::confirm`. It is never replayed: the world has moved since those
//! commands ran, so a replayed verdict would describe today rather than then.
//!
//! ## An assertion cannot refuse
//!
//! The tool already ran; there is nothing left to deny. A `deny` stance on an
//! assertion therefore speaks exactly like `advise`, the same way it does at a
//! session opening. What an assertion can do is put a FACT in front of the
//! model — "the remote is still on abc123" — which is a thing it cannot skim
//! past, unlike advice.
//!
//! ## Failing to establish something is silence
//!
//! A refspec this crate cannot read unambiguously, a remote that needs a
//! password, a deadline, a `cwd` that no longer exists: all [`Verdict::Unknown`],
//! all silent. An assertion that guessed would teach the model to ignore the
//! channel, and the channel is the entire value.

use std::ops::Range;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::rules::{Context, Evidence, Stance};
use crate::shell::Parsed;

pub mod push_landed;

/// What the command said it did. Produced by a pure `examine`.
pub struct Claim {
    /// A short label for the journal: what is about to be checked. Written by
    /// every assertion, read once the journal grows a per-claim column.
    #[allow(dead_code)]
    pub what: String,
    /// Byte range within the original command, so the journal excerpt is
    /// centred on the claim rather than the head of a 2 KB script.
    pub span: Range<usize>,
}

pub enum Verdict {
    /// The claim holds. Silence.
    Held,
    /// It does not, and here is the fact plus what to do about it.
    Broken { reason: String, remedy: String },
    /// Could not establish either way. Silence, with a reason for the journal.
    Unknown(&'static str),
}

pub struct Assertion {
    pub id: &'static str,
    pub default_stance: Stance,
    pub evidence: Evidence,
    /// PURE. No processes, no filesystem, no network.
    pub examine: fn(&Parsed) -> Option<Claim>,
    /// Runs only after `examine` fired, and only for a call that succeeded.
    pub verify: fn(&Context, &Claim) -> Verdict,
}

pub const ASSERTIONS: &[Assertion] = &[push_landed::ASSERTION];

/// Consumed by `explain` and `graduate` once this tier has a measured rate;
/// kept beside `ASSERTIONS` so the lookup has one spelling from the start.
#[allow(dead_code)]
pub fn by_id(id: &str) -> Option<&'static Assertion> {
    ASSERTIONS.iter().find(|a| a.id == id)
}

/// Every assertion whose claim is present in this command.
///
/// Panic-isolated per assertion, like `rules::examine_all`: one bad assertion
/// must not take the others with it.
pub fn examine_all(parsed: &Parsed) -> Vec<(&'static Assertion, Claim)> {
    let mut out = Vec::new();
    for assertion in ASSERTIONS {
        let found =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (assertion.examine)(parsed)));
        match found {
            Ok(Some(claim)) => out.push((assertion, claim)),
            Ok(None) => {}
            Err(_) => eprintln!("amont-agent: assertion {} panicked", assertion.id),
        }
    }
    out
}

/// How long a single `verify` may take before it is abandoned.
///
/// A hook runs with no controlling terminal. A network round trip is fine; a
/// prompt nobody can answer is not, and neither is a remote that hangs. This
/// bounds the pathological case, and the environment below prevents the common
/// one.
const DEADLINE: Duration = Duration::from_secs(5);

/// Run a command that must never block on a human.
///
/// Hooks have no `/dev/tty`, so a credential prompt does not fail — it HANGS,
/// and it hangs the session, not just this process. Every one of these
/// variables closes one door: git's own prompt, the askpass helper it falls
/// back to, ssh's password prompt, and ssh's host-key question. Without them a
/// guard installed to make pushing safer would occasionally make it impossible.
pub fn read_briefly(dir: &std::path::Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/false")
        .env("SSH_ASKPASS", "/bin/false")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -oBatchMode=yes -oStrictHostKeyChecking=accept-new -oConnectTimeout=4",
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            Ok(None) => {
                if started.elapsed() >= DEADLINE {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read;
        if stdout.read_to_string(&mut out).is_err() {
            return None;
        }
    }
    Some(out.trim().to_string())
}
