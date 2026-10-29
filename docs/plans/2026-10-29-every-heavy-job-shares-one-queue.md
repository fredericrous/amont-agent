---
canonical: github:fredericrous/amont/docs/plans/2026-10-29-every-heavy-job-shares-one-queue.md
phases: [4]
status: active
branch: feat/heavy-build-unqueued
---
# Every heavy job on the host shares one slot queue

Pointer. The plan and its review panel live in amont, with ADR-0014 (and
decisions ADR-0049, amont owns the host slots). This repository carries
phase 4: the rule `heavy-build-unqueued`, at advise and capped there
(`max_stance: Advise`), whose firings and compliance are counted for 14 days
before it is tightened (phase 5).

- Fires on a readable Bash clause running a heavy build or test not wrapped
  by `amont slot run --`, `amont run` or `amont rehearse`: the
  `test_output_trimmed` runner words, plus `cargo build`, `cargo check`,
  `xcodebuild` and `swift build`. `amont slot run --on-timeout=run -- …`
  still fires (it runs unqueued after the wait).
- `confirm` stays silent when `AMONT_HOST_SLOT` is set in the hook's
  environment, or when `amont` on PATH is older than 1.62 (no `slot run`).
- The advice names `amont slot run -- <cmd>` and explains exit 75 (no slot
  freed in `--max-wait`, the command was not launched). Holders are read only
  through `amont slots --json` (`harness.cross-tool-reads`).

## Verification

## Implementation review
