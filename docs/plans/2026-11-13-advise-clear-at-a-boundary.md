---
status: active
branch: feat/clear-at-boundary
repos: [amont-agent, decisions]
adrs: []
---
# Advise /clear at a boundary past 200k

## Review panel

👉 **Decide:** none — approve if a session that writes its handoff, clears itself and resubmits its next prompt saves more than the lost context costs. (Backend leaves `adds=screen` to you: the 10 s countdown and auto-clear were judged not to be a screen.)
📍 amont-agent, decisions · Phase 1 built and backtested · next: Phase 1b pending boundary, then 1c clear-relay mod. Delta panel: backend, lang:rust, lang:typescript, lang:python (all opus-5-5); codex: not installed.
**Changed by review:** background_busy is judged after the boundary search, so the boundary is kept as pending; the relay uses `$.clock.after` and ignores only its own prompts; split-proof measures and a relay log for the Outcome.
**Verdicts:** earlier rounds as in the reviews file; person's delta: 4 approve-with-changes, then round 2: rust approve, 3 approve-with-changes, backend approve after the one high edit. 9 earlier + 7 new carried into implementation.
📄 Full reviews: [2026-11-13-advise-clear-at-a-boundary.reviews.md](2026-11-13-advise-clear-at-a-boundary.reviews.md)

## Picture

```
 Stop --> open_phase (one git pass, 2 s deadline)
   |            |                    |
 Working     PlanOnly          None / TimedOut
   |            +---------+----------+
   |                      v
   |    clear-at-boundary, cheapest first:
   |      payload says wait?        --yes--> silent
   |      context >= 200k?          --no---> silent, no git
   |      boundary now or pending?  --no---> silent, journalled
   |      anything in flight?       --yes--> pending, journalled
   |      advised it, or 3 already? --yes--> silent
   |                      |
   |       spoke ---------+--------- silent
   |         |                         |
   |         v                         v
   |   one Continue: handoff,    PlanOnly: plan-phases-open
   |   last line "safe to        None: nothing more
   |   /clear; next: `<p>`"
   |         |
   |         v
   |   clear-relay mod (turn.complete): toast, 10 s,
   |   no keystroke / prompt / /stay --> /clear, submit <p>
   v
 plan-phases-open as today (clear-at-boundary silent)
```

A turn's Stop used to end silently however large the context. After this
plan, a Stop at a boundary past 200k with nothing in flight sends the agent
on once to write the handoff and end on the line `safe to /clear; next:
`<prompt>``. The `clear-relay` mod reads that line, runs `/clear` and
submits the prompt, so a programme goes on with no one at the keyboard.

## Goal

Programme session-boundaries, ticket "At what context size to advise /clear
at a boundary" (decisions `docs/programmes/session-boundaries/t06-size-advice.md`).
When a turn ends at a boundary (t03), nothing is in flight, and the main
chain's context is past 200k tokens, greenpiste sends the agent on once to
write the handoff and say it is safe to `/clear`, with the exact first
prompt of the next session. Sessions then clear instead of compacting: the
map's target is the compacted share falling to under 4% in the 14 days
after this ships.

## The size: 200k, and its arithmetic

Measured 2026-11-13 over the transcripts still on disk (104 main sessions
with ≥ 5 assistant messages, 32 compacted; Phase 3 lands the script):

- **Rebuild cost after `/clear`.** A fresh session starts at a median
  46k tokens of context (p90 104k). Call the rebuilt context R ≈ 100k,
  with re-orientation included.
- **Price ratio.** 99.95% of cache writes in the transcripts use the
  5-minute TTL (2,150,915,738 tokens on 5m against 1,119,695 on 1h). On
  Opus 5.5 a 5-minute write costs 1.25× the base input price and a read
  0.05× ($0.20 per MTok against $4: the claude-api skill's prompt-caching
  reference, cached 2026-11-03), so one write costs 25 reads. Units below
  are cache-read tokens, not billing.
- **Break-even.** Clearing at size S saves (S − R) read-tokens on every
  later call and costs one rewrite of R, which is 25·R read-tokens.
  It pays back after k = 25·R / (S − R) calls: 25 at 200k, 12.5 at 300k,
  8.3 at 400k.
- **Calls left after a boundary.** Measured at a worktree teardown: a
  median of 536 more API calls, p25 157, p10 23. At 200k, about 90% of
  boundaries pay back. Any S above 200k pays back more surely, but saves
  less: the read tokens above S are 64% of all cache reads at 200k, 46% at
  300k and 21% at 500k.
- **The destination does not separate the candidates.** 31 of the 32
  compacted sessions passed a teardown at ≥ T before their first
  compaction, for every T from 150k to 400k (30 at 500k).
- **Lower bound.** S must sit well above a fresh session's p90 (104k),
  or a just-cleared session is advised again almost at once: 200k is
  about twice that.
- **Choice: 200k**, the lowest size that keeps the 2× margin over a fresh
  session. Read tokens are what the cost plan cares about, and 200k saves
  the most of them.
- **Not modelled:** idle-gap rewrites of the whole context after a pause of
  more than 5 minutes. They only make a lower S pay back sooner.

## Non-goals

- No PreCompact block (out of the map's scope); no advice mid-task, ever.
- greenpiste never runs `relais` (ADR-0064: greenlift sits above
  greenpiste, and no run-time edge points up).
- No configurable size beyond the git-config stance. The size is a
  constant, with the arithmetic in its doc comment.
- The handoff step's skill text (t04) and the SessionStart resume notice
  (t05) are other tickets.
- No CI or review-journal reads. At these boundaries no PR is open on
  the branch: boundary 1 has not pushed yet, and boundaries 2–4 have
  merged and torn down.

## Code review

- amont-agent: backend, lang:rust, lang:typescript, unix, architect, tui (the `/stay` command and the countdown toast and status line)
- decisions: lang:python (backend and architect left out: a measurement script run by hand, nothing deployed)

## Behaviour

New rule `clear-at-boundary`. Default stance `advise`, max `deny` (the
person's choice of 2026-11-14, in the Decision log). A Stop `Continue` is the
deny shape, as in `plan-phases-open`: the handoff is the model's to write, so
only `deny` reaches it. `advise` sends a `UserNote` to the person, except at
a lone plan commit (`PlanOnly`), where it is silent, journalled, and
`plan-phases-open` carries the plan on; `observe` journals only.
`docs/rules.md` says so.

**Order with `plan-phases-open`.** `plan_phases` gains a pure fact,
`open_phase(&Stop) -> PhaseFact`, which is `None`, `PlanOnly` (open
phases, and the branch has no commit after the plan commit) or `Working`
(open phases, and commits after the plan commit). It runs one git spawn,
which both rules share. `src/hook.rs` handles Stop like this:

- `PlanOnly` past 200k is boundary 1. `clear-at-boundary` judges it first.
  If it speaks, `plan-phases-open` stays silent for that Stop and for every
  later Stop of the same turn (no person prompt since it spoke), so the
  handoff Stop with `stop_hook_active` set ends the turn. This follows
  t03: the plan is the whole handoff. If it is silent, `plan-phases-open`
  runs as today.
- `Working` is in flight: `plan-phases-open` runs as today, and
  `clear-at-boundary` stays silent at every stance, the cap and `observe`
  included.
- `None`: `plan-phases-open` has nothing to say, and `clear-at-boundary`
  judges.

**The checks.** They run cheapest first, and the first one that fails
means silence:

1. **Payload alone.** Silence when `stop_hook_active` is set, the
   permission mode is `plan`, or the session id is unusable
   (`background_busy` is judged after check 4, so a boundary under it is
   recorded as pending). Also silence when `last_assistant_message` ends with `?`,
   has a line starting `WAITING:`, or already contains the marker. The
   marker is `pub const MARKER: &str = "safe to /clear"` in
   `clear_at_boundary.rs`, listed in `docs/rules.md`; the handoff step
   (t04) prints exactly that constant. None of these is journalled, so
   nothing is written on most turns.
2. **Size.** Read the `transcript_path` tail backwards in growing chunks
   (64 KB, then doubled, up to 8 MB), and take the last complete
   main-chain assistant entry that carries `usage`. Skip `isSidechain`
   entries and tolerate a cut first line. The size is input +
   cache_creation + cache_read. Below 200,000 means silence, with no
   journal entry, no git and no spawn.
3. **Turn window.** Find the last person prompt (a `user` entry that is
   not a `tool_result`) in the same tail. If it is not within 8 MB, the
   result is silence, journalled `watched turn-too-long`. Checks 4 and 6
   read only this window; check 5 reads the whole tail.
4. **Boundary in this turn, or pending.** Look in the window for Bash
   commands of the shapes below. A boundary found while something is in
   flight (check 1's `background_busy`, check 5 or 6) is recorded as
   pending in the state file, and a later Stop with no boundary of its
   own judges the pending one, as long as no work started since it: no
   Edit, Write or NotebookEdit tool call, no `git worktree add`, no
   `mcp__relais__run` in the transcript after the pending boundary's
   entry. A new person prompt drops the pending boundary. A new
   `is_injected` predicate tells injected prompts apart: a
   `<task-notification>` or a prompt the engine framed as `The <name>
   plugin sent a message:` (relais's, and the relay's own). Today
   `is_person_prompt` counts both as a person prompt. The backtest showed
   why: at 6 of the 31 compacted sessions, background work (CI polls,
   review agents) was in flight at every boundary, and the turn that ended
   after it finished had no boundary command. The shapes:
   - `git … worktree remove` (boundaries 2 and 3);
   - `sweep.py … --apply`, matched on the script's base name (boundary 4,
     the sweep that ends a release). This shape is data, not a
     dependency;
   - a `git commit` whose message starts `docs(plan): ` and is not a
     `, done` or `, phase N` closing commit (boundary 1).
   No match means silence, journalled `watched no boundary`.
5. **relais, read from the transcript only.** Over the whole tail (up to
   8 MB, earlier turns included: runs are detached), count each
   `mcp__relais__run` result that starts `Started relais run` and each
   plugin message that launches a successor, less each plugin message
   that says a run finished or relais exited. A run that finished at
   `needs_review`, `needs_decision` or `interrupted` waits on this session
   until an `mcp__relais__decide` names it. Silence, journalled `watched
   relais pending <n>` or `watched relais <id> <state>`. (The backtest
   found the first version, which looked for run ids in tool results,
   never fired: Claude Code logs a run's outcome as a plugin message
   prompt.)
6. **Repositories.** Check the payload `cwd` (when it still exists inside
   a work tree) and each existing `-C <dir>` of the boundary commands. A
   removed worktree is not a repository; it counts as clean, because
   `worktree remove` refuses a dirty tree. Each repository is silence,
   journalled with the reason, when:
   - `git status --porcelain -z` lists anything; or
   - the branch has commits that are not on its upstream (or on its base
     when it has no upstream) and that touch a path outside `docs/plans/`.
     The names come from `--name-only -z`, split on NUL: the default
     `core.quotePath` quotes non-ASCII names.
   Every git child runs as `git::bytes_bounded` does: stdin null,
   `GIT_TERMINAL_PROMPT=0`, its own process group. All of them share one
   2 s deadline. A timeout is silence, journalled `watched timeout`.
   Budget: the 15–30 ms per spawn measured in `plan_phases.rs` puts the
   usual 2–4 spawns under 150 ms. The backtest measures p95.
7. **Once per boundary.** A state file
   `<journal dir>/clear-at-boundary/<session>` holds the key of the last
   advised boundary: the boundary command's text plus each checked
   repository's HEAD. The same key again means silence. The file also
   counts the advices given in the session: from the fourth on, the
   result is silence, journalled `watched cap`. A `/clear` starts a new
   session id, so the count restarts there. The file is JSON (turn, advices, key, and an optional pending boundary:
   its entry uuid, command, label and `-C` dirs); a file in the old text
   format reads as no pending boundary. A pending entry whose uuid is no
   longer in the tail is dropped, journalled `watched pending-lost`. The file is written with
   `atomic::write_atomic` and swept after 7 days, as plan-phases-open's
   are.

**The text.** When every check passes, `deny` sends one
`Decision::Continue`, built with `decision::phrase(RULE.id, fact, remedy)`
and stated as facts:

- fact: `this turn reached a boundary (<what>) with nothing in flight, and
  the context is at <N>k tokens, past 200k.`
- remedy: `The handoff is then complete when the plan or ticket states the
  next action and the turn ends with one line: "safe to /clear; next:
  `<the exact first prompt>`".`

At `advise`, the person gets the same fact as a `UserNote`, with a remedy
that works without the handoff skill (t04): reply "write the handoff, then end
with the line `safe to /clear; next: `<the exact first prompt>``", which the
clear-relay plugin then acts on, or `/clear` by hand. The outcome is
journalled `denied`, `advised` or `watched` with its reason.

**The `clear-relay` mod: the session clears and goes on by itself.** A
Claude Code plugin with one hooks module, at `plugin/clear-relay/` in this
repository, placed by `amont-agent install` as relais places its
own (each tool keeps its own install path, `harness.extensions`). It
names no greenpiste command and works without greenpiste: any answer that
ends with the line does it.

- `turn.complete` with `reason` `answer`, on the main agent (no
  `agentId`): when the answer's last non-empty line matches
  `safe to /clear; next: \`<prompt>\``, with a non-empty prompt of at most
  2,000 characters and no newline, arm a relay. Any other ending disarms.
- Armed, it shows a toast and a status line, `clearing in 10 s, then:
  <prompt, cut to 60>; type anything or /stay to keep this session`, and
  starts `$.clock.after(10_000, fire)` from `turn.complete`, which
  returns at once. It never uses `$.clock.sleep`: `command.run` rejects
  inside a hook the turn waits on. On disarm, `Timer.cancel()`. It disarms
  when the prompt box is no longer
  empty (`prompt.edit` or `$.prompt.read`), a prompt is submitted
  (`prompt.submit` from the person or another plugin), a turn starts, or the person runs
  `/stay` (a command the mod registers once at module load, since no `session.start` follows a `/clear`).
- Still armed after 10 s: `$.command.run({ command: "clear" })`, awaited, then `session.end` with reason `clear` (5 s, else a toast and stop), then
  `$.prompt.submit({ text: <prompt> })`. Each step that is refused or
  throws disarms and shows a toast saying what failed, with the prompt,
  so the person can paste it. Nothing is retried.
- State machine: idle, armed, firing. The disarm hooks (`prompt.submit`, `turn.start`, `prompt.edit`) ignore events whose `e.origin` is `{ kind: 'plugin', name }` with this mod's own name, and everything while `firing`; another plugin's prompt disarms; `session.end` with reason `clear` cancels only while armed.
- `turn.complete` returns `await next(e)` unchanged.
- At most one relay per 2 minutes (by `$.clock.now()`), so a misbehaving answer cannot loop
  `/clear`. A reload or a lost worker drops the timer and the state, so
  the relay disarms, which is the safe outcome, and nothing is retried.
- Each arm, disarm (with its cause), clear, submit and failure is appended
  as one JSON line to `<state dir>/amont-agent/clear-relay/<session>.jsonl`, with
  the session id, cwd and time. Outcome (a) and (d) read it.
- Off switch: the plugin's `userConfig` `enabled` (default true), and
  greenpiste's stance: at `observe` no Continue asks for the line, so the
  relay has nothing to read.

Programmes need no exemption: the person wants a programme to keep going,
and with the relay a boundary advice no longer waits for them.

## Phases

- [x] Phase 1 — The rule and its Stop judgement:
  `crates/amont-agent-guard/src/rules/clear_at_boundary.rs`,
  `crates/amont-agent-guard/src/rules/mod.rs`,
  `crates/amont-agent-guard/src/payload.rs` (`Stop.transcript`),
  `crates/amont-agent-guard/src/corpus.rs`,
  `crates/amont-agent-guard/tests/corpus/clear-at-boundary.cases`,
  `crates/amont-agent-process/src/clear_boundary.rs`,
  `crates/amont-agent-process/src/plan_phases.rs` (`open_phase`),
  `crates/amont-agent-process/src/lib.rs`, `src/hook.rs`,
  `src/rule_table.rs`, `tests/clear_at_boundary.rs`
- [x] Phase 1b — The pending boundary (check 4):
  `crates/amont-agent-process/src/clear_boundary.rs`,
  `tests/clear_at_boundary.rs`
- [x] Phase 1c — The `clear-relay` mod and its install:
  `plugin/clear-relay/.claude-plugin/plugin.json`,
  `plugin/clear-relay/hooks/hooks.json`,
  `plugin/clear-relay/hooks/register.ts`,
  `plugin/clear-relay/hooks/register.test.ts`,
  `src/plugin_install.rs` (writes the mod, embedded with `include_str!`,
  to `<state dir>/amont-agent/claude-marketplace/` with its
  `marketplace.json`, and adds the `extraKnownMarketplaces` and
  `enabledPlugins` entries inside `settings::plan_install`/`plan_uninstall`,
  so they share the one Plan, `would_reformat`, the dry run without `--write` and
  `RemoveFile`; the plugin files are written only with `--write`;
  `uninstall` removes both, as relais does),
  `src/main.rs`, `tests/plugin_install.rs` (which also checks the embedded
  file list against the files in `plugin/clear-relay/`)
- [x] Phase 2 — Docs and release: `docs/rules.md`, `README.md`,
  `CHANGELOG.md`, `Cargo.toml`, `Cargo.lock`, released as greenpiste
  3.35.0 through the tag-release skill (a minor version: a new rule)
- [ ] Phase 3 — Close the ticket in decisions:
  - `docs/programmes/session-boundaries/count-context.py`, which follows
    `t11-measure.py`'s shape: a shebang, a docstring, argparse, and
    `with open(..., encoding="utf-8")`. It applies one dedupe by message
    id and one session filter. It takes `--since`/`--until` on entry
    timestamps. Its first line is the compacted share of all sessions in
    the window (the map's measure); the share among sessions with ≥ 5
    assistant messages follows, labelled. Because a relay `/clear` splits
    one piece of work into several sessions and inflates that denominator,
    it also prints compactions per 1,000 deduped main-chain assistant calls
    and the mean context per call (input + cache_creation + cache_read),
    measures the split cannot move. It also counts the teardowns
    whose cwd is the removed path.
  - `docs/programmes/session-boundaries/t06-size-advice.md`: the
    Resolution carries the size, its arithmetic, the script's 2026-11-13
    output pasted as the baseline t09 compares against, the tag date, and
    the date of the first journal entry, from which t09 counts. Set
    `status = "closed"`.
  - `docs/programmes/session-boundaries/map.md`: a Decisions-so-far line,
    including that t03's "waiting on review or CI" signal is not read,
    and why, and that the `clear-relay` mod now types `/clear`, amending
    the Destination and the "only the person can type `/clear`" fact.

## Decision log

- 2026-11-13 — The person chose this ticket over the handoff step and the
  resume notice, because relais 0.59.0 (due tickets) was not installed yet.
- 2026-11-13 — The boundary is read from what the turn did, not from where
  the session stands. At only 198 of 805 teardowns was the session's cwd
  the removed worktree. Most of the rest ran from a directory outside any
  repository, or from another checkout.
- 2026-11-13 — At boundary 1, `clear-at-boundary` speaks before
  `plan-phases-open`. t03 calls a lone plan commit not in flight, so an
  approved plan carried by a large session is handed to a fresh one. Below
  200k, `plan-phases-open` carries the plan on as today.
- 2026-11-13 — Stance `deny` sends the Continue and `advise` sends a note,
  as `plan-phases-open` does. The backtest stands in for the usual
  observe period. Rollback: `git config --global
  amont.agent.clear-at-boundary.stance observe`.
- 2026-11-13 — relais state is read from this session's transcript, not
  from the `relais` binary. ADR-0064 forbids an upward run-time edge, and
  relais does not record which Claude session started a run.
- 2026-11-13 — `needs_review` and `needs_decision` count as in flight,
  although relais calls them terminal: they wait on this session's answer.
- 2026-11-13 — The map's 32-of-265 baseline cannot be recounted, because
  those transcripts are pruned. t09 compares against count-context.py's
  pasted 2026-11-13 output, which uses the same script and definitions.
- 2026-11-13 — The person capped the advice at 3 per session after the
  first backtest: 789 advices, 44 sessions over 3 (max 54), all in
  long sessions that never cleared, so every later teardown was a new
  boundary. Re-advising only after the context grew by 200k since the
  last advice still left 23 sessions over. A cap of 3 leaves 174
  advices, a maximum of 3, and the same coverage (30 of 31 compacted
  sessions advised before their first compaction), because it only
  drops later advices.
- 2026-11-13 — After the backtest the person chose that a programme keeps
  going: the session clears and continues by itself, without them. A
  Claude Code plugin can do it (`$.command.run` of `clear`, then
  `$.prompt.submit`, and a plugin outlives `/clear`). The 12% of advices
  followed by "continue" or "go" were the person carrying a programme on;
  the relay makes that their default. Costs: a fresh session starts at a
  median 46k tokens, against the 200k–900k the backtest's advised
  sessions carried.
- 2026-11-13 — The relay waits 10 s with a visible way to stop it, and
  never retries: an unwanted /clear loses context that only a summary
  could have kept.
- 2026-11-14 — Phases 1–1c measured. Backtest over 9,826 past Stops
  (branch binary, background tasks rebuilt from `<task-notification>`
  entries): 140 advices, at most 3 per session, hook p95 157 ms; 25 of 31
  compacted sessions advised before their first compaction, against the
  bar of 28. The agent judged the bar's 3 short: in the other 6, background
  agents or CI polls were in flight at every boundary and new work started
  before the session was idle, so a clear there would have cut running
  work; the pending boundary rightly drops it (`pending-work`). 46 of 140
  advices were followed by an editing turn, about a third of them
  "continue" or "go", which the relay now answers itself. Live: a scratch
  session with `--plugin-dir plugin/clear-relay` showed the countdown,
  ran `/clear`, submitted the next prompt from the plugin and answered it
  in a fresh session; this session's own Stop replayed through the branch
  build gave plan-phases-open's Continue (Working) and silence in plan mode.
- 2026-11-13 — The 14 days of "Recount compacted sessions after the size
  advice ships" start from the first `clear-at-boundary` journal entry
  after the person installs 3.35.0, not from the tag.
- 2026-11-14 — Releases as 3.35.0, not 3.34.0: main shipped 3.34.0 (the
  green-name provenance header, #250) while this branch was open. The relay
  logs one file per session, `clear-relay/<session>.jsonl`, under its
  `stateDir` setting or `~/.local/state/amont-agent`; Outcome (a) and (d)
  read those files. The plugin-install test now compares against the files
  git tracks, since `claude plugin test` leaves its scaffolding in the
  source directory.
- 2026-11-14 — The person chose to ship `clear-at-boundary` at `advise`, not
  `deny`, after the implementation review blocked on the agent waiving the
  backtest bars itself (25 of 31 against 28; 46 of 140 advices, 33%, followed
  by an editing turn against at most 10%, about 21% once the relay answers
  "continue"/"go"). At `deny` the Outcome's own (b) rollback would likely
  fire. Promotion to `deny` waits on the 7- and 14-day Outcome counts. At
  `advise` a lone plan commit is left to `plan-phases-open`, since a note
  cannot carry the plan on.
- 2026-11-14 — The relais phrases the rule reads (`Started relais run`,
  `launches successor`, `relais run <id> finished: <state>`, `relais exited
  … without an outcome`, from relais `claude-plugin/hooks/runs.ts`) are a
  contract relais never published. An end it does not recognise keeps a run
  in flight (silence); a start it does not recognise is the one miss that
  advises during a run. Recheck the phrases at each relais release; they
  live in `clear_boundary::relais_messages`.
- 2026-11-14 — The relay's helpers take its state as an explicit `Relayer`
  argument and change it. The hooks compiler accepts `$` only as an argument
  of a function declared at the top of the file: it refuses a closure inside
  `register` and a class method alike (both tried, `claude plugin validate`:
  "$ itself is passed as an argument"). Module-level state was the other
  shape, and it is shared by every registration. Recorded against
  `values.immutable-by-default`.

## Implementation review

- implementation-review: approve-with-changes, then Delta approve-with-changes, then approve (850bace); 7 blocking fixed, stance taken to the person (advise); 118k+90k+55k tokens, 368 s.
- backend approve, lang:rust approve, architect approve at the Delta (all round-1 findings fixed; typed transcript entry and public surface deliberate: shared with other modules, used by tests); 69k/64k/54k tokens.
- lang:typescript approve (relay state per registration, typed log); deliberate: helpers mutate a `Relayer` argument (hooks compiler, Decision log), `as never` test casts; 21k tokens.
- unix approve-with-changes: fixed install order and orphaned plugin dir (both paths); deliberate: exit 2 on I/O failure predates this branch (follow-up), `describe` on stdout predates it; 26k tokens.
- tui approve-with-changes: fixed countdown width, prompt in failure log, note wording; deliberate: two stderr install lines over 80 columns, note assumes the plugin `install --write` always places; 27k tokens.
- CI `tests (windows-latest)` failed on `\` paths in a test walk (f57a6fb8 fixes it); implementation-review, unix, tui approve that tree; 40k/21k/22k tokens.

## Verification

- Phase 1: `cargo test --test clear_at_boundary` feeds Stop payloads with a
  synthetic transcript and a temporary repository:
  - each boundary shape at 250k → one Continue naming it, starting
    `amont-agent/clear-at-boundary:`;
  - the same Stop again → silent (once per boundary);
  - 150k → silent, with no git spawn (a stub `git` on `PATH` logs nothing)
    and no journal line;
  - each in-flight signal → silent: `stop_hook_active`, plan mode, a
    question, `WAITING:`, the marker already said, `background_busy`, a
    dirty tree, unpushed non-plan commits, a relais run with no terminal
    state, one at `needs_decision`, one started in an earlier turn and
    still running, and a git stub that sleeps past the
    deadline (`watched timeout`);
  - `Working` (a commit after the plan commit) with plan-phases-open past
    its cap, and at `observe` → silent;
  - boundary 1 (`PlanOnly`, cwd in the worktree, open Phase 1) at 250k →
    the clear-at-boundary Continue, and plan-phases-open stays silent; at
    150k → plan-phases-open's Continue as today;
  - the Stop after that boundary-1 Continue (`stop_hook_active`, marker
    said, no prompt since) → silent from both rules;
  - a plan commit touching `docs/plans/é.md` → advised;
  - `advise` stance → a UserNote, not a Continue;
  - a removed cwd with a teardown → advised;
  - a sidechain usage after the main-chain one is ignored;
  - a first line cut by the chunk boundary is tolerated;
  - no person prompt within 8 MB → silent, `watched turn-too-long`.
- Phase 1, backtest (ADR-0060, a hook is backtested on real past inputs):
  replay the judgement against the transcripts on disk. At each turn end
  that has a boundary command, rebuild the Stop payload from the
  transcript up to that point. Count advised and silent turns, with the
  reason for each silence, how many advices each session gets, and p95
  hook time. Pass bar: an advice before the first compaction in at least
  28 of the 31 compacted sessions that passed a teardown, and no session
  with more than 3 advices, and at most 10% of advices followed by a next
  turn that kept implementing the same task (the failure the backtest
  would show). It also counts the turns whose tail read hit the 8 MB cap
  (`tail-capped`), where relais ids older than the cap go unseen.
- Phase 1b: `cargo test --test clear_at_boundary`: a boundary under
  `background_busy` → pending, journalled; the next Stop after a
  `<task-notification>` → the Continue; an Edit, Write, `git worktree
  add` or `mcp__relais__run` after it → silent; a person prompt → the
  pending boundary dropped, and a plugin message → kept; an old-format
  state file still reads. Phase 1: a fourth advice → `watched cap`.
- Phase 1b: the backtest again, rebuilding `background_tasks` from later
  `<task-notification>` entries: at least 28 of 31 compacted sessions
  advised before their first compaction, still at most 3 per session.
- Phase 1c: `claude plugin validate` and `claude plugin test
  plugin/clear-relay` pass, with tests for: the line arms and, after
  10 s on the mocked clock, runs `clear` then submits the prompt; a
  keystroke, a submitted prompt, a started turn and `/stay` each disarm;
  an aborted turn, a subagent's turn, a line that is not last, an empty
  or multi-line prompt do not arm; a refused `clear` toasts and does not
  submit; a second line within 2 minutes does not arm; the relay's own
  submit does not disarm it; another plugin's submit during the countdown does; a refused submit after a clear toasts the
  prompt; `/stay` works after a clear; the clock is the test kit's mocked
  one. `tests/plugin_install.rs`: `install` without `--write` prints and writes nothing; `--write` twice gives one entry; other `settings.json` keys survive; `uninstall` removes only its own entries. Live: in a scratch
  session started with `claude --plugin-dir plugin/clear-relay`, a turn
  ending with the line clears and runs the next prompt in a new session
  id.
- Phase 1, live: install the branch build with `--hooks-dir` only. Replay
  this session's own Stop payload with its real transcript (past 200k
  after a teardown) → the Continue text; replay it in plan mode → silent.
- Phase 2: the released binary reports 3.35.0, and `amont-agent rules
  --json` lists `clear-at-boundary` at `advise`, max `deny`. Observed
  2026-11-14 on the branch build of 21421774: `amont-agent 3.35.0`, and
  `('clear-at-boundary', 'advise', 'deny')`; the published binary is the
  tag's check. `claude plugin validate` passes, `claude plugin test` 17 pass,
  the gate's 28 checks pass.
- Phase 3: `relais programme check docs/programmes/session-boundaries`
  exits 0. `python3 -I count-context.py --until 2026-11-13` matches the
  block pasted into the Resolution when run on the same day.

## Outcome

Measured 7 and 14 days after the first `clear-at-boundary` journal entry:
- (a) the share of `denied` or `advised` entries followed, within 30 minutes, by a
  SessionStart with `source=clear` with the same payload `cwd`, split into
  relay-caused and person-typed clears;
- (b) the share of `denied` or `advised` entries whose next turn (in the same session, or
  the first turn of the session the relay started) kept implementing the
  same task;
- (d) relay outcomes from the mod's log: cleared, `/stay`, keystroke, failed;
- (c) the compacted share, from count-context.py.

Rollback (stance to `observe`): (b) above 10%, (d)'s `/stay`+keystroke
share above 10%, or any session with more than 3 advices. Target: (a) at least 50%, and (c) under 4% (the map).

<!-- panel: repos=amont-agent,decisions adds=contract reviewers=backend,lang:python,lang:rust,lang:typescript body-sha=11fe99abc0ea -->
