---
status: active
branch: feat/mockup-fidelity
repos: [amont-agent, application-landscape]
adrs: [decisions:ADR-0016, decisions:ADR-0023]
---
# Mockup fidelity is checked before approval and at review

## Review panel

👉 **Decided 2026-09-29:** `push-preview` denies a push of a mockup-mode commit with no bound approval; other UI pushes stay Advise (goes in Phase 2).
📍 application-landscape, then amont-agent · planned, nothing built · next: AL ui-handoff check PR. Panel: backend, architect, po, lang:rust, lang:typescript, platform, react, ui-design, ux-research, game-ux, tui, unix.
**Changed by review:** one after/live vocabulary shared by both checks; register diffs from the merge-base (not upstream); the check is fail-closed, runs under concurrency, and skips the full fetch for `Mockup: none`.
**Verdicts:** 12 approve-with-changes in round 1; backend approve-with-changes in rounds 2 and 2b (one blocker fixed: a shallow fetch had no merge base). 📄 Full reviews: [2026-09-29-mockup-fidelity-checks.reviews.md](2026-09-29-mockup-fidelity-checks.reviews.md).

## Context

On 2026-09-29 the application-landscape masthead History pill (PR #302) was
put up for approval although it did not look like the mockup the person had
picked (direction B). The person saw it, rejected it, and asked why nothing
caught it. Three things were missing.

- **No mechanical check.** `handoff.prove-fidelity` (ADR-0016) asks for the
  picked artboard next to a screenshot of the built route, taken at the
  artboard's width and theme. Nothing checks that. `amont-agent preview
  register` checks the guide's five sections, and a Reference image is only a
  warning. application-landscape has no `ui-handoff` PR check; duro-app does.
- **The approval never bound.** The journal records `push-preview - unbound …
  preview register: not a standalone command`: the register call was chained
  after `cp`/`python3`. The marked question also said
  `application-landscape@2fcffec`, while the label (`src/preview.rs:508`) is
  the checkout directory's name, `al-wt-history-mockup@2fcffec`. Both failures
  are silent, and a wrong label even drops the registration
  (`preview.rs:1045-1057`). The hook correctly advised at push, and the agent
  pushed anyway.
- **Heading matching is brittle.** `## 👉 Try it (about 2 minutes)` was
  refused (`src/guide.rs:51,109`).

**Goal.** A PR that carries a picked mockup's artboards cannot reach approval
without three things:
- the artboard;
- a screenshot of the built screen at the same width and theme;
- a stated list of differences.

It cannot pass the AL PR check without the same evidence. A binding failure
tells the agent at once.

**Scope.** The guide check covers the PR that commits the artboards (the
`handoff.fresh-directions-trigger` case). A later PR that edits the same
screen without new artboards is covered by the AL CI check, which asks it to
say `Mockup: none` with a reason, or to show the side-by-side anyway.

**Plan home.** This plan lives in amont-agent. application-landscape carries a
pointer file.

## One vocabulary, used by both checks and the templates

- **Proof images** are committed next to the artboards, in
  `docs/mockups/<screen>/`:
  - `artboard.png`, the design: the picked direction's artboard, rendered at
    its own width;
  - `after.png`, the built screen: taken in a real browser at that width and
    theme.
  - Optional states pair by suffix: `artboard-empty.png` with
    `after-empty.png`.
  - The checks also accept `mockup` for the design role and `live` for the
    built role.
- **A viewport line**, `Viewport: <width>px · <theme>` (for example
  `Viewport: 1120px · light`), appears in the guide's Reference and in the PR's
  `## Mockup` section.
- **A differences list**: under `## Differences from the mockup` in the guide,
  or `Differences from the mockup:` in the PR. It is either `None`, or bullets
  that each start with `fixed:` or `deliberate:` followed by a reason.
- **`Mockup: none` needs a reason.** The separator may be `-`, `--`, `–`, `—`
  or `:`, followed by non-blank text.

## Phase 1: application-landscape PR check (ships first, alone)

It depends on nothing in Phase 2, and it blocks the outcome.

- **Pointer file** `docs/plans/2026-09-29-mockup-fidelity-checks.md`, with
  `canonical:` the amont-agent plan and `phases: [1]`.
- **`.forgejo/workflows/ui-handoff.yaml`**, its own workflow like `adr.yaml`,
  so editing a PR body never reruns E2E.
  - `on: pull_request` with types `[opened, synchronize, reopened, edited]`.
  - `permissions: contents: read`, `runs-on: application-landscape`.
  - `concurrency: {group: ui-handoff-<ref>, cancel-in-progress: true}`, since
    the forge has six runner pods.
  - The body reaches the script only through `env: PR_BODY`. No `${{ }}`
    appears in any comment (Forgejo answers 422).
  - **Two steps.** The first step checks the body with no checkout: a
    `Mockup: none` line with a reason ends the job green without fetching
    anything. Otherwise the second step checks out with `fetch-depth: 0`, as
    `ci.yaml:45-47` does. `base...head` needs the merge base, and a shallow
    base fetched at depth 1 has none. The full fetch is paid only by PRs that
    claim a mockup or say nothing.
- **`.forgejo/scripts/ui-handoff.sh`** lives outside `scripts/`, so the E2E
  gate in `ci.yaml:97`, which watches `scripts`, does not fire on it. Its exit
  codes: 0 pass or skip, 1 missing evidence (`::error::` lines on stderr), 2
  setup fault.
  1. **A `Mockup: none` line** with a reason passes before any git work.
  2. **The diff runs as its own statement.** `changed=$(git diff --name-only
     "$BASE...$HEAD")`, and a failure exits 2. There is no pipe and no `||
     true` around it.
  3. **The screen filter** is a `case` statement over that list. A PR touches
     a screen when it changes any of:
     - `app/routes/**`, except `api.*` and `*.test.*`;
     - `app/components/**`, `app/hooks/**` and `app/lib/**`, except tests
       (`app/.server/**` is excluded);
     - `app/styles/**`, except the generated `duro-theme.css`;
     - `app/root.tsx`, `app/app.css` or `app/i18n/**`;
     - `docs/mockups/**`.

     No match exits 0.
  4. **Otherwise `## Mockup` is required.** HTML comments are stripped first
     (the duro-app check counts the template's commented placeholders). The
     section must have:
     - a URL;
     - `direction [A-Z]`;
     - a `docs/mockups/<screen>` path that exists in the head tree;
     - at least 2 images;
     - the viewport line;
     - a differences list in the shared vocabulary.

     Each missing piece gets one `::error::` line that quotes the exact line
     to paste. One closing hint gives the three prove steps inline.
- **`.forgejo/PULL_REQUEST_TEMPLATE.md`** has these parts:
  - `## What`;
  - `## Mockup`: canvas · direction, the artboards path, the viewport line, a
    `| artboard | after |` table and the differences;
  - the `Mockup: none — <reason>` alternative.
- **Install the duro-mockup skill:** `duro skill install` in AL. It writes
  `.claude/skills/duro-mockup/SKILL.md`, and that path is added to
  `.prettierignore`; neither is in the `ci.yaml:97` gate paths. The hint and the agent then have the
  `/duro-mockup` workflow.
- **Tests:** `scripts/ci/ui-handoff.test.ts` would not be collected, so the
  test lives at `.forgejo/scripts/ui-handoff.test.ts`, and `vitest.config.ts`
  `test.include` gains `.forgejo/scripts/**/*.test.ts`. The unit step in CI
  then runs it. Each case gets a temporary repo (`mkdtemp`,
  `GIT_CONFIG_GLOBAL=/dev/null`, a local identity, `spawnSync` with an
  explicit env). One row per case:
  - `none` with each separator passes; `none` with no reason fails;
  - a change only to `e2e/` skips;
  - `api.x.ts`, `x.test.tsx` and `duro-theme.css` each skip;
  - `app/lib/x.ts` requires the section;
  - commented template placeholders fail;
  - a complete section passes;
  - a missing `docs/mockups` path fails;
  - a differences bullet without `fixed:`/`deliberate:` fails;
  - an unreachable base exits 2;
  - a full clone of a branch off main diffs `base...head` and exits 0 or 1,
    never 2.

## Phase 2: amont-agent (one PR; release only when the person asks)

### 2a. Mockup mode in `preview register` (`src/guide.rs`, `src/preview.rs`)
- **Range.** `register()` always diffs `merge-base(HEAD, <default
  remote>/<default branch>)..HEAD`, resolved through `stale.rs:125-173`. It
  never uses `published()`'s upstream ref, which drops what was already pushed.
  If no base resolves, the command refuses with exit 1 and the reason. It
  never switches the check off.
- **Screens** are the folders under the mockups directory (`git config
  amont.agent.preview.mockups`, default `docs/mockups`; the AL script reads
  the same key) that the range changes and that hold a `*.dc.html`.
- **What mockup mode requires, on top of today's check:**
  - Reference names each screen path, holds the viewport line, and embeds a
    design image and a built image for at least the default pair.
  - Both image files exist. Relative paths resolve next to the guide and
    absolute paths are checked too; today `missing_images` (`guide.rs:735`)
    skips them.
  - `## Differences from the mockup` in the shared vocabulary.
  - When the two PNG widths, read from the IHDR, differ by more than 10%, a
    warning names both widths.
- **The refusal teaches.** It gives:
  - a line explaining the mode (`mockup mode: this commit changes
    docs/mockups/<screen>/Main.dc.html`);
  - each missing piece, with its file name and folder;
  - a ready-to-paste Reference image block and a differences skeleton naming
    the screen;
  - lines within 80 columns.
- **Render (`guide::render`).** `pair()` becomes a struct `{artboard, after,
  before: Option}`, and none of the three is also rendered inline. The
  artboard and the built image sit full width, stacked at the same scale.
  Each image links to its full-size file, and an overlay toggle (a small
  opacity slider) lays one over the other. The before image comes after. The
  captions are "Mockup, direction X" and "Built, <sha7>", with the viewport
  line under them. Alt text comes from the markdown.
- **A guide with no mockup screen** checks exactly as today. Proof: there are
  2 guides under `~/.claude/amont-agent/attestations/`.
  - `e115e20` is registered in a scratch repo with no mockups and must pass
    unchanged.
  - `2fcffec` is the #302 guide. It is registered inside a worktree of #302
    at `2fcffec` and must be refused: that is the incident this plan exists
    for.

### 2b. Headings tolerate a trailing note
A heading whose leading whole words equal a section name, followed only by a
parenthetical or a dash note, matches that section: `Try it (about 2
minutes)`. A longer section name is never swallowed by a shorter one, and a
unit test shows a heading that must stay unknown.

### 2c. Binding failures reach the agent (`hook.rs`, `preview.rs`)
- **`bind()` and `on_post_ask()` return `Option<String>`.** It merges into
  `Decision::Assert` (PostToolUse `additionalContext`, `decision.rs:106-116`).
  `bind` moves above the background early return (`hook.rs:299-305`).
  Nothing is emitted under the `observe` stance.
- **An unbound register** says "registered but NOT bound: <why>". It prints
  the full command with the real arguments (`cd <repo> && amont-agent preview
  register --repo … <url> <guide>`) and the label to put in the question.
- **Labels.** `register()` works out the aliases once and prints `label` and
  `aliases` in its JSON: the directory name, the main worktree's directory
  name, and the origin repo name. A match still needs sha7 plus one alias.
  The aliases live in a sidecar keyed by id, so the 9-field registrations line
  (`preview.rs:493`) keeps its format for older binaries. `from_json`
  tolerates the new fields.
- **A marked question with a known id but a wrong label** says so in context,
  and the registration stays pending, pushed back into `rest`. It is no longer
  dropped.
- **Docs.** The help text (`main.rs:894-930`, including `label` in the JSON
  and exit codes 0/1/2), `docs/preview.md` and the worktree-task skill's
  preview template (in `~/.claude/skills/worktree-task/`, versioned in
  dotfiles, so its own small PR there) describe the viewport line, the
  differences section and the proof images. They also say that copying the
  PNGs next to the guide is its own step, before the register.

### Tests (`make check`: fmt, clippy -D warnings, test --locked)
- **`src/guide.rs` units:**
  - a heading with a note matches; an unknown one stays unknown;
  - the differences rules;
  - `None` passes;
  - `live`/`mockup` aliases;
  - an absolute image path that doesn't exist is refused in mockup mode.
- **`tests/preview.rs`:**
  - a mockup branch with no artboard is refused, and stderr names every
    missing piece;
  - the complete guide registers, with no before image, and `index.html` has
    both figures;
  - a mockup branch already pushed with an upstream is still refused;
  - a repo with no default remote is refused with a reason;
  - the old `GUIDE` in a repo with `origin/HEAD` registers unchanged;
  - a chained register emits the "NOT bound" context, and the printed command
    binds when run;
  - a wrong label then the right label approves;
  - a question naming the main-worktree alias approves a worktree commit;
  - an alias at a colliding sha7 from another repo does not;
  - an old-format registrations line still parses.
- **Helpers:** an `answer_out` helper keeps the PostToolUse output.

## Verification (input → expected; the actual goes in the plan when run)

| Input | Expected |
|---|---|
| AL: `npx vitest run .forgejo/scripts/ui-handoff.test.ts` | ≥14 cases collected, all pass |
| AL: the Phase 1 PR itself (it touches only `.forgejo/`, `vitest.config.ts`, docs) | ui-handoff skips green; the ci.yaml gate log prints "skipping the E2E suite" |
| AL #302 (open) with its current body | ui-handoff red, naming the missing `## Mockup` pieces |
| #302: commit `docs/mockups/masthead-history/artboard.png` + `after.png`, then edit the body only | a new ui-handoff run starts from the edit (proves `edited`) and turns green; if no run starts, the next push turns it green, and the workflow header records that `edited` does not fire |
| #302: three body edits in quick succession | one run finishes, the others are cancelled |
| `merge-when-green` on #302 | waits for ui-handoff as well as CI (if it reads only CI, fix the skill in the same round) |
| amont-agent `make check` | green |
| Scratch repo, pushed branch adding `docs/mockups/x/Main.dc.html`, old guide | `register` exits 1, stdout empty, stderr lists the differences section, the viewport line and both images |
| Same with `artboard.png`, `after.png`, viewport, differences | registers; `index.html` shows both figures at the same scale with the overlay |
| Real session: a chained register | the transcript shows `additionalContext` "NOT bound" with a command that binds when run |
| Real session: a question labelled with the main repo name, from a worktree | the journal shows `approved`; no advise at push |
| `2fcffec/guide.md` registered in a worktree of #302 at `2fcffec` | refused: no artboard, viewport line or differences |
| `e115e20/guide.md` in a scratch repo without mockups | registers as before |

**Push discipline.** Neither PR changes an app screen: AL adds CI files and
#302 gets PNGs and a body. So no guided preview is needed. Each repo's checks
run locally before its push. There is one PR per repo, merged with
`merge-when-green` when the person asks.

## Verification record (2026-09-29)

| Input | Expected | Actual |
|---|---|---|
| AL `npx vitest run .forgejo/scripts/ui-handoff.test.ts` | ≥14 cases, pass | ✅ 26/26 (macOS); CI unit step on the runner green (#303) |
| AL #303 checks | ui-handoff, ADR, CI green | ✅ all three `success` on aa0897c |
| amont-agent `make check` | green | ✅ (fmt, clippy -D warnings, 491 unit + 35 preview integration + other suites) |
| Pushed branch adding `docs/mockups/x/Main.dc.html`, old guide | exit 1, stdout empty, stderr names the mode and every piece | ✅ `a_mockup_branch_without_the_side_by_side_is_refused`, `…already_pushed_is_still_in_mockup_mode` |
| Same with the images, viewport, differences | registers; page shows both figures with the overlay | ✅ `a_complete_mockup_guide_registers_and_renders_the_pair` |
| Artboards, no default remote | refused with the reason | ✅ `artboards_with_no_default_branch_to_compare_are_refused_not_skipped` |
| Chained register (hook payload) | PostToolUse context "NOT bound" + a command that binds when run | ✅ `a_chained_register_is_not_bound` (extended) |
| Question with another repo's name, then the right label | first says "bound NOTHING" and stays pending; second approves | ✅ `a_question_with_the_wrong_label_…` |
| Question naming the main checkout, from a worktree | approved; push not advised | ✅ `a_worktree_commit_is_approved_under_the_main_checkouts_name` |
| Unapproved mockup-mode push / plain UI push / observe | deny / advise / not deny | ✅ `an_unapproved_mockup_commit_is_held_and_a_plain_ui_one_is_advised` |
| `2fcffec/guide.md` registered in #302 at 2fcffec (debug build) | refused | ✅ refused: screen path, viewport, artboard/after, differences |
| `e115e20/guide.md` in a scratch repo without mockups | registers | ✅ exit 0 |
| Real session with the released binary (chained register; worktree label) | as the payload tests | pending: needs the release installed |
| #302 retrofit (edited trigger, three-edit cancel, merge-when-green reads ui-handoff) | per plan | pending: needs #303 merged |

Deviation: refusal lines are not wrapped at 80 columns; the items quote
absolute paths, and wrapping them would break copy-paste. The paste-ready
block is short.

## What to measure after release
- **Previews rejected for not matching the mockup** over 30 days. Source:
  the journal's marked answers (`Request changes`) on previews registered in
  mockup mode, read against the person's reason. Target 0; any rejection of a
  guide that passed means rework.
- **`unbound` journal entries** that were not followed by a bound register
  within the same turn. Target 0.
- **ui-handoff failures about format** (separator, heading) instead of missing
  evidence. More than 1 in 5 means loosening the parser. Also count path
  filter misses on the next 10 UI PRs.
- **Rollback.** If mockup mode refuses a guide that the person then approves
  unchanged, loosen that rule.

## Decision log
- 2026-09-29: the check keys on mockup artboards in the branch's diff, not on
  a flag the agent sets: the agent that skipped the side-by-side would forget
  a flag. Later PRs on the same screen are the CI check's job (see Scope).
- 2026-09-29: differences are the author's statement. The tools require it
  and its classification; judging the pixels stays with the person, helped by
  the full-width pair and the overlay.
- 2026-09-29: the PR check ships first. It stands alone and it is the gate a
  red run shows. There are no required status checks on these repos, so
  `merge-when-green` has to read it (see Verification).
- 👉 open: the `push-preview` stance for a commit in mockup mode (below, in
  the review panel).

<!-- panel: repos=amont-agent,application-landscape reviewers=backend,architect,po,language:rust,language:typescript,platform,react,ui-design,ux-research,game-ux,tui,unix body-sha=77cc4cf799fd -->
