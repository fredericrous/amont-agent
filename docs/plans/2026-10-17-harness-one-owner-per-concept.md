---
canonical: decisions:docs/plans/2026-10-17-harness-one-owner-per-concept.md
phases: [2]
status: active
---
# The harness has one owner per concept (pointer)

The plan lives in the decisions repository, which owns ADR-0039. This
repository carries Phase 2's amont-agent slice: the active-entry writer in
`src/quiet.rs` names amont's `docs/quiet-runs.md` as its contract, and a
`holds-until:` comment records the one known breach of
`harness.shared-code`, the copy of amont-runtime's `active` module.
