---
canonical: decisions:docs/plans/2026-11-02-preview-stale-base-unchanged-screen.md
phases: [1]
status: done
---
# The preview guard ignores a stale remote branch, and an unchanged screen needs no approval (pointer)

Canonical plan: [decisions/docs/plans/2026-11-02-preview-stale-base-unchanged-screen.md](https://github.com/fredericrous/decisions/blob/main/docs/plans/2026-11-02-preview-stale-base-unchanged-screen.md).

This repository carries Phase 1:
- `published()` no longer diffs from a remote-tracking ref already in the remote's default branch;
- `bind()` approves a byte-identical, unambiguous before/after pair as `unchanged`;
- release 3.15.0.

## Code review

- amont-agent: backend, lang:rust, unix (the register JSON gains a field)

## Verification

- Phase 1, observed on the reviewed tree:
  - `cargo test -p amont-agent --test preview` → 71 passed, 8 of them new;
  - `cargo test -p amont-agent-process` → 167 passed, 4 of them new in guide.rs;
  - clippy clean; `cargo test --workspace` → 33 binaries, 1494 passed, 0 failed (before the review fixes).
- Mutations, each turning its test red, files restored:
  - base rule reverted;
  - ancestry through `git()`;
  - bytes forced equal;
  - mockup exclusion removed;
  - path-differs removed;
  - `sole_pair` first-match;
  - bind trusting the printed flag.
- Pack refreshed: `aval resolve work.preview-approval` → active ADR-0057.

## Implementation review

- **Verdicts:** approve on 0a8628c after two Deltas: implementation-review, backend and lang:rust. unix approved 6e31dfc.
- **Fixed:**
  - names (`scratch`, the byte buffers, `merge_base`);
  - a render test that can fail;
  - bind checks the guide it compares (beside its page, outside the repo, within MAX_BYTES), journalled `unchanged-skipped`;
  - hard links refused (`distinct_files`);
  - a typed `Unchanged`;
  - stderr wording that leaves the decision to the hook;
  - a test for the guide-path check.
- **Deviations, for the canonical record:**
  - the journal writes `base=merge-base` only when the merge base is chosen;
  - the own-commits test asserts NOT advised, since already-pushed UI is not asked again.
- **Follow-up:** `approve()`'s unlocked read-modify-write predates this change.
