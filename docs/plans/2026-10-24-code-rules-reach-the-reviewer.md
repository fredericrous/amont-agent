---
canonical: decisions:docs/plans/2026-10-24-code-rules-reach-the-reviewer.md
phases: [2]
status: active
---
# Code rules reach the reviewer (pointer)

Canonical plan: [decisions/docs/plans/2026-10-24-code-rules-reach-the-reviewer.md](https://github.com/fredericrous/decisions/blob/main/docs/plans/2026-10-24-code-rules-reach-the-reviewer.md) (ADR-0044).

This repository carries Phase 2, behaviour 1:
- `amont-agent review-brief` builds the implementation-review brief, the code rules in full included, behind a `<<<RULES sha=…>>>` line.
- `amont-agent rules-sha` hashes the same `aval` output.
- The implementation-review push check journals `review=rules-missing` and `review=rules-stale` in advise mode.

Phases 4 and 5 (candidates in the brief, the write-time hook) come in later PRs.
