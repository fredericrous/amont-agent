---
status: active  # next: Phase 3b after the 2.25 release
branch: feat/plan-sha-ignores-layout
repos: [amont-agent]
adrs: []
---
# Plan sha ignores formatting

## Review panel

👉 **Decide:** none — approve if a CommonMark parse (pulldown-cmark, pinned) as the sha's basis is right: 36/44 prettier diffs absorbed, 28/28 meaningful edits detected.
📍 amont-agent · plan reviewed, worktree `amont-agent-wt-plan-sha-layout` ready · next: Phase 1 `words()`. Panel: backend, lang:rust, tui, unix.
**Changed by review:** your P1s → CommonMark parse with kind-tagged items; your P2 → stale by-path and by-title baselines; parser panic → `ask`.
📄 Full reviews: [reviews](2026-10-06-plan-sha-ignores-formatting.reviews.md)
**Verdicts:** round 1: 4 approve-with-changes; round 2: backend approve; delta after your review: backend, rust approve-with-changes, applied; final backend: 3 low, applied at implementation (`k:row`, size bound, `plan-sha` panic exit check).

## Context

A plan's review binds to `body_sha`, the sha256 of `canonical()`
(`src/plan_review.rs:81`). `canonical()` already ignores CRLF, trailing
blanks, front matter, the review section and the machine comment, but every
other byte counts. A repository whose prettier hook formats `docs/plans/`
therefore changes the sha of a plan it lands, with no wording changed:
website-builder's hook added 11 blank lines before nested lists, and the
landing check (worktree-task step 4) offered only a delta review or "accept
unreviewed". The person wants to format plans freely.

Measured on the 47 plans in `~/.claude/plans/` with prettier 3 (default
options), using a Rust prototype of the rules below (pulldown-cmark 0.13):

| | plans |
|---|---|
| total | 47 |
| changed by prettier | 44 |
| same sha after the change | 36 |
| different sha: prettier changed code or text | 8 |

The 8 are honest mismatches: prettier rewrote embedded YAML/JS (comment
spacing, `'vitest'` → `"vitest";`, re-indented YAML), glued two code spans
together, or turned a `_` in prose into `*`. A word **count** was rejected:
`deny`→`allow` or `≤50`→`≤500` keep the count. A line-based token rule was
rejected in review: without block context, `Calculate a` followed by a line
`* b` collides with `Calculate a b`.

## Goal

`plan-sha` and the `plan-review-panel` hook give the same sha to a plan and
its formatted copy, so formatting a plan before or after it lands never
invalidates its review, while any change of wording, operator, identifier,
number or code still does.

## Non-goals

- A formatter that rewrites embedded code (quotes, comment spacing,
  re-indented YAML) or merges list items: still a mismatch, by design.
- Markdown structure: heading levels, list nesting, list-marker style,
  tight vs loose lists and emphasis style are no longer part of the sha.
  The kind of a block still counts: a list item, a quote, a heading, a
  table cell and a code block each differ from a plain paragraph, and an
  ordered list's start number counts.
- `tree-sha` / the implementation review: it excludes `docs/plans/` already.

## Behaviour

`body_sha(text)` = sha256 of `words(canonical(text))`. `words` parses the
canonical body as CommonMark with `pulldown-cmark`, options tables,
strikethrough and task lists on, smart punctuation **off** (it would
rewrite quotes in prose). Markdown structure is decided by the parser in
block context, never by a token's position on a line. It emits a sequence
of items, each prefixed by its kind so that no two kinds collide:

- `c:` **code block** (fenced or indented): the whole content collected
  into one item, exactly as the parser returns it. CommonMark removes only
  the indent of the enclosing container and of the fence, so blank lines,
  multi-line string literals and inner indentation all count. One item per
  block, never per parser event, so how the parser splits a block's text
  cannot change the sha. Preceded by `k:code <info string>`.
- `s:` **code span**: its content exactly as parsed, one item, so
  `"a  b"` ≠ `"a b"` and a tab counts. `` `deny` `` ≠ the word `deny`.
- `w:` **prose words** (text, soft and hard breaks, inline HTML): split on
  whitespace after the parser has resolved escapes (`\_` → `_`) and
  removed emphasis and strong markers. A soft break is a space, so reflow
  does not count. Operators are text: `a * b`, `a + b`, `x > y` keep their
  `*`, `+`, `>`.
- `u:` **link and image destinations**.
- `k:` **block kinds**: `k:item`, `k:quote`, `k:heading`, `k:cell`,
  `k:list <start>` for an ordered list, `k:code <info>`. Every other block
  start or end (paragraph, unordered list, table row, rule) is a plain
  `k:`, and a plain `k:` next to any other `k:` item adds nothing. So a
  tight list and the same list made loose by blank lines are equal, while
  `- deny` ≠ `deny`, `> deny` ≠ `deny` and `5.` ≠ `50.`.

Items are joined with `\n` and sha256'd. `canonical()` is unchanged;
section stripping and front matter behave as today, so the legacy sha stays
computable.

**A parser panic.** The hook's `catch_unwind` (`src/hook.rs:51`) turns any
panic into silence, which would let a plan through unreviewed.
`plan_review` therefore computes the sha inside its own `catch_unwind`, and
a panic becomes `ask`: "the plan's sha cannot be computed; approve it
unreviewed?". `plan-sha` reports it on stderr and exits 1.

**Dependency.** `pulldown-cmark = { version = "=0.13.4", default-features =
false }`, pinned exactly: a parser update may change the sha of every plan,
so a bump is its own PR that runs the fixture test below. It brings `bitflags`, `memchr` and `unicase` (MSRV 1.71.1, under this
crate's 1.85.0). The `[dependencies]` comment in `Cargo.toml` names
Markdown as the third input that needs a real parser and is not ours. The
person chose this over a hand-rolled block scanner (2026-10-06). The
release binary's size before and after is recorded in Verification
(`opt-level = "s"`: size matters here).

**Transition.** The sha computed before this change is the **legacy** sha,
`sha256(canonical(text))`.
- `judge` treats a binding or a baseline as current when its sha equals the
  new **or** the legacy sha (`Facts` gains `legacy`). `panel()`
  (`src/plan_review.rs:1289`) stops repeating that comparison and calls a
  shared `is_current(baseline, facts)` with `judge`, so `plan-panel` and the
  hook never disagree.
- `baseline()` looks up, in order: by-path when its `sha` equals the new
  or the legacy sha; `by-body/<new>.json`; `by-body/<legacy>.json`; the
  by-path record **even when its sha matches neither** (same file, edited
  body); then `by-title/<key>.json` where the key is the sha256 of the H1
  and the sorted `repos=`. The last two keep what `judge` needs for a delta
  (`src/plan_review.rs:895`): an edited, approved plan needs backend plus
  the reviewers of new areas, not the full panel. The title key covers a
  fresh session, where the same plan gets a new random file name. Its risk:
  two different plans with the same H1 and repositories share a baseline,
  so the second one is judged as a delta (backend is still required on its
  body). `save` after a pass writes by-path, by-body and by-title under the
  new sha, so the store migrates itself.
- The hook's journal line carries `match=new|legacy ` at the **start** of
  its `excerpt` (`journal::Entry`, `src/journal.rs:206`): the excerpt is cut
  at 200 characters (`MAX_EXCERPT`, `src/journal.rs:47`), so a suffix could
  be lost. No reader of the journal format changes.
- `plan-sha --legacy` prints the legacy sha. It works with `--short` and
  `-`; `--legacy --block` is a usage error (exit 2: `--legacy only prints a
  sha; drop --block`), so a reviewer block never carries a legacy sha. It
  prints `amont-agent: --legacy is removed after the transition` on stderr;
  stdout stays one line. Exit codes are unchanged.
- **Removal:** a later PR deletes the legacy path once the journal shows no
  `match=legacy` for 14 days (`// LEGACY:` markers name every site).
- **Rollback cost:** reverting to 2.24 makes baselines and blocks written
  under the new sha unknown: each plan in flight needs one panel re-run.
  After removal, `by-body/<legacy>.json` files are orphans (harmless).

## Phases

- [x] Phase 1 — `pulldown-cmark` dependency, `words()` + `body_sha` over
  it + `legacy_sha()`, tests.
- [x] Phase 2 — dual acceptance (`judge`, shared `is_current`, `baseline`,
  `save`, journal `match=`), `plan-sha --legacy` with its conflict rule,
  tests.
- [x] Phase 3 — docs: `plan-sha --help` (word-sequence rule, what no longer
  counts, `--legacy` temporary), `docs/plan-review.md` "canonical body"
  paragraph, CHANGELOG `Unreleased`.
- [ ] Phase 3b (dotfiles, after the 2.25 release is installed) —
  worktree-task step 4 gains "if `plan-sha --short F` differs, run
  `plan-sha --legacy --short F`; either must equal `body-sha`".

## Decision log

- 2026-10-06 — word sequence, not word count or a prettier pass, because
  a count misses substitutions and a formatter pass ties amont-agent to each
  repo's formatter config.
- 2026-10-06 — review round 1 (backend, rust, tui, unix): structure tokens
  dropped only at line start, fences compared as written, `_`/`*` kept
  inside words, `panel()` shares `judge`'s check, `--legacy --block`
  refused, removal measured by the journal instead of a date.
- 2026-10-06 — review round 2 (backend): own fence tracker at any indent,
  `*`/`_`-only tokens kept, `match=` leads the journal excerpt, inputs
  pinned, corpus re-measured with the final rules (35/44 unchanged).
- 2026-10-06 — the person's review of the presented plan: line rules
  collided on operators across a line break (`a` / `* b`), fences dropped
  blank lines inside string literals, code spans lost inner spaces, and
  the baseline lookup dropped the stale by-path baseline that a delta
  needs. Replaced the line rules with a CommonMark parse; the person chose
  `pulldown-cmark` over a hand-rolled scanner or a narrower guarantee.
  Measured: 22/22 probes, 36/44 corpus.
- 2026-10-06 — delta review (backend, rust): items tagged by kind, block
  kinds and the ordered-list start kept, one item per code block, parser
  pinned exactly with a fixture sha, a panic answered `ask`, a by-title
  baseline for a fresh session. Measured: 28/28 probes, 36/44 corpus.
- 2026-10-06 — implementation: the final backend's low findings applied.
  A table row is `k:row` (two one-cell rows ≠ one two-cell row, tested).
  Plans in flight gain a by-title record only at their next pass, so a
  fresh session on one of them before that still gets `full`. The
  round-2 "35/44" above is superseded by 36/44.
- 2026-10-06 — implementation: a plan opening with an unclosed `---`
  (not front matter) now hashes like the plan without it, because CommonMark
  reads that `---` as a thematic break, which says nothing. The existing
  front-matter test asserts the byte difference on `legacy_sha` instead.
- 2026-10-06 — the worktree-task skill line moves to Phase 3b: it tells
  agents to run `plan-sha --legacy`, which the installed 2.24.0 does not
  have, so it lands in dotfiles once 2.25 is installed.
- 2026-10-06 — outside the phases, commit 1ff0f1f: `tests/hook.rs` ran the
  hook with the developer's global git config, so this machine's
  `amont.agent.implementation-review.stance deny` made
  `a_push_from_an_unrehearsed_tree_is_advised_and_a_stamped_one_is_not`
  fail here (on `origin/main` too) while CI passed. It blocked this
  branch's pre-push gate, so it rides in this PR: the hook now sees an
  empty global and no system git config, as `tests/plan_review.rs` does.

## Verification

Each check: input → expected → observed.
- Unit tests (`cargo test plan_review`), each a pair → sha relation.
  → **equal**: blank lines added; blank line added before a nested list;
  reflowed paragraph; padded table; `*x*`→`_x_`; `*"x"*`→`_"x"_`;
  `a\_b`→`a_b`; list marker `*`→`-`; a fenced block re-indented with
  its list item (`-   x`, content at column 4 → `- x`, column 2); a nested
  list made loose by blank lines.
  → **different**: `Body.`→`Body!`; `Calculate a * b`→`Calculate a` +
  newline + `* b`; `Calculate a b`→`Calculate a` + newline + `* b`;
  `a + b`→`a` + newline + `+ b`; `x > y`→`x` + newline + `> y`;
  `a > b`→`a >= b`; `a * b`→`a b`; `MAX_BLOBS`→`MAXBLOBS`;
  `_private`→`private`; `` `"a  b"` ``→`` `"a b"` ``; a blank line removed
  inside a fenced Python triple-quoted string; one line's indent inside a
  fence; `5`→`50`; two words swapped; a link's URL changed.
  `- deny`→`deny`; `> deny`→`deny`; `5. a`→`50. a`; `` `deny` ``→`deny`;
  `` `a<tab>b` ``→`` `a<tab><tab>b` ``.
  `canonical` idempotence kept.
- `legacy_sha(PLAN)` equals a hex constant computed with the 2.24.0 binary.
- `judge` with a binding at the legacy sha → Pass; `panel()` with a legacy
  baseline → `current`; `save` after it writes the new sha.
- Same file, edited approved plan: a by-path baseline whose sha matches
  neither → `panel()` and `judge` require backend only; with a new area
  → backend plus that area's reviewers.
- Fresh session: an approved plan copied to a new path, one line edited,
  `amont-agent plan-panel` → `panel=delta`, backend only (today: `full`).
  A different H1 at a new path → `full`.
- `body_sha(PLAN)` equals a hex constant (next to the legacy one), so a
  parser change that re-hashes plans fails `make check`.
- A panic injected in `body_sha` (test hook) → the hook's `examine` answers
  `ask`, never silence.
- Release binary size before and after the dependency, in bytes.
- Journal: a pass through a legacy binding → the written line contains
  `match=legacy`; observed by construction that it survives the 200-character
  cut, because `match=` leads the excerpt (`format!("match={matched} …")` in
  `examine`).
- CLI: `plan-sha --legacy --block f; echo $?` → `2` and one stderr line;
  `plan-sha --legacy f 2>/dev/null | wc -l` → `1`.
- Corpus, built binary: `plan-sha --legacy` over the 47 plans equals the
  2.24.0 `plan-sha` → 47/47. New sha of each plan vs its prettier copy →
  36/44 equal, the 8 listed. Mutation probe: 20 operator/identifier/number
  substitutions over 5 plans (`atomic-munching-snowglobe`,
  `cuddly-dazzling-llama`, `env-file-per-cluster`,
  `i-will-address-the-resilient-candy`, `tingly-pondering-eclipse`) → 20/20
  change the sha.
- The case that started this: `website-builder-wt-icon-picker/docs/plans/2026-10-06-icon-choice-list.md`
  (prettier-formatted, landed) vs `~/.claude/plans/tingly-pondering-eclipse.md`
  (machine comment `body-sha=0b9155c30f53`) → the new `plan-sha` is equal on
  both, and `--legacy --short` on the source gives `0b9155c30f53`.
- `make check` (fmt, clippy -D warnings, tests) clean.

### Observed (2026-10-06, before the push)

- Unit pairs: `plan_words` 11 equal + 22 different, all as expected;
  `plan_review` 26 tests pass, including the two fixture shas
  (`ab25150b…` new, `db6b774a…` legacy, the latter printed by the 2.24.0
  binary for the same plan), legacy binding → Pass, legacy baseline →
  current for `judge` and `is_current`, guarded panic →
  `Err(ShaError::ParserPanicked)`, and a panic injected in `body_sha` →
  `examine` answers `Ask("…cannot be computed")`, excerpt `sha unavailable`.
- Integration (`tests/plan_review.rs`, 18 pass): a formatted copy keeps its
  review and a one-word edit does not; an approved plan at a new path with
  one line edited → `panel=delta` and `plan-review-backend` only, and the
  hook passes with backend alone; another H1 → `panel=full`; reviews whose
  blocks carry the byte sha → pass, journal line has `match=legacy`, and
  its baseline is saved as `by-body/<new sha>.json`; a pass on reviews of
  the new sha journals `match=new`; a baseline only 2.24 wrote
  (`by-body/<byte sha>.json`), found from a new path → `panel=current` and
  a silent hook;
  `--legacy` → one stdout line plus the stderr notice; `--legacy --block`
  → exit 2, empty stdout, `--legacy only prints a sha; drop --block`.
- Corpus, release binary: `plan-sha --legacy` = 2.24.0 `plan-sha` on 48/48
  plans (the 47 plus this one). Prettier copies: 36/44 equal; different:
  fluttering-conjuring-thacker, iridescent-snacking-biscuit,
  jolly-hugging-bunny, let-s-build-the-cloud-side-squishy-gray,
  let-s-find-q-solution-proud-aurora, let-s-plan-a-full-cosmic-newell,
  let-s-plan-the-vpn-sleepy-russell, vault-paths-kebab-case (embedded
  YAML/JS rewritten, code spans glued, `_` turned into `*`).
- Mutation probe: 19/19 change the sha (number, identifier, operator, word
  swap in each of the 5 plans; `tingly-pondering-eclipse` has no
  `snake_case` identifier in the probed range, so 19, not 20).
- website-builder: the landed copy differs from its source, because landing
  added a `## Decision log` line ("Accepted unreviewed by the person").
  With that line removed, the prettier-formatted copy hashes `999ff9c23ce5`
  like the source; its legacy sha is `9930b135698b`, the mismatch the person
  reported, and the source's legacy sha is `0b9155c30f53`, its `body-sha`.
- Release binary: 1,975,328 → 2,209,392 bytes (+234,064, +11.9%).
- `make check`: fmt and clippy clean; `cargo test --no-fail-fast`: 517 unit
  tests and every integration suite pass except `tests/hook.rs`
  `a_push_from_an_unrehearsed_tree_is_advised_and_a_stamped_one_is_not`,
  which failed the same way on `origin/main` (2a3aabb) on this machine and
  passes in CI there: the developer's global git config leaking into the
  test (fixed in 1ff0f1f, see the Decision log).
- That test, with the global `amont.agent.implementation-review.stance deny`
  still set: before 1ff0f1f → fails (3 runs here, 1 on `origin/main`),
  denied as "the refspec source does not resolve to a commit"; after →
  passes, and `tests/hook.rs` 35/35.
  `make msrv`: builds on 1.85.0.
- `--legacy --short` and `-`: one stdout line (integration test).

## Implementation review

approve-with-changes, then approve-with-changes on the delta; every finding fixed.
Fixed: exhaustive `Event` match; dependency ownership comment; typed `ShaError`; `match=legacy` only when the pass needs the byte sha; tests for panic → `ask`, store migration, a 2.24-only baseline.
Fixed in the record: observed results and test counts updated, the 300-character journal check stated as by construction.
Round 1: 66k tokens, 68 s; delta: 35k, 34 s.
The test-isolation commit (tree ad1ff3f6…): approve-with-changes, 35k, 23 s; both findings were record gaps, now fixed.

## Outcome

Shipped in this branch: the CommonMark sha, the legacy transition, the
by-title baseline, the panic → `ask` guard, `plan-sha --legacy`, docs.
Not yet: Phase 3b (dotfiles skill line, after the release), and the legacy
removal PR (when the journal shows no `match=legacy` for 14 days).
Surprise: the person's own case also carried a Decision-log line added at
landing, which no formatter rule can absorb, nor should.

<!-- panel: repos=amont-agent reviewers=backend,lang:rust,tui,unix body-sha=92922faf9fdd -->
