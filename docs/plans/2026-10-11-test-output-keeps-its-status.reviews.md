# Full reviews: 2026-10-11-test-output-keeps-its-status

Round 1 (sha 93cd716c0182):
- **backend** (31k, 20 s) approve-with-changes: [high] masked-failure rate unmeasured, accepted; measured 2,179/8,776. [high] advise leaves masking, accepted; outcome reworded and promotion criterion added. [medium] Bash `agent_id` for a subagent `cat`, accepted. [medium] clear on `clear`/`resume`, rejected: resume keeps the transcript and `/clear` starts a new session. [medium] concurrent log clobber and log-write failure, accepted. [medium] 6-line excerpt cuts panics, accepted. [low] duplicated heuristic drifts, accepted (shared fixtures). [low] amont adoption can't be journalled, accepted (count from transcripts).
- **rust** (59k, 90 s) approve-with-changes: [high] `Stage::finish` needs the verdict. [high] capture must not depend on `amont.progress`. [medium] no regex dependency. [medium] `quiet` needs tests. [medium] child wiring (one file, stdin null). [low] `agent_key` must be `pub(crate)` and its `None` means no record. [low] observed live: a subagent was told a parent-only file was already read. All accepted.
- **tui** (29k, 27 s) approve-with-changes: [high] CI under `auto`, accepted (full log on failure in CI) plus `--quiet`/`--raw`. [high] 6-line block contradicts ADR-0026, accepted. [medium] success lines match `error|fail`, accepted (exit code decides). [medium] silent long runs, accepted (running/interrupted lines). [medium] ASCII pass line with the path last, accepted. [low] per-run log names, accepted. [low] shared fixtures, accepted. [low] verification rows, accepted.
- **unix** (38k, 39 s) approve-with-changes: [high] agent key path traversal, accepted (`usable_id`). [high] `quiet` streams, signals, stdin and EPIPE, accepted. [medium] own exit codes 2/126/127, accepted. [medium] temp file and rename, accepted. [medium] CI log path, accepted. [low] pipefail syntaxes, accepted. [low] bound precedence, accepted.

Round 2 (sha ee02b988f747):
- **backend** (38k, 47 s) approve-with-changes: round-1 findings 1–6 and 8 resolved; 4 rejected with reason, accepted. Carried into implementation, folded into the phases after the person's review:
  1. [medium] The `running … log <path>` line promises a tailable log, but output reaches disk only at rename. Stream to `<path>.tmp`, print that path, and have pruning skip `*.tmp`.
  2. [low] The 200-line per-block cap contradicts "each failure in full". Keep only the 16 KB bound, or state the exception.
  3. [low] Pin one sha256 of the fixture set in both suites.
  4. [low] Under `2>&1` a pass is two lines (running + ok). Print `running` only after 5 s, so a quick pass stays one line.

Person's review (after round 2):
- 1. [HIGH] Different file windows are one read: `head`, `tail` and `sed` ranges and Read offset:limit are recorded exactly; a projection whose range cannot be stated is `uncertain` and advises only — accepted, folded into phases
- 2. [HIGH] A parsed `cat` is not proof of receipt: a record is `certain` only when the clause runs unconditionally, the result is not an error and was delivered whole; only `certain` records deny — accepted, folded into phases
- 3. [HIGH] Any earlier pipefail is unsafe: the pipefail state comes from the shell analysis and only a definitely-on state (`Tri::Yes`) exempts; child-shell options never count — accepted, folded into phases
- 4. [HIGH] The verdict cannot tell a clean pass from a nonblocking problem: `Stage::finish` gets the check's `Outcome`; only `Passed` collapses, and warnings, fixes and unavailable tools show their messages — accepted, folded into phases
- 5. [MEDIUM] The log fallback needs a capture design: one shared pipe, a tailable `.tmp`, a 64 KB bounded excerpt accumulator, and storage errors that keep draining and still print the excerpt and status — accepted, folded into phases
- 6. [MEDIUM] Malformed agent IDs collapse into main: absent is `main`, a valid id is that agent, an invalid one gives no record and no deny; the LSP records use the same parser — accepted, folded into phases

Delta (sha fb15ae48a263): backend (56k, 69 s) approve-with-changes; all six findings folded into the phases:
- 1. [high] `setopt pipefail` havocs to `Tri::Maybe` (`interp.rs:1505`, `state.rs:290-292`) and the per-pipeline state is not exposed (`interp.rs:683`) — phase 5 names the `setopt`/`unsetopt` model and the per-pipeline `Tri` as new work; four corpus rows; Verification updated
- 2. [medium] "not truncated" had no signal — phase 6 (c): `tool_response.stdout` (`payload.rs:602`) at or above `BASH_MAX_OUTPUT_LENGTH` or 30,000 characters, or a persisted-output marker → `uncertain`; tested at exactly the cap
- 3. [medium] fixture cross-check deadlocks on merge order — runs only in amont, fetching amont-agent's `MANIFEST.sha256` over the public GitHub raw URL (forge token if not public, recorded under Decisions); a fetch failure fails the step
- 4. [low] CI full-log rule had no storage-error fallback — shared CI rule: excerpt plus `log not written: <reason>`
- 5. [low] exit codes misattributed — Decisions and phase 2: 0/1 from `Verdict::exit_code`, 2 from `amont run` for an unknown or ambiguous check; skipped is `Inert`
- 6. [low] `--max-bytes` uncapped — phase 4 accepts 1–48 KB (bytes or `k`), else exit 2 with usage; the shared accumulator holds the ring to `60 KB − excerpt bound` so 64 KB holds; tests and Verification rows added
- Delta (sha 2dfea6085dcf): backend (45k, 57 s) approve-with-changes. Folded in: multi-option `setopt` order, `set -o nopipefail`, uncertain records never speak, `chars().count()`, CI excerpt exception.
- Delta (sha 5f02aa58f379): backend (27k, 15 s) approve-with-changes. "all five" → "all six".
- Delta (sha 2bf1761ef261): backend (26k, 9 s) **approve**.
