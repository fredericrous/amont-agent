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

## Phase 3 implementation review

Items 1, 2 and 6 (item 3, `whole-file-dump`, follows in its own PR: it runs through the generic Bash rule engine; `refuse_once` is ready for it). Tree c5d84777; round 1 and deltas via `review-brief --interdiff`; profile seats read the new slim briefs.
- implementation-review: approve-with-changes → approve-with-changes → approve. Fixed: titled index (ADR-0065), help and module doc, normalised key, `refuse_once` helper, missing tests, unrecorded Read no longer journalled `denied`. Deliberate: the feedback names size, not line count, because the rule stays stat-only.
- backend / lang:rust / unix / tui: approve-with-changes → approve. Fixed: refusal loop when the record cannot be kept (high), size out of the key (growing logs), `agent_key`, journal stance, refusal wording. Every profile seat reported its slim brief was enough.

## Phase 3 verification (observed)

| Input | Expected | Actual |
|---|---|---|
| Replay of the baseline window, reviewer agents exempt | would-refuse counted and sampled | 272 calls / 5.5 MB; sample of 50: 39 source files, 6 markdown, 2 temp files, 2 other, 1 `~/.claude`, all whole reads the rule targets; 0 false interventions (without the exemption, 332 / 15.1 MB were review briefs) |
| Hook tests | refuse once, allow the retry, per agent and file; window and reviewers never refused; unrecorded advises | 7 hook tests + reviewer-kinds and index unit tests pass |
| Profile brief, this branch | own judge rules, titled index, RULES line | backend 156 KB → 95 KB, unix 146 KB → 65–70 KB; implementation-review brief unchanged |
| Delta seats | small fresh launches | 21–33k tokens on 3.6–22 KB briefs |
| Gate | green | 27/27 |

Activation follows the release: `amont-agent graduate read-unbounded-large --to deny`, then the seven-day observation.
