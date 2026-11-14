//! `land-retry`: did an `amont land` push the commit the guards judged?
//!
//! A land retries once when its pre-push hook fast-forwards HEAD, and the
//! retry pushes a commit nobody judged: the implementation review and the
//! preview looked at the sha before the hook ran. A plain `git push` retry is
//! a new command and is judged again; a land's is not, so this checks after
//! the fact, from the land's own receipt (amont 1.54.0), never from the
//! tracking ref, which git moves on its own from 2.56.
//!
//! PreToolUse recorded the judged sha (`preview::record_before`, every
//! repository). The receipt written after that time for the same branch is
//! the one this call made: the shape the guards accept holds exactly one land.
//!
//! - `landed`, `unverified` or `not-observed` with another sha: said, as an
//!   unreviewed push (the last two: the remote accepted it, so possibly);
//! - `refused`, `unreachable`, `interrupted`: nothing was published;
//! - no record, no receipt, or more than one: `Unknown`, journalled as
//!   `retry-check skipped: …`, never a silent pass.
//!
//! It runs on `PostToolUseFailure` too: a land that ends `unverified` or
//! `not-observed` exits non-zero after the remote accepted the push.

use crate::assertions::{Assertion, Claim, Verdict};
use crate::rules::{Context, Evidence, Stance, Trend};
use crate::shell::Parsed;

pub const ASSERTION: Assertion = Assertion {
    id: "land-retry",
    prevents:
        "A land retry pushing a commit that the implementation review and preview never judged",
    irreversible: false,
    default_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-10-14",
        trend: Trend::Rare,
    },
    examine,
    verify,
};

fn examine(parsed: &Parsed) -> Option<Claim> {
    let cmd = parsed.judgeable().find(|c| crate::land::is_land(c))?;
    if cmd.is_dry_run() {
        return None;
    }
    Some(Claim {
        what: "landed what was judged".to_string(),
        span: cmd.at..cmd.end,
    })
}

fn verify(ctx: &Context, _claim: &Claim) -> Verdict {
    let Some(rec) = crate::push_record::land_judged(ctx.tool_use_id) else {
        return Verdict::Unknown("retry-check skipped: no judgement was recorded before the land");
    };
    let found = crate::land::receipts_since(&rec.receipt_dir, &rec.branch, rec.since);
    let receipt = match found.as_slice() {
        [one] => one,
        [] => return Verdict::Unknown("retry-check skipped: the land left no receipt"),
        _ => return Verdict::Unknown("retry-check skipped: more than one receipt for this land"),
    };
    judge(&rec.judged, &receipt.status, &receipt.pushed)
}

/// Pure: the judged sha against what the receipt says was pushed.
fn judge(judged: &str, status: &str, pushed: &str) -> Verdict {
    let published = match status {
        "landed" => true,
        "unverified" | "not-observed" => false,
        // Nothing was published.
        "refused" | "unreachable" | "interrupted" => return Verdict::Held,
        // A status a newer amont added: not a pass on a guess.
        _ => {
            return Verdict::Unknown(
                "retry-check skipped: a receipt status this version does not know",
            )
        }
    };
    if pushed == judged {
        return Verdict::Held;
    }
    let short = |s: &str| s.chars().take(7).collect::<String>();
    let how = if published {
        "pushed"
    } else {
        "possibly pushed"
    };
    Verdict::Broken {
        reason: format!(
            "This land {how} {} after its pre-push hook moved HEAD, but the guards judged {}: \
             the commit on the remote is one no review or preview has seen.",
            short(pushed),
            short(judged)
        ),
        remedy:
            "Review the commit that was pushed (worktree-task F4b on the branch as it is now), \
                 and do not merge the pull request until that review is settled."
                .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    #[test]
    fn a_land_is_claimed_and_a_dry_run_is_not() {
        assert!(examine(&lex("amont land --json")).is_some());
        assert!(examine(&lex("cd x && amont land")).is_some());
        assert!(examine(&lex("amont land --dry-run --json")).is_none());
        assert!(examine(&lex("git push origin feat")).is_none());
    }

    #[test]
    fn the_judged_sha_landing_holds() {
        assert!(matches!(judge("aaa", "landed", "aaa"), Verdict::Held));
    }

    #[test]
    fn a_retried_sha_is_said() {
        assert!(matches!(
            judge("aaa", "landed", "bbb"),
            Verdict::Broken { .. }
        ));
        match judge("aaa", "not-observed", "bbb") {
            Verdict::Broken { reason, .. } => assert!(reason.contains("possibly")),
            _ => panic!("an accepted push of another sha is said"),
        }
    }

    #[test]
    fn an_unknown_status_is_skipped_not_passed() {
        assert!(matches!(
            judge("aaa", "landed-twice", "bbb"),
            Verdict::Unknown(_)
        ));
    }

    #[test]
    fn nothing_published_is_not_said() {
        for status in ["refused", "unreachable", "interrupted"] {
            assert!(
                matches!(judge("aaa", status, "bbb"), Verdict::Held),
                "{status}"
            );
        }
    }
}
