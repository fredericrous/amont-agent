---
status: active
branch: feat/plan-slices
repos: [amont-agent, greenharness-skills]
adrs: [ADR-0061, ADR-0063]
---
# A sliced reviewer is bound to the part of the plan it reviews

## Review panel
👉 **Decide:** four choices, recommendation first: (1) slices share ADR-0061's list only; (2) slices checked in deltas only; (3) old baselines keep today's rule; (4) unix/tui bind the whole body.
📍 amont-agent + greenharness-skills · plan ready, nothing built · next: Phase 1 in an amont-agent worktree. Panel: backend, architect, po, lang:rust, tui, unix (opus-5-5), codex (gpt-6-astra).
**Changed by review:** the plan now says it adds re-runs and saves no seat (14-day measure, rollback bar); per-span shas name the moved phase; the skill ships before 3.23.0 installs.
**Verdicts:** 6 approve-with-changes in round 1, backend approve-with-changes twice in round 2, Codex approve-with-changes; 5 findings carried into implementation.
📄 Full reviews: [2026-11-07-sliced-reviewer-bound-to-slice.reviews.md](2026-11-07-sliced-reviewer-bound-to-slice.reviews.md)

## Goal
A plan reviewer whose seat came from a file type (`lang:<x>`, react,
ui-design, platform) is bound to its slice of the plan: the shared sections,
plus the phases that name a path of its type. Two things follow:
- When a later edit changes only another seat's phases, this seat stays
  current, as today.
- When a delta changes its own slice, this seat is required again, which
  today it is not.

This tightens the hook. It saves no seat, because today a delta already re-asks only backend. It adds re-runs, which t10's 150k panel budget must absorb. The token saving comes only from the narrowed ask (the `Your slice:` line).

Backend, architect, product owner, unix, tui, ux-research and game-ux keep
binding the whole body. This builds proportional-ship t06 and ADR-0061's
`work.review-slices`, plan half.

## Context
- **One sha for the whole body.** `plan_text::canonical`
  (`crates/amont-agent-process/src/plan_text.rs:28`) and `body_sha`
  (`:108`) hash the whole body. `plan_review::block` (`plan_review.rs:130`)
  prints `<<<PLAN path=… sha=… [lang=…]>>>`. `parse_blocks` (`:1064-1099`)
  reads it into `Binding { role, path, sha }` (`:661`).
- **The gap.** In a delta, `judge` (`:1162-1167`) requires backend plus
  the roles of areas that are new. A role already seated is never asked
  again, even when its own phase was rewritten: after the person asks for
  a change to a Rust-only phase, `lang:rust`'s round-1 review of the old
  body still counts. `panel()` (`:1912-1921`) repeats the same rule.
  `is_current` (`:1192`) and the `now` closure in `judge` (`:1150`) are shared so the two never disagree; `now` must accept a sliced role's slice sha, not only the body sha.
- **Full mode** (no baseline, `:1155-1158`): every role except backend
  only has to have been bound to this path at some body. Round 2 is the
  skill's own rule: re-run `rework` verdicts and backend (`plan-review`
  SKILL.md §5).
- **Which file types a seat reads already exists.**
  `file_types::profile_reads` (`file_types.rs:314-324`) maps `lang:<x>` to
  that language, react and ui-design to UI files, and platform to ops
  files. Every other role reads the whole diff. The code-review brief uses
  it, so a plan's slice and a code-review slice can come from one table.
  `file_types::phases` (`:119-169`) returns each phase's label and paths,
  but not its lines.
- **When seats come from paths is decided by the body alone.** Paths seat
  only when every phase names one (`seat`, `plan_review.rs:1805-1818`).
  So `plan-sha` can tell paths from areas without reading a repository
  tree.
- **Two readers match bindings to the whole-body sha.** `plan-apply` finds
  a reviewer's patches by `role` and `sha == body` (`plan_apply.rs:388`).
  The outcome ledger picks the transcript whose bindings hold the current
  body (`review_outcomes.rs:264`). A block whose `sha=` became a slice sha
  would break both.
- **The baseline.** Stored as JSON under the journal
  (`plan_review.rs:1198-1296`): `sha`, `roles`, `classes`, `names`,
  `path`. `load` (`:1210`) already tolerates missing fields.
- **The real headings:**
  - The plan template (greenharness-skills
    `parts/workflow/skills/worktree-task/plan-template.md:22-74`): Goal,
    Non-goals, Code review, Behaviour, Preview, Phases, Decision log,
    Verification, Outcome. No Context.
  - Recent amont-agent plans add Context. Context is the first section of
    `2026-11-03-suppressions-through-bash-and-commit.md`, which has no
    Goal.
  - Decisions plans use Context, Approach, Non-goals, Code review,
    Verification, with no Phases at all
    (`2026-11-06-harness-part-reports-its-cost.md`).
  - Verification rows are written per phase (template `:69-72`).
- **Nothing would be sliced today.** The 3.22.0 backtest seated all 45
  past panels from areas (`docs/plans/2026-11-07-review-seats-follow-named-files.md:183-195`).
  Slices only apply once plans name files in every phase. That landed on
  2026-11-07: ADR-0063 (decisions#166), the plan template asking for files
  per phase and the backend checking `adds=` (greenharness/skills #14, #15),
  pinned by grh at skills 0.3.10. Plans written from now on can be sliced.
- **What decision 1 costs (to measure before it is answered).** Over the 45 saved round-1 → final pairs, count the edits confined to `## Phases`, and those that also touch Decision log, Tests, Verification or Context. Under the recommended option, the second group re-requires every sliced seat; under ADR-0061's list it does not. Report both as n/45.

## Non-goals
- Slicing the code-review brief: shipped in 3.22.0 (`review_brief.rs`).
- Slicing a panel seated from areas. Its seats bind the whole body, as
  today.
- Narrowing the shared brief. Each reviewer still gets the whole brief and
  the whole plan file. Only what binds it, and what it is asked to review,
  narrows.
- Splitting Verification rows by phase.
- A slice for a decisions plan with no Phases.

## Code review
- amont-agent: backend, lang:rust, unix, tui (the CLI's `plan-sha` and `plan-panel` output and the hook change)
- greenharness-skills: none — markdown only

## Behaviour
- **Sliced roles.** A role is sliced when all of these hold:
  - `profile_reads(role)` is `Some`: `lang:<x>`, react, ui-design or
    platform;
  - the panel was seated from paths;
  - at least one phase names a path the role reads.

  Otherwise the role binds the whole body. A seat from `adds=lang:<x>` with
  no phase of that type has nothing of its own to slice, so it binds the
  whole body.
- **The slice text.** It is computed on the canonical body. Preserve the effective reference-link and image definitions used by retained content, resolving them against the complete canonical body. Test that changing a used definition in an excluded section moves the affected slice sha, while changing an unused definition does not.
  - The shared sections are Goal, Non-goals, Behaviour and Code review
    (decision 1, ADR-0061's list), plus any `## Phases` text before its
    first item and every `🧑` decision item. Context, Tests, Decision log,
    Verification and Outcome are outside every slice: an edit there leaves
    sliced seats current, and backend, which binds the whole body, reviews
    it. A test pins this for a Decision-log edit and a Verification row.
  - The phases kept are the ones that name a path the role reads.
  - Every other phase item is removed: its top-level line and its
    continuation lines.

  The slice sha is `sha256(words("slice/1\n" + slice))`, which uses the
  same CommonMark reading as `body_sha`. A later change to the slice rule
  bumps `slice/1`, so old slice shas turn stale, never falsely current.
- **`amont-agent plan-sha --block --slice <role> <plan.md>`** prints:
  - first line: `<<<PLAN path=<realpath> sha=<body sha> slice=<slice sha>>>>`,
    with `lang=<x>` added for a `lang:` role. `sha=` stays the body sha,
    so `plan-apply` and the outcome ledger keep working;
  - second line: `Your slice: Phase 2, Phase 4 and the shared sections. The other phases belong to other seats: read them only to follow yours, and raise a finding there only if it blocks.`

  `PLAN_SHA_USAGE` documents `--slice` and replaces "Prints one line on
  stdout" (`src/main.rs:1054`) with: one line, or two with `--slice`.

  Errors and edge cases:
  - `--slice backend` (or any whole-body role) exits 2: `backend binds the whole body; drop --slice`.
  - A plan seated from areas, or a role with no phase of its own, prints
    the plain block (no `slice=`), with
    `amont-agent: <role> binds the whole body (seats=areas)` on stderr, and
    exits 0, so the skill never has to branch.
  - `--slice lang:go --lang rust` exits 2: `--slice lang:go conflicts with --lang rust; pass --lang go or drop --lang`.
  - `--slice` without `--block` exits 2: `--slice needs --block: amont-agent plan-sha --block --slice <role> <plan.md>`.
  - `--slice` with no value exits 2: `--slice needs a value`.
  - an unknown role exits 2: `unknown role <x>; roles: <list>`.
- **`plan-panel`** adds ` --slice <role>` to each sliced agent line (for
  example `plan-review-language --lang rust --slice lang:rust`). The
  header line is unchanged. A delta lists the roles the hook will require,
  computed by the same function.
- **The hook (`judge`):**
  - A binding is current for a whole-body role when its `sha` is this body
    (or the legacy sha). For a sliced role it is current when it is this
    body, or when its `slice` equals the role's current slice sha.
  - No baseline: as today (decision 2).
  - Delta: backend, plus the roles of new areas, plus every sliced role
    seated before whose slice sha differs from the baseline's, plus a sliced
    role with no entry in a baseline that has `slices` (it was whole-body
    then), plus a role in the baseline's `slices` that now binds the whole
    body.
  - Baseline from before this release (the `slices` key absent; `slices: {}` is a 3.23.0 baseline with nothing sliced): today's rule
    (decision 3), with `slices=unknown` in the journal excerpt.
  - The refusal names a moved slice in its own group, apart from
    `stale (reviewed an older body)`, and ends with the re-run line:
    `stale (its slice changed: Phase 2) for repos=…: lang:rust; re-run: amont-agent plan-sha --block --lang rust --slice lang:rust <plan.md>`.
- **The baseline** gains `slices: {role: {sha, spans: {label: sha}}}` for
  every sliced role on a pass (shared spans labelled `shared`), so the
  refusal can name the span that moved. `load` reads it when present, and `names` stays 2. A `slices` value of the wrong shape is read as absent (decision 3's rule), never as a missing baseline; a unit test covers it.
  `by-body` and `by-title` keys are unchanged. Downgrade: 3.22.0's `load` ignores `slices`, and a pass it writes drops them, so the next 3.23.0 delta uses the old-baseline rule and never refuses on a load error.
- **The skill.** `plan-review` §3.2 and §5.2 run the line `plan-panel`
  prints (`plan-sha --block [--lang x] [--slice role]`), and §3.3 changes
  "the review block line, verbatim" to "the review block's stdout lines
  (one, or two with `--slice`), verbatim"; paste stdout only, never stderr. §5 adds: in a delta, re-run backend, `rework` verdicts and
  every seat `plan-panel` names, and resume a sliced seat with its slice's
  diff.
- `docs/plan-review.md` and `CHANGELOG.md` say all of this.

## Phases
- [x] Phase 1 — `crates/amont-agent-process/src/file_types.rs`: `phase_spans` returns each phase's line range (`phases` becomes a wrapper), and `🧑` items and the Phases preamble become shared spans; `crates/amont-agent-process/src/plan_text.rs`: `slice(text, keep)` and `slice_sha`; unit tests beside each.
- [x] Phase 2 — `crates/amont-agent-process/src/plan_review.rs`: `Binding.slice`, `parse_blocks` reads `slice=` from the whole tail (today `find_map` consumes it), `block` takes the slice, `sliced_roles(body)`, `Baseline.slices` in `load`/`save`, one `delta_needs` shared by `judge` and `panel`, and the stale wording; `crates/amont-agent-process/src/plan_apply.rs` and `crates/amont-agent-process/src/review_outcomes.rs` adjusted for the new `Binding` field only; `tests/plan_review.rs`.
- [x] Phase 3 — `src/main.rs`: `plan-sha --slice` (flags, conflicts, usage text) and `plan-panel`'s ` --slice` suffix; `tests/plan_review.rs` CLI cases.
- [ ] Phase 4 — greenharness-skills `parts/workflow/skills/plan-review/SKILL.md` §3.2, §5; released through grh as its own PR in the same session, before 3.23.0 is installed (older amont-agent without `--slice`: the skill falls back to the plain block when `plan-panel` prints no `--slice`).
- [x] Phase 5 — `docs/plan-review.md`, `CHANGELOG.md`; set `docs/plans/2026-11-07-review-seats-follow-named-files.md` status `done` (map.md: "rides the slice-binding branch").
- [ ] Phase 6 — `Cargo.toml` version: release amont-agent 3.23.0 after the merge, once greenharness-skills ships Phase 4, and install it.

## Tests
- Pure, `file_types`:
  - spans cover every Phases line exactly once;
  - a fenced example inside Phases stays inside its phase;
  - `🧑` and the preamble are shared.
- Pure, `plan_text`:
  - an edit in a phase of another type keeps the slice sha;
  - an edit in its own phase, or in Goal, Non-goals, Behaviour or Code
    review, moves it; an edit in Context, Decision log or a Verification
    row keeps it (decision 1 as recommended);
  - a reformat that keeps `body_sha` keeps the slice sha;
  - the `slice/1` prefix means a slice holding every phase is not equal to
    `body_sha`.
- Pure, `judge`, as a table:
  - delta with a rust-only edit → requires backend and `lang:rust`;
    `lang:typescript` stays current;
  - delta with a ts-only edit → `lang:typescript`;
  - delta from an old baseline with no slices → backend only;
  - delta that turns an areas-seated plan into a paths-seated one → every newly sliced role required;
  - delta that removes a role's last phase → that role required;
  - areas-seated plan → no role sliced, today's outcome;
  - full mode → unchanged;
  - a binding with `slice=` for a whole-body role is ignored;
  - a `slice=` that is not 64 hex characters binds nothing.
- `parse_blocks`: `lang=` and `slice=` in either order; an old block with
  no `slice=` reads as today.
- CLI (`tests/plan_review.rs`):
  - `--slice` output, with its two lines;
  - every conflict exits 2 with its message;
  - an areas plan gives the plain block and the stderr line;
  - `plan-panel` prints the suffix.
- `plan-apply` still finds a sliced reviewer's patches, by `sha=` body.

## Decision log
- 2026-11-07 — the person approved the plan with the four 👉 decisions as recommended: (1) ADR-0061's shared sections; (2) slices checked in deltas only; (3) a baseline without slices keeps the old rule; (4) unix and tui bind the whole body. Decision 4 departs from ADR-0063's file-type seating of unix and tui only in what they bind, not in where they sit.
- 2026-11-07 — Codex's carried medium (a slice-rule version bump bypassed by the `current` fast path and the `sha=` fallback): deliberate. A review of the same body read the same text whatever the slice rule says, so the body sha stays current; the rule version only keeps an old slice sha from matching a different body.
- 2026-11-07 — the fleet pack refresh (`.adr/packs/decisions.pack`) is its own commit at the branch's start (worktree-task Start), named by no phase.
- 2026-11-07 (code review) — a pass on a baseline from before slices records `slices` absent, not the current slices: its delta asked backend alone, so no sliced seat reviewed them (backend). Refusals name each case in words (`Moved`), and `slice_for` says why a role binds the whole body (`WholeBody`), so `plan-sha` prints the reason the hook used (rust).
- 2026-11-07 (draft) — `sha=` stays the body sha and `slice=` is added,
  instead of a slice sha replacing it, because `plan_apply.rs:388` and
  `review_outcomes.rs:264` match reviews by body sha.
- 2026-11-07 (draft) — t06's "every phase that names no path" clause never
  applies. Under 3.22.0 such a plan is seated from areas, and areas bind
  the whole body.
- 2026-11-07 (draft) — the sliced roles are exactly `profile_reads`'s
  `Some` roles, so a plan slice and a code-review brief can never disagree
  about a file.

## Verification
Risk triggers (ADR-0060): a hook backtested on real past inputs; a persisted
format (the baseline JSON); a contract another repository consumes (the block
and `plan-panel` lines the skill pastes); the release path.
- **Backtest.** Run the branch binary's `plan-panel`, and the hook
  replayed, over the 45 saved round-1 bodies, each paired with its final
  body, under a scratch `CLAUDE_CONFIG_DIR`. Expected: 0 sliced roles
  (all seated from areas), and outcomes identical to 3.22.0. Add a second backtest over plans that name a file in every phase, where slices are non-empty; this is the only check that exercises the sliced path on real input. A difference
  would mean the change leaks into areas plans.
- **Real delta.** Take this plan after the panel, then edit Phase 3's
  text only (`src/main.rs`, rust) and run the hook with a baseline.
  Expected: refused, `stale (its slice changed: Phase 3) … lang:rust`.
  Today it passes with backend alone, the gap this closes.
- **Two sliced seats.** A scratch plan over a Rust and a TypeScript fixture repository, every phase naming a file. After a pass, edit only the TypeScript phase. Expected: refused naming `lang:typescript` alone; `lang:rust` absent from the refusal.
- **Old baseline.** Copy a real baseline JSON written by 3.22.0 into the
  scratch store, then run a delta. Expected: loads, backend-only rule,
  `slices=unknown`. A load error would refuse every old plan.
- **Downstream readers.** On one real session with a sliced reviewer:
  - `plan-apply` applies that reviewer's patches;
  - `review-outcomes` records the row with the body sha.

  Either failing would mean the block format broke them.
- **After installing 3.23.0:** the next plan that names files in every
  phase prints a `--slice` line, and the hook accepts its panel.
- **14-day measure (DUE 11-21).** For each delta on a plan seated from paths, record the sliced seats the hook required again, the accepted high or blocking findings those re-runs made (from the review-outcomes ledger), and the panel's tokens. Roll back the delta rule (keep `slice=` in the block) if 5 or more such re-runs make 0 accepted high or blocking findings, or if the median panel goes over 150k tokens. The journal line carries `sliced=<roles>` and, on a delta, `slice-stale=<roles>`; the measure also counts the deltas that kept a sliced role current (0 means slicing saved nothing).


### Results (2026-11-07, branch binary, before the push)
- **Backtest** (`/tmp/claude-501/slice-backtest.WPWN/backtest.py`, 3.22.0 vs branch): 46 saved round-1 panels compared (41 bodies not saved, 15 with no round-1 rows, 2 unresolvable repos). `plan-panel` identical on all 46 apart from the ` --slice` suffix. One body has a sliced seat: this plan's round 1 (`lang:rust`), whose round-1 → final pair moves the slice (review edits touched Behaviour, a shared section). The second backtest has no other input yet: no other saved plan names a file in every phase.
- **Real delta** (this plan, this session's transcript, scratch store): the original body passes and saves `slices` for `lang:rust` with spans Phase 1, 2, 3, 5 and `shared`; Phase 3's text edited → refused: `stale (reviewed an older body) … backend; stale (its slice changed: Phase 3) … lang:rust; re-run: amont-agent plan-sha --block --lang rust --slice lang:rust …`. Expected and observed. Journal: `sliced=lang:rust slice-stale=lang:rust`.
- **Old baseline** (a copy of the real 3.22.0 store): the same edit → `stale … backend` only, journal `slices=unknown`. No load error.
- **Two sliced seats**, run as an integration test, not on scratch repositories by hand (`tests/plan_review.rs` `a_delta_to_one_language_phase_re_requires_that_seat_alone`, a Rust + TypeScript fixture repository): a TypeScript-phase edit refuses `lang:typescript` alone with its re-run line; a Context edit then asks backend alone.
- **CLI** (`plan_sha_slice_prints_the_block_and_the_slice_line`, and by hand on this plan): `--slice lang:rust` prints 2 stdout lines, empty stderr; every conflict exits 2 with its message; an areas plan prints the plain block and `binds the whole body (seats=areas)`.
- **Downstream readers:** `parse_blocks` keeps `sha=` as the body sha with `slice=` beside it (unit test), which is what `plan-apply` and the outcome ledger match on; not yet run on a real session with a sliced reviewer, which the next plan naming files in every phase will be.
- Old baseline, then two deltas (`a_pass_on_a_baseline_from_before_slices_keeps_the_old_rule`, after code review): the pass on a 3.22.0 baseline records no slices, so a second Rust delta is still judged by the old rule.
- `plan-apply` finds a sliced language reviewer's patches by the body `sha=` (`tests/plan_apply.rs` `a_sliced_reviewers_patches_are_found_by_its_body_sha`).

## Implementation review
- implementation-review: round 1 approve-with-changes (3 blocking code-rule findings: out-parameter and flag in `slice_flag`, a banner, long-lived short names; all fixed), delta approve. 105k+114k, 238 s+37 s.
- backend: approve-with-changes (an old-baseline pass recorded unreviewed slices; empty span list; whitespace in labels; all fixed), delta approve; deliberate: the `holds-until` end condition. 85k+111k.
- lang:rust: approve-with-changes (sentinel strings → `Moved`, recomputed reason → `WholeBody`, out-parameter, generic `guarded`, labels; all fixed), delta approve-with-changes; deliberate: `NAMED_ROLES` stays beside `roles`, `describe` over `Display`, shell-quoting beyond whitespace (harness plan paths hold no `'` or `$`). 79k+106k.
- unix: approve-with-changes (`=` forms, help wins; fixed), delta approve-with-changes; deliberate: the same path quoting. tui: approve-with-changes (backticked re-run, wording, roles list, `no phases`; fixed), delta approve; deliberate: the `=` forms and help-wins go in the release's CHANGELOG and usage, not this tree. 56k+83k, 52k+78k.

## Outcome
Phases 1–3 and Phase 5's docs on this branch; Phase 4 in greenharness-skills `feat/plan-review-slices`. Next: merge both, release the skills, then amont-agent 3.23.0, and install it. 14-day measure DUE 2026-11-21.

<!-- panel: repos=amont-agent,greenharness-skills adds=contract reviewers=backend,architect,po,lang:rust,tui,unix,codex body-sha=60b87cfbf4b7 -->
