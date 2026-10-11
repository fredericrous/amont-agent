---
status: done
branch: feat/lsp-record
repos: [amont-agent]
adrs: [ADR-0022]
---

# A session remembers what it asked LSP

## Review panel

👉 **Decide:** none — approve if recording LSP calls, and keeping a week of journal for the 10-18 count, are worth one PR now.
📍 amont-agent · plan reviewed, delta for journal generations · next: generations, then LSP record + rule gate, v2.33.0. Panel: backend.
**Changed by review:** one path normal form; records keyed by agent; journal rotates by renames, not read-rewrite.
**Verdicts:** backend approve-with-changes in all four rounds; 10 findings carried into implementation, incl. real `agent_id` keys and a `create_new` header.
📄 Full reviews: [2026-10-11-session-remembers-lsp.reviews.md](2026-10-11-session-remembers-lsp.reviews.md)

## Context

amont-agent 2.32.0 (2026-10-11) advises a whole-file Read of code toward
LSP (`code-read-not-lsp`). It cannot see LSP calls: nothing is hooked on the
LSP tool. Two consequences:

- A Read that follows an LSP answer about the same file is advised anyway,
  although LSP was used first — the advice is then noise.
- The rule cannot be raised to `deny`. Claude Code refuses an Edit of a
  file not Read first, so a Read deny must have a way through, and the
  natural one is "ask LSP about this file, then the Read passes". That
  needs the session's LSP calls on record. The 2.32.0 plan named this as
  the prerequisite of any deny (its Non-goals) and the person asked to
  build it now, so it is not forgotten.

It also gives the outcome check due 2026-10-18 a direct count of LSP calls
and LSP errors, where today they are scraped from transcripts.

Measured from transcripts (2026-10-08..11): an LSP error is returned as a
normal tool result (`Error performing <op>: …`), not as a failed call, so
`PostToolUse` sees it. A successful answer names files as headings,
relative to the session cwd (`src/doctor.rs:` then `  Line 454:61`;
`relais/crates/relais/src/coordinator/mod.rs:` in a workspace session).
Input: `operation`, `filePath`, `line`, `character`, and `query` for
`workspaceSymbol`.

## Approach

### The journal keeps a week — generations, not halving

Measured 2026-10-11: `journal.log` holds 1,222 records spanning **about
17 hours**. At 256 KiB, `compact` keeps the newer half, so a weekly
count (the 10-18 check, `rules`' `seen` column) reads less than a day,
and one `lsp` line per LSP call shortens it further. `compact` also reads
and rewrites the file while other sessions append to it, so a record
written in that window is lost.

- `journal::record`: **before** the write, if `journal.log` is over
  256 KiB, rotate: drop `journal.log.10`, rename `.9`→`.10` … `.1`→`.2`,
  `journal.log`→`journal.log.1`. The record then goes to a fresh
  `journal.log` with its `FORMAT` header. Total budget about 2.75 MiB,
  about 8 days at the measured rate. Rotating before the write keeps
  `journal.log` present, so doctor's liveness mtime stays correct.
- Renames only, no read-rewrite. A writer that opened the old file before
  the rename appends to `journal.log.1`, so it loses nothing. Two
  sessions rotating at once shift one extra generation, which drops at
  most the oldest file early. That is a lost count, never a wrong one,
  and the module rule allows it.
- `journal::paths()` lists the generations oldest first, and `tally` reads
  all of them, with the `since` floor as today. `compact` is deleted.
- The existing `journal.log` carries on as generation 0; no migration.
- `doctor` prints the generation count and the oldest record's age, so a
  window shorter than 7 days is visible.

### Step 0: the payload, before any code

Capture two real `PostToolUse` `LSP` payloads with a dump hook (a success
and an `Error performing` result) as test fixtures, and read the shape of
`tool_response` (string, content array or object) from them. A response
that is not text, or does not parse, records `filePath` alone and is
journalled `lsp`: never dropped.

### Record each LSP call — `PostToolUse` `LSP`

- `settings.rs` `TARGETS`: a new `("PostToolUse", Some("LSP"))` block (its
  own block, as `Grep` got in 2.32.0; `doctor` names it if missing).
- `payload.rs`: `Event::PostLsp(Box<LspCall>)` with `operation`,
  `file_path` (absolute against cwd), `response` (the tool result text,
  the first 64 KiB parsed), `cwd`, `session`, `permission_mode`, and
  `agent` (as on the other events). The variant reaches the one
  exhaustive `match` on `Event` (`hook.rs` `decide`).
- `lsp.rs`: `asked_files(call) -> Vec<PathBuf>` — pure:
  - `filePath`, except for `workspaceSymbol`, whose `filePath` only picks
    the repository;
  - every heading line of the response that names a code file
    (`<path>:` alone on a line, or `<path>:<line>[:<col>]`), resolved
    against the cwd, kept only when it is under the cwd's repository.
- Paths in one normal form, stored and compared: lexical normalisation
  (`./`, `..`), then `canonicalize` when the file exists (macOS `/tmp` is
  `/private/tmp`; a worktree path stays its own). The Read's path goes
  through the same function before the lookup.
- `lsp.rs`: `record_asked(session, agent, files)` appends
  `asked <agent> <path>` lines to the same `<journal>/sessions/<session>.lsp`
  record that `told` uses (distinct prefix); `asked(session, agent, path)
  -> bool`. `<agent>` is `main` or the payload's `agent_id`: subagents share
  the parent's `session_id`, and one agent's LSP call must not let another
  agent's Read through. A path containing a newline, or a record that cannot
  be written, is `Unrecorded` and journalled `unrecorded`, as `told` does.
- Cost: about one record line per file an answer names (`findReferences`
  measured at 7 files, `workspaceSymbol` at 2–6). Budget: the PostToolUse
  LSP hook under 15 ms p95 on a 64 KiB answer, measured with the release
  build.
- An answer that starts `Error performing` counts as asked too: the agent
  tried, and a deny must never block a Read because the server is broken.
- Journal one line per call: rule `-`, outcome `lsp` or `lsp-error`,
  excerpt `<operation> <file>`. This is the count the 10-18 check reads.
- Silent: `PostToolUse` never speaks here.

### `code-read-not-lsp` reads the record

- Before advising, `asked(session, path)` → silent, journalled with
  outcome `lsp-first` (the case the advice was meant to produce).
- At `deny` (not the default), the refusal text says the way through:
  "Ask LSP about this file (`documentSymbol`), then Read it once LSP has
  answered — the Read passes once LSP was asked, or if LSP errors." The
  default stays `advise`; raising it is the person's call after the 10-18
  numbers.
- An LSP call and a Read issued in the same turn race: the Read's
  `PreToolUse` can run before the LSP `PostToolUse` has recorded. Under
  `advise` that costs one needless advisory; under `deny` the agent Reads
  again after the answer, as the refusal says. This is one more reason the
  default stays `advise`.

## Non-goals

- Raising any stance. `deny` stays a `git config` the person sets.
- A gate for `symbol-grep-not-lsp`: a grep pattern does not map to one LSP
  call; it stays advise-only.
- Parsing every LSP answer format exactly: a heading the parser misses
  only means a later Read is advised, the safe direction.
- Quieting `path-operand-missing`'s unconfirmed records (543 of the
  1,222 lines). Graduation reads those reasons; a separate change.
- A sweep for `attestations/` (71 MB, unbounded). It needs its own
  retention, decided from what reads it; a separate plan.

## Phases

1. ✅ Journal generations, then hook, payload, record, rule gate, deny text,
   tests, docs, v2.33.0 — one PR.
2. ✅ Release (`tag-release`), `brew upgrade amont-agent`,
   `amont-agent install --write`; confirm a live LSP call is journalled and
   a following whole Read is silent.

## Verification

| check | input | expected |
|---|---|---|
| unit | `record` past 256 KiB, repeated until 12 rotations | at most `journal.log` + `.1`…`.10`, oldest dropped, each file starts with `FORMAT` |
| unit | `tally_at` across generations, with records in `.3` inside `since` and in `.9` outside | `.3` counted, `.9` not |
| unit | 8 threads writing records across the rotation threshold | every record found in some generation, none torn |
| doctor | after a rotation | liveness still reads `journal.log`; the window line shows generations and oldest age |
| fixture | capture 2 real `PostToolUse` `LSP` payloads (a success, an `Error performing` result) before coding | field names (`tool_response` shape) confirmed |
| unit | `asked_files` on real answers: `documentSymbol`, `findReferences` (headings), `workspaceSymbol` (workspace-relative headings, `filePath` ignored), `hover`, an error | the files listed, none outside the repository |
| hook e2e | PostToolUse LSP `documentSymbol` on `src/lib.rs`, then a whole Read of it (20 KB) | Read silent; journal `lsp` then `code-read-not-lsp … lsp-first` |
| hook e2e | `findReferences` answer naming `src/other.rs`, then a whole Read of `other.rs` | silent |
| hook e2e | error answer, then the Read | silent (asked, even if it failed) |
| hook e2e | `code-read-not-lsp` at `deny`: Read refused with the way through; after an LSP call, the same Read passes | refusal text names `documentSymbol`; second Read not denied |
| hook e2e | another session's LSP call | does not silence this session's Read |
| install/doctor | settings without the LSP block | `missing: PostToolUse LSP`; after install, none |
| unit | path normal form: a symlinked cwd (`/tmp` → `/private/tmp`), a `./src/x.rs` heading, a path with a newline | same key for both spellings; newline → `Unrecorded` |
| hook e2e | a subagent's LSP call (`agent_id` a1), then the main agent's Read of that file | main's Read still advised |
| timing | PostToolUse LSP hook, release build, 100 calls incl. a 64 KiB `findReferences` answer | p95 under 15 ms |
| live | after release: one `claude -p` session that calls LSP then Reads the file | journal shows `lsp`, and no advisory on the Read |
| live | after release, `code-read-not-lsp` at `deny` in a scratch repo's `git config`: a whole Read with no LSP call first | refused, text names `documentSymbol`; after an LSP call the Read passes |

## Verification record (Phase 1, before the push)

| check | actual |
|---|---|
| fixture | captured live from `claude -p` (documentSymbol, findReferences): `tool_response` is an object, answer text in `result`. A missing file is a `tool_use_error` that never reaches PostToolUse; the `Error performing` fixture uses a real transcript text in the success envelope. ✅ |
| unit, generations | 12 rotations leave journal.log + .1….10, each with FORMAT; tally across generations honours `since`; 8 processes × 120 records across the threshold: none lost, none torn, every generation starts with its header. ✅ CI caught a lost record (Linux, macOS) in the first push; reproduced locally by widening the header race to 20 ms (old code lost a record 3/3, fixed code 0/5). |
| unit, asked_files | the three fixtures, workspaceSymbol (filePath ignored, outside-repo and non-code headings dropped), hover. ✅ |
| unit, normal form | symlinked dir, `./`, `..` give one key; newline → Unrecorded. ✅ |
| hook e2e | documentSymbol then Read silent with `lsp` before `lsp-first`; findReferences-named file and error answer silent; unparsed answer `lsp-unparsed`; other session and agents a1/a2 still advised, own agent silent (main and a1); deny refuses naming documentSymbol and passes after an LSP call. ✅ (10 KB files, see Decision log) |
| install/doctor | without the block: `missing: PostToolUse LSP`; after install none. ✅ |
| doctor | `doctor_reads_the_window_across_generations`: `.3`, `.1` and the live file seeded → `journal: 3 generations, oldest record 6d ago`, liveness `last ran` from `journal.log`. On a copy of the live (unrotated) journal: `1 generation, oldest record 17h ago`. ✅ |
| timing | release build, 40 calls each: small answer p95 7.8 ms; 64 KiB repeated files 10.3 ms; 64 KiB naming 129 distinct files 13.3 ms; all 120 p95 12.5 ms (budget 15). After the CI fix, under load average 71: silent-hook baseline p95 7.6 ms, small 8.2, 64 KiB 14.5. ✅ |
| make check | fmt, clippy -D warnings, tests: green. ✅ |
| live (Phase 2) | v2.33.0 released (6 archives + SHA256SUMS), `brew upgrade` 2.32.0 → 2.33.0, `install --write`, doctor reports nothing missing. A `claude -p` session doing documentSymbol then a whole Read of `src/analysis/effects.rs` (11 KB): journal `lsp` then `code-read-not-lsp advise lsp-first`, record `asked main …/effects.rs`, no advisory in the transcript. ✅ |
| live, deny (Phase 2) | stances are read from `--global` only, so a scratch `GIT_CONFIG_GLOBAL` (including the real one, plus `code-read-not-lsp.stance = deny`) was scoped to one `claude -p` child: a whole Read of `domain.rs` was `denied` with the text naming `documentSymbol`; after the LSP call the same Read passed, journalled `deny lsp-first`. The real global stance stayed unset. ✅ |

## Decision log

- 2026-10-11: journal retention added as part of Phase 1 (delta review, backend approve-with-changes) after measuring a 17 h window.
- Rotation uses a `journal.log.0` lock file (create_new, stale after 60 s) so only one writer shifts generations: concurrent rotations cannot drop a generation early. Stricter than the plan's "one extra shift is acceptable".
- deliberate: `lsp-first` quiets `code-read-not-lsp` only. `read-unbounded-large` still speaks past 16 KB, since after LSP names a line the remedy is the window around it. The plan's 20 KB "silent" rows were written against the wrong threshold; those e2e rows use 10 KB, and `an_lsp_call_leaves_the_size_advice_speaking` pins the 20 KB case.
- Timing: the first build took 41 ms p95 on a 64 KiB answer (one `canonicalize` per heading line). Heading spellings are now deduplicated, and relative headings are joined lexically onto the cwd's normal form (one canonicalize per answer). A symlink inside the repository may then miss, which only means a later Read is advised.
- At `deny`, every whole Read is refused until LSP is asked (not once per file), and the once-per-file advice key is per agent: the refusal's way through has to stay true on the second try.
- A `workspaceSymbol` payload without `filePath` is still recorded and counted (headings only).
- A live file that cannot be rotated leaves a `rotate-failed` record in it; a header that cannot be written drops the record instead of leaving a file without one.
- The header is staged in a per-call file and hard-linked into place (atomic, fails if the name exists), and the append opens without creating, retrying through the header path when a rotation moved the file. `create_new` then `write_all` let another process append to the empty file, and the header overwrote that record at offset 0.
- `the_fixtures_are_parsed_as_real_payloads` is `#[cfg(unix)]` (its fixtures carry Unix paths); `one_normal_form_for_every_spelling` runs everywhere, its symlink steps Unix-only.
- A file system without hard links falls back to an append-mode `create_new` header (a racing record may precede it, never sit under it); the staged name is per call.
- On Windows each heading is canonicalized (the canonical cwd is a `\\?\` path where a lexical join never matches); the 15 ms budget is measured on macOS.
- Implementation via relais: generations (run-65d8b340b227c-15496, accepted, sonnet); LSP record (run-65d8b504185f3-1666f, needs_decision on Cargo.toml/Cargo.lock: only the version bump, accepted by hand, then the two fixes above).

## Implementation review

- approve (round 2, Delta), after approve-with-changes in round 1 (75k, 70 s; Delta 85k, 24 s).
- Fixed: doctor generations test, `rotate-failed` record, `holds-until` comment, rules doc per agent and deny-every-time, `child_writer` ignored.
- deliberate: a stuck rotation adds one `rotate-failed` line per write, so the journal grows twice as fast until it is fixed; being loud is the point.
- After CI failed (lost record; Windows paths), a fresh round 1 on the fix: approve-with-changes (53k, 62 s), 3 findings fixed; Delta approve (57k, 20 s); Windows heading Delta approve-with-changes (60k, 17 s), one plan word fixed.

<!-- panel: repos=amont-agent adds= reviewers=backend body-sha=f93bcc397a39 -->
