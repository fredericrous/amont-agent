---
status: done
branch: fix/pointer-and-plan-only (amont-agent), fix/pointer-code-review (greenharness-skills), chore/pin-0.1.7 (grh)
repos: [amont-agent, greenharness-skills, grh]
adrs: [ADR-0052]
---
# Plan: a pointer carries its own code-review line, and a plan-only branch needs no profile

## Review panel

👉 **Decide:** none. Approve if a branch that changes only its plan should skip the profile reviews (it keeps implementation-review), and if a pointer plan should declare through its own copied `## Code review` line.
📍 amont-agent, greenharness-skills, grh · done: amont-agent 3.14.0, skills 0.3.2, grh 0.1.7 · next: the two live checks (plan-only on this push, the pointer carry on the next cross-repository plan). Panel: backend (opus), codex (gpt-6-astra).
**Changed by review:**
- The plan-only check is its own `git diff --quiet`, which can't be fooled by git config and fails closed. A pointer's section must validly declare this repository.
- A pointer's stale line is journalled as `pointer-line-drift`. The test command names the right package.
📄 Full reviews: [2026-11-02-a-pointer-carries-its-own-line.reviews.md](2026-11-02-a-pointer-carries-its-own-line.reviews.md)
**Verdicts:** backend approve-with-changes three times; Codex approve-with-changes. 9 findings carried into implementation:
- `--no-relative` and top-anchored pathspecs;
- a gitlink mutation check;
- the drift test in the list;
- the plan-only skip runs before the declaration read;
- a git helper that returns the exit status;
- the full test list;
- `key=None` for a gitlink-only branch;
- a baseline test count;
- one redundant command.

## Context

Two gaps are left from ADR-0052, recorded in `docs/plans/2026-11-02-an-approval-carries-from-the-session.md`. The person chose both answers on 2026-11-02:

1. **A plan-only branch still needs every declared profile.** On amont-agent #183, a branch that changed nothing outside `docs/plans/` needed implementation-review plus `code-review-backend` and `code-review-language` to review an empty diff. Profiles review code, so they have nothing to read there. The person chose: skip the profiles, and keep implementation-review, which checks the plan record (on #183 it caught a stale status line and an unobserved claim).
2. **A pointer plan cannot carry an approval from the session.** A pointer only says `canonical: <repo>:<path>`. The hook resolves it through the plan root's working-tree checkout (`code_review::declared_text`, `crates/amont-agent-process/src/code_review.rs:287`), so what a pointer declared at an earlier commit cannot be proven, and `declaration_at` returns no declaration (`Canonical::Unproven`, `implementation_review.rs:786,805`). The person chose: the pointer carries its own repository's `## Code review` line, copied from the canonical plan when the pointer lands. The hook reads that line, which is versioned with the branch.

## Goal

- A branch whose diff outside `docs/plans/` is empty passes the profile gate. It still needs implementation-review.
- A pointer plan with its own `## Code review` section declares through that section, at every commit. The session carry then works for cross-repository plans too.

## Non-goals

- No change for a pointer without its own section: it still declares through its canonical plan, read from the checkout, and gets no session carry.
- No change to implementation-review on a plan-only branch: it still reviews the plan record.
- No backfill of existing pointers. They gain the line when a branch next lands one.

## Code review
- amont-agent: backend, lang:rust (unix and tui left out: no command line, output or message changes)
- greenharness-skills: none — markdown only
- grh: none — pin bump only

## Behaviour

### A plan-only branch (amont-agent)

`judge_push` already computes the branch patch outside `docs/plans/` (`patch_key`, `implementation_review.rs:96-118`, which errs "the branch changes nothing outside docs/plans/").
- Plan-only is computed on its own, not taken from `key` being `None` (that is also a git failure): `git diff --quiet --no-ext-diff --no-textconv --no-relative --ignore-submodules=none <merge-base> <pushed> -- ':(top)' ':(top,exclude)docs/plans/'` exit 0 = plan-only, exit 1 = code, any other result = today's path (no skip). Add a regression test with an implementation-review pass, backend declared, and a committed gitlink update outside docs/plans/ while `diff.ignoreSubmodules=all` is configured: the profile remains required and `profiles-plan-only` is absent.
- When that diff is empty, `ProfileGate::hold` returns no required profile, journals `profiles-plan-only`, and leaves the implementation-review gate unchanged.
- The provenance note then carries no `code-review` line.

### A pointer's own line (amont-agent)

- `declared_text(plan_text)` is unchanged for a plan without `canonical:`. For a pointer, if the pointer's own body has a `## Code review` section, the pointer is the declaration and the canonical is not read. Otherwise the canonical is read from the checkout, as today. At the pushed commit (`Canonical::Follow`), when the canonical is also readable and its line for this repository differs from the pointer's, the hook journals `pointer-line-drift` (pointer line, canonical line); a test covers it.
- A pointer's own section must parse successfully and contain a declaration for the current repository, either profiles or explicit `none — <reason>`. Enforce this in both the push declaration reader and `declared_line`; an empty section, a section naming only another repository, or malformed content is a declaration error, never an implicit exemption or canonical fallback. Canonical fallback remains only for pointers with no section. Test these cases under deny with code changes and an implementation-review pass, and test that review-brief reports the declaration error.
- `declaration_at` (the session carry's historical read) treats a pointer with its own valid repository declaration like any plan. An invalid local declaration yields no carry; test this at the reviewed commit. A pointer without a section keeps `Canonical::Unproven`, so it still gets no carry.
- `review-brief`'s `Declared code review for <repo>` line follows the same rule, since it calls `declared_text` (code_review.rs:310); `plan-panel` does not read pointers and is unchanged.

### The pointer template and the landing step (greenharness-skills)

- `worktree-task/plan-template.md` "Pointer files": the template gains a `## Code review` section holding this repository's line, copied verbatim from the canonical plan's section.
- `worktree-task/SKILL.md` "Land the plan" step 3: "write a pointer file" says to copy this repository's `## Code review` line into it.

## Phases

- [x] Phase 1 — amont-agent:
  - The plan-only skip in `ProfileGate::hold` and `declared_text` preferring the pointer's own section.
  - Tests in `tests/implementation_review.rs`:
    - a plan-only branch with `backend` declared and only an implementation-review pass → allowed, journal `profiles-plan-only`;
    - the same branch with no implementation review → refused, as today;
    - git diff failing (bad merge-base) with `backend` declared → not treated as plan-only: the profile is still required;
    - a pointer with its own `- app: backend` section and no readable canonical → refused naming `code-review-backend`, not `profiles-unreadable`;
    - a session carry through a pointer with its own section → allowed, `carried-from`;
    - a pointer without its own section → no session carry, as today.
  - Unit tests in code_review.rs for `declared_text` on a pointer with and without its own section.
  - CHANGELOG and docs/implementation-review.md. Release 3.14.0 (tag-release), after checking the version is free.
- [x] Phase 2 — greenharness-skills: the template and landing-step wording. Release the next free patch version.
- [x] Phase 3 — grh: pin both. Release the next free version, then `brew upgrade` and `grh install workflow --yes`.

## Verification

- Phase 1, observed:
  - `cargo test -p amont-agent-process code_review::tests` → 17 passed.
  - `cargo test -p amont-agent --test implementation_review -- plan_only gitlink pointer carr session_` → 25 passed: the 17 that pass on main and 8 new.
  - `cargo test -p amont-agent-guard diff_quiet` → 1 passed; `cargo test -p amont-agent-process declaration_at_needs` → 1 passed.
  - Mutations:
    - plan-only forced off, with `declared_text` back to canonical-only → `a_plan_only_branch_needs_no_profile` and `a_pointer_declares_through_its_own_line` FAILED;
    - `--ignore-submodules=none` removed → `a_gitlink_change_is_not_plan_only` FAILED;
    - `require_own_line` removed from `declaration_of` → `declaration_at_needs_the_pointers_own_line_for_this_repository` FAILED.
    Both files were restored and compared with `cmp`.
  - `cargo test --workspace` → 32 test binaries, 0 failed.

- Planned: Phase 1: the new tests pass and `cargo test -p amont-agent --test implementation_review carr` → every test that passes on main 860e5fc still passes, plus the new carry tests, 0 failed. The workspace suite (`cargo test --workspace`) → 0 failed. With the plan-only skip removed, the plan-only test fails; with `declared_text` back to canonical-only, the pointer-section test fails. `cargo test` → 0 failed.
- Planned: Live check, after 3.14.0 is installed: this plan's own closing record branch, which is plan-only, lands with no profile review, and the journal holds `profiles-plan-only`.
- Planned: Live check, on the next cross-repository plan whose pointer declares a profile: the pointer push after a session pass shows `carried-from` in the journal and no `profiles-unreadable`; record it in Outcome.
- Planned: Phase 2: `scripts/check.sh` → exit 0.
- Phase 2, observed: `scripts/check.sh` → exit 0; implementation-review approve after one Delta (greenharness/skills#6, released v0.3.2).
- Phase 3, observed: the five pins equal their SHA256SUMS lines (four amont-agent 3.14.0 targets and the skills 0.3.2 tarball, hashed after download); grh 0.1.7 released (greenharness/grh#11). After `brew upgrade grh amont-agent` and `grh install workflow --yes`: `amont-agent --version` → 3.14.0, `amont-agent doctor` → every check ✓.
- Planned: Phase 3: each pin equals its SHA256SUMS line. After the install, `amont-agent --version` reports 3.14.0 or the version chosen, and doctor → healthy.

## Implementation review

- approve on e01ac4f, after two Deltas. implementation-review: 51k + 69k + 76k, 46 s + 29 s + 15 s; code-review-backend: 58k + 73k + 76k; code-review-language (rust): 63k + 78k + 81k.
- Fixed: a unit test for the git failure, `profiles-plan-only-unknown`, three missing tests, the stale counts, `Declared` compared directly, the imports, and a guard unit test that can fail.
- deliberate: no `PlanSource` enum (logged above); the new unit test's scratch dir stays behind on a panic: the next run removes it first.

## Decision log
- 2026-11-02 — From the code reviews:
  - the planned "git diff fails" test is a unit test of `git::diff_quiet_in` (exit 0, exit 1, a failure), because through the hook a bad merge-base already ends as `unknown` before the plan-only diff runs;
  - an unanswerable plan-only check journals `profiles-plan-only-unknown` with its error;
  - new tests: the review-brief error, an empty own section, and no carry from an invalid pointer line;
  - `Declared` values are compared directly, and the error text is "exited N" or "killed by a signal".
  - the guard that gives no carry from an invalid pointer line is a unit test of `declaration_at`: the integration test's two declarations differ, so it proves only the outcome (delta review);
  - Deliberate: no `PlanSource` enum. `own` is computed once per plan, and a plan is small.
- 2026-11-02 — Implemented by hand. From the carried findings:
  - `--no-relative` and `:(top)` / `:(top,exclude)docs/plans/` pathspecs;
  - a `git::diff_quiet_in` helper that returns the exit status, where exit 1 and a failure stay distinct;
  - the plan-only skip runs before the declaration read;
  - the gitlink mutation check;
  - the drift test.
  `profiles-plan-only` is journalled on every plan-only push, not only when the declaration would have found profiles: reading the declaration first would bring back the hold the skip removes.
- 2026-11-02 — the person chose: skip the profiles on a plan-only branch, keeping implementation-review; a pointer copies its repository's Code review line.

## Outcome

- amont-agent 3.14.0 (#187, #188, #189), skills 0.3.2 (#6), grh 0.1.7 (#11), installed on the laptop.
- The first v3.14.0 tag push was refused by the release audit: RUSTSEC-2026-0433 (pulldown-cmark 0.13.5, published 2026-10-28). Since the tag had never been pushed, it was deleted locally, the exact pin was bumped to 0.13.6 in #189 (the pinned fixed-plan sha test passes, so plan shas are unchanged), and v3.14.0 was tagged on that merge.
- The hook installed before 3.14.0 refused the skills pointer's land: its canonical plan was not yet in the live amont-agent checkout. That is the gap this plan closes. A fast-forward of the live checkout let the land go through.
- Open: the two live checks. This closing branch is plan-only, so its push should journal `profiles-plan-only`. The pointer carry waits on the next cross-repository plan whose pointer declares a profile.

<!-- panel: repos=amont-agent,greenharness-skills,grh reviewers=backend,codex body-sha=834a6a970c68 -->
