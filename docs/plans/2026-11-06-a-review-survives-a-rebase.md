---
canonical: decisions:docs/plans/2026-11-06-a-review-survives-a-rebase.md
phases: [1, 2]
status: active
---
Part of [A review survives a rebase over a release](https://github.com/fredericrous/decisions/blob/main/docs/plans/2026-11-06-a-review-survives-a-rebase.md), approved 2026-11-06 (body-sha 3ba332d06606).

## Code review

- amont-agent: backend, lang:rust, unix, tui

## Implementation review

Section 1 (notes-tolerant carry), tree 68e00b42; round 1, Delta, final delta; all Opus.
- implementation-review: approve-with-changes → approve. Fixed: `w` → `world`, unit tests per key property, `by-notes-patch/`, positional hunk parsing. Deliberate: profile-B-not-carried rests on per-profile slug paths.
- backend / lang:rust / unix: approve-with-changes → approve. Fixed: `--`/`++` header filter (trust hole), `--text` (binary changelog), PatchMatch enum, hex check, Refile, parallel test dirs. Deliberate: a failed notes key is an unjournalled clean miss.
- tui: approve-with-changes → approve (one name `patch-notes-moved`, docs wrapped).

Verification observed: 4 hook tests + 5 unit tests green; real relais replay (cc26dfff..92aef136 vs f52d97cd..8ac6de74): exact keys differ, notes keys equal; gate 27/27. Sections 2–3 (interdiff fallback, skill text) next.
