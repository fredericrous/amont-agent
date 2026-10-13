---
canonical: decisions:docs/plans/2026-10-13-agent-guidance-follows-writing-for-agents.md
phases: [2]
status: active
---
Part of [Agent guidance follows writing-for-agents](https://github.com/fredericrous/decisions/blob/main/docs/plans/2026-10-13-agent-guidance-follows-writing-for-agents.md).

This repository carries Phase 2: the write-branch collector in `src/hook.rs`
and two write-tier rules, `agent-guidance-written` and `agent-guidance-shape`
(both advise, ceiling advise), released as 2.39.0.

## This repository's slice (copied from the canonical plan, as approved)

### two write-tier rules (both advise; ceiling advise)
Both follow `lint-suppression-added` (`src/rules/lint_suppression_added.rs`):
`examine` returns `None`, pure helpers, wired in `src/hook.rs` `on_file`.
The write branch today returns `lint_suppression(op)` directly; it becomes a
collector of every write rule's text, joined with `"\n\n"`, as the Read
branch does (hook.rs:515–640). The implementation-review guard keeps its
early deny (hook.rs:493–513); any rule's Deny makes the decision Deny and
carries every text; otherwise advice is joined.
1. `agent-guidance-written` (default advise, max advise): a Write/Edit/
   MultiEdit to a guidance path (`SKILL.md`, `CLAUDE.md`, `CLAUDE.md.tmpl`,
   `AGENTS.md`, `MEMORY.md`, `.claude/agents/*.md`, `dot_claude/agents/*.md`)
   advises one line: "`<file>` is agent guidance: the `guidance.*` and
   `prose.*` rules apply (ADR-0026, ADR-0032); `aval relevant --path
   <file>` lists them." Memory files other than MEMORY.md are exempt. Told
   once per session and file (`first_telling`, keyed `write <path>` plus
   `@agent` for a subagent, as `code-read-not-lsp` does at hook.rs:580–593);
   later writes are journalled only.
2. `agent-guidance-shape` (default advise, max advise until the index-rewrite
   plan brings MEMORY.md and CLAUDE.md under budget — auto-memory grows
   MEMORY.md on every save): checks the text the write produces, rebuilt by
   calling `lint_suppression_added::reconstruct` (pub; its `read_bounded` is
   private) and only when the result is `Mode::Exact`; a `Mode::Fragment`
   rebuild is silent and journalled. Checks:
   - a `SKILL.md` whose front matter (the lines between the first two
     `---`, parsed by hand, no YAML crate) has no
     `disable-model-invocation: true` and whose `description:` is missing,
     is a `>`/`|` block scalar or otherwise spans more than one line, or has
     no trigger clause (`Use when`, `Use whenever`, `Use for`, `Use at`,
     `Use before`, `Use after`, `Use proactively`);
   - an always-on file (`CLAUDE.md`, `CLAUDE.md.tmpl`, `AGENTS.md`,
     `MEMORY.md`) whose rebuilt `after` exceeds 8,192 UTF-8 bytes (CRLF
     normalised) and is larger than `before` — a ratchet: shrinking or
     same-size writes pass; a write that crosses the budget fires too.
   Advice names the file, the failed check with numbers (size, the 8,192
   budget, bytes this write adds), the remedy (one-line `description:` with
   a `Use when …` clause; move text behind a pointer), and the rule id
   (`guidance.pointer-names-when` / `guidance.always-on-is-an-index`).
Each rule: unit tests inline, `tests/hook.rs` cases, `tests/corpus/<id>.cases`
(shell negatives, ≥ 4 `nomatch`, header comment as for
lint-suppression-added) registered in `src/corpus.rs`,
`tools/agent-guidance-rate.py` reporting writes per 1,000 for each guidance
and always-on file (run before release; its numbers go in `Evidence` and the
CHANGELOG), `docs/rules.md` rows, `CHANGELOG.md`. Released as 2.39.0
(`tag-release`), installed with `brew upgrade`.

### Carried into implementation by the plan review
- Tell `agent-guidance-shape`'s advice once per session and file too
  (`first_telling` keyed `shape <path>`), so MEMORY.md saves do not repeat it.
- For the size ratchet, a new file's `before` is 0.
- The plan's Verification (Phase 2) lists the hook tests this slice must
  pass; they are the acceptance of the relais run.
