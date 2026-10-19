# Reviews: amont-agent's guard and work process, two components

Plan: [2026-10-19-guard-and-work-process-components.md](2026-10-19-guard-and-work-process-components.md)


**Round 1, backend** (rework, 30k, 81 s). All accepted:
- blocking: `push_preview`'s `confirm` calls preview;
- high: the boundary test must match inline paths;
- medium: the cycles are cleanup, and there is no `preview` ↔ `plan_phases` cycle;
- blocking: the panel comment needs `adds=ops`;
- medium: use the workspace dry-run;
- medium: state the rollback;
- medium: make the checks observable.

**Round 2, backend** (approve-with-changes, 39k, 26 s). All accepted:
- medium: the Design table still said "four declarations";
- medium: Context still described the cycles as blocking;
- medium: keep the rule order;
- low: the list of `RULES` readers was wrong.

**Delta, platform** (approve-with-changes, 29k, 26 s). All accepted:
- blocking: the release reads the version with a grep;
- blocking: a single `cargo publish` at a virtual root;
- blocking: `include_str!` paths leave the guard crate;
- high: build with `-p amont-agent`;
- high: a partial publish needs a dry-run gate, and the tap must wait for crates.io;
- high: the token's scope;
- medium: `--workspace` in the Makefile and CI.

**Final review, codex** (rework, high effort). All accepted:
- blocking: `record_before` computes preview policy;
- high: the stdout architecture check must scan all three crates;
- high: the move commit must build.

**Backend, binding** (approve-with-changes, 33k, 53 s): high, tests that loop over the whole rule table move to the binary crate; low, the table type is `&[Rule]`. Both accepted.

**Platform, binding** (approve-with-changes, 21k, 25 s): medium, run the dry-run gate in the `guard` job; low, the publish timeout. Both carried into implementation.

**Backend, last binding pass** (approve, 19k, 19 s): low, take the 45-rule count from `rules --json` on main. Carried into implementation.

**Carried into implementation**
- Before Phase 1, record the rule count from `amont-agent rules --json` on main. Phase 2's assertion compares against that recorded number, not a literal.
- `push_record` and preview share preview's private `dir()` and `ensure` (`preview.rs:88-92`). It moves to `push_record` with the paths unchanged.
- The `cargo publish --workspace --dry-run` gate runs in the release's `guard` job, not inside `publish-crates`, so a packaging failure stops the release before the GitHub release goes out (`release.yaml:310`).
- Watch how long the first three-crate `publish-crates` run takes against its 20-minute `timeout-minutes`. A rerun resumes, thanks to the "already uploaded" tolerance.
