---
canonical: github:fredericrous/decisions/docs/plans/2026-10-31-board-and-programme-map.md
phases: [2]
status: active
branch: feat/work-board
---
# Board and programme map

Pointer. The plan and its review panel live in decisions, with ADR-0051
(`work.check-in-flight-before-starting`, `harness.tool-boundaries`). This
repository carries phase 2: the per-session activity record, `amont-agent
board`, and the advisory `work-overlap` rule.

## Verification

- Session-name probe: ListAgents' short id (`fb740f`) is not a prefix of the hook's session id (`2ecc7c14…`) → the board prints the id and the plan title (canonical plan, Decision log).
- Activity record and `work-overlap`: the hook tests the plan lists (`tests/hook.rs`, injected clock `AMONT_AGENT_TEST_NOW`, scratch `CLAUDE_CONFIG_DIR`) → expected each case → actual pass in the commit gate (25 checks).
- Board fixture tests (`tests/board.rs`: two worktrees, a plan naming `repos: [a, b]`, two sessions, a merge, a `prog/x/t02` branch, stale branches, linked worktrees under the root) → expected the plan's listing, word for word → actual pass.
- `amont-agent board` on the real root (~100 repositories, hundreds of stale squash-merged branches) → expected a check fast enough to run before each task → first build 455 s for `--repo decisions` and 625 git calls for `--repo relais` (linked worktrees scanned as siblings, plans read on stale branches); after three fixes: `--repo decisions` 4.6 s, `--repo relais` 3.2 s, `--repo amont-agent` 2.7 s, the whole root 12.4 s (31 repositories listed).
- Live peers matched to worktrees: runs after the release installs the new hook (canonical plan, Verification).

## Implementation review
