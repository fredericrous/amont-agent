# Reviews: a step that can fail is not joined to a consequential step by `;`

## Full reviews (reference)

- **backend**: round 1 approve-with-changes (39k, 85 s); round 2 approve-with-changes (40k, 84 s); delta approve (24k, 13 s). Carried into implementation: assert `make` is A and `git push` is B in the unit test; confirm the `shell.rs:318-319` citation; count firings inside `then`/`do` bodies in the backtest.
- **lang:rust**: round 1 approve-with-changes (41k, 77 s); round 2 approve-with-changes (35k, 52 s). Carried: use `has_short('e')` or `-o errexit` for the `set -e` skip and add a `set -euo pipefail` test; name `Evidence.trend`.
- **unix**: round 1 approve-with-changes (38k, 88 s); round 2 approve-with-changes (45k, 111 s). Carried: `backtest` counts per rule only, so classify `explain` matches by the byte before `A.end` to split `;` from newline; `set +e` ends the skip.
- **tui**: round 1 approve-with-changes (36k, 74 s); round 2 approve-with-changes (24k, 16 s). Carried: the remedy prints the exact `<A> && <B>` command, trimmed to 60 characters; check the advise text wraps at 80 columns; confirm the `amont.agent.<id>.stance` key against the stance loader before the pilot step; say that `amont-agent rules` lists the stance.
- **codex**: skipped. Plan mode allows no side-effect command, and `codex exec` runs without a sandbox.
