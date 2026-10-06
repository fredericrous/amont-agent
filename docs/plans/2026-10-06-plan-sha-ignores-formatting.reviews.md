# Full reviews: Plan sha ignores formatting

- **unix** (round 1) — approve-with-changes, 30k, 26 s. `--legacy` with `--block` undefined → refuse (exit 2); usage text describes bytes → describe words; warn before removing a flag (clig.dev) → stderr line; exit codes unchanged.
- **backend** (round 1) — approve-with-changes, 34k, 49 s. Structure tokens dropped anywhere hide `>`→`>=` (high) → line-start only; fences not fence-aware; no measure of legacy use → journal `match=`; rollback cost; numbers table; observable verification.
- **lang:rust** (round 1) — approve-with-changes, 39k, 51 s. Same structure-token blocker plus `*`/`\` in globs and regexes (high); `panel()` repeats the baseline check (high) → shared `is_current`; `_` stripped inside identifiers; legacy hex fixture test; baseline order and `save` migration; `--legacy --block`.
- **tui** (round 1) — approve-with-changes, 34k, 50 s. Same structure-token blocker (high); `_`/`*` inside words; `--legacy` flag combinations; usage text; one exact landing command for the skill.
- **backend** (round 2) — approve-with-changes, 33k, 64 s. Fences at indent ≥4 read as prose (high) → own tracker; `*`/`_`-only tokens; `match=` carrier; pin inputs; span state per line.
- **backend** (round 2, re-bind) — approve-with-changes, 30k, 36 s. `match=` suffix lost to `MAX_EXCERPT` 200 (high) → prefix; stale corpus numbers → re-measured 35/44; `_private` = `private` → Non-goal.
- **backend** (round 2, re-bind) — approve-with-changes, 28k, 24 s. Test list put an equal pair in the "different" group → split into two groups.
- **backend** (round 2, final) — approve, 24k, 12 s. Low: write `\_` as the pair `\_`→`_` — applied in the test, not the body.

- **backend** (delta) — approve-with-changes, 35k, 73 s. Stale by-path fallback misses a fresh session (high) → by-title key; parser update re-hashes silently → exact pin + fixture sha; header stale; kind-less boundaries (`- deny` = `deny`); untagged items.
- **lang:rust** (delta) — approve-with-changes, 37k, 57 s. Per-event code-block items + caret pin (high) → one item per block, `=0.13.4`, fixture; kind collapse loses item/quote/ordered start; panic in the hook fails open; span tab splits; binary size.
- **backend** (delta, final) — approve-with-changes, 31k, 39 s. All resolved. Low, applied at implementation and logged then: table row boundary as `k:row` (two one-cell rows ≠ one two-cell row); state that in-flight plans gain a by-title record only at their next pass; size check gets a bound; `plan-sha` panic → exit 1 test; mark the round-2 "35/44" as superseded.
