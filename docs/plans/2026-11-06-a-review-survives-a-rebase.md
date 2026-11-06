---
canonical: decisions:docs/plans/2026-11-06-a-review-survives-a-rebase.md
phases: [1, 2]
status: done
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

### Section 2 (interdiff fallback), tree 6c609909

- Deviations from the approved text, deliberate: (a) no `git range-diff`: it lists a commit added on top of the reviewed head as a header only, hiding the fix (a test caught it); the brief is the plain diff when the head is an ancestor, else the diff between the two branch patches. (b) by-branch records are per role and keyed by the same-named branch (`refs/heads/<branch>`), what `git push`/`amont land` update. (c) an overrule records no head: it is answered outside a push.
- implementation-review: approve-with-changes → approve-with-changes (this record) → done. Fixed: help text, journal `interdiff-brief`, Delta lead, `--profile` hint, 0700 temp dir.
- backend / lang:rust / unix / tui: approve-with-changes → approve. Fixed: key mismatch (high), typed `DeltaRefusal` and `BranchPass`, hex-validated `--interdiff` (exit 2), pinned diff options, per-role records, refusals naming the head; end-to-end and negative tests.
- Delta cost observed: fresh interdiff seats 34–45k tokens (round 2, 32 KB briefs), 21–25k (round 3, 5–6 KB briefs), against 43–82k for resumed seats earlier. The plan's ~15k estimate was low: each launch carries ~20k of fixed agent context. The delta brief omits rule texts (pinned by RULES sha), so a reviewer rules on candidates from the code.
- Verification observed: 8 tests (5 CLI, 3 hook incl. end to end) green; gate 27/27.
