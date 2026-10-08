//! `unbounded-background-push` — a push sent where no clock reaches it.
//!
//! `git push` runs the pre-push gate inside it: a test suite, a lint pass, an
//! audit. In the foreground the Bash tool's timeout ends the call; with
//! `run_in_background: true` nothing does, and nothing says the push is still
//! going. On 2026-10-08 a push seeding a fork ran a whole JS suite for forty
//! minutes (amont#301) and the person found a "hanging shell" before anyone
//! looked (amont-agent#86).
//!
//! The remedy is the wrapper the push guards already read through:
//! `timeout <s> git push …` ends at the deadline with a failure that says so,
//! instead of holding the session open.
//!
//! ## `confirm` reads the flag, not the command
//!
//! Whether the call runs in the background is a sibling field of the payload,
//! so `examine` sees only the command and `confirm` declines a foreground
//! push. The backtester never runs `confirm`: a replayed rate counts
//! foreground pushes too — an overcount, said here rather than hidden.

use crate::rules::{Confirmed, Context, Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "unbounded-background-push",
    // Advises: it refuses nothing, and the push it names is legitimate — only
    // unbounded.
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-10-08",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: Some(confirm),
};

fn examine(parsed: &Parsed) -> Option<Finding> {
    let cmd = crate::push_target::find(parsed)?;
    // A dry run sends nothing and runs no gate; `timeout` is the deadline.
    if cmd.is_dry_run() || cmd.has_short('n') || cmd.wrapped_by("timeout") {
        return None;
    }
    Some(Finding {
        reason: "a push in the background has no deadline: the pre-push gate runs inside it \
                 (a test suite, a lint pass), no clock ends it, and nothing says it is still \
                 going — a seeding push held a session for forty minutes this way."
            .to_string(),
        remedy: "Bound it with `timeout <seconds> git push <remote> <ref>` (push-preview and \
                 implementation-review read through `timeout`); rehearse first with \
                 `amont rehearse --wait` so the push itself skips the suite."
            .to_string(),
        span: cmd.at..cmd.end,
    })
}

fn confirm(ctx: &Context, _f: &Finding) -> Confirmed {
    if !ctx.background {
        return Confirmed::No("in the foreground the tool's own timeout bounds the push");
    }
    Confirmed::Yes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn an_unwrapped_push_is_examined() {
        assert!(fires("git push origin dev"));
        assert!(fires(
            "cd repo && git push -u origin feat/x > /tmp/p.log 2>&1"
        ));
        assert!(fires("nice git push origin dev"));
    }

    #[test]
    fn a_timeout_or_a_dry_run_is_not() {
        assert!(!fires("timeout 1800 git push origin dev"));
        assert!(!fires("timeout -k 30 1800 git push origin dev"));
        assert!(!fires("timeout --signal=TERM 900 git push origin dev"));
        assert!(!fires("git push --dry-run origin dev"));
        assert!(!fires("git push -n origin dev"));
        assert!(!fires("git status"));
    }

    #[test]
    fn timeout_as_an_argument_is_not_a_wrapper() {
        assert!(fires("git push origin timeout"));
    }
}
