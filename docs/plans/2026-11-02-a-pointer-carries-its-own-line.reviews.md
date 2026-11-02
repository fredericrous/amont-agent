# Full reviews: a pointer carries its own line

Reference for [the plan](2026-11-02-a-pointer-carries-its-own-line.md).

- **backend** round 1 (34k, 52 s), approve-with-changes, all applied:
  - (high) plan-only is computed on its own and fails closed;
  - (medium) `pointer-line-drift`;
  - (low) plan-panel does not read pointers;
  - (medium) a live pointer check;
  - (low) name the test filter.
- **backend** round 2 (25k, 26 s), approve-with-changes:
  - (blocking, applied) the test package is `amont-agent`;
  - (low, carried) the drift test in the Phase 1 list.
- **codex** (gpt-6-astra, high effort), approve-with-changes:
  - (high, applied) git config must not hide code: `--ignore-submodules=none`, with a gitlink test;
  - (high, applied) an empty or wrong-repository pointer section is a declaration error, with no carry;
  - (medium, folded into backend's blocker) the test target and the workspace suite.
- **backend** delta (30k, 34 s), approve-with-changes, carried into implementation:
  - (medium) `--no-relative` and `:(top)` pathspecs;
  - (low) a gitlink mutation check;
  - (low) drop the redundant `cargo test`.
- **backend** fresh bind on the final body (31k, 56 s), approve-with-changes, carried into implementation:
  - (medium) the plan-only skip runs before the declaration read, so an unreadable canonical can't hold a plan-only push;
  - (medium) a `git::diff_quiet_in` helper returning the exit status;
  - (medium) the full Phase 1 test list;
  - (low) a gitlink-only branch has `key=None`;
  - (low) a baseline count on 860e5fc.
