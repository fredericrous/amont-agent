---
canonical: github:fredericrous/decisions/docs/plans/2026-10-28-each-harness-tool-runs-alone.md
phases: [3]
status: done
branch: feat/review-brief-without-aval
---
# Each harness tool runs alone

Pointer. The plan and its review panel live in decisions, with the ADR
(harness.standalone-tools). This repository carries part 3 of the plan's
Changes: a missing sibling is reported as a gap, never a failure, unless the
repository asks for it explicitly.

## Verification

- `make check` (no wall limit; relais's run was cut by its budget at load ~600) → exit 0, 33 suites ok.
- Sandbox PATH = only amont-agent + `/usr/bin:/bin`, repo with `.adr.yaml` → `review-brief` exit 0, "aval is not installed" section, no RULES line, one aval notice on stderr (plus adr-lint's existing one).
- Same sandbox → `doctor` exit 0; a PreToolUse payload through `hook` returns its deny decision with no sibling installed.

## Implementation review

approve-with-changes, round 1 (55k tokens, 53 s).
- deliberate: the fleet-pack refresh is its own commit (7315512), required by worktree-task's Start step; the PR body says so.
- Delta (9c7bb30: the no-aval test writes no stub adr-lint, which timed out at load ~500): approve, no new finding (58k, 14 s).
- Carried: re-run the sandbox checks on the released binary; the Windows PATHEXT branch is untested (compiles on CI's targets).
