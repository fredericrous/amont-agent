---
canonical: greengate:docs/plans/2026-11-11-greengate-renames-data-names.md
phases: [1, 5, 6]
status: active
---
Part of [greengate 4.0 renames its data names](https://github.com/greenharness/greengate/blob/main/docs/plans/2026-11-11-greengate-renames-data-names.md): this repository takes Phase 1, greenpiste reading greengate 4.0's names beside the old ones; Phase 5 (`GREENHARNESS_HOST_SLOT` in the install window) and Phase 6 (the census) come on later branches.

## Code review
- amont-agent: backend, lang:rust

Phase 1 here touched `src/suppressions.rs` and `src/main.rs` (the index pair), `crates/amont-agent-guard/src/guidance.rs` (the AGENTS.md markers), `crates/amont-agent-guard/src/rules/suppressions_override.rs` (the severity key), `crates/amont-agent-guard/src/rules/adr_lint_written.rs` and `src/hook.rs` (the list envelope, asked of the gate on PATH), and the tests `tests/hook.rs`, `tests/suppressions.rs`, `tests/adr_lint_written.rs`. `land.rs` reads the receipt directory from the gate's JSON and names nothing; `heavy_build_unqueued.rs` reads only the slot variable, which waits for Phase 5; the `no_verify` remedy naming `amont.severity` moves in Phase 5 too. For the census (Phase 6): the old reads in `suppressions.rs` (`AMONT_INDEX_*`), `adr_lint_written.rs` (`amont-list-v1`), `guidance.rs` and `suppressions_override.rs` go.

## Verification

- `GREENGATE_INDEX_FILE` + `GREENGATE_INDEX_GIT_DIR` naming an index with a bare suppression → exit 1 naming `a.py:1`; the same with an `AMONT_INDEX_*` pair naming the clean index beside it → still exit 1 (the new pair wins) → `greengate_index_file_selects_the_alternate_index_and_wins_over_amont` passes.
- The pair lookup: new wins, old read alone, a mixed pair keeps no git-dir, an empty `GREENGATE_INDEX_FILE` falls back to the old pair, the names carried for messages → `the_new_index_pair_wins_and_the_old_pair_is_read_alone` passes; the existing `AMONT_INDEX_*` tests pass unchanged.
- An AGENTS.md with `<!-- greengate:start -->` and a drifted gate → the session is told → `a_session_opening_on_a_stale_guidance_block_is_told` passes (the `amont:start` case still told).
- `git -c greengate.severity.pre-commit-suppressions=warn commit`, `git config greengate.severity… warn`, `GIT_CONFIG_PARAMETERS` → refused; `=block` → allowed → `each_channel_is_refused`, `ordinary_commits_and_a_whole_pre_commit_skip_pass` pass.
- A gate printing `greengate-list-v1` → the adr-lint gate opens → `a_greengate_list_envelope_opens_the_gate` and `the_gate_needs_the_format_and_the_row_id_trusted_or_not` pass; `greengate-list-v2` stays unexpected.
- The hook tests stub both gate names: on this machine an installed `greengate` 3.0 answered instead of an `amont` stub, and three tests failed at the commit gate before; all `tests/hook.rs` pass now.

## Implementation review

- implementation-review: approve-with-changes, then approve on the delta; fixed: `reply` for `r`, files and census list named here (47k+27k).
- backend: approve-with-changes, then approve; fixed: messages name the pair read, empty new name falls back, the adr-lint gate asks the gate on PATH; deliberate: one shared stub log, `gate.rs`'s `greengate_wins_only_when_an_executable_is_on_path` pins the choice (65k+25k).
- lang:rust: approve-with-changes, then approve; deliberate: `IndexHandoff`'s public fields and the `=block` substring check (pre-existing) wait for the census (40k+25k).
