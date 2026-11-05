# Suppressions: through Bash, and at commit — reviews

## Full reviews (reference)

- **lang:rust** (round 1, approve-with-changes, 40k, 72 s): bounded `cat-file` in Layer 1 (high); sizes before reading (medium); a missing HEAD blob exits 2 (medium); `*.zsh` glob (medium); the `GIT_CONFIG_COUNT` channel (medium). All applied.
- **unix** (round 1, approve-with-changes, 35k, 73 s): `GIT_INDEX_FILE` scrub misses `commit -a`/`<path>`/`--only` (blocking); an `amont.severity` lowering and `GIT_CONFIG_*` in the override rule (high); extensionless scripts (medium); a missing binary is a non-blocking gap, check runners (medium); stdout/stderr split (low). All applied. `hook.skip=pre-commit` still allowed (low): rejected, the person approved it for config-only commits.
- **backend** (round 1, approve-with-changes, 35k, 108 s): `D` in the diff filter (high); vendor only after the decisions PR (high); command-scoped `allow` (high); the failure path too (medium); reasoned in the key (medium); rollback by stance (medium; its missing-binary half rejected per unix); suite row (low). Round 2 (approve-with-changes, 37k, 47 s): every round-1 item resolved; the advise stance exits 1 (high, applied); lows 2–3 no edit. Re-run (approve, 23k, 24 s).
- **tui** (round 1, approve-with-changes, 30k, 97 s): every hit with its fix (high); the override line labelled for the person (high); no truncation at 3 (medium); deny text (medium); exit-2 message (medium); Layer 1 per-hit lines (medium, in part: no claim about which command typed a hit); skipped count (low). Applied.
- **codex** (final, rework, high effort): amont scope grammar refuses the pack globs (blocking, verified at `amont-runtime/src/manifest.rs:1171-1200`); rename across kinds (high); cumulative memory budget (high). Applied; backend re-run approve-with-changes (27k, 25 s). Codex left an untracked `tools/__pycache__/` in the amont-agent checkout (not kept; removed after plan mode).
- **Carried into implementation:**
  1. codex#4 (medium): `on_post_bash_failure` returns `Decision::AssertAfterFailure`, not `Assert` (`decision.rs:116`, `hook.rs:723`); test both emitted events.
  2. backend (codex re-run, medium): an extensionless path is a shell candidate; its blobs count in the size budget, and it is classified by its first line only after the budget passes.
  3. backend round 2 (low): the journal row for the Edit/Write config gap names the stance key too.
