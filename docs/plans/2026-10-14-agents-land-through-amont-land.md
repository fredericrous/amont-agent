---
canonical: amont:docs/plans/2026-10-14-agents-land-through-amont-land.md
phases: [2]
status: active
---
Part of [Agents land through `amont land`](https://github.com/fredericrous/amont/blob/main/docs/plans/2026-10-14-agents-land-through-amont-land.md).

This repository carries Phase 2: amont-agent treats `amont land` as a push. A land clause is resolved by running `amont land --dry-run --json` (amont 1.54.0; "1.53" in the canonical body reads 1.54, see its Decision log) once per hook call with a 3 s timeout; only `[cd <literal path> &&] amont land [--json] [--force]` in the foreground resolves, anything else (other clauses, pipes, redirections, env assignments, wrappers, `run_in_background`) is unresolvable. The implementation-review and preview gates judge the land as they judge a `git push`. A PostToolUse/PostToolUseFailure assertion reads the land's receipt (`receipt_dir` from the dry-run) and journals a retried push of an unjudged sha.

## Verification (Phase 2, this repository)

- `make check` (fmt, clippy twice, every test binary): green. New: 18 unit tests (`land` shapes, dry-run answers, receipts; `land-retry` judgement, including an unknown receipt status skipped, not passed), 6 end-to-end through the built hook (`tests/implementation_review.rs`: an unreviewed land held under deny and passed after a review, the `cd <dir> &&` form read; ten unsupported shapes and a background land held, each naming the readable shape; a land on main not held; a retried land said on PostToolUse and on PostToolUseFailure; a land that pushed what was judged, or nothing, not said; a missing record journalled as `retry-check skipped`. `tests/preview.rs`: a UI land advised like a push, a docs-only one not), and reviewed corpus lines for push-preview, implementation-review and pipe-to-tail. Falsified: with `find` not matching a land, the review test fails (fresh compile).
- Real amont 1.54.0: `amont land --dry-run --json`, 20 runs at load ~29: p50 96 ms, p95 112 ms, max 114 ms, against the 3 s budget.
- Live, real amont 1.54.0 and the built hook, scratch repository, tracking ref locked: PreToolUse recorded `aef4c44` as judged; the land's pre-push hook fast-forwarded HEAD, land retried (`retrying once … aef4c44 -> 232e53a`, `retried:true`, landed `232e53a`, `git ls-remote` = `232e53a`); PostToolUse said `land-retry: This land pushed 232e53a … but the guards judged aef4c44` and journalled `land-retry advise broken`.

## Decision log

- 2026-10-14 — The end-to-end tests use a fake `amont` that answers `land --dry-run --json` as 1.54.0 does: CI installs no amont, and the tests must not depend on a machine's version. It is primed once per test, because the first exec of a fresh script can be held by Gatekeeper beyond the 3 s budget under a parallel suite (seen: three tests timed out until it was). The real amont is exercised by the live run above.
- 2026-10-14 — Whether Claude Code fires `PostToolUseFailure` for a Bash call that exits non-zero, and what that payload carries, is checked live after the release, once `amont-agent install` has registered the event: the docs confirm the reply shape (`hookEventName: "PostToolUseFailure"` with `additionalContext`), not the trigger.

## Implementation review

approve, after round 1 (approve-with-changes: verification recorded, unknown receipt status skipped, record failures said, corpus and preview tests added, records pruned) and two Deltas (both lows fixed).
Still to observe live after the release and `amont-agent install`: PostToolUseFailure firing for a failing land, and a UI land held under the preview gate's deny.
80k + 86k + 88k tokens; reviewed tree 08354c9.
