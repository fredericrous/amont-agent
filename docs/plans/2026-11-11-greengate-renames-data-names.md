---
canonical: greengate:docs/plans/2026-11-11-greengate-renames-data-names.md
phases: [1, 5, 6]
status: active
---
Part of [greengate 4.0 renames its data names](https://github.com/greenharness/greengate/blob/main/docs/plans/2026-11-11-greengate-renames-data-names.md): this repository takes Phase 1, greenpiste reading greengate 4.0's names beside the old ones; Phase 5 (`GREENHARNESS_HOST_SLOT` in the install window) and Phase 6 (the census) come on later branches.

## Code review
- amont-agent: backend, lang:rust

## Verification

- Phase 1: greenpiste tests read each new name and its old one; released; installed here.
