//! `forge-merge-by-hand` — merging a pull request by POSTing to the forge API.
//!
//! `POST /repos/{owner}/{repo}/pulls/{n}/merge` merges. It does not look at
//! check runs, and it answers `200` identically whether the run concluded
//! success, concluded failure, or has not started. The forge's own web UI
//! greys the button out; the API has no such courtesy, and neither does a
//! shell.
//!
//! `gh pr merge` at least prints the check state it saw. A `curl` to the merge
//! endpoint prints an HTTP code, so the one fact that decides whether the
//! merge was safe never enters the transcript at all.
//!
//! ## What makes this a rule rather than advice in a prompt
//!
//! The failure is silent, which is this crate's admission test. A merge onto
//! red is indistinguishable at the call site from a merge onto green — same
//! request, same `200`, same one-line result — so no correcting loop can form
//! from the outcome. It is found later, in `main`, by someone else.
//!
//! It is also the shape a model reaches for once it has improvised the
//! procedure by hand: the poll loop and the merge are both `curl`, the pair
//! works, and every later merge in the session repeats it rather than
//! re-reading the instruction that named a skill for exactly this.
//!
//! ## Measured 2026-09-10, and it is getting worse
//!
//! 221 matches in 34,905 Bash calls across five weeks, per 1,000 calls:
//!
//! ```text
//! 2026-08-10   0.3
//! 2026-08-17   4.7
//! 2026-08-24   4.9
//! 2026-08-31  13.8
//! 2026-09-07   7.1
//! ```
//!
//! That is the opposite of the shapes this crate deliberately does NOT guard.
//! `--no-verify` fell 25.9 → 5.4 and `git add -A` 42.5 → 5.4 once they had
//! consequences a loop could see; both were left alone for it. This one starts
//! near zero and climbs roughly twentyfold, because improvising the procedure
//! WORKS: the merge succeeds, the session continues, and the shape is repeated
//! for every later merge rather than the instruction naming a skill being
//! re-read. Nothing in the outcome argues against it.
//!
//! Ships at `Advise` rather than `Observe` for that reason. The ladder's first
//! rung exists to get a baseline before a rule speaks, and the backtester has
//! now supplied one retroactively — which is what it is for. `Advise` is the
//! untried intervention: this has never once been said out loud at the moment
//! it happened, only written down somewhere read hours earlier.
//!
//! Not `Deny`. Merging through the API is legitimate when the checks really
//! were read first, and a rule that cannot tell the two apart must not be the
//! one to refuse.
//!
//! ## No `confirm`, for `gh-pr-merge-auto`'s reason
//!
//! The question worth asking — did the checks for this head SHA conclude
//! successfully? — is a network round-trip to a forge that may be behind mTLS
//! and may be slow. This runs before every shell command the model issues, and
//! a hook that can hang is worse than a hook that is occasionally imprecise.
//! The reason states the condition; the reader settles it.
//!
//! ## Deliberately not `gh pr merge`
//!
//! That command is the LAST STEP of the documented procedure, so firing on it
//! would fire on correct use. Only the raw endpoint is matched, because
//! reaching for the endpoint is itself the evidence that the procedure was
//! skipped — there is no other reason to hand-write it.

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "forge-merge-by-hand",
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        // p95 of the weekly rate, not the mean: the worst week is what a guard
        // has to hold up in.
        per_1000: 13.8,
        measured: "2026-09-10",
        // "Not improving on its own" is the claim Flat makes, and it holds
        // with room to spare — the rate rose across the five weeks measured.
        trend: Trend::Flat(5),
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.judgeable() {
        // `else { continue }`, never `?`. A `?` here would return from
        // `examine` on the FIRST clause that is not an HTTP call, and the
        // merge is rarely the first clause — the corpus case that caught this
        // opens with a bare `TOK=$(awk …)` assignment, which has no program at
        // all, so the rule went silent before it ever reached the `curl`.
        let Some((program, span)) = crate::publish_cmd::api_merge(cmd) else {
            continue;
        };
        return Some(Finding {
            reason: format!(
                "`{program}` against a pull request's merge endpoint merges immediately. \
                 The endpoint does not consult check runs — it answers 200 whether the \
                 run passed, failed, or has not started — so nothing in this command \
                 establishes that CI was green."
            ),
            remedy: "Use the merge-when-green skill: poll the checks to completion, read \
                     the conclusion as its own step, then merge. Never chain the poll and \
                     the merge into one command — the merge runs regardless of what the \
                     poll printed."
                .to_string(),
            span,
        });
    }
    None
}
