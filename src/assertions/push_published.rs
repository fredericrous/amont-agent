//! `push-published` — did a push to a repository with a user interface
//! actually publish, and was that commit an approved preview?
//!
//! The record the preview soak is read from (ADR-0023). A command exiting 0
//! is not a publication: "Everything up-to-date" is exit 0, and so is a push
//! of a branch nobody changed. So before the push, the hook remembers where
//! each branch destination stood (`crate::preview::record_before`, keyed by
//! `tool_use_id`); after it, this asks the remote with one `git ls-remote`
//! per destination and journals one of:
//!
//! - `published-approved` / `published-unapproved` — the remote now holds the
//!   commit and did not before, and the commit carried interface changes;
//! - `published-no-ui` — published, with nothing a person could preview;
//! - `already-present` — the remote held the commit before the push;
//! - `unverified` — the remote does not hold it, or could not be asked.
//!
//! It never speaks. It observes.

use crate::assertions::{read_briefly, Assertion, Claim, Verdict};
use crate::rules::{Context, Evidence, Stance, Trend};
use crate::shell::Parsed;

pub const ASSERTION: Assertion = Assertion {
    id: "push-published",
    default_stance: Stance::Observe,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-09-28",
        trend: Trend::Routine,
    },
    examine,
    verify,
};

fn examine(parsed: &Parsed) -> Option<Claim> {
    let cmd = crate::push_target::find(parsed)?;
    if cmd.is_dry_run() || cmd.has_short('n') {
        return None;
    }
    Some(Claim {
        what: "published".to_string(),
        span: cmd.at..cmd.end,
    })
}

fn verify(ctx: &Context, _claim: &Claim) -> Verdict {
    let recorded = crate::preview::take_before(ctx.tool_use_id);
    if recorded.is_empty() {
        return Verdict::Unknown("not a push to a repository with a user interface");
    }
    // The destination a person would care about first: a UI change.
    let mut outcome = "unverified";
    for b in recorded.iter() {
        let answer = read_briefly(&b.repo, &["ls-remote", "--", &b.remote, &b.dst]);
        let remote = answer
            .as_deref()
            .and_then(|a| a.split_whitespace().next())
            .unwrap_or_default();
        let this = if remote != b.src {
            "unverified"
        } else if b.before.as_deref() == Some(b.src.as_str()) {
            "already-present"
        } else if !b.ui {
            "published-no-ui"
        } else if b.approved {
            "published-approved"
        } else {
            "published-unapproved"
        };
        if rank(this) > rank(outcome) {
            outcome = this;
        }
    }
    Verdict::Noted(outcome)
}

fn rank(outcome: &str) -> u8 {
    match outcome {
        "published-unapproved" => 5,
        "published-approved" => 4,
        "published-no-ui" => 3,
        "already-present" => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    #[test]
    fn a_push_is_a_claim_and_a_dry_run_is_not() {
        assert!(examine(&lex("git push -u origin feat/x")).is_some());
        assert!(examine(&lex("git push --dry-run origin feat/x")).is_none());
        assert!(examine(&lex("git status")).is_none());
    }

    #[test]
    fn an_unapproved_publication_outranks_everything() {
        assert!(rank("published-unapproved") > rank("published-approved"));
        assert!(rank("published-approved") > rank("already-present"));
        assert!(rank("already-present") > rank("unverified"));
    }
}
