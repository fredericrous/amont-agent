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
//! ## What `examine` can see, and what `confirm` decides
//!
//! Whether the call runs in the background is a sibling field of the payload;
//! `examine` sees only the command. So `examine` keys on the shape a
//! backgrounded push takes in the text — its output sent to a file to read
//! afterwards (`git push … > push.log 2>&1`), which every such push in the
//! 2026-10-08 transcripts had — and `confirm` declines when the payload says
//! the call runs in the foreground. An ordinary `git push origin main` stays
//! silent in `check`, the backtester and the precision corpus. A background
//! push with its output left to the task's own log is missed: a recall cost,
//! said here rather than hidden.

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
    // `2>&1` is an fd duplicate (empty target); `> push.log` is a file.
    let to_a_file = cmd
        .redirects
        .iter()
        .any(|(op, target)| op.contains('>') && !target.raw.is_empty());
    if !to_a_file {
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
    fn an_unwrapped_push_to_a_log_is_examined() {
        assert!(fires("git push origin dev > /tmp/p.log 2>&1"));
        // Not `&> file`: the lexer reads that `&` as a background operator,
        // so the redirect never reaches the push clause.
        assert!(fires(
            "cd repo && git push -u origin feat/x > /tmp/p.log 2>&1"
        ));
        assert!(fires("nice git push origin dev >> push.log"));
    }

    #[test]
    fn a_push_whose_output_is_not_kept_is_not() {
        assert!(!fires("git push origin main"));
        assert!(!fires("git push origin main 2>&1"));
    }

    #[test]
    fn a_timeout_or_a_dry_run_is_not() {
        assert!(!fires("timeout 1800 git push origin dev > p.log 2>&1"));
        assert!(!fires("timeout -k 30 1800 git push origin dev"));
        assert!(!fires("timeout --signal=TERM 900 git push origin dev"));
        assert!(!fires("git push --dry-run origin dev > p.log"));
        assert!(!fires("git push -n origin dev > p.log"));
        assert!(!fires("git status"));
    }

    #[test]
    fn timeout_as_an_argument_is_not_a_wrapper() {
        assert!(fires("git push origin timeout > p.log"));
    }
}
