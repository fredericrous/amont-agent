---
canonical: decisions:docs/plans/2026-10-24-code-rules-reach-the-reviewer.md
phases: [2]
status: done
---
# Code rules reach the reviewer (pointer)

Canonical plan: [decisions/docs/plans/2026-10-24-code-rules-reach-the-reviewer.md](https://github.com/fredericrous/decisions/blob/main/docs/plans/2026-10-24-code-rules-reach-the-reviewer.md) (ADR-0044).

This repository carries Phase 2, behaviour 1:
- `amont-agent review-brief` builds the implementation-review brief, the code rules in full included, behind a `<<<RULES sha=…>>>` line.
- `amont-agent rules-sha` hashes the same `aval` output.
- The implementation-review push check journals one `rules-checked` line per passing push in advise mode: `review=rules-current`, `rules-missing`, `rules-stale` or `rules-unknown`.

Phases 4 and 5 (candidates in the brief, the write-time hook) come in later PRs.

## Verification

Behaviour 1 ships journal-only here, which this codebase calls `observe`: a missing or stale rules line is journalled, the agent is not told, and the push passes. The canonical plan says `advise`; its wording is corrected in the decisions PR that records this phase. The canonical plan's entry "refused, and the message names `amont-agent review-brief`" applies once phase 8 switches the reasons to `deny`.

- `amont-agent rules-sha` in this worktree → equal to `aval --json rules --level constraint --governs code --full | shasum -a 256` → both `aad31427…c12d5`.
- `amont-agent review-brief --plan=<canonical plan> -C.` → exit 0; its TREE line equals `tree-sha --block`; its RULES line equals `rules-sha`; the body of `names.reveal-intent` ("No placeholder names") is in it → exit 0, both lines equal, the body is there.
- `AMONT_AGENT_AVAL=/nonexistent amont-agent rules-sha` → exit 3, one stderr line naming the command, empty stdout → exit 3, one line.
- `rules-sha --bogus --help`, `review-brief --bogus --help` → exit 0; `review-brief --nope` → exit 2 → 0, 0, 2.
- A passing push whose review prompt carries a matching, stale or no RULES line → the push passes and one `rules-checked` line names the outcome → by test, each with the journal line asserted:
  - transcript, matching line → `review=rules-current` (`a_matching_rules_line_journals_rules_current`, `a_reviewed_tree_passes_and_recording_the_review_keeps_it_reviewed`);
  - tree memory on a second push, stale line → `review=rules-stale` (`a_remembered_pass_runs_the_rules_check_on_the_second_push`);
  - patch memory after a conflict-free rebase → `review=rules-current`, the line before `unconfirmed reviewed-by-patch` (`a_conflict_free_rebase_keeps_the_review_by_patch`);
  - a pass stored before this change → `review=rules-unknown` (`a_pass_remembered_before_the_sha_was_stored_journals_rules_unknown`).
- `rules-sha aé` and `review-brief aé` (second byte inside a character) → exit 2, no panic → exit 2 (`a_usage_error_exits_2`).
- `cargo test --test implementation_review --test review_brief` with no `aval` on PATH → passes → passes (53.7 s).
- `make check` on the pushed tree → passes → passes (208 s).

## Implementation review

- approve, after a first review (approve-with-changes, 6 findings, then 4 more in its delta) and a fresh round 1 on the fixed tree (approve-with-changes, 5 findings, all resolved in its delta).
- Fixed: POSIX flag shapes and a multibyte-word panic, `rules-current` on a match, a `PromptRules` enum, aval output validated (rules, relevant `keys`, empty resolve), aval stderr in errors, hermetic tests on every platform.
- deliberate: ships journal-only (`observe`); the canonical plan's `advise` wording is corrected in the decisions PR that records this phase.
- deliberate: a `keys` row without a string `key` is still dropped silently; phase 4 rewrites this decisions builder and validates rows there.
- About 89k + 101k + 113k + 120k tokens over four review rounds.
