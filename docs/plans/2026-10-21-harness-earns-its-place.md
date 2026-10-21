---
canonical: decisions:docs/plans/2026-10-21-harness-earns-its-place.md
phases: [2, 3]
status: active
---
# A harness part earns its place on each model (pointer)

The plan lives in decisions, which owns ADR-0042. This repository carries
Phase 2 (rules and assertions name their failure, the shadow switch, the
`harness-review` report and its new-model check) and the amont-agent half of
Phase 3 (`plan-review stats` and the implementation-review outcome ledger).

## Implementation review

- Phases 2 and 3 (amont-agent): approve after one delta. Fixed: shadowed deny rules replayed as observe (shadow share only), doctor refuses a shadow rate on an irreversible part, clock failure exits 2, `--out` keeps fixtures out of the real review directory. Rejected: the impl-review disposition enum (the plan's parenthetical names the plan-review schema). Deliberate, low: the deny test's live-arm assertion cannot fail yet (add a live row in a later change). Plan lines 69 and 82 are clarified in decisions when the plan closes. Next: dotfiles (Phase 3), relais (Phase 4).
