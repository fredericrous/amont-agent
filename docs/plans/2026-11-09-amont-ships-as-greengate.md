---
canonical: greengate:docs/plans/2026-11-09-amont-ships-as-greengate.md
phases: [0]
status: active
---
Part of [amont ships as greengate](https://github.com/greenharness/greengate/blob/main/docs/plans/2026-11-09-amont-ships-as-greengate.md): this repository takes Phase 0, greenpiste's guards reading `greengate` as `amont` before greengate 3.0.0 is tagged.

## Code review
- amont-agent: backend, lang:rust (the land/push guard prerequisite)

## Verification

- `greengate land` / `greengate run` / `greengate slot run` get the same verdict as `amont` in land, chain-with-and, pipe-to-tail, heavy-build-unqueued, test-output-trimmed → one test per module, passing (`cargo test -p amont-agent-guard`).
- shape: `greengate`, `greengate-fleet`, `amont-fleet` read as verb programs → `greengate_programs_are_verb_programs` passes.
- push-preflight: a hook naming only `greengate` reads as guarded (git config isolated, exact next reason asserted) → passes.
- a land's dry-run spawns the binary the clause names → `the_dry_run_asks_the_binary_the_clause_names` passes.
- Remaining for this phase: the greenpiste release installed on this machine before the greengate 3.0.0 tag (a release step, not this branch).

## Implementation review

- implementation-review: approve-with-changes, then approve on two deltas; fixed: the fleet check routed through `gate.rs`, `TABLE_NAME` for pipe-to-tail, the doc move, a test pinning the named binary (43k+38k+21k).
- backend: approve-with-changes, then approve; fixed: the land dry-run asks the clause's own binary, gate-neutral messages, a `holds-until` marker; deliberate: the exact-reason push-preflight assertion (48k+23k).
- lang:rust: approve; fixed: `amont-fleet` read like `greengate-fleet`, a drop-guard in the PATH test (47k).
