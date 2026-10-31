---
canonical: github:fredericrous/decisions/docs/plans/2026-10-31-board-review-fixes.md
phases: [2]
status: active
branch: fix/board-review
---
# Board review fixes

Pointer. The plan and its review panel live in decisions. This repository
carries phase 2 (A0–A6): squash-merged branches read as merged, the advice
names the peer's own plan, the hook no longer scans every session on every
write, an atomic trim, a single-pass decode, and the small board fixes;
released as 3.11.0.

## Verification

- Live refs before the change: `chore/release-0.42.0` and `feat/dataset-seed` exist in relais, local and on origin. After it, with the branch build, `board --repo relais` lists neither (refs still present), and labels `relais-wt-ledger-busy [fix/ledger-close-busy]` `(merged: …)`: relais#362 merged at 20:20, a true positive.
- Tests: the commit gate (25 checks, the hook and board suites) passed on each commit: squash-merged absent, also after main edits the same file; merged worktree labelled, `"merged"` on every entry; a fresh worktree and an empty change not merged; the peer's own plan named, none on main; both hanging-git shims answer within 2.5 s with no `sleep` left; the FIFO tests (already told; older than the horizon); the invalid-UTF-8 tail; the trim byte target (no further trim over 100 appends, an oversized line kept alone); the rename fault; 8 concurrent writers; the exhaustive decode against the old function; an unterminated tail is not a line; fetch in order with prune. `cargo check --target x86_64-pc-windows-msvc` for the guard crate: rc 0.
- Hook bench, `hookbench2.py` on a `cp -Rp` copy of the real sessions directory (1,004 entries, 8 inside the window), 100 Edits per cohort, two rounds, machine loaded by other sessions. p95, worse round: no peer 3.9.0 41.2 ms, 3.10.0 53.1 ms, branch 40.4 ms; already told 3.9.0 31.5 ms, 3.10.0 112.3 ms, branch 27.4 ms. Gate (within 2 ms of 3.9.0, at most 0.2 ms over 3.10.0): passed in both cohorts. p50 no peer: 3.9.0 17.6–20.1, branch 25.4–25.8 ms (the place record each write costs). Scaling (no peer, synthetic files 30 days old): 2 entries p50 20.4 ms, 9,582 entries p50 145.7 ms on the branch, against 1,038 ms on 3.10.0.
- A first bench of this branch failed the no-peer gate (p95 70.5 and 91.1 ms): the window was read from git config (a spawn) before any peer was known. Commit `ca99bd1` bounds the first pass by the week-long sweep horizon and reads the window only once a same-repository peer exists; the bench above is after it. The FIFO test now asserts files older than the horizon (a week) are never opened; files between the window and the horizon are read for their last line only.
- Board across the root, 3 runs each, no `--fetch`: 3.10.0 53.8 / 28.2 / 23.5 s, branch 51.7 / 31.4 / 33.4 s; medians 28.2 and 33.4 s, 1.18× (bar: under 2×).

## Implementation review
