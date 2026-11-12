---
canonical: fredericrous/decisions:docs/plans/2026-11-12-greengate-signs-notes-under-green-names.md
phases: [2, 4, 5]
status: active
repos: [amont-agent]
---
# greengate signs its notes under green names (greenpiste's part)

The canonical plan lives in the decisions repository (ADR-0068). greenpiste's
share: Phase 2 reads both notes name sets (`greengate-*` first, `amont-*` only
when the green ref has nothing for the object, the header always matching the
ref) in `shipped.rs` and `push_preflight.rs`; Phase 4 switches the provenance
writer (`provenance.rs`) to `greengate-provenance-v1`; Phase 5 removes the
legacy reads at the census.

## Code review
- amont-agent: backend, lang:rust (unix, tui, architect left out: two readers and one header)
