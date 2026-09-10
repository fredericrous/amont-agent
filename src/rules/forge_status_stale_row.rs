//! `forge-status-stale-row` — reducing an append-only status list by position.
//!
//! `GET /repos/{owner}/{repo}/commits/{sha}/statuses` does not return the
//! current state of each check. It returns every TRANSITION, newest first, one
//! row per context per change:
//!
//! ```text
//! pending | 17:38:51 | Has started running
//! success | 17:38:51 | Has been skipped
//! pending | 17:35:56 | Waiting to run
//! pending | 17:34:56 | Blocked by required conditions
//! ```
//!
//! So it has to be deduped by `context`, and the obvious reduction — keep the
//! first row seen, since the list is newest-first — is wrong. Those top two
//! rows carry the SAME timestamp: a job that started and was skipped inside
//! one second emits both, and the API returns the `pending` one first. Keeping
//! the first row pins that check at `pending` for as long as anyone asks.
//!
//! ## Why this is a rule and not a footnote
//!
//! The failure is silent and it is shaped like patience. An `until` loop
//! written on top of that reduction never exits — not because anything is
//! broken, but because the answer it keeps reading is a row from the past. The
//! run is green, the PR is mergeable, the poll waits forever, and nothing in
//! the output looks wrong: it prints `pending`, which is exactly what a
//! not-yet-finished check prints.
//!
//! It is the mirror of [`poll_blank_verdict`](super::poll_blank_verdict),
//! which fires when a wait stops TOO EARLY because a blank was read as a
//! verdict. This one is a wait that never stops because a stale row was read
//! as the present. Same family — the poll's answer was not the answer — and
//! opposite directions, which is the reason both are worth having.
//!
//! ## Measured 2026-09-10
//!
//! 5 matches in 35,165 Bash calls across five weeks — all five on one day,
//! 0.8 per 1,000 calls in that week and nothing at all in the four before it.
//! That shape is the point: the reduction had been written into a skill the
//! day before as "keep the FIRST (newest) entry", so every later poll copied
//! it. One of them pinned a homelab deploy PR at `pending` while all 22 of its
//! checks were green.
//!
//! A defect that arrives by being written down does not trend; it appears at
//! full rate the moment the instruction lands, which is why this ships on
//! cost-of-a-miss rather than on frequency.
//!
//! ## Ships observing
//!
//! One day is not a rate. This crate's bar is a corpus, not the quality of the
//! argument, so it ships at [`Stance::Observe`] and gets promoted if the
//! backtest keeps finding it — and, just as importantly, shows how often it
//! fires on CORRECT ones. `examine` deliberately does not
//! try to recognise the FIXED reduction: every spelling of "prefer a terminal
//! status" is a different shape, and a rule that guesses at the remedy's
//! syntax goes stale the first time someone writes it differently. The remedy
//! text carries what right looks like instead.

use crate::rules::{Evidence, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "forge-status-stale-row",
    default_stance: Stance::Observe,
    evidence: Evidence {
        // 5 matches in 35,165 calls, all inside one day: 0.8/1000 that week,
        // 0.0 in the four before. Kept for the cost of a miss — a wait that
        // never ends on a PR that is already green — not for frequency.
        per_1000: 0.8,
        measured: "2026-09-10",
        trend: Trend::Rare,
    },
    examine,
    confirm: None,
};

fn is_http_client(program: &str) -> bool {
    matches!(
        program,
        "curl" | "wget" | "http" | "https" | "httpie" | "xh"
    )
}

/// `/commits/{sha}/statuses` — the append-only list. Deliberately NOT
/// `/status` (singular), which is the roll-up and carries one combined state
/// per commit, so none of this applies to it.
fn is_commit_statuses_path(text: &str) -> bool {
    let mut rest = text;
    while let Some(i) = rest.find("/commits/") {
        let after = &rest[i + "/commits/".len()..];
        // the sha segment, then the endpoint
        let seg_end = after.find('/').unwrap_or(after.len());
        let tail = &after[seg_end..];
        if seg_end > 0
            && (tail == "/statuses"
                || tail.starts_with("/statuses?")
                || tail.starts_with("/statuses/"))
        {
            return true;
        }
        rest = after;
    }
    false
}

/// Reductions that keep whichever row arrived first and never look at the one
/// already stored — the shape that cannot tell a stale `pending` from a
/// current one.
const FIRST_WINS: &[&str] = &[
    "not in seen",
    "not in best",
    "not in got",
    "not in acc",
    "setdefault(",
    "map(.[0])",
    "next(iter(",
    "head -1",
    "head -n 1",
    "head -n1",
];

/// True when a first-wins reduction appears with no alternative branch.
///
/// `if c not in seen: seen[c] = st` keeps the first row unconditionally, which
/// is the defect. `if c not in best or (best[c] not in TERMINAL and ...)` is
/// the REMEDY wearing the same opening clause — the guard has an `or`, so the
/// stored row can still be replaced. Firing on the fix would be worse than
/// staying silent, so the alternative is what separates them.
fn bare_first_wins(text: &str) -> bool {
    for m in FIRST_WINS {
        let mut from = 0usize;
        while let Some(i) = text[from..].find(m) {
            let abs = from + i;
            let line_end = text[abs..].find('\n').map_or(text.len(), |j| abs + j);
            let rest = &text[abs + m.len()..line_end];
            if !rest.contains(" or ") && !rest.contains("||") {
                return true;
            }
            from = abs + m.len();
        }
    }
    false
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.judgeable() {
        // NOT `program()`: the call is often the CONDITION of the wait it
        // breaks — `until curl … ; do sleep 20; done` — and there the program
        // is `until`. The client is what matters, wherever it sits.
        let Some(client) = cmd
            .words
            .iter()
            .find(|w| !w.quoted && is_http_client(&w.text))
            .map(|w| w.text.clone())
            .or_else(|| {
                (cmd.program() == Some("gh") && cmd.subcommand() == Some("api"))
                    .then(|| "gh api".to_string())
            })
        else {
            continue;
        };
        // Quoted included: a URL in quotes is still a URL.
        let Some(hit) = cmd.words.iter().find(|w| is_commit_statuses_path(&w.text)) else {
            continue;
        };
        // The reduction usually rides in the same command, inside the quoted
        // body of the `python3 -c` / `jq` that consumes the response.
        if !parsed
            .clauses()
            .iter()
            .flat_map(|c| c.words.iter())
            .any(|w| bare_first_wins(&w.text))
        {
            continue;
        }
        return Some(Finding {
            reason: format!(
                "`{client}` reads a commit's `/statuses` list, which is append-only — one \
                 row per context per TRANSITION, newest first — and this keeps the FIRST \
                 row seen, which reads a stale one as the present. A job that starts and \
                 is skipped inside one second emits `pending` and `success` with the SAME \
                 timestamp, and the API returns the pending row first, so that check reads \
                 `pending` forever and a wait built on it never exits."
            ),
            remedy: "Dedupe by context with a TERMINAL-wins rule, not first-wins: a row \
                     whose status is success/failure/error/skipped always replaces a \
                     pending one for that context. Count `skipped` as passing — a \
                     path-filtered job legitimately never reports success. The roll-up \
                     `/commits/{sha}/status` (singular) sidesteps this, but it will not \
                     tell you WHICH check is red."
                .to_string(),
            span: hit.at..cmd.end,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    /// The 2026-09-10 shape: a poll that pinned a green PR at `pending`.
    #[test]
    fn the_incident_shape_fires() {
        assert!(fires(
            r#"curl -sS -H "Authorization: token $TOK" "https://git.daddyshome.fr/api/v1/repos/o/r/commits/861a02d3/statuses" | python3 -c "
import sys,json
d=json.load(sys.stdin)
seen={}
for r in d:
    c=r.get('context')
    if c not in seen: seen[c]=r.get('status')
""#
        ));
    }

    /// The same defect wrapped in the wait it makes unterminating.
    #[test]
    fn the_until_loop_form_fires() {
        assert!(fires(
            r#"until curl -sS "https://forge/api/v1/repos/o/r/commits/$SHA/statuses" | python3 -c "
seen={}
for r in rows:
    if r['context'] not in seen: seen[r['context']]=r['status']
"; do sleep 20; done"#
        ));
    }

    #[test]
    fn the_jq_first_of_group_form_fires() {
        assert!(fires(
            r#"gh api "/repos/o/r/commits/abc123/statuses" | jq 'group_by(.context) | map(.[0])'"#
        ));
    }

    /// The ROLL-UP is a different endpoint with one state per commit; none of
    /// the append-only reasoning applies to it.
    #[test]
    fn the_singular_status_rollup_is_silent() {
        assert!(!fires(
            r#"curl -sS "https://forge/api/v1/repos/o/r/commits/$SHA/status" | python3 -c "
seen={}
if c not in seen: seen[c]=1
""#
        ));
    }

    /// Reading the list without reducing it by position is fine — printing it,
    /// or handing it to a human, cannot pick the wrong row.
    #[test]
    fn reading_without_a_first_wins_reduction_is_silent() {
        assert!(!fires(
            r#"curl -sS "https://forge/api/v1/repos/o/r/commits/$SHA/statuses" | jq -r '.[] | "\(.status) \(.context)"'"#
        ));
    }

    /// The REMEDY must not fire. It opens with the same `not in` clause, so
    /// the alternative branch is the only thing separating it from the defect
    /// — if this ever regresses, the rule starts arguing with its own advice.
    #[test]
    fn the_terminal_wins_fix_is_silent() {
        assert!(!fires(
            r#"curl -sS "https://forge/api/v1/repos/o/r/commits/$SHA/statuses" | python3 -c "
TERMINAL={'success','failure','error','skipped'}
best={}
for r in rows:
    c=r.get('context'); st=r.get('status')
    if c not in best or (best[c] not in TERMINAL and st in TERMINAL):
        best[c]=st
""#
        ));
    }

    /// A first-wins dedupe that has nothing to do with a status list.
    #[test]
    fn a_first_wins_reduction_elsewhere_is_silent() {
        assert!(!fires(
            r#"cat log.json | python3 -c "
seen={}
for r in rows:
    if r['k'] not in seen: seen[r['k']]=r['v']
""#
        ));
        assert!(!fires("git status"));
    }
}
