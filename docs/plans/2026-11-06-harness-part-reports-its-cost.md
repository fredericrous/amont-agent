---
canonical: decisions:docs/plans/2026-11-06-harness-part-reports-its-cost.md
phases: [1]
status: active
branch: feat/seat-cost-columns
---
# A harness part reports its cost beside its catches

Pointer. The plan and its review panel live in decisions. This repository
carries Phase 1: `plan-review stats` puts each reviewer seat's tokens and
seconds beside its findings (`ktok/review`, `s/review`, `ktok/serious`).
Phase 2 (guard rules' cost per firing) is a later branch here, gated on a
hand count.

## Code review

- amont-agent: backend, lang:rust, unix, tui (architect left out: the new walker and `cost.rs` stay inside the guard crate, no new crate boundary; po left out: report fields only)

## Verification

(filled at the close)
