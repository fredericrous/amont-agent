---
status: active
branch: fix/profile-line-in-header
repos: [amont-agent]
adrs: [ADR-0052]
---
# Plan: count only the review prompt's own Profile line

## Review panel

👉 **Decide:** none. Approve if the hook should read only the prompt's header for `Profile:`, so inlined briefs and diffs never void a review.
📍 amont-agent · not started · next: Phase 1, then this branch's own F4b as the ADR-0052 dogfood. Panel: backend (opus), codex (gpt-6-astra).
**Changed by review:**
- The header also ends at an unfenced `diff --git` or `@@`, and a heading means `#{1,6}` then a space.
- The inlined-brief test asserts exactly one pass file. The token bar is set at 1.29M.
📄 Full reviews: [2026-11-01-count-only-the-prompts-profile-line.reviews.md](2026-11-01-count-only-the-prompts-profile-line.reviews.md)
**Verdicts:** backend approve-with-changes twice; Codex approve-with-changes. 4 findings carried into implementation: tilde fences, a Delta test that proves the resumed round counted, the docs, and the unit-test count.

## Context

ADR-0052's push hook counts a code-review profile's review only when the prompt holds exactly one `Profile: <role>` line (`code_review::profile_lines`, `crates/amont-agent-process/src/code_review.rs:132`). Today it reads every line of the prompt. That causes two problems:
- **An inlined brief voids the review.** `review-brief --profile` prints its own `Profile:` line. If the brief is pasted inline, the prompt holds two Profile lines, and the hook counts the review for nothing. The skills (worktree-task step 2c) work around this by passing the brief by path.
- **An inlined diff can void a Delta.** A profile's Delta round carries "the diff since" inline. A diff line that trims to `Profile: …` would add a second Profile line and void the round. This is an open follow-up of plan `2026-10-31-plan-names-its-code-reviewers`.

The fix belongs in the hook, so the rule holds whatever the author pastes.

## Goal

The hook counts only the `Profile:` lines in the prompt's header. The header is the lines before the first Markdown heading, the first code fence (backticks or tildes), or an unfenced diff. An inlined brief, which starts with `# … review brief`, and an inlined diff, which sits in a fenced block, can no longer add a Profile line.

## Non-goals

- The skills' advice to pass a profile's brief by path stays. It is still the cheaper prompt. Its reason ("a prompt with two counts for nothing", worktree-task SKILL.md:264) is reworded at grh's next release.
- grh's amont-agent pin moves at grh's next release, not in this plan. The laptop takes 3.11.1 with `brew upgrade`.
- No change to the binding rules themselves: exactly one Profile line, the agent map, and a resumed round inheriting its launch's profile.

## Code review
- amont-agent: backend, lang:rust (unix and tui left out: no command line, output or message changes; only which prompt lines the hook reads)

## Behaviour

- `profile_lines(prompt)` scans the prompt from its first line. It stops at the first line whose trimmed text is a Markdown heading (`#{1,6}` then a space), starts with three or more backticks or tildes (a fence), or starts with `diff --git` or `@@` (an unfenced diff). It returns the `Profile:` values of the lines before that point, as today.
- A prompt with `TREE`, `Round 1.`, `RULES`, the `CANDIDATES` block and `Profile: unix`, then an inlined brief → one Profile line (`unix`). The brief's own `Profile: unix` comes after its `# Implementation review brief` heading and is not read.
- A Delta message holding the block, `Delta.` and a fenced diff with a `+Profile: x` or ` Profile: x` line → no Profile line from the diff, so the round inherits its launch's profile.
- A prompt whose only `Profile:` line sits inside the inlined brief → no Profile line. A fresh launch counts for nothing, as with a missing line today. The skills already put the line in the header.

## Phases

- [x] Phase 1 — amont-agent:
  - `profile_lines` reads only the header.
  - Unit tests in code_review.rs: a header line counts; a line after a heading is ignored; a line inside a fence is ignored; a ` Profile: x` context line after an unfenced `diff --git` is ignored; a header line `#174 …` does not end the header; the existing indentation test still holds.
  - Integration tests in `tests/implementation_review.rs`: a fresh launch with an inlined brief binds and files exactly one pass for its profile and tree; a resumed Delta with a fenced diff carrying a `Profile:` line binds under its launch's profile.
  - CHANGELOG entry (`## Unreleased`).
- [ ] Phase 1b — release 3.11.1 (tag-release), after the merge; the release commit renames the entry.
- [ ] Phase 2 — dogfood (the ADR-0052 plan's Phase 6): this branch's F4b launches implementation-review, `code-review-backend` and `code-review-language` (`Profile: lang:rust`) in one message.
  - The push hook must accept the push only once all three are bound.
  - The provenance payload must hold the two `code-review` lines.
  - Record the F4b tokens against the generic-only baseline: implementation-review alone on the amont-agent #174 branch took 131k + 146k + 153k.

## Verification

- Phase 1, observed:
  - `cargo test -p amont-agent-process code_review::tests` → 14 passed, the five new tests included: header, heading, fence (backticks, tildes, ````` ````diff `````), unfenced diff, `#174`.
  - With the old `profile_lines` body swapped back in → 3 failed (heading, fence, unfenced diff) and the header and `#174` tests passed. The file was then restored and compared with `cmp`.
  - `cargo test --test implementation_review -- inlined_brief delta_with_a_diff resumed_round` → 5 passed:
    - an inlined brief binds and files exactly 1 pass;
    - the Delta from a `rework` launch binds the new tree, and without the Delta that tree has no pass and the push is advised.

- Phase 1, as planned: `cargo test -p amont-agent-process code_review` → the five new unit tests and the existing indentation test pass, and the heading, fence and unfenced-diff tests fail on the old `profile_lines` (observed above). `cargo test --test implementation_review` → the two new tests pass, and every existing one is unchanged.
- Phase 2:
  - Before the profile reviews run: `amont land` → refused with `missing code review of amont-agent tree <12 hex>: code-review-backend, code-review-language --lang rust`. After they run: allowed.
  - The provenance record under `~/.claude/amont-agent/provenance/amont-agent/<commit>` holds `code-review backend reviewed <tree>` and `code-review lang:rust reviewed <tree>`.
  - Tokens: the sum of the three reviewers, against implementation-review alone. The bar is 3× (ADR-0052 Consequences): 131k + 146k + 153k = 430k, so 3× is 1.29M.

## Decision log
- 2026-11-01 — Implemented by hand, not through relais: every relais run on amont-agent today hit its wall time, and this change is 20 lines. The fence rule also covers tildes, and the Delta test starts from a `rework` launch on an earlier tree, so its pass can come only from the resumed round (both from Codex's review).

## Outcome

<!-- panel: repos=amont-agent reviewers=backend,codex body-sha=8af33c62082b -->
