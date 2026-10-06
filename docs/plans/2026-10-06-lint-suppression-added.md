---
status: active
branch: feat/lint-suppression-added
repos: [amont-agent]
adrs: [decisions:ADR-0011]
---
# Lint suppression added

## Review panel
👉 **Decide:** none — approve if advising on added suppressions, measured over a 2-week soak, is the right first step before deny.
📍 amont-agent · plan reviewed · next: worktree, Phases 1–5, then release 2.26.0 and brew upgrade. Panel: backend, lang:rust, tui, unix.
**Changed by review:** markers compared instead of whole lines; bounded, FIFO-safe read with journaled fallback reasons; exact advise text and journal excerpt.
**Carried to implementation** (backend, final pass): a `ReadFixture::with_content` helper for Phase 4; the reported line is approximate when the marker already exists, so it is listed as a known approximation with a test.
**Verdicts:** round 1: 4 approve-with-changes; round 2 and final: backend approve-with-changes, every finding resolved.
📄 Full reviews: [2026-10-06-lint-suppression-added.reviews.md](2026-10-06-lint-suppression-added.reviews.md)

## Goal
Agents add lint suppressions instead of fixing the finding. A transcript scan
on 2026-10-06 found 101 Edit/Write calls that added one, out of 99,112 tool
calls: 1.02 per 1,000, or about 1.7% of code edits (the planning scan; the
shipped script measures 1.01 over 99,810 calls, see Observed). Week 41 already has 22.
Examples: `#[allow(clippy::too_many_arguments)]`,
`// eslint-disable-next-line react-hooks/exhaustive-deps`, and
`# pyright: reportPrivate…` headers. A new rule, `lint-suppression-added`,
tells the agent at the moment of the edit that `general.no-disabled-safety`
applies (decisions ADR-0011, tightened in decisions#41). It covers an added
suppression and a lint configuration made looser. Closes amont-agent#65.

## Non-goals
- No deny at release. The default is `advise` and the ceiling is `deny`, so a
  soak of about 2 weeks can graduate it, as with read-unbounded-large.
- No NotebookEdit: it uses `notebook_path` and cell JSON, and the scan found
  no suppressions there.
- No linter-side enforcement: that is `general.linters-police-suppressions`,
  set up per repository.
- No Bash writes (`sed -i`, heredocs). The transcripts show these edits go
  through Edit and Write.
- No judgement of the reason text. The advice asks for a cause outside the
  repository; the rule cannot verify one.
- No new crate. TOML, YAML and JSON are scanned by hand, line by line, and
  the forms the scanner does not handle are known false negatives (listed
  below).

## Behaviour

### Rebuilding the change
On **PreToolUse** for Edit, MultiEdit and Write, the hook rebuilds the file
before and after the change.

- **Write:** before is the file on disk, or empty for a new file. After is
  `content`.
- **Edit and MultiEdit:** before is the file on disk. After is that text with
  each `old_string` → `new_string` applied in order, honouring `replace_all`.
- **Bounded read:** the hook calls `metadata(path)` and reads only when
  `is_file()` and `len <= 1 MiB`, so a FIFO or device is never opened. It
  reads through `File::open(..).take(1 MiB + 1)`.
- **Fragment fallback:** content that is not UTF-8, any I/O error, a missing
  file (for Edit) or an `old_string` that is not found all fall back to the
  fragments: before is `old_string`, after is `new_string` or `content`.
  For MultiEdit, before is all the `old_string` values joined with `\n`
  and after is all the `new_string` values joined the same way. The
  fallback has no section context. The reason (`missing`, `too-large`,
  `not-file`, `not-utf8`, `io`, `no-match`) goes to the journal. It never
  denies and never panics.
- **Line endings:** each line is split on `\n` and a trailing `\r` is
  trimmed on both sides before matching, so a CRLF file with an LF
  `old_string` still matches after normalisation.

### What counts as added
`examine_change(path, before, after) -> Vec<Hit>` is pure. It extracts a
**multiset of normalised suppression markers** from each side, not a
multiset of whole lines. A marker is `(kind, codes)`, for example
`("type: ignore", "arg-type")`, `("noqa", "E501")`,
`("eslint-disable-next-line", "react-hooks/exhaustive-deps")` or
`("allow", "clippy::too_many_arguments")`. A config setting is
`(section, key, value)`. A hit is a marker that occurs more often in
after than in before. When a marker occurs n times in before and m > n
times in after, its occurrences in after past the first n, in line order,
are the hits; their line numbers are the ones reported. So:

- editing code on a line that keeps its suppression is silent;
- moving a suppression is silent;
- changing the codes, for example `noqa: E501` → `noqa: E501,F401`, fires
  on the new code only.

| File | Added marker that fires |
|---|---|
| `.py` `.pyi` | `# type: ignore[…]`, `# noqa[: …]`, `# pyright: ignore[…]`; headers `# pyright: basic\|off`, `# pyright: reportX=false\|none\|…`, `# ruff: noqa` |
| `.ts` `.tsx` `.js` `.jsx` `.mjs` `.cjs` `.mts` `.cts` `.vue` `.svelte` | `eslint-disable*` in a `//` or `/*` comment; `@ts-ignore`, `@ts-nocheck`, `@ts-expect-error` |
| `.rs` | `#[allow(…)]`, `#![allow(…)]`, `#[expect(…)]`, `#![expect(…)]` as the first non-whitespace token of the line, indented or not; `#[cfg_attr(…, allow(…))]` also fires |
| `.go` | `//nolint[:…]` |
| `tsconfig*.json` | `strict`, `noImplicitAny`, `strictNullChecks` or another `strict*` option set to `false` |
| `eslint.config.*`, `.eslintrc*` | a rule set to `"off"`, `"warn"`, `0` or `1` |
| `pyrightconfig.json`, `[tool.pyright]` in `pyproject.toml` | `report*` set to `none`, `false`, `warning` or `information`; `typeCheckingMode` set to `off` or `basic`; `enableTypeIgnoreComments` set to `true` |
| `ruff.toml`, `.ruff.toml`, `[tool.ruff*]` | an `ignore`, `extend-ignore` or `per-file-ignores` key, or an entry added inside one of those arrays (array membership is tracked) |
| `Cargo.toml` `[lints.*]`, `[workspace.lints.*]` | a lint set to `"allow"`, or `level = "allow"` |
| `.golangci.yml`, `.golangci.yaml` | an item under `disable:` or under an `exclude*` or `exclusions` key (found by indentation); `disable-all: true`; `default: none` |

**Known false negatives** are named in the module doc, and each one has a
test asserting silence:
- dotted TOML keys (`lints.clippy.x = "allow"` at the top level);
- inline tables;
- YAML flow sequences (`disable: [a, b]`);
- markers that only the fragment fallback sees, with no section context;
- a swap in one edit: the same marker removed in one place and added in
  another, which leaves the count unchanged.

A comment marker is required: a string literal that only contains
`# noqa` or `eslint-disable` does not fire. Markdown and other file types
are never examined.

### What the agent and the journal see
**Advise text**, built by `decision::phrase`, with the absolute path as
`read_unbounded_large` shows it. Exact form in whole-file mode:

```
amont-agent/lint-suppression-added: this edit adds 2 lint suppressions to /abs/src/x.py:
  line 42: # type: ignore[arg-type]
  line 57: # noqa: E501
Fix the finding in the code that raised it (general.no-disabled-safety). A suppression is allowed only for a cause outside the repository, named on the same line, in the tool's narrowest form. If you cannot fix it, stop and tell the person.
```

- At most 3 hits are shown, one per line, followed by `and N more`.
- Fragment mode prints `(line unknown)` in place of `line N`.
- A config hit says `loosens <file> [<section>] <key> = <value>`. Its
  remedy says that loosening a configuration needs its own commit and the
  person's agreement.
- Both forms are pinned in a unit test of `phrase`.

**Journal:** `note_file` takes an excerpt of the form
`<path>:<line> <kind>[<codes>] (+N) exact|fragment:<reason>`, within the
512-byte cap. `amont-agent explain` can then split fires by marker kind.

**Order in the write branch:**
1. the implementation-review guard (unchanged, still denies);
2. `session_state::record` of the write, before any advice is built;
3. this rule, through `stance::resolve` (`amont.agent.lint-suppression-added.stance`
   and the kill switch both apply).

**Latency budget:** the added work on an Edit or Write is p99 under 20 ms
for a 1 MiB file, and under 5 ms for a typical file of 30 KB or less, on
the silent path. A firing edit adds the `git config` spawn of
`stance::resolve`, as every advising rule already does; it is measured and
reported, not budgeted. Both are measured before and after the change.

## Phases
- [x] **Phase 1, Payload.** `FileOp` gains `change: Option<Change>`, where
  `Change` is `Write { content }` or `Edits(Vec<Edit { old, new, replace_all }>)`.
  It is parsed in `src/payload.rs:264-314` for PreToolUse writes only. Read
  and Post events are unchanged.
- [x] **Phase 2, Rule.** Add `src/rules/lint_suppression_added.rs`:
  - `RULE`: Advise, ceiling Deny, `Examine::Legacy` returning `None` (as in
    `read_unbounded_large.rs:18-38`), evidence 1.01 per 1,000
    (2026-10-06, `Flat`).
  - The pure functions `examine_change`, `markers` and `phrase`.
  - `reconstruct(path, change) -> Rebuilt { before, after, mode: Exact | Fragment(Reason) }`,
    the bounded read. It is the only function that touches the disk, and
    the hook calls it, keeping effects at the boundary.

  Unit tests:
  - every table row, positive and negative;
  - a line edited with its suppression kept;
  - a moved suppression;
  - a code added to an existing suppression;
  - string literals in `.rs` and `.py`;
  - an indented attribute and `cfg_attr`;
  - a Markdown file;
  - every fallback reason, including a FIFO (copied from
    `read_unbounded_large.rs:156-165`) and a file over 1 MiB;
  - a CRLF `pyproject.toml`;
  - section context in `pyproject.toml`, `Cargo.toml` and `.golangci.yml`;
  - each known false negative, including a swap;
  - both forms of `phrase`, with line numbers taken from the hits past the
    before count.
- [x] **Phase 3, Wiring.** The write branch of `on_file`
  (`src/hook.rs:440-463`) follows the order above, using the stance match
  pattern from `hook.rs:502-513` and `note_file` with the excerpt above. Also:
  - register the rule in `rules/mod.rs`;
  - add `tests/corpus/lint-suppression-added.cases`, with Bash negatives only,
    as for the other file-tool rules;
  - add its `EMBEDDED` entry in `src/corpus.rs`.
- [x] **Phase 4, Integration tests** in `tests/hook.rs`, using
  `ReadFixture::file_event`:
  - an Edit adding `# type: ignore` advises and does not deny;
  - an Edit that changes code on an already-suppressed line is silent;
  - a Write of a new `tsconfig.json` with `"strict": false` advises;
  - the stance override to `observe`, set through `file_event_with_global`
    (a pinned global git config), is silent and journals `watched`;
  - the implementation-review guard still denies;
  - an advised Edit followed by a Read of the same file: `file-reread`
    sees the write.
- [x] **Phase 5, Measurement and docs.**
  - **Shared fixture:** `tests/fixtures/suppressions.txt` holds tagged
    lines (`+` for positive, `-` for negative, with a file extension).
  - **Rate script:** add `tools/suppression-rate.py`. It works on fragments
    only, since transcripts never hold the file on disk, and its header
    names the config rows it undercounts. It has the same flags as
    read-rate (`--root`, `--weeks`), plus `--until YYYY-MM-DD` and
    `--self-test`, which classifies the shared fixture. Other behaviour:
    - a missing root goes to stderr and exits 1;
    - a usage error exits 2;
    - `BrokenPipeError` exits quietly;
    - rows fit in 80 columns.

    The same `BrokenPipeError` fix and exit-code header go into
    `tools/read-rate.py`.
  - **Parity:** a Rust unit test classifies the same fixture through the
    fragment path, and `make check` runs `python3 tools/suppression-rate.py
    --self-test`.
  - **Docs:** a `docs/measuring.md` section next to the Read-tier one, a
    `docs/rules.md` row, and a CHANGELOG `### Added` entry under Unreleased.
- [ ] **Phase 6, Release** (asked for by the person, `work.release-on-request`).
  After the merge, open the release PR `chore: release 2.26.0`. It is minor
  because it adds a rule (2.25.0 shipped meanwhile; see the Decision log).
  Tag with the `tag-release` skill. Check that the release run concluded
  `success` and that `publish-tap` pushed `Formula/amont-agent.rb` at 2.26.0.
- [ ] **Phase 7, Upgrade.** `brew update && brew upgrade amont amont-agent`.
  Then check that `amont-agent --version` is 2.26.0, record
  `amont --version`, and close #65 from the PR.

## Decision log
- 2026-10-06 — **The ceiling is `deny` and the default is `advise`.** The
  issue asks for a measured soak before deny, and a ceiling of `advise`
  would need a code change to graduate.
- 2026-10-06 — **Diff the reconstructed whole file, not the fragments.**
  Only the whole file gives TOML and YAML section context. Fragments are the
  fallback, recorded in the journal.
- 2026-10-06 — **Compare markers, not lines** (review: backend, rust).
  Comparing whole lines fired on every edit to a line that was already
  suppressed.
- 2026-10-06 — **`@ts-expect-error`, `#[expect]` and `cfg_attr(allow)`
  fire too.** Under the rule, every suppression needs an outside cause. At
  advise, the text reminds the agent without blocking. The soak can split
  them by marker kind.
- 2026-10-06 — **Implementation path:** Phases 1–5 go through `relais`
  (acceptance `make check`) if `relais doctor` is green. Otherwise they are
  done by hand in the task worktree.
- 2026-10-06 — **The script self-test runs from a Rust test, not from
  `make check`.** `tests/suppression_rate.rs` runs `tools/suppression-rate.py
  --self-test`, because `ci.mirrors-the-local-check` would force a CI workflow
  change if `make check` ran it.
- 2026-10-06 — **Known approximation: the reported line can be an earlier
  occurrence.** When the same marker already exists and the edit adds another,
  the hits are the occurrences past the first n in line order, so the line
  reported can be the pre-existing one. A unit test pins it.
- 2026-10-06 — **A keyword pre-check before any read, and a read cap of
  256 KiB instead of 1 MiB.** The first latency run showed +26 ms p99 for a
  silent edit at 1 MiB, over budget. A source file can gain a marker only if
  the inserted text names one, so `worth_rebuilding` skips the read when it
  does not. The Rust keywords are attribute forms only, because `.expect(`
  is in most Rust edits. The full rebuild costs about 22 ms per MiB, so the
  cap now keeps the worst silent rebuild near 6 ms. A larger file falls back
  to the fragments and loses only the line number. The pre-check is listed
  as a known false negative.
- 2026-10-06 — **The release is 2.26.0, not 2.25.0.** Another session
  released 2.25.0 (#69) while this branch was open. It carried
  read-unbounded-large and the #62 fix, so this release carries only this
  rule.
- 2026-10-06 — **Test isolation fix in `tests/hook.rs`.** `send_inner` now
  points `GIT_CONFIG_GLOBAL` at a file under the test home that does not
  exist, and sets `GIT_CONFIG_NOSYSTEM=1`. Since
  today, the developer's global `implementation-review.stance deny` leaked
  into `a_push_from_an_unrehearsed_tree_…`, which failed on `main` too. The
  relais repair attempt found the fix.
- 2026-10-06 — **Relais outcome.** The first run was interrupted at dispatch
  by an API outage (ENOTFOUND), with no edits, and was abandoned. In the
  second run, attempt 1 produced the full candidate and the repair timed out
  at the 20-minute wall. The candidate was salvaged by hand: the CHANGELOG
  entry was moved to a fresh `## Unreleased` and the latency fix above was
  added. The relais decision stays open until the merge, then
  `--answer salvaged`.
- 2026-10-06 — **A Write over an existing file the hook cannot read is
  silent** (implementation review). Comparing the content against an empty
  "before" reported every suppression the file already held. The content is
  now compared with itself, which is listed as a known false negative and
  pinned by a test. Also from that review: `Marker` carries an `Origin` enum
  instead of a `config` flag, the `File` match is exhaustive, the rate script
  counts skipped files and lines on stderr, and the evidence is 1.01 per
  1,000 over 99,810 calls.
- 2026-10-06 — **The fire path costs about 100–200 ms, and that predates
  this rule.** `read-unbounded-large` firing on `main` measures 215 ms p50.
  The cost is in the shared advise path, not in this rule, and a follow-up
  issue is filed.

## Verification
- **Phases 1–4:** `make check` (fmt, clippy `-D warnings`, `cargo test
  --locked`, and the script self-test) and `make msrv` both pass, with no
  change to `Cargo.lock`. A green suite alone is not the check.
- **Piloted run:** `CLAUDE_CONFIG_DIR=$scratch target/debug/amont-agent hook < event.json`
  with real PreToolUse JSON. stdout must hold only the JSON answer, and
  stderr must be empty.
  - An Edit of a scratch `x.py` adding `foo()  # type: ignore` →
    `additionalContext` matches the Behaviour sample byte for byte, and
    `$scratch/amont-agent/journal.log` holds `type: ignore … exact`.
  - An Edit changing `f(a)` → `f(b)` on a line carrying `# type: ignore` →
    no output.
  - A Write of `tsconfig.json` with `"strict": false` → advise.
  - An Edit of `docs/x.md` containing `# noqa` → silent.
  - An Edit whose `file_path` is a FIFO (`mkfifo`) → returns within 50 ms;
    the journal shows `fragment:not-file`.
  - With `amont.agent.lint-suppression-added.stance=observe` written to
    `$scratch/gitconfig` and the hook run under
    `GIT_CONFIG_GLOBAL=$scratch/gitconfig` → silent; the journal shows
    `watched`.
- **Latency:** `hyperfine -N --runs 100` on the hook, on `main` and on the
  branch, for these Edits:
  - silent: a 1 MiB `.py` file, a 30 KB file and a missing file;
  - firing: a 30 KB file.

  The added p99 on the silent path must be within the budget. The firing
  path is reported.
- **Rate script:**
  - `python3 tools/suppression-rate.py --until 2026-10-06` → 101 edits,
    about 1.0 per 1,000;
  - `… | head -1; echo $?` → no traceback, exit 0;
  - `--root /nonexistent` → exit 1.
- **Phase 6:** `gh run view <release run> --json conclusion` → `success`;
  the tap formula shows `2.26.0` with a matching sha256.
- **Phase 7:** `amont-agent --version` → 2.26.0, and a piloted Edit through
  the installed binary advises.

### Observed (2026-10-06, before push)
- `make check` → 735 passed, 0 failed; after the implementation-review fixes,
  736 passed, 0 failed. `make msrv` → builds on 1.85.0.
  `Cargo.toml` and `Cargo.lock` are unchanged.
- Piloted run, release and debug builds, with `CLAUDE_CONFIG_DIR` and
  `GIT_CONFIG_GLOBAL` pointing at a scratch directory. Every case printed
  only JSON on stdout and 0 bytes on stderr:
  - adding `# type: ignore[attr-defined]` → advise text in the pinned form,
    `line 2: # type: ignore[attr-defined]`; journal `x.py:2 type: ignore[attr-defined] (+1) exact`;
  - `f(1)` → `f(2)` on a suppressed line → silent;
  - a new `tsconfig.json` with `"strict": false` → advise
    `line 3: loosens tsconfig.json strict = false`;
  - `docs/x.md` with `# noqa` → silent;
  - a FIFO → never opened, journal `fragment:not-file`. A silent FIFO edit
    takes 8.8 ms median and 9.4 ms max; a firing one takes about 105 ms,
    the shared fire path;
  - stance `observe` → silent, journal `watched`.
- Latency, measured with a Python harness of 200 runs (hyperfine is not
  installed), release builds, `origin/main` against the branch, as the added
  p50 / p99:
  - silent 30 KB: −0.4 / within noise;
  - silent missing file: +0.0 / noise;
  - silent 1 MiB (no keyword): +0.2 / +2.3 ms;
  - silent full rebuild at the 256 KiB cap: +4.5 / −0.3 ms;
  - silent 1 MiB with a keyword (fragment path): +0.2 / −1.6 ms.

  The silent path is within budget. Firing at 30 KB adds about 80–150 ms,
  the same order as `read-unbounded-large` firing on `main`, so it is
  reported, not budgeted. The p99 swings by ±15 ms run to run even where no
  new code runs, which is process-spawn noise on a loaded machine.
- Rate script:
  - `--until 2026-10-06` → 101 edits, 1.01 per 1,000 (99,810 calls);
    stderr empty (0 bytes), so no file or line was skipped;
  - `| head -1` → exit 0, no traceback;
  - `--root /nonexistent` → exit 1;
  - a bad flag → exit 2;
  - `--self-test` → 66 samples agree;
  - no row wider than 80 columns.

## Outcome

<!-- panel: repos=amont-agent adds= reviewers=backend,language,tui,unix body-sha=1fe8b954d11c -->
