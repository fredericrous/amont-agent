---
status: active
branch: feat/test-output-quiet
repos: [amont-agent, amont]
adrs: [ADR-0022]
---

# Test output that is cheap and keeps its exit status; a re-read that can be refused

## Review panel

👉 **Decide:** none. Approve if a quiet runner in both binaries, an advise rule for trimmed runners, and a deny-safe `file-reread` are worth two PRs and two releases.
📍 amont-agent (plan, `quiet`, rule, read record) then amont (pointer, `amont run` quiet) · reviewed after your six points · next: amont-agent PR. Panel: backend, rust, tui, unix (opus-5-5).
**Changed by review:** reads record exact windows and certainty; pipefail read from the interpreter's yes/no/maybe state (with zsh `setopt`); streaming capture with a bounded memory excerpt, and Outcome-aware quiet.
📄 Full reviews: [2026-10-11-test-output-keeps-its-status.reviews.md](2026-10-11-test-output-keeps-its-status.reviews.md)
**Verdicts:** round 1: 4× approve-with-changes. Round 2: backend approve-with-changes. Your review: 6 accepted. Then 3 backend deltas, last one `approve`. 0 findings carried: all folded into the phases.

## Context

Measured 2026-10-11 over 30 days of transcripts (2,625 sessions, 123k tool
calls, 230 MB of tool results):

- Test/lint/typecheck runs are **4.4%** of tool-result bytes (10.1 MB,
  11,492 calls, p50 369 B); with commit/push hook output ~6–7%. Cheap.
- **76% of those runs are trimmed with `| tail`/`| head`/`| grep`** (8,769
  calls). A pipeline's status is the last command's, so `cargo test | tail`
  exits 0 on a failure. Only 0.5 MB came back `is_error`. `pipe-to-tail`
  denies this only for mutating commands (push/commit/tag/apply/publish);
  test runners are outside its list, and no rule covers them.
- Of 8,776 trimmed test/lint runs, 2,179 (25%) printed failure text
  (`test result: FAILED`, `panicked at`, `[1-9] failed`, `error[E…]`,
  `error TS…`, `Traceback`) yet returned success (no `is_error`). Only 38
  failures came back as errors; 17 of 8,776 used pipefail. In nearly all
  of them the failure text was visible in the trimmed output. The harm is
  to the exit status, which breaks any chain such as `cargo test | tail && git push`,
  and to failures that `tail` cut off.
- The trimming exists because there is no cheap correct alternative:
  `amont run <check>` streams the tool's raw output even on a pass (a named
  run installs no `live::Stage` slot, `dispatch.rs:836` → `status_streamed`
  falls back to inherited stdio; clippy pass = 3.4 KB of download lines).
- Reads are the large cost (~61%), and five advise rules already cover them.
  The person chose to leave them on advise until the 10-18 outcome check,
  **except `file-reread` → deny now** (re-reads of unchanged files: 7.3 MB
  / 15% of Read bytes last week). Deny is unsafe today: the read record
  (`session_state.rs`, `sessions/<session>.log`) is keyed by session only,
  and a subagent shares its parent's `session_id`, so a subagent would be
  refused a file only the parent read; and nothing clears the record on
  compaction, so a re-read after a summary would be refused.

Outcome: a test run costs one line on a pass and only the failure on a
fail, with the real exit code. The runner paths (`amont run <check>`,
`amont-agent quiet -- <cmd>`) make that cheap and correct, and the new rule
advises toward them; whether it ever denies is decided by the criterion in
phase 9. `file-reread` can deny without blocking a legitimate read, once the
window and certainty rules of phase 6 hold.

## Decisions

- Split as the person chose: **amont** owns the repo's declared checks
  (`amont run <check>` quiet), **amont-agent** owns one-off commands
  (`amont-agent quiet -- <cmd>`) and the rule. The plan lives in
  amont-agent (it owns the rule); amont carries a pointer file.
- Sequencing: the amont-agent PR (phases 4–7, the canonical fixtures, the rule) merges first; the amont PR (phases 1–3) follows and carries the fixture copy. Releases follow the same order.
- The new rule ships **advise** (the usual ladder; its per_1000 from the
  backtester), not deny: unlike a push, a trimmed test still shows its
  summary line most of the time. It is `Examine::Analysis` (phase 5), because the pipefail state comes from the shell analysis.
- The excerpt matcher is hand-written over lowercased lines with word-boundary checks; no `regex` dependency is added to either crate (neither has one; amont-agent MSRV 1.85).
- The excerpt code is duplicated in both binaries (~80 lines each): the two crates share no library, and a shared crate for one function is more coupling than it saves. The canonical fixtures live in amont-agent `tests/fixtures/excerpt/` with a `MANIFEST.sha256`; amont carries a copy (phase 3).
- amont's `Stage::finish` receives the check's `Outcome` (`check.rs:285`), not the `Verdict`. Only `Passed` collapses to the one ok line. amont's exit codes are unchanged: 0/1 from `Verdict::exit_code` (`check.rs:321-325`), 2 from `amont run` (`run_mode`) for an unknown or ambiguous check name. `amont-agent quiet` preserves the child's status exactly.
- Pass vs fail: amont-agent decides by the child's exit code; amont decides as it does today, by the check's `Outcome`. The excerpt heuristic never decides either.
- Pipe primitive (decided in phase 4): a four-line `extern "C"` `pipe`/`fcntl`/`poll` wrapper in `src/quiet.rs`, as `main` already does for SIGPIPE; the MSRV stays 1.85 (`std::io::pipe` needs 1.87, and raising the floor would be a release note and a `ci.yaml` change for no gain).
- In CI (`CI` set to a non-empty value other than `false`/`0`), quiet does not apply to failures: a failing check prints its full log (shared rules), since the log path would vanish with the runner. A pass still prints one line.
- Phase 5 evidence (backtest 2026-09-11 → 2026-10-11, 83,027 calls): `test-output-trimmed` 2,797 matches, 33.7/1000, flat 4 weeks; `pipe-to-tail` 141 matches with the `Tri::Yes` helper and 141 without, week for week, so the helper commit stays. The relais run hit its 20-min wall time during the backtest; its preserved patch passed every phase-5 acceptance command and was finished in the session.
- Phases 6 and 7 land as one commit: the compaction reset is a few lines in the same `session_state`/`hook` code the per-agent record rewrites, and splitting the hunks would leave an intermediate commit with a half-keyed record. The relais run hit its 20-min wall time; its preserved patch passed `make check` and the seven acceptance tests unchanged.
- Compaction is detected from `SessionStart` with `source: "compact"` (already installed) rather than a new `PreCompact` registration, so no settings re-install is needed. Only `compact` clears the record: `resume` reloads the transcript with context intact, and `/clear` starts a new `session_id` (a new record already). This is why the review suggestion to also clear on clear/resume was not taken.

## Shared rules (both binaries)

- **Running line.** `running <check>, log <path>.tmp` goes to stderr only when the run is still going after 5 s. A run that ends sooner prints no running line, so a quick pass is exactly one line even under `2>&1`.
- **Capture.** Child stdout and stderr share one pipe, so their order is kept. A reader drains it continuously, appends the bytes to `<path>.tmp` (0600), and feeds the excerpt accumulator. When the child exits the reader renames `.tmp` to `<path>`, then the result prints. On SIGINT/SIGTERM the same happens, then `interrupted, full log: <path>`. Pruning never deletes a `*.tmp`.
- **Accumulator.** Hard memory bound 64 KB whatever the output size: the failure-block state machine (phase 3) runs line by line; a ring of the last 40 lines; the line assembler keeps at most 1 KB of each line; every excerpt line is cut at 1 KB with a marker (the full line is in the log); excerpt text is held to the excerpt bound (16 KB; amont-agent `--max-bytes`, at most 48 KB); the 40-line ring is held to `60 KB − excerpt bound` bytes, oldest lines dropped first. Worst case about 58 KB at the default, under 64 KB at any `--max-bytes`.
- **Excerpt bound.** No per-block line cap: each failure block prints in full (ADR-0026). The only bound is the excerpt total (16 KB, or amont-agent's `--max-bytes`): past it, one line `… N more lines, full log: <path>`. The 12 summary lines print after the bound, always.
- **Storage errors.** On any error (create, mid-run write, rename) the reader stops writing the file, records the error, and keeps draining the child and feeding the accumulator. The excerpt and the real status still print, then `log not written: <reason>`, or `log left at <path>.tmp` if only the rename failed.
- **CI.** Under CI a failing check prints its full log content, streamed from the file. After a storage error there is no whole file to stream: it prints the excerpt and `log not written: <reason>` instead. The excerpt bound applies to the excerpt only, and CI failures print no excerpt, except after a storage error.
- **Pass line.** One ASCII line: `ok <check> <N>s log=<path>` (amont-agent: `ok <N>s log=<path>`), with the final path after the rename.
- **Test injection.** `AMONT_TEST_LOG_FAIL_AFTER=<bytes>` (amont) and `AMONT_AGENT_TEST_LOG_FAIL_AFTER=<bytes>` (amont-agent) make the log writer fail after N bytes. They exist only behind a `test-hooks` cargo feature, because integration tests spawn the binary and `cfg(test)` does not reach it.

## Phases

### amont (one PR, pointer plan)

- [ ] **1. A named run is captured.** `run_named` installs a one-slot `Stage` (no live region) whenever `live::quiet` is on, regardless of `amont.progress` (today `pre_push` builds one only under `live::enabled`, `dispatch.rs:911`), so the shared capture reader feeds it and `slot.buf` no longer grows. `Stage::finish` (`live.rs:236-261`) takes `(settings, idx, outcome)`, where `outcome` is the check's `Outcome` (`check.rs:285`: `Passed`, `Failed`, `Warned`, `Fixed`, `Unavailable`, and the rest of that enum, matched with no catch-all arm). Every call site in `dispatch.rs` is updated. Test `amont.progress false` with quiet on.
- [ ] **2. Quiet result.** When `live::quiet` is on (`amont.quiet` `auto` = stderr not a terminal, which is how an agent runs it, or `always`), output is captured per the shared rules to `<git-dir>/amont/logs/<check>.<pid>.log` (0600), newest 10 per check kept. `Passed` → `ok clippy 12.3s log=<path>` and nothing else. Any other outcome prints the check's own messages exactly as today, with the tool's raw output replaced by the excerpt (phase 3) and `full log: <path>`. `Failed` also shows the runner's exit code in its failure line (`cargo-test failed, runner exit 101`). amont's exit code is unchanged (0/1 from `Verdict::exit_code`, 2 from `amont run` for an unknown or ambiguous check); the runner's code never becomes amont's. `Warned`, `Fixed`, `Unavailable` and `Inert` (skipped, `check.rs:304`) use the same format and keep the exit code they have today. On SIGINT/SIGTERM the shared interrupt rule applies; a write failure follows the shared storage-error rule and the real exit code is kept. The pass line is ASCII; ANSI stripping applies to the excerpt only; `NO_COLOR` is respected as today. `amont run --quiet` and `--raw` override `amont.quiet` for that run. This applies to the stage blocks too (commit/push hook output): a passing check prints its `running` line (only after 5 s) and its ok line, nothing else. `amont.quiet never` keeps today's output; a terminal keeps today's output under `auto`. In CI, failures print the full log (shared rules).
- [ ] **3. Excerpt.** ANSI stripped. A block starts at a matched line and runs until the next blank line or runner section header (`---- `, `failures:`, `test result:`, `=====`, `FAIL `); for cargo test the whole section between `failures:` and `test result:` is taken. Every failure block is printed in full (`guidance.gate-output-is-failures`, ADR-0026); there is no per-block line cap. Matches are case-insensitive words (`error`, `failed`, `failure`, `panicked`, `assert`, `Traceback`, `Exception`, `✗`, `×`, `FAIL`), checked by hand over lowercased lines with word-boundary checks. They must not match success lines: `0 failed`, `0 errors`, `no errors found`, or test names containing `error`. Blocks are de-duplicated and printed in order, then the last 12 lines (the runner's summary). The only bound is the shared 16 KB total (shared rules). Nothing matched → last 40 lines. Build noise (`Compiling`, `Downloaded`, progress bars) is never matched.
- Golden fixtures: the canonical set lives in amont-agent `tests/fixtures/excerpt/` with a `MANIFEST.sha256`; amont carries a copy under its `tests/`. Contents: a failing cargo test with a multi-line assert diff and a multi-line panic; a failing pytest traceback; a vitest failure; an eslint failure; a passing run containing `0 failed`, `test result: ok. 0 failed` and `no errors found`. Each repo's suite asserts that its copy's `MANIFEST.sha256` equals a pinned constant; amont-agent's suite asserts only that. The cross-check runs only in amont (the follower, so merge order cannot deadlock): a CI step fetches amont-agent's `tests/fixtures/excerpt/MANIFEST.sha256` from its default branch over the public GitHub raw URL (no credentials) and fails on mismatch. If the repository is not public at implementation time, the step uses the CI's existing forge token and the choice is recorded under Decisions. A fetch failure fails the step with a clear message, never passes silently. Both suites assert identical excerpts. Changes land in amont-agent first, then amont, within this plan.
- Tests in `crates/amont/tests/run_mode.rs` style: passing declared check → one ok line and the log file exists with the full output; failing check → exit 1, excerpt contains the failing line, `full log:` path; `amont.quiet never` → raw output as today; `CI=true` with a failing check → full log content printed.
- Release via `tag-release`; install locally (`~/.local/bin/amont`).

### amont-agent (one PR, owns the plan)

- [x] **4. `amont-agent quiet [--max-bytes N] -- <cmd…>`.** Spawns argv directly (a pipeline is the caller's `sh -c`). Capture follows the shared rules: child stdout and stderr share one pipe, drained into `<journal dir>/logs/<UTC-ts>-<program>.log.tmp` (0600, dir 0700 via `journal::private`), renamed to `.log` once the child exits; newest 50 kept; `*.tmp` never pruned. Stdin is `Stdio::null()`. The child runs in its own process group; SIGINT/SIGTERM are forwarded to it; an interrupted run prints `interrupted, full log: <path>` after the rename, so no `.tmp` is left. EPIPE on our stdout is ignored: the wrapper keeps draining and logging and exits with the child's code. Bound: `--max-bytes N` sets the excerpt total, N in bytes or with a `k` suffix, accepted 1–48 KB (default 16 KB, no config key); outside that range → exit 2 with a usage message. The 64 KB memory bound is fixed (shared rules). Summary and excerpt go to stdout; the wrapper's own errors go to stderr. Pass → `ok 12.3s log=<path>`; fail → `exit N 12.3s`, the excerpt (phase 3), `full log: <path>`. Exit codes: 2 usage (missing `--`, no command, `--max-bytes` out of range), 127 command not found, 126 cannot execute, otherwise the child's code, 128+signal for a signal. Document in `--help` and `docs/rules.md`. `Sub::Quiet` in `src/main.rs`.
- [x] **5. Rule `test-output-trimmed`** (`src/rules/test_output_trimmed.rs`, `Examine::Analysis`, advise, max deny). Fires when a clause whose program is a runner — `cargo test|nextest|clippy`, `go test|vet`, `pytest`, `python -m pytest`, `vitest`, `jest`, `playwright test`, `npm|pnpm|yarn|bun [run] test|lint|check|typecheck|e2e`, `make test|check|lint`, `tsc`, `ruff`, `mypy`, `pyright`, `eslint`, `just test|check`, `tox` — pipes (via `pipeline_sink`, reused from `pipe_to_tail`) into `tail`/`head`/`grep`, and the pipeline's pipefail state is not `Tri::Yes`. The analysis tracks pipefail as a three-valued state (`src/analysis/state.rs:201` `pipefail: Tri`, set at `src/analysis/interp.rs:1606`, joined across branches at `state.rs:218`, read by `pipe_status` at `interp.rs:2232`), but reads it only inside `pipeline()` (`interp.rs:683`), and treats zsh `setopt` as an unsupported builtin (`interp.rs:1505`) that havocs options to `pipefail: Tri::Maybe` (`state.rs:290-292`). Two pieces of new work: (a) the interpreter models zsh `setopt`/`unsetopt` for `pipefail`/`PIPE_FAIL`/`pipe_fail` (case- and underscore-insensitive, as zsh is), with `setopt nopipefail`/`no_pipefail` as off; every other `setopt` option keeps today's havoc, applied first, and pipefail spellings are then applied in argument order, so `setopt extendedglob pipefail` ends `Tri::Yes`; the existing `set -o`/`+o` handler (`interp.rs:1605-1606`, today exact word `pipefail` only) uses the same name normaliser, so zsh's `set -o nopipefail` turns it off; (b) the analysis result exposes, per pipeline, the pipefail `Tri` in effect when it runs. Only `Tri::Yes` exempts: `Maybe` (conditional) and `No` fire. A child shell's option (`bash -o pipefail -c …`) never changes the outer state. Exempt: `--help`, `--version`, `--list`, `-- --list`, `cargo test --no-run`. Remedy names `amont run <check>` for a declared check, `amont-agent quiet -- <cmd>` otherwise, and `set -o pipefail;` as the minimal fix. Corpus `tests/corpus/test-output-trimmed.cases` embedded in `src/corpus.rs`: ≥4 nomatch rows, plus negative rows that must match: `set -o pipefail; set +o pipefail; cargo test | tail`, `true || set -o pipefail; cargo test | tail`, `bash -o pipefail -c true; cargo test | tail`, `setopt pipefail; unsetopt pipefail; cargo test | tail`, `setopt nopipefail; cargo test | tail`, `set -o pipefail; set -o nopipefail; cargo test | tail`; and positive rows that must nomatch: `set -o pipefail; cargo test | tail`, `set -euo pipefail; cargo test | tail`, `setopt pipefail; cargo test | tail`, `setopt PIPE_FAIL; cargo test | tail`, `setopt extendedglob pipefail; cargo test | tail`. Benign rows in `tests/precision.rs`; row in `docs/rules.md`; evidence from `amont-agent backtest --rule test-output-trimmed --since 2026-09-11`.
- Separate commit, after the negative rows pass: the same `Tri::Yes` helper is applied to `pipe_to_tail` (a deny rule; its doc says pipefail is accepted, the code never checked), with its own corpus rows. If the `pipe-to-tail` backtest turns any existing match into a nomatch other than a true pipefail case, that commit is dropped.
- [x] **6. `file-reread` records per agent, window and certainty.** The record file becomes `sessions/<session>.<agent>.log`; the agent part is `lsp::agent_key` (made `pub(crate)`) passed through `session_state::usable_id`, so `/` and `..` cannot escape `sessions/`. The payload's `agent_id` is parsed into three states before normalisation: absent → `main`; a valid id (a non-blank string that passes `agent_key` and `usable_id`) → that agent; present but invalid (non-string, blank, whitespace, `/`, `..`) → no record written and no deny. The `Bash` payload's `cat` path uses the same parse. The same parser fixes the LSP records in `lsp.rs` (one parser, one behaviour). Old files are never matched.
  Each record stores the exact window and direction: `head -N` → `0:N`; `tail -N` → `tail:N`, counted from the end; `sed -n 'a,bp'` → `a-1:b-a+1`; Read → its `offset:limit`, normalised to a 0-based first line and a count. An unwindowed Read records `full` when the delivered result is the whole file (under the Read tool's 2,000-line cap), otherwise `0:2000`. A projection whose range cannot be stated exactly (grep, awk, `sed` with other scripts, byte counts, unknown flags) is recorded `uncertain`.
  Each record is `certain` or `uncertain`. A Bash dump is `certain` only when (a) the shell analysis (`src/analysis`) says its clause runs unconditionally (no `||`; not inside an if, case, loop, function or subshell branch; not after a clause that may exit), (b) the Bash result is not an error, and (c) the tool result was delivered whole: `tool_response.stdout` (`payload.rs:602`) is shorter than the harness cap (`BASH_MAX_OUTPUT_LENGTH` from the environment when set, else 30,000, counted as `chars().count()`, not bytes) and carries no persisted-output marker. At or above the cap, a persisted-output marker, or an unrecognised shape → `uncertain`. A Read is `certain` for the window delivered; a Read result persisted to a file is `uncertain`.
  Only a `certain` record of the same agent, covering the new read's window (same direction; `full` covers any window of that file), on an unchanged file, can deny, and only when the new read is itself an exact window. A `certain` record whose new read is not an exact window advises; an `uncertain` record never speaks (no advice, no deny). A `tail` record never covers a head window, or the reverse.
  Tests in `tests/hook.rs`: after `head -20 f`, `tail -20 f` and `sed -n '101,120p' f` are not re-reads, and repeating `head -20 f` is; `true || cat f` then `cat f` → no deny; a persisted result, or a Bash `stdout` of exactly the cap length (30,000 characters, and `BASH_MAX_OUTPUT_LENGTH` when set), then the same read → no deny; a 3,000-line file Read unwindowed then Read with offset 2000 → not a re-read; a subagent with a different `agent_id` is not told "already read" after the parent's read (write this test first); a subagent Bash `cat` payload is keyed to the agent, not `main`; `agent_id` 123, `""` and `"../x"` reading `f` after a main read → no `file-reread` and nothing written outside `sessions/`; the same agent re-reading a certain window still fires.
- [x] **7. Compaction clears the main record.** `SessionStart` parses `source`; on `compact`, the session's `main` record (`sessions/<session>.main.log`) is replaced by an empty record (written to a temp file and renamed over the old one, not truncated in place). Subagents keep theirs: their context was not compacted. Test in `tests/hook.rs`: `SessionStart source: compact` then main re-reads → no `file-reread`.
- Tests for `quiet` (`tests/hook.rs` style, spawning the binary): `quiet -- sh -c 'echo x; exit 3'` → exit 3, log has `x`, file mode 0600; `quiet -- sh -c 'kill -TERM $$'` → 143; missing `--` → 2; nonexistent command → 127; pruning keeps 50 and never deletes `*.tmp`; `--max-bytes` bound honoured, `48k` accepted, `0` and `49k` → exit 2 with a usage message; shared golden fixtures; 50 MB of output keeps peak memory bounded; `AMONT_AGENT_TEST_LOG_FAIL_AFTER` mid-run → the child finishes, excerpt and real exit code print, then `log not written: <reason>`; log dir not creatable → same; rename failure → `log left at <path>.tmp`.
- Release via `tag-release`; `brew upgrade amont-agent`.

### Live (agent-observed)

- [ ] **8. Stance.** After 2.34 is installed with phase 6 in it: `git config --global amont.agent.file-reread.stance deny`.
- [ ] **9. Observe.** Over the following 2 days: journal `file-reread denied` rows — each one a true repeat (same agent, no compaction between, file unchanged, certain record); any false denial → back to advise (one command) and fix. Count `test-output-trimmed advised` and how many next calls used `amont run`/`amont-agent quiet`. After 7 days, the `test-output-trimmed` promotion check: from transcripts (amont's ok line has no journal row), the share of advised calls whose next runner call in the same session used `amont run`, `amont-agent quiet` or pipefail. If it is under 50%, propose `deny` to the person with the numbers. Also measure: `amont run clippy 2>&1 | wc -c` on a passing tree before and after (3.4 KB → under 200 B), and with `amont.progress false`. File-reread denials split by main/subagent and post-compaction; target zero denials of an agent that never read the file.

## Non-goals

- Raising any other read rule; the 10-18 outcome check decides those.
- Review-panel plan reads (11.2 MB/week, exempt by design).
- Passing extra arguments through `amont run <check>` (one-off filtered runs use `amont-agent quiet`).
- Cleaning up a `*.tmp` left by a SIGKILLed wrapper: pruning never deletes `*.tmp`, so it is removed by hand.
- Repairing or reading records written before phase 6: old files are ignored.

## Verification

| Input | Expected |
|---|---|
| `amont run clippy --all-files` piped, in amont-agent (passing tree) | one `ok clippy 12.3s log=…` line; log holds the full output; no `running` line unless the run takes over 5 s |
| a declared check made to fail (fixture repo) | exit 1, excerpt shows the failing line, `full log:` path |
| same, from a terminal / `amont.quiet never` | today's raw output |
| `amont run <check>` with `CI=true` and a failing check | full log content printed, streamed, not capped at 16 KB |
| `amont run --raw` | today's raw output |
| a failing check that prints nothing | `exit N`, `(no output)`, log path |
| passing run whose output contains `0 failed`, finishing under 5 s, under `2>&1` | exactly one ok line, no `running` line |
| run still going at 5 s, under `2>&1` | `running …` line first, then the ok line |
| `amont-agent quiet -- cargo test` with one failing test | exit 101, excerpt names the test and its full panic message, no `Compiling` lines |
| `amont-agent quiet -- sh -c 'sleep 5'` sent SIGINT | exit 130, `interrupted, full log: <path>` with the final path, no orphan (`pgrep`), no `.tmp` left |
| `amont-agent quiet -- cargo test \| head -1` with a failing test | no EPIPE message on stderr, `pipestatus[1]` is 101 |
| `amont-agent quiet --max-bytes 49k -- true`; `--max-bytes 0` | exit 2, usage message |
| `amont run <check>` with `CI=true`, a failing check and the log write failing (`test-hooks` injection) | excerpt, `log not written: <reason>`, exit code as today |
| `amont-agent check 'cargo test 2>&1 \| tail -20'` | `test-output-trimmed` advises |
| `set -o pipefail; cargo test \| tail` / `setopt pipefail; cargo test \| tail` / `setopt PIPE_FAIL; cargo test \| tail` / `setopt extendedglob pipefail; cargo test \| tail` / `cargo test` (no pipe) | silent |
| `set -o pipefail; set +o pipefail; cargo test \| tail`; `true \|\| set -o pipefail; cargo test \| tail`; `bash -o pipefail -c true; cargo test \| tail`; `setopt pipefail; unsetopt pipefail; cargo test \| tail`; `setopt nopipefail; cargo test \| tail`; `set -o pipefail; set -o nopipefail; cargo test \| tail` | `test-output-trimmed` fires in all six; `set -euo pipefail; cargo test \| tail` stays silent |
| `head -20 f`, then `tail -20 f`, then `sed -n '101,120p' f`, then `head -20 f` again | the first two are not re-reads; the repeat of `head -20 f` fires `file-reread` |
| `true \|\| cat f` then `cat f`; a persisted Read result then the same Read; a `cat f` whose `stdout` is exactly the cap length then `cat f`; unwindowed 3,000-line Read then Read with offset 2000 | no `file-reread` in any of the four (uncertain records never speak; the fourth is a different window) |
| warning-only, fixing, unavailable-tool and `Inert` (skipped) checks under `amont run` quiet | each shows its own message plus excerpt and log path, exit code as today; a passing check still prints one ok line |
| log dir not creatable; write fails mid-run (`test-hooks` injection); rename fails; 50 MB of output | excerpt and true exit code print with `log not written: <reason>` or `log left at <path>.tmp`; the child finishes; peak memory stays bounded; a `*.tmp` is never pruned |
| payload with `agent_id` 123, `""` or `"../x"` reading `f` after a main read of `f` | no `file-reread`; nothing created outside `sessions/` |
| hook: subagent reads a file the main read | no `file-reread` |
| hook: `SessionStart source=compact` then main re-reads | no `file-reread` |
| hook: same agent re-reads a certain same window | `file-reread` fires |
| `MANIFEST.sha256` of one repo's fixture copy edited without the other | that repo's pinned-hash test fails, or amont's CI cross-check step fails |
| amont's cross-check cannot fetch amont-agent's `MANIFEST.sha256` (network) | the step fails with a clear message, never passes |
| live, after phase 8 | denials only on true repeats |

<!-- panel: repos=amont-agent,amont reviewers=backend,language:rust,tui,unix body-sha=2bf1761ef261 -->
