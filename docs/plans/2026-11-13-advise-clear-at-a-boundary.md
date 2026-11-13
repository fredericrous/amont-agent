---
status: active
branch: feat/clear-at-boundary
repos: [amont-agent, decisions]
adrs: []
---
# Advise /clear at a boundary past 200k

## Review panel

👉 **Decide:** none — approve if one Continue at a boundary past 200k makes sessions clear instead of compacting.
📍 amont-agent, decisions · plan reviewed, not started · next: Phase 1 rule and Stop judgement. Panel: architect, backend, lang:python, lang:rust, po, tui, unix (all opus-5-5); codex: not installed.
**Changed by review:** relais read from the transcript only (ADR-0064); boundary 1 judged before plan-phases-open via a typed `PhaseFact`; stance `deny` sends the Continue, `advise` a note.
**Verdicts:** round 1: 6 approve-with-changes, 1 rework (rust); round 2: 6 approve-with-changes, 1 approve; backend delta approve-with-changes. 9 carried into implementation.
📄 Full reviews: [2026-11-13-advise-clear-at-a-boundary.reviews.md](2026-11-13-advise-clear-at-a-boundary.reviews.md)

## Picture

```
 Stop --> open_phase (one git pass, 2 s deadline)
            |                 |                      |
         Working           PlanOnly            None / TimedOut
            |                 |                      |
            |                 +----------+-----------+
            |                            v
            |          clear-at-boundary checks, cheapest first:
            |            payload says wait? --yes--> silent
            |            context >= 200k?   --no---> silent (no clear-at-boundary git)
            |            boundary this turn? --no--> silent, journalled
            |            anything in flight? --yes-> silent, journalled
            |            advised this boundary? -yes> silent
            |                            |
            |                spoke ------+------ silent
            |                  |                   |
            |                  v                   v
            |     one Continue: handoff +   PlanOnly: plan-phases-open
            |     "safe to /clear; next:    None: nothing more
            |     `...`"; plan-phases-open
            |     silent for the rest of
            |     the turn
            v
     plan-phases-open as today (clear-at-boundary silent)
```

A turn's Stop used to end silently however large the context. After this
plan, a Stop at a boundary past 200k with nothing in flight sends the agent
on once to write the handoff and say it is safe to /clear.

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

- amont-agent: backend, lang:rust, unix, architect (tui left out: no command
  or flag changes, and the only new output is hook text, whose wording the
  panel already reviewed)
- decisions: lang:python (backend and architect left out: a measurement script run by hand, nothing deployed)

## Behaviour

New rule `clear-at-boundary`. Default stance `deny`, max `deny`. A Stop
`Continue` is the deny shape, as in `plan-phases-open`: the handoff is the
model's to write, so only `deny` reaches it. `advise` sends a `UserNote` to
the person, and `observe` journals only. `docs/rules.md` says so.

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
   permission mode is `plan`, `background_busy` is set, or the session id
   is unusable. Also silence when `last_assistant_message` ends with `?`,
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
4. **Boundary in this turn.** Look in the window for Bash commands of
   these shapes:
   - `git … worktree remove` (boundaries 2 and 3);
   - `sweep.py … --apply`, matched on the script's base name (boundary 4,
     the sweep that ends a release). This shape is data, not a
     dependency;
   - a `git commit` whose message starts `docs(plan): ` and is not a
     `, done` or `, phase N` closing commit (boundary 1).
   No match means silence, journalled `watched no boundary`.
5. **relais, read from the transcript only.** Take the `run-…` ids that
   `mcp__relais__run` results returned anywhere in the tail already read
   (up to 8 MB, earlier turns included: runs are detached). For each, take
   the latest state that a later relais tool result in that tail shows.
   Silence, journalled `watched relais <id> <state>`, when a run shows no
   terminal state, or shows `needs_review` or `needs_decision`: those
   wait on this session.
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
   repository's HEAD. The same key again means silence. The key is
   written with `atomic::write_atomic`. The file is swept after 7 days,
   as plan-phases-open's are.

**The text.** When every check passes, `deny` sends one
`Decision::Continue`, built with `decision::phrase(RULE.id, fact, remedy)`
and stated as facts:

- fact: `this turn reached a boundary (<what>) with nothing in flight, and
  the context is at <N>k tokens, past 200k.`
- remedy: `The handoff is then complete when the plan or ticket states the
  next action and the turn ends with one line: "safe to /clear; next:
  `<the exact first prompt>`".`

At `advise`, the same text goes to the person as a `UserNote`. The outcome
is journalled `denied`, `advised` or `watched` with its reason.

## Phases

- [ ] Phase 1 — The rule and its Stop judgement:
  `crates/amont-agent-guard/src/rules/clear_at_boundary.rs`,
  `crates/amont-agent-guard/src/rules/mod.rs`,
  `crates/amont-agent-guard/src/payload.rs` (`Stop.transcript`),
  `crates/amont-agent-guard/src/corpus.rs`,
  `crates/amont-agent-guard/tests/corpus/clear-at-boundary.cases`,
  `crates/amont-agent-process/src/clear_boundary.rs`,
  `crates/amont-agent-process/src/plan_phases.rs` (`open_phase`),
  `crates/amont-agent-process/src/lib.rs`, `src/hook.rs`,
  `src/rule_table.rs`, `tests/clear_at_boundary.rs`
- [ ] Phase 2 — Docs and release: `docs/rules.md`, `README.md`,
  `CHANGELOG.md`, `Cargo.toml`, `Cargo.lock`, released as greenpiste
  3.34.0 through the tag-release skill (a minor version: a new rule)
- [ ] Phase 3 — Close the ticket in decisions:
  - `docs/programmes/session-boundaries/count-context.py`, which follows
    `t11-measure.py`'s shape: a shebang, a docstring, argparse, and
    `with open(..., encoding="utf-8")`. It applies one dedupe by message
    id and one session filter. It takes `--since`/`--until` on entry
    timestamps. Its first line is the compacted share of all sessions in
    the window (the map's measure); the share among sessions with ≥ 5
    assistant messages follows, labelled. It also counts the teardowns
    whose cwd is the removed path.
  - `docs/programmes/session-boundaries/t06-size-advice.md`: the
    Resolution carries the size, its arithmetic, the script's 2026-11-13
    output pasted as the baseline t09 compares against, the tag date, and
    the date of the first journal entry, from which t09 counts. Set
    `status = "closed"`.
  - `docs/programmes/session-boundaries/map.md`: a Decisions-so-far line,
    including that t03's "waiting on review or CI" signal is not read,
    and why.

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
- 2026-11-13 — The 14 days of "Recount compacted sessions after the size
  advice ships" start from the first `clear-at-boundary` journal entry
  after the person installs 3.34.0, not from the tag.

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
- Phase 1, live: install the branch build with `--hooks-dir` only. Replay
  this session's own Stop payload with its real transcript (past 200k
  after a teardown) → the Continue text; replay it in plan mode → silent.
- Phase 2: the released binary reports 3.34.0, and `amont-agent rules
  --json` lists `clear-at-boundary` at `deny`.
- Phase 3: `relais programme check docs/programmes/session-boundaries`
  exits 0. `python3 -I count-context.py --until 2026-11-13` matches the
  block pasted into the Resolution when run on the same day.

## Outcome

Measured 7 and 14 days after the first `clear-at-boundary` journal entry:
- (a) the share of `denied` entries followed, within 30 minutes, by a
  SessionStart with `source=clear` in the same repository;
- (b) the share of `denied` entries whose next turn kept implementing the
  same task;
- (c) the compacted share, from count-context.py.

Rollback (stance to `observe`): (b) above 10%, or any session with more
than 3 advices. Target: (a) at least 50%, and (c) under 4% (the map).

<!-- panel: repos=amont-agent,decisions adds=contract reviewers=architect,backend,lang:python,lang:rust,po,tui,unix body-sha=5f3cf2a2d15f -->
