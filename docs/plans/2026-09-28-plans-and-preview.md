---
canonical: decisions:docs/plans/2026-09-28-plans-and-preview.md
phases: [2]
status: active
---
Part of [Plans in the repo, one PR per plan, a verification loop before
push](https://github.com/fredericrous/decisions/blob/main/docs/plans/2026-09-28-plans-and-preview.md).
This repository carries Phase 2: the `push-preview` rule, the
`push-published` assertion, preview registration and approval.

## Decision log (this slice)

- 2026-09-28, spike: PostToolUse `AskUserQuestion` carries the person's
  selection in `tool_response.answers` (question text → label), plus
  `tool_use_id`, `prompt_id` and `duration_ms`, which is the answer latency
  the soak needs.
- 2026-09-28, spike: `answers` is also an *input* parameter of the tool, so
  an agent could pre-fill it. PreToolUse records, by `tool_use_id`, whether
  the model supplied `answers`. A marked question that arrives pre-answered
  never approves, and under deny it is refused before it runs. This adds a
  `PreToolUse:AskUserQuestion` install target.

## Verification record (this slice)

- `make check` (fmt, clippy `-D warnings`, `cargo test --locked`) passes:
  255 unit tests plus the integration suites, including 14 new end-to-end
  tests in `tests/preview.rs` that drive the real binary with the payload
  shapes captured on 2026-09-28.
- Falsified with three deliberate mutations:
  - `approve()` made to write nothing: 3 tests go red.
  - Typed approval widened to accept "yes": 1 test goes red.
  - The PostToolUse check of `prefilled` removed: stays green. The
    PreToolUse side already stops a pre-answered question (it never marks
    its registrations as asked), so the second check is defence in depth
    that no test can reach alone. This is documented in `docs/preview.md`.
- Piloted run on a scratch clone of duro-app (a real UI repository with a
  `dev` script), with commit `feat: pilot preview gate` changing one `.tsx`.
  In order:
  1. The unapproved push was advised.
  2. `preview register` refused a dirty tree and printed its JSON on a clean
     one.
  3. The marked Approve was journalled as `approved … by option,6s`.
  4. The same push was then silent, journalled as `every interface change is
     approved`.
- The real `AskUserQuestion` payloads captured by the spike replay through
  the new hook silently (exit 0).
- Not yet done: a run in a live session with the released binary installed.
  That comes in Phase 3, after `amont-agent install --write`.
