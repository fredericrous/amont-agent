# Full reviews: Plan-review edits apply without an Opus subagent

## Full reviews (reference)

Round 1
- **backend** (45k, 153 s): approve-with-changes. High: the `router-observe` UPDATE keys on session and hardcodes `outcome_source='parent'`, so a new `record_merge_outcome(task_id, agent_id, outcome, rank)`; `--check` fails correct appends. Medium: `{role, finding}` has no round; baseline numbers disagree and Phase 4 lacks a threshold. Low: exit 3 new to the crate, atomic.rs docs; overlap rule after sequential edits. All accepted.
- **lang:rust** (44k, 156 s): approve-with-changes. High: closing-fence search breaks on patches containing a fence (serde streaming deserializer instead); `--check` same flaw. Medium: `recorded()` collects ids from every schema; overlap/offset coordinates and `match_indices`; relais UPDATE path. Low: `pub(crate) Lock`; shas before write; whitespace-labelled absence. All accepted.
- **tui** (22k, 14 s): approve-with-changes. High: stdout/stderr split and the APPLY block placement; no distinct exit for a missing or malformed `Patches:` block. Medium: write rule contradicted exit 3; `next:` line and 80 columns; APPLY block may overflow. Low: usage text; `--json` merge counts. Accepted except the `--quiet` flag (rejected: the exit code carries it).
- **unix** (32k, 52 s): approve-with-changes. High: an editor save between read and rename is lost (compare bytes under the lock). Medium: symlinked plan (canonicalize); exit codes for `--check` and `--record-fallback`; fallback row dedupe. Low: TTY stdin; APPLY block last. All accepted.

Round 2 (backend only; no reviewer said rework)
- **backend** (43k, 125 s): approve-with-changes. All 6 resolved. New high: `--check`/`--record-fallback` select by the current sha after the plan changed; applied directly in the session (one sentence). Medium: `recorded_merges` reader. Low: mixed rounds.
- **backend** (32k, 100 s): approve-with-changes. Resolved. New high: round-2 reviews are resumes, so `completed_agents_resumable`; medium: sha width across selectors, `--check` must read the `command` row. All three applied in one sentence.
- **backend, binding read** (25k, 15 s): approve. All resolved, no blocker. Note: a second `plan-apply` on an edited plan needs `--before`; document it.
- **backend, re-bind after the fence fix** (21k, 25 s): approve. Behaviour 1's example moved to a four-backtick outer fence after the body sha was seen to follow the review text; measured after the fix: the sha is identical with and without the review sections.

carried into implementation
- `recorded_merges(path)` reader for the fallback-row dedupe, tested beside the schema filter.
- Mixed rounds across roles: record the max round, or exit 2.
- A second `plan-apply` run on an edited plan: `--before` required, said in `docs/plan-review.md`.
- `relais report --json` merge keys; the merge line within 80 columns.
- Usage text: stdin example, three modes, `try \`amont-agent plan-apply --help\``.
- Exit 3 in `docs/plan-review.md`'s table; `src/atomic.rs` doc line.
- Whitespace-normalized retry labels `absent (whitespace)` only.
- Overlap predicate `s < end && s+old.len() > start` in current-text coordinates.
- `Lock` and `lock()` both `pub(crate)`.
- `shas(new)` computed before the write; `ParserPanicked` still writes, `body_sha_after: null`.
- Phase 4 thresholds: match rate ≥ 70 %, merge tokens ≤ 1/4 of 320k per review.
- TTY stdin → exit 2 with usage (also fixes `record`, which hangs today).
