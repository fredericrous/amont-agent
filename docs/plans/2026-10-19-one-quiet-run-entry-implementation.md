---
canonical: amont:docs/plans/2026-10-19-one-quiet-run-entry-implementation.md
phases: [2]
status: done
---
# One implementation of the quiet-run active entry (pointer)

The plan lives in amont, which owns the contract and the `amont-quiet`
crate. This repository carries Phase 2: `src/quiet.rs` stops copying amont's
active-entry writer and calls `amont-quiet` (released in amont 1.60.0), and
the `holds-until:` comment that recorded the copy goes with it.

## Implementation review

approve-with-changes, then approve-with-changes on the Delta, with every code finding fixed.
- `finish()` ends the entry once the log is placed, before the excerpt prints.
- The relative-config test now proves the ended log holds the run's last line.
- The wrapper's decision record and the Phase 2 Outcome are written in the canonical plan, in amont's closing PR.
