# Full reviews: count only the review prompt's own Profile line

Reference for [the plan](2026-11-01-count-only-the-prompts-profile-line.md).

- **backend** round 1 (29k, 46 s), approve-with-changes, all applied:
  - (medium) an unfenced diff → stop at `diff --git` or `@@`;
  - (low) the heading pattern;
  - (low) one pass file for an inlined brief;
  - (low) the skill's by-path reason;
  - (low) the token bar.
- **backend** round 2 (22k, 16 s), approve-with-changes. All resolved. Carried into implementation: (low) the unit-test count, six not four.
- **codex** (gpt-6-astra, high effort), approve-with-changes, carried into implementation:
  - (medium) tilde fences;
  - (medium) the Delta test must start from a `rework` launch and a changed tree, so the pass comes only from the resumed round;
  - (low) update `docs/implementation-review.md` and the doc comments, with a heading-first example that does not bind.
