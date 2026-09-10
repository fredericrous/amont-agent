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

use crate::rules::{Evidence, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "forge-merge-by-hand",
    default_stance: Stance::Advise,
    evidence: Evidence {
        // p95 of the weekly rate, not the mean: the worst week is what a guard
        // has to hold up in.
        per_1000: 13.8,
        measured: "2026-09-10",
        // "Not improving on its own" is the claim Flat makes, and it holds
        // with room to spare — the rate rose across the five weeks measured.
        trend: Trend::Flat(5),
    },
    examine,
    confirm: None,
};

/// True for a URL path that merges a pull request on either forge:
/// `…/pulls/<digits>/merge`, with anything or nothing after it.
///
/// The digits matter. `/pulls/10` reads a pull request and `/pulls` lists
/// them; only the numbered `merge` child mutates, so requiring an index is
/// what keeps every read-only call out of this rule.
fn is_pr_merge_path(text: &str) -> bool {
    let mut rest = text;
    while let Some(i) = rest.find("/pulls/") {
        let after = &rest[i + "/pulls/".len()..];
        let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 {
            let tail = &after[digits..];
            if tail == "/merge" || tail.starts_with("/merge?") || tail.starts_with("/merge/") {
                return true;
            }
        }
        rest = after;
    }
    false
}

/// The HTTP clients a shell reaches for. `gh api` is included: it is the same
/// raw endpoint with authentication attached, and skips the same checks.
fn is_http_client(program: &str) -> bool {
    matches!(
        program,
        "curl" | "wget" | "http" | "https" | "httpie" | "xh"
    )
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.judgeable() {
        // `else { continue }`, never `?`. A `?` here would return from
        // `examine` on the FIRST clause that is not an HTTP call, and the
        // merge is rarely the first clause — the corpus case that caught this
        // opens with a bare `TOK=$(awk …)` assignment, which has no program at
        // all, so the rule went silent before it ever reached the `curl`.
        let Some(program) = cmd.program() else {
            continue;
        };
        let raw_client = is_http_client(program);
        let gh_api = program == "gh" && cmd.subcommand() == Some("api");
        if !raw_client && !gh_api {
            continue;
        }
        // Scan every argument, quoted included: a URL in quotes is still a
        // URL. (`has_flag` skips quoted words because a quoted word is never a
        // flag; that reasoning does not extend to an operand.)
        let Some(hit) = cmd.args().iter().find(|w| is_pr_merge_path(&w.text)) else {
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
            span: hit.at..cmd.end,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::is_pr_merge_path;

    #[test]
    fn matches_a_numbered_merge_child() {
        assert!(is_pr_merge_path(
            "https://git.daddyshome.fr/api/v1/repos/fredericrous/sre-agent/pulls/10/merge"
        ));
        assert!(is_pr_merge_path(
            "https://api.github.com/repos/o/r/pulls/1234/merge?foo=1"
        ));
    }

    #[test]
    fn ignores_reads_and_unnumbered_paths() {
        // Reading a pull request, listing them, and checking mergeability are
        // all the same prefix — the index plus `/merge` is the whole signal.
        assert!(!is_pr_merge_path(
            "https://git.daddyshome.fr/api/v1/repos/o/r/pulls/10"
        ));
        assert!(!is_pr_merge_path(
            "https://git.daddyshome.fr/api/v1/repos/o/r/pulls"
        ));
        assert!(!is_pr_merge_path(
            "https://git.daddyshome.fr/api/v1/repos/o/r/pulls/merge"
        ));
        assert!(!is_pr_merge_path("https://example.com/merge"));
    }
}
