---
canonical: decisions:docs/plans/2026-11-02-preview-stale-base-unchanged-screen.md
phases: [1]
status: active
---
# The preview guard ignores a stale remote branch, and an unchanged screen needs no approval (pointer)

Canonical plan: [decisions/docs/plans/2026-11-02-preview-stale-base-unchanged-screen.md](https://github.com/fredericrous/decisions/blob/main/docs/plans/2026-11-02-preview-stale-base-unchanged-screen.md).

This repository carries Phase 1:
- `published()` no longer diffs from a remote-tracking ref already in the remote's default branch;
- `bind()` approves a byte-identical, unambiguous before/after pair as `unchanged`;
- release 3.15.0.

## Code review

- amont-agent: backend, lang:rust, unix (the register JSON gains a field)
