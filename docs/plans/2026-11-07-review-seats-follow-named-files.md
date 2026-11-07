---
status: active
branch: feat/seats-from-file-types
repos: [amont-agent]
adrs: [ADR-0061, ADR-0060]
---
# Review seats follow the files a plan names

## Review panel
👉 **Decide:** none — approve if a plan's panel may come from the files its Phases name (plans naming none keep today's panel, with advice), backtested on 47 past panels before release.
📍 amont-agent · plan ready, nothing built · next: Phase 1 in a new worktree. Panel: backend, lang:rust, unix, tui (all opus-5-5), codex (gpt-6-astra).
**Changed by review:** Codex found `git diff` rejects `--pathspec-from-file` (verified, exit 129), renames hiding a language's side, and Kubernetes manifests losing platform; `large`→`contract` normalised so baselines stay current; `seats=` last, `none` field kept with the reason in a third field.
📄 Full reviews: [reviews](2026-11-07-review-seats-follow-named-files.reviews.md)
**Verdicts:** 4 approve-with-changes, then backend approve-with-changes twice; Codex rework (3 findings applied); 1 rejected (backend's area union, reason in the Decision log); 6 carried into implementation.

## Goal
`amont-agent plan-panel`, the ExitPlanMode hook and `plan-panel --code-review`
seat reviewers from the file types of the paths a plan's Phases name, not
from every area its repositories have (ADR-0061, `work.review-seats`). A
code-review profile's brief carries only its file types' part of the diff
(`work.review-slices`, the brief half). A plan that names no paths keeps
today's area seats and is advised to name them. That turns on the seat and
brief halves of ADR-0061; slice binding is a later plan.

## Context
- Today `classes_of` (`crates/amont-agent-process/src/plan_review.rs:1569`)
  unions `repo_areas` for every repository in the machine comment, and
  `roles()` (`:253-269`) seats from that. A Rust repository with a `ui` area
  seats react, ui-design, ux-research and game-ux on every plan, whatever the
  plan touches. `plan-panel` (`panel()`, `:1608`) and the hook
  (`examine`, `:1377`, `judge`, `:1098`) both go through `classes_of`, so it
  is the one seam for the plan panel.
- `code_review::proposal` (`code_review.rs:173`) proposes profiles from
  `repo_areas` per repository. The push hook does not recompute them: it
  reads the plan's declared `## Code review` line
  (`implementation_review.rs:1110-1175`).
- `review_brief::build` (`review_brief.rs:445`) gives every profile the same
  diff (`:556-559`, under 60 KB inline). `interdiff_brief` (`:840-849`) builds
  the Delta's patch the same way.
- Measured: 47 of the 52 round-1 panels in
  `~/.claude/amont-agent/plan-review/outcomes.jsonl` have their body saved
  under `plan-review/bodies/`, enough to backtest. unix and tui earn their
  seats: 0.91 and 1.04 accepted serious findings per Opus review, 49 and 46
  of them on plans with the `cli` area. Several came from files that are not
  entry points (on amont #430, the 80-column line and the env precedence were
  in `dispatch.rs`).
- `plan-review-panel` is a guard rule with a stance
  (`crates/amont-agent-guard/src/rules/plan_review_panel.rs:21`, default
  `deny`), so a new advice can ship as its own rule at `advise`.
- `CLAUDE_CONFIG_DIR` moves the plan-review store
  (`crates/amont-agent-guard/src/settings.rs:41`), so a backtest can run with
  no stored baselines.

## Non-goals
- Slice binding (`plan-sha --slice`, the slice-aware hook): the next plan.
- The skills (the plan template asking for files per phase, `plan-review`'s
  table and brief steps) and relais `due`: their own build items.
- Narrowing unix and tui to entry-point files: the measure above says those
  seats catch real findings in ordinary code files, so this plan keeps them on
  any code path in a repository with the `cli` area.
- Refusing a plan that names no paths: advice only, until the plan template
  asks for files per phase.

## Code review
- amont-agent: backend, lang:rust, unix, tui (the guard and the CLI's printed lines change)

## Behaviour
- **Named paths.** A path is a backticked span in `## Phases` with no space
  that contains a `/` or ends in a known extension or names a known file
  (`Cargo.toml`, `Dockerfile`, `Makefile`, …); a `:<line>` suffix is
  dropped. Only Phases count: context and evidence cite files a plan does not
  change. Each path is attributed to the plan's repository whose
  default-branch tree holds it or a parent directory of it: the
  `ls_tree_z` listing `repo_areas` already reads, so no new git call, with
  directory prefixes built from its files (it lists files only). A path no
  repository holds is attributed to every repository. A tree that cannot be
  read counts as holding nothing, with one warning on stderr; the exit code is
  unchanged.
- **File types to classes** (new `crates/amont-agent-process/src/file_types.rs`):
  - `.rs`, `Cargo.toml`, `Cargo.lock` → `lang:rust`; `.go`, `go.mod`,
    `go.sum` → `lang:go`; `.py`, `.pyi`, `pyproject.toml` → `lang:python`;
    `.ts`, `.js` and their variants, `package.json`, `tsconfig.json` →
    `lang:typescript`.
  - `.tsx`, `.jsx`, `.css`, `.html` (`preview::is_ui_path`, `preview.rs:125`)
    and `*.stories.*` → `ui`.
  - `ops_name` (`plan_review.rs:512`), `Dockerfile`, `*.tf`, `*.tf.json`, and
    `.github/workflows/` or `.forgejo/workflows/` paths → `ops`.
    Kubernetes, Helm and Flux manifests also earn `ops`, including ordinary
    filenames such as `deployment.yaml`, `release.yaml` and `values.yaml`.
    Preserve recognition from already-read tree/blob data; conservatively
    classify unresolved `.yaml` and `.yml` paths as `ops`, including new files.
    Use the same conservative rule in brief filtering. Phase 1 tests cover
    these filenames and Terraform JSON. Backtest real Kubernetes, Helm and
    Flux manifest changes through panel seating, code-review proposals and
    platform briefs; require zero missed platform seats or omitted manifests.
  - any code path attributed to a repository whose areas include `cli` →
    `cli`.
  - markdown and anything else → nothing: backend alone.
- **Explicit classes.** `adds=` keeps `cli`, `ui`, `ops`, `lang:<x>`, and
  gains `screen` (ux-research and game-ux, when the Behaviour adds a screen
  or flow) and `contract` (product owner and architect, when the plan adds a
  module, crate or repository or changes a contract another repository
  consumes). `large` stays valid as an alias of `contract`, rewritten to `contract` in `meta`'s adds and when a stored baseline loads, so an old `large` baseline stays current (unit test). `roles()`: `ui`
  seats react and ui-design only; `screen` seats ux-research and game-ux;
  `contract`/`large` seat po and architect. Backend always sits.
- **Seat source.** `classes_of` takes the plan body. When Phases name at least
  one path, classes are the named paths' classes plus `adds=`. When they
  name none, classes are today's `repo_areas` union plus `adds=`, and the new
  guard rule `plan-names-no-paths` (default and max stance `advise`) says:
  `name the files each phase touches in backticks under ## Phases; this
  panel was seated from the repositories' areas`. `plan-panel` prints the same
  text on stderr, stdout unchanged. When some phases name paths and others
  name none, seats come from the areas, as when none is named, and the advice
  names each phase that has none (`Phase 5 names no file; …`); see the
  Decision log entry of the backtest. `plan-panel`'s first line
  gains `seats=paths|areas` as its last field, and `PLAN_PANEL_USAGE`
  (`src/main.rs:1490-1495`) states the new line and the path seat source.
- **Code-review proposal.** `plan-panel --code-review` proposes, per
  repository, the profiles of the classes its attributed paths earn (`ui` →
  react, ui-design; `ops` → platform; `cli` → unix, tui; `contract` →
  architect; `lang:<x>` → itself; backend always), or `none` when they earn
  no code class. The output stays `<repo>\t<profiles|none>` and gains a third
  tab-separated field, the reason (`markdown only`, `no language area`), so
  `cut -f2` still reads `none` and a line pastes into `## Code review` as
  `none — <reason>`. With no named paths it proposes from areas as today.
- **Profile briefs.** `review-brief --profile` and the Delta brief filter the
  diff for `lang:<x>`, react and ui-design (the `ui` types) and platform (the
  `ops` types): enumerate each patch range with
  `git diff --no-renames --name-only -z` and classify it with `file_types`.
  Generate filtered patches with `--no-renames` too, representing moves as
  deletions and additions so each language sees its side. Pass kept paths as
  `:(literal)` pathspecs and list the rest as omitted. For rebased deltas,
  independently enumerate and filter both branch patches before comparing
  them. Phase 4 tests cover a cross-language rename in a full brief, an
  on-top delta and a rebased delta, asserting the source-language deletion
  and destination-language addition. backend,
  architect, unix and tui keep the whole diff. A filtered brief keeps the full
  `--stat`, lists the omitted paths under `## Not in this brief`, and every
  profile brief carries the plan's Goal and Behaviour beside Non-goals.
- **Ledger.** Each outcomes row's `plan` object gains `seat_source`
  (`paths` or `areas`) and `seat_classes`, while `areas` stays the repositories' `repo_areas` union, so the ADR-0061 budget measure can split rows before
  and after.
- `docs/plan-review.md` and `CHANGELOG.md` say all of this.

## Phases
- [x] Phase 1 — `crates/amont-agent-process/src/file_types.rs`: path
  extraction from Phases, attribution through `ls_tree_z`, the type table;
  unit tests beside it.
- [x] Phase 2 — `crates/amont-agent-process/src/plan_review.rs`:
  `classes_of` takes the body, `roles()`, `remit()` (and the class list in
  `remit_is_the_inverse_of_roles`) and `valid_class` for `screen`,
  `contract` and the narrowed `ui`; `src/main.rs` prints `seats=`;
  `crates/amont-agent-guard/src/rules/plan_names_no_paths.rs` beside
  `plan_review_panel.rs`, registered in `crates/amont-agent-guard/src/corpus.rs`;
  `tests/plan_review.rs` (the `BODY` fixture names no paths and is updated; new cases: a body with paths prints `seats=paths`, and one with no paths gets the `plan-names-no-paths` advice with ExitPlanMode allowed).
- [x] Phase 3 — `crates/amont-agent-process/src/code_review.rs`:
  `proposal` and `code_roles` from attributed paths;
  `crates/amont-agent-process/src/review_outcomes.rs`: `seat_source`;
  tests: a proposal for markdown-only paths gives `none — markdown only`, a `.tsx` path gives react and ui-design, and an outcomes row carries `seat_source`.
- [x] Phase 4 — `crates/amont-agent-process/src/review_brief.rs`: the profile
  pathspecs in `build` and `interdiff_brief`, `## Not in this brief`, Goal and
  Behaviour. Supersede the panel's carried `--pathspec-from-file` requirement:
  `git diff` does not support it. Pass `:(literal)` pathspecs through bounded
  argv batches, accounting for argument and environment size, and combine
  their patches deterministically without duplicate files. An empty selection
  emits an empty patch without invoking an unrestricted diff. Keep the full
  stat independent of batching. `tests/review_brief.rs`: add a second language
  to `two_commit_repo`, test empty selections, and exercise enough paths to
  exceed the argument budget of a single invocation.
- [ ] Phase 5 — `docs/plan-review.md`, `CHANGELOG.md`; release amont-agent
  3.22.0 and install it.

## Decision log
- 2026-11-07 — unix and tui sit on any code path in a `cli` repository,
  wider than ADR-0061's "entry points, argument parsing, help and message
  text", because the ledger shows them catching serious findings in ordinary
  code files. A narrower trigger waits for the budget measure.
- 2026-11-07 — a plan whose Phases name some paths is seated from those
  paths even when another phase names none (backend round 1 asked for the
  area union then). Release and docs phases routinely name no file, so the
  union would give nearly every plan today's panel back. The advice names each
  such phase, and the backtest's zero-loss bar would show under-seating.
- 2026-11-07 (backtest, the person's decision) — the entry above is
  reversed: paths seat the panel only when every phase names one; otherwise
  the areas do. The backtest over 45 saved round-1 bodies found 13 seated
  from paths, all partial, and they dropped seats that had made 95 accepted
  high or blocking findings (13 po, 15 lang:rust, 10 unix, 10 tui, 8
  architect, 8 game-ux, 7 ux-research, 7 lang:typescript, …). Old plans name
  components, not files, so one incidental path (`release.yml`,
  `check-namespace-owners.sh`) flipped a plan to path seats. Under the
  reversed rule: 0 seats dropped, 0 findings lost, all 45 seated from areas.
  That shows no regression and saves no seat on history: savings arrive when
  plans name files in every phase (the skills build), and 36 of the 95 came
  from po, architect, ux-research and game-ux, which a full-path plan still
  drops unless it writes `adds=contract` or `adds=screen`.
- 2026-11-07 — no paths means advice and area seats, not a refusal, because
  the plan template does not ask for files per phase yet and other sessions
  are in plan mode now.

## Verification
Risk triggers: a classifier inside a hook (backtest), and a contract other
sessions consume (the panel list the hook enforces).
- **Backtest** on real past inputs: the branch binary's `plan-panel` over the
  47 saved round-1 bodies, each with its recorded `repos`, under a scratch
  `CLAUDE_CONFIG_DIR`. Expected: per plan, the new seats beside the seats that
  sat. Report the seats dropped in total and the accepted high or blocking
  findings those dropped seats made, from the ledger. Bar: zero accepted high or blocking findings from dropped seats; any such finding is listed and the person decides before release. It would show a
  classifier that drops seats that catch serious findings, or seats from
  cited files.
- `plan-panel` on this plan → `seats=paths`, backend, `lang:rust`, unix, tui.
  On the decisions plan `2026-11-07-review-follows-what-it-touches`
  (markdown-only Phases) → backend alone. Seats from cited context would show
  `lang:rust` there.
- A plan whose Phases name no paths → `seats=areas`, today's seats, and the
  hook's `plan-names-no-paths` advice in the journal, with ExitPlanMode
  allowed. A refusal would block every session in plan mode.
- `review-brief --profile lang:typescript` on relais #385's merged range →
  diff bytes before and after (expected about 96% fewer), the full stat, and
  the omitted Rust paths listed. A filter that dropped the stat or the list
  would hide what the profile did not read.
- `review-brief --profile lang:typescript --delta` on a two-round fixture →
  the patch filtered the same way. An unfiltered Delta would re-read the
  whole diff each round.
- The ledger reader on a pre-3.22.0 outcomes row → `seat_source` read as `areas`, no error. A reader that fails would break the budget measure on old rows.
- After installing 3.22.0, the next real plan's `plan-panel` prints
  `seats=paths` and the hook accepts that panel.

## Outcome
- Built phases 1–4 in `a72d4ab` and `da3b366`; the backtest above decided
  the partial-phase rule; review fixes in `67081d1` and `4e7481c`; the Windows CI fixture fix in
  `4fe7e55`. Phase 5's
  docs are in; its release and install of 3.22.0 follow the merge, with the
  post-install `plan-panel` check.
- Where the build differs from the text above:
  - Literal paths go through `git --literal-pathspecs` with bare paths, not
    `:(literal)` prefixes: the same literal matching, and it holds whether
    or not `GIT_LITERAL_PATHSPECS` is set in the environment.
  - A repository tree that cannot be read still asks the person, as before:
    attribution reads the same `ls_tree_z` listing as the areas, so a failed
    read leaves no areas to seat from either.
  - `none` lines also give `no reviewed file type` (named files, none of a
    type a profile reviews) and `no file named` (no named path is attributed
    to that repository).
  - A list item under Phases without a `Phase <n>` label counts as a phase
    and is named by its place (`phase 3`); a `🧑` item is not a phase.
  - Baselines carry `"names": 2`; one without it is read in the old names
    (`large` as `contract`, `ui` with `screen`).
  - `.sh` earns no class, so a shell script alone seats no `unix`/`tui`.
- Verification:
  - Backtest: as in the Decision log (45 plans; 2 more skipped because their
    `## Code review` lines do not parse, before and after this change).
  - This plan: `seats=paths`, backend, `lang:rust`, unix, tui. The decisions
    plan `2026-11-07-review-follows-what-it-touches`: backend alone (its
    Phase 4 names no file, and `decisions` has no area).
  - relais #385, `--profile lang:typescript`: 5 of 31 files, 15,682 patch
    bytes against 223,976 unfiltered (93% fewer, not the 96% expected), the
    stat whole, 26 paths under `## Not in this brief`.
  - homelab's last 10 manifest commits: each seats and proposes `platform`;
    the 10-manifest range's platform brief carries 10 of 10 files.
  - No-paths, partial, delta (on-top and rebased), cross-language rename,
    empty selection, a 1500-path batch and a pre-3.22.0 ledger row: tests in
    `tests/plan_review.rs`, `tests/review_brief.rs` and
    `crates/amont-agent-process/src/review_outcomes.rs`.

## Implementation review
- implementation-review (opus): awc → awc → approve, then approve on the Windows fixture fix (`d*.ts` → `d[a].ts`, `4fe7e55`). Fixed: bool flags, a swallowed discard, weak names, a test that could not fail. 55k+70k+29k+26k.
- backend: awc → awc → approve. Fixed: typed `seat_source` and stderr, batch errors, delta no-patch line, byte test, YAML ceiling. Deliberate: git stderr (`bytes_in` drops it), no drop guard. 91k+41k+25k.
- lang:rust: awc → awc → approve. Fixed: `Proposal`/`NoReviewReason`, `SeatSource::Areas { unnamed }`, `run_git`, checkbox trim. Deliberate: string classes, ceiling on `roles()`. 91k+41k+25k.
- unix: awc → approve. Fixed: `--code-review` help, non-UTF-8 names listed, plans left out by prefix. Deliberate: git stderr. 85k+41k. tui: awc → approve. Fixed: help, 80 columns, `no reviewed file type`. Deliberate: advice wording is the plan's. 84k+40k.

<!-- panel: repos=amont-agent adds= reviewers=backend,language,tui,unix,codex body-sha=4b4cc92612da -->
