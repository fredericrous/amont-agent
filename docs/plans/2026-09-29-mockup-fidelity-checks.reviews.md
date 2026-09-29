# Mockup fidelity is checked before approval and at review — full reviews

## Full reviews (reference)

Round 1 (all approve-with-changes; tokens, time):
- **backend** (43k, 49 s): published() range goes blind after a push; after/live mismatch; bind/on_post_ask need Option<String>; measure existing guides; input→expected rows; red check blocks nothing without merge-when-green; alias collision test.
- **architect** (45k, 60 s): same range issue; 1c has no route to the model (hook.rs); a wrong label drops the registration; after/live; viewport width and theme; trigger mismatch goes in the decision log; skill template versioned in dotfiles.
- **po** (29k, 29 s): trigger mismatch for later PRs on the same screen; Advise let the push through; ship the AL check first; measure after release, with rollback; after/live.
- **lang:rust** (57k, 70 s): range goes through stale.rs, refused when there is no base; bind path through Decision::Assert; absolute image paths escape the check; aliases in a sidecar so the registrations line stays compatible; an answer_out test helper.
- **lang:typescript** (37k, 44 s): vitest include misses scripts/ (blocking); scripts/ triggers the E2E gate; fail-closed diff; case filter with one test per exclusion; accept several separators; isolated temporary git repos.
- **platform** (38k, 45 s): range issue; a concurrency group because of runner capacity; avoid fetch-depth 0 when it isn't needed; merge-when-green must read the check; goal wording.
- **react** (38k, 49 s): pair() needs a before image, so a new screen can't render; missing app/hooks and app/lib in the filter; viewport line; alt text.
- **ui-design** (34k, 37 s): after/live; a 466px side-by-side is too small, so full width plus links; viewport caption; state pairs by suffix; the paired struct; captions.
- **ux-research** (35k, 56 s): after/live; overlay rather than side-by-side (Gleicher 2018); the same differences rule in both checks; accept several separators (NN/g); unbacked: fixed/deliberate labels (measure them).
- **game-ux** (46k, 64 s): after/live and absolute paths; /duro-mockup missing in AL, so install it or print the steps; IHDR width check; paste-ready refusal; decide the push stance.
- **tui** (36k, 40 s): after/live; the PNG copy is the chaining trap, so it's its own step; name the mockup mode in the refusal; a copy-paste bind command; whole-word heading match; one hint.
- **unix** (41k, 52 s): a failed diff read as a pass; an unlistable diff must refuse; exit codes and the label in the help; separators; config key shared with the script.

Round 2 (backend, approve-with-changes, 43k, 48 s): round-1 items 1–7 resolved. New blocker: a shallow checkout plus a depth-1 base has no merge base, so every PR would exit 2. Also: the attestation re-check was N=2 (2fcffec should be refused); the paths `duro skill install` writes; the metric had no source. All were edited into the body.

Round 2b (backend on the final body, approve-with-changes, 34k, 51 s): all resolved, no new blockers. Carried to implementation without a body edit:
- step 1 (inline shell, before checkout) sets an output that step 2's `if:` reads, so `Mockup: none` really skips the fetch;
- the refusal row's precondition holds, because #302 commits `docs/mockups/masthead-history/*.dc.html` (eec43a8).
