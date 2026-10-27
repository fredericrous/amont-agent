---
canonical: decisions:docs/plans/2026-10-24-code-rules-reach-the-reviewer.md
phases: [2, 4, 5, 7]
status: active
---
# Code rules reach the reviewer (pointer)

Canonical plan: [decisions/docs/plans/2026-10-24-code-rules-reach-the-reviewer.md](https://github.com/fredericrous/decisions/blob/main/docs/plans/2026-10-24-code-rules-reach-the-reviewer.md) (ADR-0044).

Phase 2 (behaviour 1) shipped in amont-agent 3.2.0 (#142, #143).

This branch carries Phase 4's amont-agent part, behaviour 3:
- `review-brief` embeds the candidates of adr-lint 0.2.0 (`adr-lint.candidates/1`) as an inline `<<<CANDIDATES …>>>` block;
- the implementation-review push check reads the reviewer's `Candidates:` rulings, journals undisposed ones without holding the push, and writes each ruling as an `adr-lint.ruling/1` record.

It also carries Phase 2's leftovers:
- `review-brief` in a repository with no `.adr.yaml`;
- a `keys` row without a string `key`.

## Verification (phase 4)

Input → expected → actual:
- This branch's own `review-brief`, run with adr-lint 0.3.0 → an inline `<<<CANDIDATES sha=… n=18>>>` block (16 names, one stdout print, one `_ =>` arm), no duplicate ids, no test `unwrap`s → as expected. adr-lint 0.2.0 had given 40 candidates with a duplicate id; adr-lint#9 fixed that.
- The implementation reviewer, given that block inline → a `Candidates:` block ruling on every listed id → round 1 ruled all 18 `fine`, each quoting a rule-body clause; the delta ruled all 18 again, including the one new id whose line text the fix changed.
- After the fix commit, the 18 ids → unchanged where only the line moved → 17 unchanged; `f520c59cccdf` new, because its text changed.
- The push check: `candidates-ruled`, `-undisposed`, `-unlisted`, `-unavailable`, `-unknown` journalled, and the judgment never changed → by test (`tests/implementation_review.rs`).
- The rulings file: records written once per (review, candidate), 0600 in 0700, `rulings-write-failed` on an unreadable file → by test. The records pass the real `adr-lint ruling-record --check` of adr-lint 0.3.0, with no skip printed.
- `review-brief` in a repository with no `.adr.yaml` → exit 0, no RULES line, a "no aval corpus" section → by test.
- `make check` → pass → pass. One `tests/preview.rs` run failed once under load; it passed three times alone and on the full re-run, and this branch touches no preview file.

The person chose (2026-10-25) that candidates ship journal-only, like the rules check, and switch to refusing at phase 8 after the same soak. The canonical plan records this in the decisions PR for phase 4.

## Implementation review (phase 4)

- approve-with-changes (5 findings, 2 blocking), then approve-with-changes on the Delta.
- Fixed:
  - the doc comment moved;
  - the rulings file is read once per push;
  - an explicit `unlisted` state, with unknown states reading as not recorded.
- The plan gap (refusal versus journal) was put to the person, who chose journal-only until phase 8.
- 110k + 119k tokens. Code by relais runs `run-65ea45b9b131f-1afa` → `run-65ea4a25db21c-347a` → `run-65ea4c0745c4f-c064`, the last salvaged with hand-written tests.

## Phase 5: write-time hook (behaviour 4)

This branch carries Phase 5: a PostToolUse rule on Edit, Write and MultiEdit of a `.rs`, `.ts`, `.tsx`, `.go` or `.py` file.
- **When it runs:** only when the file's repository vendors adr-lint's `check` row. The repository is resolved from the file's git root, not the session cwd, and the row is read through `amont list --json` (`harness.cross-tool-reads`), once per session per repository.
- **What it runs:** `adr-lint check --json -- <path>`, killed at 1 s.
- **What it says:** findings come back as advice (`path:line:col: rule: statement` and `aval rule <id>`). It never denies.
- **What it journals:** each finding as (rule, path, line text), so phase 8 can count the fixes. A missing binary, exit 2 and exit 3 are journalled once per session.

Measured before building: `adr-lint check -- <one .rs file>` takes p50 151 ms and p95 163 ms (20 runs), against a 300 ms budget; `amont list --json` takes about 1 s, hence the cache. adr-lint 0.5.0 ships the `amont.pack` row and `text` in `check --json`. The new `PostToolUse` `Edit|Write|MultiEdit` matcher arrives through `amont-agent install --write`, and `doctor` names it when it is missing, like its neighbours. There is no dotfiles PR: the chezmoi source of `~/.claude/settings.json` already lacks six amont-agent matchers and holds machine-local settings, so reconciling it is the person's call. One edit can be told about one suppression twice: `lint-suppression-added` speaks before the write, and this rule after it when `general.no-disabled-safety` matches. Both only advise, and each journals under its own id. That overlap is accepted until phase 8 measures it.

## Verification (phase 5)

Input → expected → actual:
- `make check` → pass → pass (the hook's 9 integration tests, plus unit and corpus cases).
- The release binary's `hook` was fed a `PostToolUse` Edit payload whose cwd is a different directory, with real adr-lint 0.5.0 and real amont. The file has a reasonless `#[ignore]`, in a scratch repository that vendors adr-lint's pack (untrusted) → advice naming `tests.first-properties` and `aval rule tests.first-properties` → `additionalContext`, with the finding, the `aval rule` line and the waiver.
- The same payload for a relais file (relais vendors no row) → no output → none.
- 10 calls with the gate cached → p95 ≤ 300 ms → 150 to 181 ms. The first call also pays `amont list --json`, about 1 s.
- The journal → each finding as (rule, path, text) under the file's repository → `adr-lint-written advise advised … tests.first-properties src/lib.rs #[ignore]` under the scratch repository's name, not the cwd's.
- The gate is matched by row id, because `amont list --json` shows an untrusted vendored row as `unusable` with `command: null`, as seen after `amont add`.

### Implementation review (phase 5)

- approve-with-changes (1 high, 3 medium, 1 low), then approve-with-changes on the Delta, with the 3 plan items owed to this commit; 72k + 78k tokens, 77 s + 18 s.
- Fixed:
  - `amont list` gets 5 s, inside the hook's 10 s, so a hung amont's closed gate is cached;
  - docs/rules.md describes the gate by id;
  - this record;
  - the dotfiles line;
  - the overlap sentence.
- The reviewer ruled all 14 adr-lint candidates `fine`.
- The relais run `run-65ead843594b0-15be8` hit its wall time, with the hook built and its tests passing. I finished it by hand: the id gate, the journal repo, same-line reasons, two more corpus negatives. It is recorded as salvaged at aa907c1.

## Phase 7: the cleanup in amont-agent

The 2026-10-27 survey found one exact finding here, a reasonless `#[ignore]` (tests.first-properties) at tests/plan_review.rs:847, and 18 Rust `#[allow]` attributes without a reason. The workspace MSRV is 1.85, above the 1.81 that `reason =` and `#[expect]` need, so:
- the `#[ignore]` gets its reason;
- each `#[allow]` becomes `#[expect(…, reason = "…")]`, or a `cfg_attr` where the lint fires on some targets only;
- `adr-lint sync` denies clippy `allow_attributes` and `allow_attributes_without_reason` in `[workspace.lints.clippy]`;
- CI runs `adr-lint sync --check`.

The `#[allow(…)]` text inside lint_suppression_added.rs's test strings is fixture data, not an attribute, and stays.

## Verification (phase 7, cleanup)

(filled at finish)
