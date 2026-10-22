---
status: done
branch: feat/review-rounds-record-themselves
repos: amont-agent
adrs: [ADR-0042]
---
# Plan: plan-review rounds record themselves

## Review panel

👉 **Decide:** the retire bar counts accepted plus unresolved findings, and old rows are not backfilled (the 20-review bar restarts from this release).
📍 amont-agent · plan ready · next: parse findings in `record`. Panel: backend (four launches: two rounds, then two fresh checks on the final body). ⚠ The tool computed a delta panel (plan file path reused), so the language and CLI seats did not run.
**Changed by review:** `applied_labels` (not reusing `applied`), matching on body sha; dispositions derived when stats are read, so a later plan-apply still counts; resumed rounds recorded (dedupe on agent id and body sha); a disposition row that wins over derived values; the hook budget and lock bound.
**Verdicts:** round 1 approve-with-changes (7 accepted); round 2 approve-with-changes. Final check: approve. Carried into implementation: lock taken with LOCK_NB and retries, plus a way to catch up a skipped final approval; a disposition-row test and a lock test (list them in Verification); one escaped `<p>` and one repeated "dedupe" in the text.

## Context

ADR-0042 judges a reviewer seat by its accepted high or blocking findings
per review (`plan-review stats`). The first harness review (2026-10-22)
could not trust those numbers: `lang:rust` on Opus showed 19 reviews and no
findings, although it raised blocking findings on the bug-nudge plan.

The cause, measured in amont-agent:
- `plan-review record` writes one row per completed reviewer, with its
  verdict parsed from the result text (`review_outcomes.rs:269`), but its
  `findings` come only from JSON the orchestrator passes on stdin
  (`parse_findings`, `:121`). Without stdin a row has `findings: []`.
- Nothing parses a reviewer's own `Findings:` list (`1. [high] … Edit: …`),
  which every seat's output contract prescribes.
- `plan-apply` writes a merge row with counts and an `unmatched` list, but
  not which findings it applied (`plan_apply.rs:722-735`).
- Recording is a step the orchestrator must remember. On 2026-10-21 it was
  done for one seat of four, and on the bug-nudge plan not at all.

The person chose automatic recording, planned.

## Approach

1. **Parse each reviewer's findings.** A `findings_of(result)` in
   `review_outcomes.rs` reads the numbered lines under `Findings:` and
   returns `(number, severity)` for each `N. [blocking|high|medium|low]`.
   An entry that does not match is skipped, never guessed, and its count is stored as the row's `findings_unparsed`; `stats` lists seats whose verdict is approve-with-changes or rework but which have 0 parsed findings.
2. **plan-apply records which findings it applied.** Its merge row
   (`amont-agent-review-merge-v1`) gains an `applied_labels` list of `role#n`
   labels next to `unmatched`; the existing `applied` count (`plan_apply.rs:732`) keeps its type. This is an additive field.
3. **`record` fills every row's findings.** For each reviewer row:
   findings passed on stdin for that role are used as they are, since an
   explicit disposition wins. Otherwise its parsed findings are written
   as `(n, severity)` only, and `stats` derives at read time (so a `plan-apply` run after the hook still counts) a disposition: `accepted` when a merge row whose `body_sha_before` equals this row's `body_sha`
   lists `role#n` in `applied_labels`, else `unresolved`. `unresolved` is a new
   disposition value; `rejected` and `decision` still come only from stdin.
4. **The hook records on its own.** `record` reads `launches_resumable` (as `plan-apply` does) and dedupes on `(agent_id, body_sha)`, so a round resumed by SendMessage gets its own row. PostToolUse on ExitPlanMode
   (`PostPlanExit`, `src/hook.rs:100-107`) runs `record` for the plan after
   the person answers, so a late delta round is included. `PlanAnswered`
   (`payload.rs:205-219`) gains the transcript path. The hook never fails
   the call: an error is journalled, and `record`'s lock and `(agent_id, body_sha)` dedupe
   make a repeat run safe. A manual `record` with stdin after the hook appends a disposition row keyed by `agent_id` and exempt from that dedupe; for a `role#n` a disposition row wins over the merge-derived value, and `stats` takes the latest disposition row. The hook runs on approval and on rejection alike. Budget: `record` takes
   under 1 s on the largest transcript of the last week, measured before the
   release. The hook waits at most 2 s for the ledger lock (`record` itself
   blocks with no timeout, `review_outcomes.rs:313`); if it cannot get the
   lock in time, it journals the skip and the next run catches up.
5. **Stats count what is unresolved.** `plan-review stats` shows
   unresolved findings beside accepted ones. A seat is a retire candidate
   only when its accepted plus unresolved high-or-blocking findings per
   review stay under 0.05 over 20 reviews. An unresolved finding is not
   evidence that a seat caught nothing, so this is stricter than accepted
   alone and keeps ADR-0042's bar.

No backfill: the ledger is append-only, and old rows keep `findings: []`.
The 20-review bar is counted from rows recorded after this change. `stats`
says how many reviews per seat predate it.

## Decisions for the person

1. The retire bar counts accepted plus unresolved (proposed), not
   accepted alone.
2. No backfill of old rows (proposed); the bar restarts from this release.

## Verification

- Unit tests for `findings_of`: the backend contract's example parses into
  numbers and severities; a malformed line is skipped and counted; `None.`
  gives no findings.
- A `record` test over a fixture transcript with two reviewers: rows hold `(n, severity)` for rust#1 and rust#2 with no disposition, and the stdin findings for the other role are kept as given. A `stats` test over the same fixture with a merge row applying `rust#1`: rust#1 is `accepted`, rust#2 `unresolved`.
- A hook test: a PostToolUse ExitPlanMode payload with a transcript writes
  the rows; a second identical payload writes nothing; an unreadable
  transcript journals and returns silently.
- A `plan-apply` test: the merge row lists the applied labels.
- A `record` test with a fresh round 1 and a SendMessage-resumed round 2 of one agent: two rows, each with its own body_sha and findings. A resumed round on an unchanged body (same sha) writes no second row.
- `plan-review stats` on a fixture where the merge row is appended after the review rows: the applied finding shows as accepted; unresolved shown; a seat with only
  unresolved serious findings is not a retire candidate.
- `cargo test --workspace` → exit 0, new tests listed by name; `cargo clippy --workspace -- -D warnings` → exit 0.
- Hook cost: `time amont-agent plan-review record --plan <p> --transcript <largest of last week>` → under 1 s; record the measured value.
- After release: for the next plan reviewed, `jq` over outcomes.jsonl shows one row per launched seat, each `findings` count equal to that reviewer's numbered list, `findings_unparsed` 0, and no manual `record` in the transcript.

## Observed

| check | input | expected | observed |
|---|---|---|---|
| tests named in Verification | `cargo test --workspace` | pass | pass, incl. `a_disposition_row_after_the_hook_wins_over_the_derived_value`, `a_held_ledger_lock_makes_the_hook_journal_one_skip_within_the_budget`, `an_unreadable_transcript_is_journalled_and_the_hook_stays_silent` |
| clippy, Windows target | `-D warnings`; `--target x86_64-pc-windows-msvc` | pass | pass |
| dedupe key | drop `body_sha` from the key | the changed-body resumed test fails | exactly it fails |
| read-time derivation | disable the `applied_labels` lookup | the appended-merge test fails | exactly it fails |
| cost | `record` on this session's 14 MB transcript | under 1 s | 0.19 s, 17 rows |
| cost | `record` on the week's largest transcript (91 MB) | under 1 s | 0.19 s |
| parse | the same 17 rows | `findings_unparsed` 0 | 2 before the bold-after-number fix, 0 after |
| hook timeout | installed `ExitPlanMode` PostToolUse | above 2 s + record | 10 s |

## Implementation review

- approve-with-changes. Fixed in this closing commit: the plan's escaped `<p>` and repeated "dedupe", the three tests listed by name, the observed values. Deliberate: no test for an unwritable ledger (the path journals and returns; the reviewer read it as correct), and `record`'s two `eprintln!` notes stay (stdout is empty and the exit code is 0).

<!-- panel: repos=amont-agent reviewers=backend body-sha=b63300f7be2e -->
