---
status: active
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

- [ ] Phase 1 — `pulldown-cmark` dependency, `words()` + `body_sha` over
  it + `legacy_sha()`, tests.
- [ ] Phase 2 — dual acceptance (`judge`, shared `is_current`, `baseline`,
  `save`, journal `match=`), `plan-sha --legacy` with its conflict rule,
  tests.
- [ ] Phase 3 — docs: `plan-sha --help` (word-sequence rule, what no longer
  counts, `--legacy` temporary), `docs/plan-review.md` "canonical body"
  paragraph, CHANGELOG `Unreleased`. Outside this repo, same session:
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
- A panic inside `words` (test hook) → `judge` answers `ask`, never silence.
- Release binary size before and after the dependency, in bytes.
- Journal: a pass through a legacy binding on a plan whose excerpt runs
  over 300 characters → the written line contains `match=legacy`.
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

## Outcome

<!-- panel: repos=amont-agent reviewers=backend,lang:rust,tui,unix body-sha=92922faf9fdd -->
