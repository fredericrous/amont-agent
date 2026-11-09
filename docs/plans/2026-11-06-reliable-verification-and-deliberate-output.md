---
canonical: relais:docs/plans/2026-11-06-reliable-verification-and-deliberate-output.md
phases: [2, 3, 4, 5]
status: active
---
Part of [Reliable verification and deliberate output](https://github.com/fredericrous/relais/blob/fix/verification-terminal-isolation/docs/plans/2026-11-06-reliable-verification-and-deliberate-output.md), approved 2026-11-06.

## Code review

- amont-agent: backend, lang:rust, unix, tui

## Implementation review

Phase 2 (`amont-agent output-audit`), tree ef472736; round 1, Delta (interdiff), final delta; all Opus.
- implementation-review: approve-with-changes → approve. Fixed: `Scan` with methods instead of out-params, unreadable/truncated files counted, tests for copies, `until`, mtime skip. Deliberate: `next_line` reads through `&mut BufReader`.
- backend / lang:rust / unix: approve-with-changes → approve. Fixed: agent launches deduped (high), `ci-log-failed` split, `Kind` and `Context` enums, ranged dates, bounded oversized lines, POSIX `--`, help entry. Deliberate: exit 0 on a partial walk, said in help, footer and `coverage.complete`.
- tui: approve-with-changes → approve-with-changes → approve. Fixed: 76-column table, readable sizes, footer one problem per line, plurals.

## Phase 2 verification (observed)

| Input | Expected | Actual |
|---|---|---|
| Host proofs (real-host fixture, 2026-11-08, claude 2.1.319, manual mode) | Ask enforced; machine vs human deny distinct; rewrite safety known | Ask: Yes ran, No and Esc did not. Human deny/cancel interrupt the turn; hook deny returns `PreToolUse:Bash hook error: <reason>` and the turn continues, no rephrase. `updatedInput` alone and with Ask: the prompt names and runs the rewritten command. |
| 14-day window [2026-10-23T10:03:38Z, 2026-11-06T10:03:38Z) | reproducible aggregate | 2,420 files read, every line; 49 sessions, 1,113 agent launches. Main 27.4 MB: shell 40%, Read 21%, check 7%, ci-log 0.5% + ci-log-failed 0.6%. Subagents 121.9 MB (Read 68%). Opus 12.3G cache-read tokens. Private json sha256 5ab436d0. |
| Unit tests | green | 5/5 |
| Gate | green | 27/27 |

Decision 2026-11-09 (the person): Phase 3 re-targeted, no prompts, to autonomous limits on Reads and shell output, subagents first; CI logs get no special path. A plan delta precedes its code.
