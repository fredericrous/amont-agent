---
status: done
branch: fix/carry-from-transcript (amont-agent), fix/brief-reason (greenharness-skills), chore/pin-0.1.6 (grh)
repos: [amont-agent, greenharness-skills, grh]
adrs: [ADR-0052]
---
# Plan: an approval carries from the session, and the review header is one rule

## Review panel

👉 **Decide:** none. Approve if an `approve` given in this session should carry to the pushed tree without a re-run. A cross-repository (pointer) plan gets no session carry for now, because a pointer records no canonical revision.
📍 amont-agent, greenharness-skills, grh · not started · next: Phase 1 (amont-agent 3.13.0). Panel: backend (opus), codex (gpt-6-astra).
**Changed by review:**
- The session is read before filed passes, so a newer rework blocks any carry. An unreadable declaration at the reviewed commit means no carry.
- A pointer's historical declaration needs a fixed revision, and without one there is no carry. The live check is now a sequence that always runs.
📄 Full reviews: [2026-11-02-an-approval-carries-from-the-session.reviews.md](2026-11-02-an-approval-carries-from-the-session.reviews.md)
**Verdicts:** backend approve-with-changes three times; Codex approve-with-changes. 7 findings carried into implementation:
- a 0-re-run bar due 11-15;
- seven carry tests;
- a Missing-declaration test;
- the rollback line's placement;
- a per-commit plan list;
- Missing falls through to filed passes;
- an unchanged-pointer test.

## Context

The ADR-0052 dogfood (amont-agent #177) left four items open in the canonical plan `decisions/docs/plans/2026-10-31-plan-names-its-code-reviewers.md`:

1. **An approval never carries in the usual flow.** `ProfileGate::carried` (`crates/amont-agent-process/src/implementation_review.rs:814`) reads only pass files the hook filed when it judged an earlier push. A session that reviews, fixes and then pushes once never judged a push at the approved tree, so the profile's `approve` is ignored. On #177, both profiles re-ran on a docs-only diff. `work.code-review-delta` promises the carry; today it only works across two pushes.
2. **implementation-review's own block scan reads the whole prompt.** `reviews()` (`implementation_review.rs:280`) calls `parse_blocks(&c.prompt)`. A `<<<TREE …>>>` block inside an inlined brief or diff can bind a tree the reviewer did not read, the defect #177 fixed for the profiles.
3. **The skill's reason is now untrue.** greenharness-skills worktree-task step 2c and the seven agent files say a brief goes by path because "a prompt with two Profile lines counts for nothing". Since amont-agent 3.11.1, only the header counts.
4. **Two lows from the lang:rust review of #177.** `code_review::header` is `pub` with one in-crate caller, and the definition of the body sits on `profile_lines`, not on `header`.

## Goal

An `approve` a profile gave in this session to an earlier tree of the branch carries to the pushed tree under the same conditions as a filed pass. Both review scans read the prompt's header only. The skill says why a brief still goes by path. The code tidy-ups are done.

## Non-goals

- No change to the carry's conditions (ADR-0052 `work.code-review-delta`): verdict `approve` only, a commit in `pushed ^merge-base`, the same plan and declaration, and implementation-review passing the pushed tree. Only where the earlier approval is found changes.
- No carry for implementation-review itself: it always reviews the pushed tree, as today.
- No change to the refusal text.
- The application-landscape findings (double undo, touch editing, raw px, stale pack) stay with that repository's owner.

## Code review
- amont-agent: backend, lang:rust (unix and tui left out: no command line, output or message changes)
- greenharness-skills: none — markdown wording only
- grh: none — pin bump only

## Behaviour

### The carry also reads the session (amont-agent)

When a declared profile has no pass on the pushed tree and no review of it in the transcript, `carried` looks at the transcript first, then at filed passes. If this profile's latest transcript review of any branch tree is not `approve`, nothing carries, a filed pass included.
- **The latest review only.** It takes this profile's latest completed review, in launch order, whose block names this repository and a tree of a commit in `pushed ^merge-base`, other than the pushed tree. A later `rework` or `approve-with-changes` of an earlier tree therefore blocks the carry.
- **Its verdict** must be `approve`.
- **The declaration must match.** The declaration is read at that commit, the newest branch commit whose content id is the reviewed tree. This uses the same `declaration()` logic, refactored to take the commit it reads. Its plan path and digest must equal the current ones. For transcript carry, a pointer plan's historical declaration must come from an immutable canonical revision tied to the reviewed commit; without that evidence, the transcript candidate yields `Missing`. Never resolve a historical declaration through today's canonical working-tree file. Add an integration test with no filed pass: approve A through a pointer plan, change the canonical declaration, then push B after implementation-review; no transcript carry is allowed. A declaration that cannot be read at that commit means no carry (`Missing`), never an `unknown` refusal.
- **implementation-review must pass the pushed tree**, as for every carry today (`hold()` only reaches the profiles after that).
- **When it carries,** it writes the pushed tree's pass with `how = "carried-from <12 hex>"` and the reviewed commit as its basis, and journals `profile-carried … carried-from <12 hex> <shortstat>`. That is the shape of today's carry, so a carry chain still records the first reviewed commit.

### One header for both scans (amont-agent)

- `reviews()` reads review blocks from `code_review::header(&c.prompt)`, like `profile_reviews`.
- An implementation-review prompt then binds only the TREE block before its first heading, fence or unfenced diff. A prompt that opens with the inlined brief and carries no TREE line before it binds nothing: the push is refused with "launch the implementation-review agent". worktree-task already puts the TREE line first.
- `parse_rules_sha` and the CANDIDATES read are unchanged.

### Tidy-ups (amont-agent)

- `header` becomes `pub(crate)`.
- The definition of the body (heading, fence, unfenced diff) moves to `header`'s doc comment, and `profile_lines` points to it.

### The skill's reason (greenharness-skills)

- In worktree-task step 2c and the seven `code-review-*.md` input paragraphs, the reason becomes: the brief is long, so passing it by path keeps the prompt small. The hook reads only the header, so the prompt's own `Profile:` and TREE lines must come before any pasted text.

## Phases

- [x] Phase 1 — amont-agent:
  - The transcript carry, `declaration()` taking a commit, `reviews()` on the header, and the tidy-ups.
  - Tests in `tests/implementation_review.rs`:
    - a backend `approve` of tree A, a fix at B, implementation-review of B, one push → allowed and journalled `carried-from`, with no push judged at A;
    - the same with a later `rework` of A → refused;
    - with `approve-with-changes` → refused;
    - a pass filed by a push at A, then a transcript `rework` of A, then a push at B → refused;
    - with the declaration changed between A and B → refused;
    - after a rebase that moves A out of `pushed ^merge-base` → refused;
    - an implementation-review prompt whose only TREE block for the pushed tree sits inside a fenced body → refused, and the same block in the header → allowed.
  - CHANGELOG and docs/implementation-review.md. Release 3.13.0 (tag-release): a minor version, since what binds changes.
- [x] Phase 2 — greenharness-skills: the reworded reason. Release 0.3.1.
- [x] Phase 3 — grh: pin amont-agent 3.13.0 and skills 0.3.1. Release 0.1.6 (0.1.5 was taken), then `brew upgrade` and `grh install workflow --yes` on the laptop.

## Verification

- Phase 1, observed (the swap-back on 76369ed, the 17-test run on ae5b72b):
  - On the fix round: `cargo test --test implementation_review -- session_ carry carries implementation_review_block_in_the_body a_declaration_changed_since an_unreadable_declaration a_pointer_plan_gets a_rebased_branch` → 17 passed: the 8 existing carry tests and 9 new ones, including `a_rebased_branch_does_not_carry_from_the_session` and the header test `an_implementation_review_block_in_the_body_binds_nothing`.
  - With the old behaviour swapped in (`SessionCarry::Undecided` in place of the session read, and `parse_blocks(&c.prompt)` in `reviews()`), the session-carry test and the header test → FAILED. Restored, and compared with `cmp`.
  - The gate (clippy, the full suite) passed at commit.

- Phase 1: the new integration tests pass, and the existing carry tests still pass. With `carried` restored to pass files only, the transcript-carry test fails; with `reviews()` back on the whole prompt, the fenced-block test fails. `cargo test` → 0 failed. Live check (deferred: see Decision log 2026-11-02, holds-until the first plan PR after 3.13.0): after both profiles approve, commit a docs-only fix, run implementation-review on it, push once → the push is allowed, with no profile re-run, and the journal holds `profile-carried … profile=backend carried-from <12 hex>` and the same for `lang:rust`. Production: over 14 days, count `profile-carried` lines against profile re-runs on diffs of 10 lines or fewer.
- Phase 2, observed on greenharness/skills #5 and release 0.3.1: `check.sh` → exit 0; `grep -rn "counts for nothing" parts` → 0 lines; the 0.3.1 tarball checks `OK`, and its seven agents hold the new reason. Planned: `scripts/check.sh` → exit 0, no "counts for nothing" phrase is left under `parts/`, and the implementation-review launch text in worktree-task puts its TREE line before any heading or fence (grep). Rollback for Phase 1: pin amont-agent 3.12.x again.
- Phase 3, observed on greenharness/grh #10 and release 0.1.6:
  - each pin equals its SHA256SUMS line;
  - on the laptop: `brew upgrade`, then `grh install workflow --yes` → grh 0.1.6, `amont-agent --version` 3.13.0;
  - the seven installed agents and worktree-task hold the new reason, with 0 old phrases left;
  - doctor → `healthy: true`.
- Phase 3, as planned: the pins equal each release's SHA256SUMS lines. After the install, `amont-agent --version` → 3.13.0, the agents' text holds the new reason, and doctor → healthy.

## Implementation review

- Phase 1, profiles declared `backend, lang:rust`:
  - implementation-review: approve-with-changes twice. Fixed: a session-path rebase test and the exact test record. Plan text only after that.
  - code-review-backend: approve-with-changes, then approve. Fixed: the rebase test, and the journal lines saying why a carry did not happen.
  - code-review-language: approve-with-changes twice. Fixed: derived `Default`, named `Carry` fields and the journal lines. Deliberate: `Result<Option<_>>`, no per-commit cache, `unwrap_or_else` (clippy passed in the gate).
- Tokens: about 59k + 80k, 60k + 64k, 56k + 62k, so 381k.

## Decision log
- 2026-11-02 — grh 0.1.5 was released by another session (#9, pinning amont-agent 3.12.0), so this ships as grh 0.1.6.
- 2026-11-02 — Another session released 3.12.0 (#179) during this work, so this ships as 3.13.0. The plan's versions now say 3.13.0. The two "(3.12.0)" mentions in `docs/implementation-review.md` are corrected in the 3.13.0 release commit, so the reviewed tree stays bound. skills 0.3.1 and grh 0.1.5 are checked as free before tagging.
- 2026-11-02 — From the code reviews:
  - added a session-path rebase test, and journal lines `profile-carry-blocked` and `profile-carry-declaration-differs`, so a carry that does not happen says why;
  - `#[derive(Default)]` on `Declaration` and named `Carry` fields;
  - deliberate: `declaration_of` keeps `Result<Option<_>>`, where `None` is a pointer under `Unproven`, and there is no per-commit cache, since branches are short.
- 2026-11-02 — The live check cannot run on this branch: the push hook that judges its push is the installed 3.11.1, not this code. It runs at the first plan PR pushed after 3.13.0 is installed: `holds-until:` that push, recorded in the canonical ADR-0052 plan's follow-ups.
- 2026-11-02 — Built by hand, not through relais: every relais run on amont-agent hit its wall time. One change came from implementing it: a content id leaves `docs/plans/` out, so a plan-only commit shares its tree with the commit before it. "The newest commit with the reviewed tree" could therefore read a declaration changed after the review. The declaration must now match at every branch commit with that tree. The test `a_declaration_changed_since_the_session_approve_does_not_carry` failed on the first version and passes on this one.
- 2026-11-02 — Done from the carried findings: an unreadable declaration means no carry, not unknown (tested under deny); an unchanged pointer gets no session carry either (tested); Missing falls through to filed passes; each commit's plan list is computed against the merge-base (`declaration_at`). Not done: a separate restored-plan test (Codex). The per-commit plan list covers it by construction, and no test pins it.

## Outcome

Shipped: amont-agent 3.13.0 (#180, #181), skills 0.3.1 (#5), grh 0.1.6 (#10), installed on the laptop.
- **The session carry:** a profile's `approve` from earlier in the session now carries to a fix commit, so profiles that already approved no longer re-run.
- **The block scan:** implementation-review's block is read from the header only.
- **The skill wording:** it gives the true reason a brief goes by path.

Still open:
- **The live check:** the first plan PR pushed under 3.13.0 should show `profile-carried … carried-from` for its approving profiles, with no re-run.
- **The production bar:** due 2026-11-15, 0 profile re-runs on diffs of 10 lines or fewer after an approve.
- **A pointer's canonical revision:** recording it would let a cross-repository plan carry from the session too.

<!-- panel: repos=amont-agent,greenharness-skills,grh reviewers=backend,codex body-sha=c7bf8dc59c72 -->
