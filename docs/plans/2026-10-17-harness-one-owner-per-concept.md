---
canonical: decisions:docs/plans/2026-10-17-harness-one-owner-per-concept.md
phases: [2]
status: done
---
# The harness has one owner per concept (pointer)

The plan lives in the decisions repository, which owns ADR-0039. This
repository carries Phase 2's amont-agent slice: the active-entry writer in
`src/quiet.rs` names amont's `docs/quiet-runs.md` as its contract, and a
`holds-until:` comment records the one known breach of
`harness.shared-code`, the copy of amont-runtime's `active` module.

## Outcome

| Check | Expected | Actual |
|---|---|---|
| `git grep -n 'quiet-runs.md' -- ':!CHANGELOG.md' ':!docs/plans'` | only amont's page | `src/quiet.rs:270`, naming amont `docs/quiet-runs.md` |
| `holds-until:` at `active_dir` | names its limit and what ends it (ADR-0020), and lists every divergence from amont-runtime's `active.rs` | five divergences, each checked against both codes by the review: an empty `HOME`, no `%LOCALAPPDATA%` path, ESRCH-only liveness, a pid that does not fit an i32, and the dot-temp sweep |
| Code diff | comments only | 11 comment lines in `src/quiet.rs` |
| amont-agent pre-commit | pass | 25 checks passed |

## Decision log

- 2026-10-18: the fleet pack refresh rides this branch (`guidance.generated-block-rides-the-change`); `aval check` reports 41 records and no findings.

## Implementation review

approve-with-changes, then approve on the Delta. Fixed: two divergences the first list missed. Round 1 took 29k tokens and 52 s; the Delta, 31k and 21 s.
