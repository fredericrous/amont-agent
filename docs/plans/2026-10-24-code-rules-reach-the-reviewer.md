---
canonical: decisions:docs/plans/2026-10-24-code-rules-reach-the-reviewer.md
phases: [2, 4, 5]
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

Measured before building: `adr-lint check -- <one .rs file>` takes p50 151 ms and p95 163 ms (20 runs), against a 300 ms budget; `amont list --json` takes about 1 s, hence the cache. adr-lint 0.5.0 ships the `amont.pack` row and `text` in `check --json`. A dotfiles PR adds the matcher to the chezmoi-managed `settings.json`.

## Verification (phase 5)

(filled at finish)
