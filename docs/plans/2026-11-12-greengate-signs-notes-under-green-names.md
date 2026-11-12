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

## Implementation review

- implementation-review: approve-with-changes ×2, then approve (3f1e17c6). Fixed: discards given reasons, `world` names, stronger assertions, patch-id and unreachable-origin tests, the read error surfaced.
- backend: approve-with-changes ×2. Fixed: only provenance/attest refs fetched (a gate stamp is never overwritten), one bounded fetch, replay test. Deliberate: force-fetch of provenance/attest is unchanged from main.
- lang:rust: approve-with-changes, then approve. Fixed: refs built from `NAMES`, `find_map`, named wait arm. Deliberate: object order wins in `lookup` (plan: "for the object"); the status stays a string, the JSON contract.
- The human row prints `verified under <namespace>`; the JSON `status` stays `verified`.
