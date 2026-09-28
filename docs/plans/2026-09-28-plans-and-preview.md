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
