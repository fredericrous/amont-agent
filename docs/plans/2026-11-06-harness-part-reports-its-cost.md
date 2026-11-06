---
canonical: decisions:docs/plans/2026-11-06-harness-part-reports-its-cost.md
phases: [1]
status: done
branch: feat/seat-cost-columns
---
# A harness part reports its cost beside its catches

Pointer. The plan and its review panel live in decisions. This repository
carries Phase 1: `plan-review stats` puts each reviewer seat's tokens and
seconds beside its findings (`ktok/review`, `s/review`, `ktok/serious`).
Phase 2 (guard rules' cost per firing) is a later branch here, gated on a
hand count. Status `done` is for this slice only.

## Code review

- amont-agent: backend, lang:rust, unix, tui (architect left out: the new walker and `cost.rs` stay inside the guard crate, no new crate boundary; po left out: report fields only)

## Verification

Input → expected → actual, observed before the push.

- Unit, `review_outcomes.rs::a_seats_cost_sums_only_the_rows_that_carry_it`: four plan-review rows for one seat (tokens 30k, 50k, none, none; seconds 100, none, 51.5, 48.5) → `ktok/review=40 s/review=66 ktok/serious=40 cost-missing=2`, JSON `tokens` 80000, `seconds` 200.0, `seconds_rows` 3 → pass. Dividing seconds by the token rows (2) would print 100, so the divisor is pinned.
- Unit, no serious finding → `ktok/serious=-` and `tokens_per_serious` null → pass. Unit, an implementation-review row → `ktok/review=- s/review=- ktok/serious=- cost-missing=1` → pass.
- Integration, `tests/outcomes_stats.rs::stats_shows_a_seats_cost_from_the_rows_that_carry_it`, through the real binary on a ledger written the way `record` writes it → `ktok/review=40 s/review=16 ktok/serious=80`, JSON `seconds_rows` 2 → pass. The three older stats tests pin the new fields on rows with no cost → pass.
- Real ledger, a copy taken 2026-11-06 (334 backend Opus rows), branch binary against jq on the same copy:
  - backend Opus: `ktok/review=36` (jq 36), `s/review=61` (jq 61).
  - backend Opus `ktok/serious=33`: 12,244,698 tokens over the binary's 365 serious findings (accepted 46+314, unresolved 0+5). jq counting only explicitly accepted findings (343) gives 35, which is the figure the plan expected. The code follows `serious/review`, which counts unresolved findings too.
  - platform: 42 ktok, 86 s, 23 ktok/serious. lang:python (14 reviews): 34 ktok, 52 s, **36 ktok/serious**. The 222 ktok/serious reported before the build came from a jq that counted only explicitly accepted findings; with unresolved findings counted, lang:python is not an outlier.
- The commit gate (26 checks) passed on each of the four commits. A first fix commit was refused for unformatted Rust and made again after `cargo fmt`.

## Implementation review

- implementation-review: approve-with-changes, then approve on the third tree (54k, 54 s; 64k, 38 s; 66k, 14 s). Fixed: ktok/serious evidence, `seconds_rows`, weak test names, fourth fixture row.
- tui: approve-with-changes, then approve (56k, 21 s; 61k, 10 s). Fixed: CHANGELOG denominators, help text. Deliberate: the dash columns always print (the plan fixed it, JSON unchanged).
- backend: approve-with-changes, then approve (63k, 47 s; 71k, 24 s). Fixed: `seconds_rows`, an integration test with cost. Deliberate: a malformed `tokens` counts as missing, the single writer stores an integer.
- lang:rust: approve-with-changes, then approve (63k, 39 s; 71k, 28 s). Fixed: integer division, a named `SERIOUS` range, `map_or_else`, `seconds_rows`.
- unix: approve-with-changes, then approve (45k, 38 s; 55k, 28 s). Fixed: `seconds_rows` in the JSON, the tests and the CHANGELOG.

## Decision log

- 2026-11-06: the relais run for this phase (run-65f9fd9da1188-11845) failed on `same_failure_recurrence`. The write scope I gave left out `tests/outcomes_stats.rs`, which pins the old stats rows. The candidate was applied by hand, the six pinned rows were updated, and the run is recorded `salvaged` at afedfb7.
- 2026-11-06: main released 3.20.0 while this branch was in review. The rebase had no conflict but placed the CHANGELOG bullet under `## v3.20.0`, already tagged; it was moved back under Unreleased in its own commit. The tests were re-run on the rebased tree (outcomes_stats and review_outcomes pass).
- 2026-11-06: the reviewers' two non-blocking asks that the plan had already settled stayed deliberate: always-printed columns, and no separate counter for a malformed `tokens`.
