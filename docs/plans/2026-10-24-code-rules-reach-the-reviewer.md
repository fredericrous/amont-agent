---
canonical: decisions:docs/plans/2026-10-24-code-rules-reach-the-reviewer.md
phases: [2, 4]
status: active
---
# Code rules reach the reviewer (pointer)

Canonical plan: [decisions/docs/plans/2026-10-24-code-rules-reach-the-reviewer.md](https://github.com/fredericrous/decisions/blob/main/docs/plans/2026-10-24-code-rules-reach-the-reviewer.md) (ADR-0044).

Phase 2 (behaviour 1) shipped in amont-agent 3.2.0 (#142, #143).

This branch carries Phase 4's amont-agent part, behaviour 3:
- `review-brief` embeds the candidates of adr-lint 0.2.0 (`adr-lint.candidates/1`) as an inline `<<<CANDIDATES …>>>` block;
- the implementation-review push check reads the reviewer's `Candidates:` rulings, journals undisposed ones without holding the push, and writes each ruling as an `adr-lint.ruling/1` record.

It also carries Phase 2's leftovers:
- `review-brief` in a repository with no `.adr.yaml`;
- a `keys` row without a string `key`.
