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
- Activity record and `work-overlap`, hook tests (`tests/hook.rs`, injected clock `AMONT_AGENT_TEST_NOW`, scratch `CLAUDE_CONFIG_DIR`), each passing in the commit gate:
  - two Edits of one session in one worktree → one line; an Edit in a worktree of another repository → a second line; a Read → no line;
  - a payload 3 h later in the second worktree → no new line, mtime moved; then another session's first Edit in the second repository advises, and one in the first repository is silent (superseded line);
  - session 2's first Edit advises once and names session 1's worktree, branch and plan title; its second Edit is silent; session 1's own Edits are silent; session 3 inside session 1's worktree advises; session 1 with its mtime set back 3 h and a recent `ts` is silent;
  - a first Edit that meets no peer is silent and does not spend the telling: once a peer is live, the next Edit advises once;
  - `(cd <repo> && git commit …)` records that repository.
- Hook latency, `hookbench.py` (100 Edits of a missing string, scratch config) on the installed 3.9.0 and on this branch, two rounds → expected p95 within 1 ms of today → actual p95 7.9–8.5 ms on 3.9.0 and 9.4–9.5 ms on the branch (+1.0 to +1.5 ms, budget missed by up to 0.5 ms; deliberate: the activity write and peer lookup cost about 1 ms on an Edit). p50 7.3–7.5 → 8.0–8.3 ms. The branch's p99 spikes (500 ms, then 33 ms) came on a freshly built binary's first runs.
- Unmerged branches across the root: 2,752 unmerged refs; 58 with a tip in the last 24 h. Only live branches are read.
- Board fixture tests (`tests/board.rs`: two worktrees, a plan naming `repos: [a, b]`, two sessions, a merge, a `prog/x/t02` branch, stale branches, linked worktrees under the root) → expected the plan's listing, word for word → actual pass.
- `amont-agent board` on the real root (~100 repositories, hundreds of stale squash-merged branches) → expected a check fast enough to run before each task → first build 455 s for `--repo decisions` and 625 git calls for `--repo relais` (linked worktrees scanned as siblings, plans read on stale branches); after three fixes: `--repo decisions` 4.6 s, `--repo relais` 3.2 s, `--repo amont-agent` 2.7 s, the whole root 12.4 s (31 repositories listed).
- Live peers matched to worktrees: runs after the release installs the new hook (canonical plan, Verification).

## Implementation review

- Round 1 approve-with-changes (110k tokens, 111 s). Fixed: the advice names the plan title; the telling is spent only on a live peer; the `Resolved` match is exhaustive; the help text states the live-branch rule; the 3 h test case completed; the latency bench and branch counts recorded.
- Deliberate: a stale branch with an active plan is not listed (live-branch rule; the canonical Decision log records it); the whole-root listing skips the sibling scan; p95 +1.0 to +1.5 ms against a 1 ms budget.
