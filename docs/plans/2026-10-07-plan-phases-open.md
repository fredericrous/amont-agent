---
status: active
branch: feat/plan-phases-open
repos: [amont-agent]
adrs: [ADR-0022]
---
# Plan phases open

## Review panel
👉 **Decide:** none — approve if a Stop hook that blocks until the branch's plan has no open phase is what you want.
📍 amont-agent · plan ready · next: worktree + Phase 1 plumbing. Panel: backend, lang:rust, tui, unix.
**Changed by review:** cap release now tells the person and resets on a new prompt; NUL-safe scope with cheap exits; session-id guard, fail-open counter.
📄 Full reviews: [2026-10-07-plan-phases-open.reviews.md](2026-10-07-plan-phases-open.reviews.md)
**Verdicts:** round 1 four approve-with-changes; round 2 backend approve-with-changes → delta approve (one low note carried to Verification).

## Goal
An agent working a plan stops after every phase and waits for "continue",
though ADR-0022 makes a plan one branch and one PR, with its phases as
commits. The new `plan-phases-open` rule is a Claude Code **Stop** hook. When
the agent tries to end its turn while the plan its branch carries still has
an open phase that is not a `🧑 decision:`, the hook sends it on to that
phase. It ships at stance `deny`, the person's choice on 2026-10-07.

## Non-goals
- `SubagentStop`. Subagents are not where phases stall.
- Any change to `plans.rs`'s SessionStart notice beyond sharing its parser.
- Dotfiles (worktree-task skill and plan template) and the decisions corpus.
  A `work.phases-run-through` rule and a line in the plan template about the
  `WAITING:` escape are follow-ups. The block reason documents the escape
  itself, so nothing depends on them.
- Judging whether a phase is really finished. The tick is the signal; the
  implementation-review agent judges ticks at push.
- A `WAITING:` used as a reflex. The journal counts it; the soak decides.

## Behaviour
On `Stop`, the hook judges `cwd`'s repository. It reads `session_id` and `cwd`
and needs both: if either is empty, or the session id contains `/` or `..`
(the same guard as `session_state::file_for`), the hook is silent and never
falls back to the process cwd. It reads `last_assistant_message`,
`permission_mode`, `background_tasks` and `stop_hook_active` when present;
any of these may be absent.

1. **Cheap exits first.** Walk up from `cwd` to the first `.git` entry.
   No `docs/plans/` beside it → silent with no git call. A detached HEAD
   (mid-rebase) or the default branch → silent.
2. **Scope.** The candidates are the `docs/plans/*.md` files (excluding
   README.md and `*.reviews.md`) that differ between
   `merge-base(HEAD, <default base>)` and the working tree. They are read NUL
   separated through the raw helper: `git diff -z --name-only
   --diff-filter=d <mb> -- docs/plans/` plus `git ls-files -z --others --exclude-standard
   docs/plans/` for a plan not yet added. The default base comes from
   `stale::default_base`, with no fetch.
3. **Which plan.** Only `status: active` plans count. A pointer file
   (`canonical:`) is skipped: it has no phases here. If several remain, the
   one whose `branch:` equals the current branch wins; otherwise the oldest
   by file name (the plan the branch opened with). A candidate that cannot
   be read is skipped alone, never silencing the others.
4. **Next phase.** The first unindented `- [ ] ` line inside `## Phases`. The
   section ends at the next `## `, and lines inside ``` fences are ignored.
   Checkboxes in other sections do not count. If no box is open, the hook is
   silent. The phase is quoted by its label: the text before the first ` — `,
   capped at 80 characters with `…`.
5. **Allowed stops** (silent, journaled `watched` with the reason):
   - the open phase contains `🧑` (a decision point, `work.human-decisions-offer-options`);
   - `permission_mode` is `plan`;
   - `background_tasks` is non-empty (the agent will be re-invoked);
   - a line of `last_assistant_message` starts with `WAITING:` once trimmed:
     a preview awaiting approval, a deferred phase, a real blocker. The agent
     has to say why;
   - **the cap.** After 3 continuations for the same (plan, phase) in this
     session, the 4th stop is let through. It emits a `systemMessage` to the
     person: `amont-agent/plan-phases-open: stopped with "<label>" still open in <plan> after 3 continuations; say "continue", or tick it.`
     and is journaled `released`. The 5th and later stops on that phase are
     silent (`watched`). The counter resets when the open phase changes,
     and on `UserPromptSubmit` for that session: a person who said
     "continue" gets three fresh continuations. It lives in
     `<journal dir>/plan-phases-open/<session>` (`atomic::write_atomic`,
     0600), and `plan_phases::sweep()` in the SessionStart arm deletes files
     older than 7 days. A counter that cannot be read or written means
     silence (fail open). Claude Code's own cap (8 consecutive, reset on a
     tool call) is the outer bound.
6. **Otherwise, by stance:**
   - `deny`: stdout `{"decision":"block","reason":…}`, exit 0. The reason
     is built with `decision::phrase` as a fact plus a remedy, never as an
     order, and passes through `decision::clamp`:
     fact `<plan file> has "<label>" open, and it is not a 🧑 decision (ADR-0022, work.commits-follow-phases).`
     remedy `Ticking it and committing moves on to the next phase; a line starting with "WAITING: <reason>" ends the turn (preview approval, a deferred phase, a blocker).`
   - `advise`: a `systemMessage` to the person only, with its own wording and
     no `WAITING:` instruction:
     `amont-agent/plan-phases-open: the agent stopped with "<label>" open in <plan>; say "continue" to run it.`
     The agent stops.
   - `observe`: journal only.
   Every firing journals `denied`/`advised`/`watched`/`released` through
   `journal::record`. The excerpt carries `stop_hook_active`, so the journal
   shows which cap ended a loop.
7. Any other failure (no repo, git error, no readable active plan) is silent. A
   guard that cannot establish the fact never keeps the agent running.
8. **Rollback** without uninstalling:
   `git config --global amont.agent.plan-phases-open.stance observe`.

## Phases
- [x] Phase 1 — plumbing: `Event::Stop { session, cwd, permission_mode, last_message, background_busy, stop_hook_active }` in `src/payload.rs` (hand-read, like `PlanExit`); `Decision::Continue(String)` (top-level `decision: block`) and `Decision::UserNote(String)` (`systemMessage`) in `src/decision.rs`, with a unit test pinning both as top-level keys with no `hookSpecificOutput`; `("Stop", None)` in `settings::TARGETS`; the `tests/hook.rs` `Reply` learns the top-level `decision`; doctor and settings tests follow the new target count, plus a test that an old settings.json makes `doctor` report the missing Stop event.
- [x] Phase 2 — the rule: `src/rules/plan_phases_open.rs` (declaration only, default and max `Deny`, `Evidence{0.0, "2026-10-07", Rare}`, `Examine::Legacy` → `None`, the same as `plan_review_panel.rs`), registered in `rules/mod.rs`; `src/plan_phases.rs` holds the judgement (cheap exits, NUL scope, plan choice, the Phases parse sharing `plans::split_front_matter` made `pub(crate)`, the allowed stops, the cap, `sweep`); the Stop arm, the SessionStart `sweep` call, and `plan_phases::on_prompt(&prompt)` in the `Event::Prompt` arm of `src/hook.rs` (deletes `<journal dir>/plan-phases-open/<session>` behind the same session-id guard; empty, `/` or `..` → no-op); `tests/corpus/plan-phases-open.cases` (nomatch) and its `corpus.rs` EMBEDDED entry. Unit tests cover the parser: fences, indented boxes, a box only in Verification, the label cap. `tests/plan_phases_open.rs` (temp repo with an `origin`, `World` pattern from `tests/plan_review.rs`) has one e2e test for each of: block; tick → silent; `🧑`; `permission_mode: plan`; `background_tasks`; `WAITING:` with leading spaces; pointer file; main checkout; untracked plan; two active plans choose by `branch:`; empty and `../x` session ids silent on Stop and on UserPromptSubmit, with nothing written or deleted outside the dir; an unwritable `CLAUDE_CONFIG_DIR` silent; 3 blocks then `released` with a `systemMessage`, then silent; the counter resetting on a new phase and on a new prompt; a detached HEAD silent; a deleted plan on the branch beside an active one; advise → `systemMessage` only.
- [x] Phase 3 — docs: `docs/rules.md` gets a row plus a paragraph on the escapes, the cap and the rollback; the README and docs/index.md rule counts and their deny breakdown are rewritten from `RULES.len()` and the stances as they stand after this change; `CHANGELOG.md` Unreleased → Added, stating that existing installs must re-run `amont-agent install` for the Stop event.

## Decision log
- 2026-10-07 — Ship at `deny`, the person's choice. Stop has no
  model-facing advisory channel: anything the model reads also continues
  the turn. So `advise` means a note to the person, and only `deny` changes
  behaviour.
- 2026-10-07 — Scope is plans changed on the current branch, the person's
  choice. A plan on main with a deferred phase (plan-sha's Phase 3b) must
  not block every session in the repo.
- 2026-10-07 — Use `decision: block`, not `hookSpecificOutput.additionalContext`
  on Stop. Both continue the turn, but `block` has been in the contract
  longest. Its "hook error" label is cosmetic.
- 2026-10-07 — Read `last_assistant_message` from the payload and never the
  transcript, which the docs say can lag. If the field is absent, there is
  no escape line, and the cap still bounds the loop.
- 2026-10-07 — (review) The cap resets on a new prompt: after a release,
  the person's "continue" must not leave a later stall unseen for the rest
  of the session.
- 2026-10-07 — (review) A released cap tells the person, because a silent
  release is the very stall this rule exists to surface.

## Verification
Drive the release binary on real input. Each entry gets an `actual:` filled
before the push.
- A scratch repo with an `origin`, a branch carrying an active plan, and
  Phases `[x] 1, [ ] 2`; pipe a Stop payload → stdout is `decision: block`,
  the reason names `Phase 2` and is under 300 bytes even when the phase line
  is 500 → actual:
- Tick Phase 2 → silent; make the next box `🧑 decision:` → silent → actual:
- `last_assistant_message` "…
  WAITING: preview at :5173" → silent → actual:
- The same blocked payload 5 times → 3 blocks, then a `systemMessage`
  (`released`), then silent; then a UserPromptSubmit payload and the same
  Stop → blocks again → actual:
- This repo's primary checkout on main (plan-sha's open Phase 3b) → silent → actual:
- `permission_mode: plan` → silent; stance `advise` via git config →
  `systemMessage` with no `WAITING:` → actual:
- `session_id: "../x"` → silent, and `find "$CLAUDE_CONFIG_DIR" -newer <stamp>`
  shows nothing outside `plan-phases-open/` → actual:
- `amont-agent install` into a temp `CLAUDE_CONFIG_DIR` → a matcher-less
  `Stop` entry, and `doctor` green; the old settings.json → `doctor` names
  Stop as missing → actual:
- Latency: `hyperfine` of the Stop hook, p50/p95, on main, on a branch with
  a plan, and in a repo without `docs/plans/`; budget p50 < 60 ms on the
  branch → actual:

## Outcome

<!-- panel: repos=amont-agent reviewers=backend,language,tui,unix body-sha=d4c7f2b7d0f1 -->
