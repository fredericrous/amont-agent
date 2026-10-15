---
canonical: decisions:docs/plans/2026-10-15-gates-keep-unchanged-verdicts.md
phases: [1, 2]
status: done
---
# Gates keep unchanged verdicts (pointer)

The plan lives in the decisions repository, which owns its ADR-0034. This
repository carries phases [1, 2].

## Verification (phases 1 and 2)

Driven through the built `amont-agent hook` with real git repositories, the real `amont` 1.55.0 answering `land --dry-run --json`, and the rule under `deny`:

- Phase 1. Input: approve a UI commit, reword it, then `amont land --json`. Expected: not held, journal `approved-by-tree`. Actual: no output; journal `push-preview - approved-by-tree - app - e7d8552→f02979a`. Input: then change `app/routes/home.tsx` and land. Expected: held. Actual: `permissionDecision: deny`, push-preview.
- Phase 2. Input: reviewed, `main` gains `src/other.rs`, `git rebase origin/main`, then `amont land --json`. Expected: not held, `reviewed-by-patch`. Actual: silent; journal `implementation-review unconfirmed reviewed-by-patch s1 app default amont land --json`. Input: then a rebase with a resolved conflict on the reviewed line, and land. Expected: held as stale. Actual: `Deny("… read tree 2bdf8ffaceee; this push carries tree e1e302352fd1 (stale) …")`.
- Tests: `tests/preview.rs` (reword, amend, plan-only, UI change, 4-field line) and `tests/implementation_review.rs` (clean rebase, context line, rules-only change, resolved conflict, code edit, whitespace, overrule, `git patch-id` failing). Falsified: with the by-patch lookup disabled, the carry-over tests fail. `make check` on fad5363: green (271 s).
- Live, after the release: the relais and amont branches of this plan land under the released hook; recorded in the canonical plan's closing PR.

## Implementation review

- approve-with-changes, then a delta: 4 of 5 resolved (patch-id failure test, reason when the key is missing, `overruled-by-patch`, overrule limitation documented). Finding 1 (plan checks not run) was answered by the hook-driven `amont land` checks above, run before the push.
- Deliberate: the `how` prefix stays 12 characters (`short()`, the module's convention); a land journals `approved-by-tree` twice (two callers of `approved`), journal noise only. 45k + 50k tokens, 77 s + 27 s.
