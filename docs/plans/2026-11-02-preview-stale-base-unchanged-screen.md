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

## Follow-up: a squash-merged stale ref (3.15.1)

- The Phase 3 live check failed under 3.15.0. duro-app's push was still held, because its stale `origin/chore/refresh-fleet-pack` (4f80f4e) was squash-merged: that commit is in neither main nor the new branch, so 3.15.0's "already in main" condition did not fire.
- The fix: the tracking ref stays the base only for a branch whose remote commits are its own (the pushed commit descends from it and main does not contain it). Any other tracking ref uses the merge-base with main.
- A rewritten branch is now diffed from main: it is judged by every file it carries over main, so nothing it adds can hide behind its old ref. Recorded in the canonical plan's Decision log at close.
- Without `refs/remotes/<remote>/HEAD` the tracking ref is kept, journalled `base=tracking reason=no-default` with `git remote set-head <remote> -a`, checked first so a merged ref is journalled too. A default branch that shares no history with the push is journalled as that, without the set-head hint.
- Observed on the final tree: `cargo test -p amont-agent --test preview` → 76 passed, 5 new (squash, UI over squash, force-pushed, no remote HEAD, no shared history); clippy clean.
  - With the condition line set back to 3.15.0's `if !(descends && contains(&tracking, &default))`, the squash, force-pushed and no-shared-history tests FAILED.
  - With the `rev-parse --verify` check of `<remote>/HEAD` removed, the no-remote-HEAD test FAILED.
  - The file was restored and compared with cmp after each mutation.
