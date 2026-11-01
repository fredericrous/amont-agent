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
- Tests: the commit gate (25 checks, the hook and board suites) passed on each commit: squash-merged absent, also after main edits the same file; merged worktree labelled, `"merged"` on every entry; a fresh worktree and an empty change not merged; the peer's own plan named, none on main; the three hanging-git shims answer within 2.5 s of the hook's own start (measured by a call that looks nothing up, on the same machine, so load does not count against the budget) with no `sleep` left; the cutoff evidence `a_peer_file_last_written_before_the_cutoff_is_not_a_peer` (regular files on each side of the cutoff); the two FIFO tests remain as regression guards only (a FIFO is skipped before its time is read, so they cannot show the cutoff); the invalid-UTF-8 tail; the trim byte target (no further trim over 100 appends, an oversized line kept alone); the rename fault; 8 concurrent writers; the exhaustive decode against the old function; an unterminated tail is not a line; fetch in order with prune. `cargo check --target x86_64-pc-windows-msvc` for the guard crate: rc 0.
- First overlap (`hookbench2.py … first`: a live peer on main, a new untold session per call, so the advice fires each time), 50 calls, two rounds: first run of the branch p50 780 / 788 ms against 3.10.0's 171 / 153 ms, because deciding the default branch spawned up to seven git calls. After `fc33bcf` (the default branch read from `refs/remotes/origin/HEAD` on disk, else main or master; a peer there costs no git call): branch p50 100.5 / 105.5 ms, p95 163.8 / 163.3 ms, against 3.10.0's p50 84.7 / 89.4 ms, p95 149.2 / 144.2 ms. It fires once per session and repository.
- Hook bench, `hookbench2.py` on a `cp -Rp` copy of the real sessions directory (1,004 entries, 8 inside the window), 100 Edits per cohort, two rounds, machine loaded by other sessions. p95, worse round: no peer 3.9.0 41.2 ms, 3.10.0 53.1 ms, branch 40.4 ms; already told 3.9.0 31.5 ms, 3.10.0 112.3 ms, branch 27.4 ms. Gate, as written in the plan (within 2 ms of 3.9.0 and no higher than 3.10.0; the backend bind's carried 0.2 ms tolerance is not needed): passed in both cohorts. p50 no peer: 3.9.0 17.6–20.1, branch 25.4–25.8 ms (the place record each write costs). Scaling (no peer; the 958-entry point is the real directory, 1,004 entries at bench time; synthetic files 30 days old): 2 entries p50 20.4 ms, 1,004 entries p50 25.4–25.8 ms, 9,582 entries p50 145.7 ms on the branch (about 13 µs per entry, the readdir and metadata), against 1,038 ms at 9,582 on 3.10.0.
- A first bench of this branch failed the no-peer gate (p95 70.5 and 91.1 ms): the window was read from git config (a spawn) before any peer was known. Commit `ca99bd1` bounds the first pass by the week-long sweep horizon and reads the window only once a same-repository peer exists; the bench above is after it. Files older than the horizon (a week) are skipped from their metadata; files between the window and the horizon are read for their last line only.
- Board across the root, 3 runs each, no `--fetch`: 3.10.0 53.8 / 28.2 / 23.5 s, branch 51.7 / 31.4 / 33.4 s; medians 28.2 and 33.4 s, 1.18× (bar: under 2×).

## Implementation review

- Round 1 approve-with-changes (106k tokens, 146 s). Fixed: the three discard reasons on their lines; the vacuous FIFO tests backed by a `peers_in` test over regular files (a file last written before the cutoff is no peer); the bench record completed (the 958 point and slope, the first-overlap cohort, the gate as written, `hookbench2.py`). The week horizon in place of the window (A3) was put to the person, who chose it (2026-11-01); the canonical Decision log records it in the closing PR. The already-told early return has no behavioural observable: the bench (already-told cohort) is its evidence.
- Delta approve-with-changes (117k tokens, 46 s). Fixed: this record names the `peers_in_dir` test as the cutoff evidence. Deliberate (follow-up, next amont-agent change): with no `refs/remotes/origin/HEAD` and both `main` and `master` present, a peer on `master` is taken as on the default and named without a plan; no hook test yet covers origin/HEAD naming a branch other than main.
- Rebased onto main after #171, #172, #174 (ADR-0052, the zsh guard): round 1 on the new tree approve-with-changes (129k tokens, 66 s). Fixed: the budget tests keep the plan's 2.5 s, net of the hook's measured start-up (an interim 4 s bound is gone). The plan has no `## Code review` section (it predates ADR-0052), so the push hook keeps today's behaviour.
- Delta approve (132k tokens, 15 s).
