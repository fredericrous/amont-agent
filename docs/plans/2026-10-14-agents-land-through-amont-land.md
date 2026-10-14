---
canonical: amont:docs/plans/2026-10-14-agents-land-through-amont-land.md
phases: [2]
status: active
---
Part of [Agents land through `amont land`](https://github.com/fredericrous/amont/blob/main/docs/plans/2026-10-14-agents-land-through-amont-land.md).

This repository carries Phase 2: amont-agent treats `amont land` as a push. A land clause is resolved by running `amont land --dry-run --json` (amont 1.54.0; "1.53" in the canonical body reads 1.54, see its Decision log) once per hook call with a 3 s timeout; only `[cd <literal path> &&] amont land [--json] [--force]` in the foreground resolves, anything else (other clauses, pipes, redirections, env assignments, wrappers, `run_in_background`) is unresolvable. The implementation-review and preview gates judge the land as they judge a `git push`. A PostToolUse/PostToolUseFailure assertion reads the land's receipt (`receipt_dir` from the dry-run) and journals a retried push of an unjudged sha.
