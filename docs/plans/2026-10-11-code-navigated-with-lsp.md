---
status: done
branch: feat/code-navigated-with-lsp
repos: [amont-agent]
adrs: [ADR-0022]
---

# Code is navigated with LSP: two amont-agent rules

## Review panel

👉 **Decide:** none — approve if advising (not denying) whole-file code Reads and symbol greps is the right first step.
📍 amont-agent · plan reviewed · next: worktree, payload fixtures, then the two rules. Panel: backend, lang:rust, tui, unix.
**Changed by review:** rg/git grep flag tables, stop-list and silencers; read-unbounded-large still speaks when the new rule is observe; doctor names missing hooks and LSP gates.
📄 Full reviews: [2026-10-11-code-navigated-with-lsp.reviews.md](2026-10-11-code-navigated-with-lsp.reviews.md)
**Verdicts:** 4 approve-with-changes (round 1), backend approve-with-changes (round 2); 4 findings carried into implementation.

## Context

fleet-lsp 0.5.2 (2026-10-11) gives every session and every subagent a note:
navigate code with the LSP tool, Read only the range it points to. A note is
advice the model can ignore, and the transcripts say it does. Measured over
2026-09-01..10-11 (`/tmp/lspmeasure/rate*.py`, per 1,000 tool calls):

| ISO week | calls | whole-file Read of code | symbol grep in Bash | Grep tool, symbol | LSP calls |
|---|---|---|---|---|---|
| W37 | 7,061 | 6.9 | 38.8 | 0.0 | 0 |
| W38 | 18,691 | 15.5 | 48.9 | 0.0 | 0 |
| W39 | 29,781 | 17.7 | 47.6 | 0.3 | 1 |
| W40 | 21,392 | 11.2 | 43.5 | 3.8 | 0 |
| W41 | 45,158 | 21.2 | 43.8 | 4.5 | 51 |

The person asked for amont-agent to enforce LSP for code. Decided with them
(2026-10-11): **advise now, raise to deny later by `git config`**, covering
whole-file Reads of code, symbol greps in Bash, and the Grep tool on code.

Why not deny now: Claude Code refuses an Edit of a file not Read first, so
a Read deny blocks editing; 23 of 48 LSP calls in 10-01..10 errored (most
were pre-0.5.1 workspace errors); and some agents have no LSP tool at all.

## Approach

Two rules, so each can be raised to `deny` on its own:

- **`code-read-not-lsp`** (Read tool) — `advise`, ceiling `deny`.
- **`symbol-grep-not-lsp`** (Bash `grep`/`rg`/`git grep`, and the Grep tool) —
  `advise`, ceiling `deny`.

Rollback: `git config --global amont.agent.<rule>.stance observe` turns
either rule off. The Grep hook block is harmless when left installed.

### Shared: does LSP serve this file, for this agent? — new `src/lsp.rs`

Impure, so it runs only where a rule has already fired (`confirm`, or the
file path of the hook). Each hook call is a new process, so there is no
cache. Order the checks cheap-first:

1. **Language** (pure): the extension is one fleet-lsp serves — `rs go py pyi
   ts tsx js jsx mts cts mjs cjs` (the same list as fleet-lsp's
   `plugin.json`). Then the size and pattern tests of the rule (pure).
2. **Server**: `fleet-lsp` on `PATH`.
3. **Repository**: the file (or the grep's directory) is inside a git
   working tree (walk up for `.git`, file or directory), since fleet-lsp
   answers nothing outside one. For a grep, the directory comes from the
   directory operand, or `git -C <dir>`, before the cwd.
4. **Plugin enabled**: `enabledPlugins` has a `fleet-lsp@…` key set to
   `true`. Settings are merged in this order, the most specific wins: user
   `~/.claude/settings.json`, then project `<repo>/.claude/settings.json`,
   then `<repo>/.claude/settings.local.json`.
5. **The agent has the tool**: read `agent_type` from the hook input. Before
   relying on it, capture 2–3 real PreToolUse payloads from a subagent
   session as fixtures (like the Grep payloads) and verify the field name
   and values. Then:
   - Main conversation and built-in agents (Explore, Plan, general-purpose,
     fork) have LSP.
   - A custom agent is looked up in `<cwd>/.claude/agents/<type>.md`, then
     `~/.claude/agents/<type>.md`; a plugin agent (`plugin:name`) under the
     `installPath` that `~/.claude/plugins/installed_plugins.json` gives for
     that plugin.
   - A definition whose `tools:` is present and lacks `LSP` → silent.
     Parse `tools:` both as a comma line and as a YAML list (multi-line).
   - A custom or plugin agent whose definition is not found → silent.
   - `agent_type` absent but the payload's transcript path is a subagent's
     (`/subagents/`) → silent.

Failure and cost: any I/O or parse error → silent (exit 0, nothing on
stdout or stderr). Budget: under 5 ms per fire.

### `code-read-not-lsp` (hook file path, `src/hook.rs` `on_file`)

Fires on a PreToolUse Read with window `full`, of a regular file over
**4 KB** (the `whole-file-dump` threshold; smaller files cost less than the
advice), that `lsp::serves` accepts, and not exempt per
`read_unbounded_large::exempt`. It fires only when `!persisted` and
`!reread.seen`: a Read of a persisted tool-result path stays silent, and a
file already seen stays silent.

- Ordering: after `file-reread`. `read-unbounded-large` is silenced for that
  Read only when the stance of `code-read-not-lsp` resolves to `advise` or
  `deny` (same pattern as the `!reread.seen` guard at `hook.rs:522-524`), so
  one Read gets one advisory. With `observe` set, `read-unbounded-large`
  still speaks.
- Text — reason: a whole-file Read of code lands every line in the context,
  and every later turn carries it; LSP answers the question in a few lines.
  Remedy: `documentSymbol` for the file's outline, `goToDefinition` /
  `findReferences` / `hover` for one symbol, then Read with `offset`/`limit`
  around the line LSP gave. The "to edit" sentence is worded after the edit
  gate check in Verification: if a windowed Read satisfies the Edit gate,
  say "To edit, Read the window you will change"; if not, say "To edit, a
  whole Read is fine". If LSP errors with a fix, apply it or say so, then
  Read.
- `examine` returns `None` (file-tier rule, like `read_unbounded_large`); a
  `nomatch`-only corpus file, as the others have.

### `symbol-grep-not-lsp`

**Bash** (pure `examine` on `Parsed`, so the backtester can replay it):
a simple command whose program is `grep`/`egrep`/`rg`, or `git grep`
(`Simple::subcommand`), that is **not fed from a pipe** (`prev` is not `|`),
whose pattern is **identifier-shaped**: optional `\b`, optional one
declaration keyword (`fn def func class struct impl interface type const
let`), one identifier of 3+ chars, optional `\b`/`\(`. And it targets code:
a code-file operand, a `--include`/`-g`/`--type` naming a code language, or
recursive (`-r`/`-R`, `rg`, `git grep`) with no non-code file operand.
`confirm`: `lsp::serves` for the first code operand, else the repository
(see Shared, step 3) with the language of the include.

Pattern operand tables (helpers from `path_operand_missing::PROGRAM_FIRST`
for the first operand):

- `rg`: value flags `-e -f -g -t -T -A -B -C -m -j -M --type-add --glob
  --type`, and their long forms; the pattern is the first operand or the
  value of `-e`.
- `git grep`: the args after `subcommand()`; a `--` starts the pathspec.
- `grep` long forms with a space: `--include '*.rs'`, `--exclude-dir
  node_modules`, `--max-count N`.

Each table entry has a corpus case.

Silent cases:

- Stop-list: patterns `TODO FIXME XXX HACK NOTE` (comment markers are
  text). `grep -rn "TODO"` stays a `nomatch` case.
- Silencers: `-f FILE`, more than one `-e`, `-v`, `-c`.

**Grep tool** (new): a separate `("PreToolUse", "Grep")` block in
`settings.rs` `TARGETS` (a new block, so the existing
`Read|Edit|Write|MultiEdit` matcher is untouched and nothing duplicates); a
`payload.rs` branch for `tool_name: "Grep"` (`pattern`, `path`, `glob`,
`type`, real shapes captured from transcripts as fixtures) into a new
`Event::PreGrep`; same identifier test on `pattern`; code target when
`path` is a code file, or `glob`/`type` names a code language, or `path` is
a directory with no glob/type.

The new variant reaches every exhaustive `match` on `Event`. Before coding,
list them with LSP `findReferences` on `Event`, and update each. A
PostToolUse Grep is ignored: there is no PostToolUse block for Grep.

### Both rules: text and repeat

Text — reason: a text search for a symbol finds strings, comments and
same-named symbols alike, and returns lines without what they mean.
Remedy: `workspaceSymbol` to find the definition, `findReferences` /
`incomingCalls` for its uses; grep stays right for text LSP cannot see
(strings, comments, config, logs).

Reason plus remedy is at most 60 words for each rule, asserted by a unit
test.

Each advisory fires once per (session, pattern) for `symbol-grep-not-lsp`,
and once per (session, file) for `code-read-not-lsp`. The record is a
separate per-session file, not the read log of `session_state`: its
`last_read` treats any non-read kind as `None`.

### Remedies that would now contradict the advice

- `whole_file_dump.rs:58-63` remedy ("`grep -n '<anchor>' <path> | head`
  finds the line first") and `read_unbounded_large::phrase` ("Find the part
  you need first with the Grep tool …"): when the path is a code file that
  `lsp::serves`, name LSP first (`documentSymbol`/`goToDefinition`), grep
  for text only. Otherwise unchanged. The `phrase` edit shows exactly in the
  `observe` case, where `read-unbounded-large` still speaks.

### Docs and bookkeeping

- `RULES` in `src/rules/mod.rs`, `EMBEDDED` in `src/corpus.rs`, new
  `tests/corpus/symbol-grep-not-lsp.cases` (match/nomatch from real
  W41 commands, incl. the `| grep foo` filter, `grep -c X log.txt`,
  `grep -rn "TODO"`), `tests/corpus/code-read-not-lsp.cases` (nomatch only).
- Evidence, per branch the backtester can replay: `symbol-grep-not-lsp`
  43.8/1k (Bash), measured 2026-10-11, `Flat(5)`; the Grep-tool branch
  (4.5/1k) is verified by fixtures only. `code-read-not-lsp` 21.2/1k,
  `Rising(5)` (or the closest `Trend`). Report the Bash and Grep-tool rates
  separately.
- `docs/rules.md` two rows; each lists when the rule is silent (no
  fleet-lsp or plugin off, outside a git tree, agent without LSP,
  comment-marker or filter greps). `CHANGELOG.md` `## Unreleased`;
  `tests/settings_merge.rs` asserts the new block.
- `doctor.rs:194`: `amont-agent doctor` compares the installed (event,
  matcher) pairs with `TARGETS` and names a missing one (`missing:
  PreToolUse Grep`), not only a count of events. It also prints an "LSP
  rules" line: the fleet-lsp path, whether the plugin is enabled, or the
  gate that fails.

## Non-goals

- Deny. Raising either rule is a later, measured step; its prerequisite —
  recording LSP calls (PostToolUse `LSP`) so a deny can say "until LSP was
  asked about this file" — is not built here.
- Fixing LSP error rates (fleet-lsp work).
- Advising `cat`/`sed -n` of code: `whole-file-dump` owns those.
- Agents outside this machine's fleet-lsp install.

## Phases

1. [x] Rules, `src/lsp.rs`, Grep payload + matcher, remedies, tests, docs — one PR.
2. [x] Release (`tag-release`), `brew upgrade amont-agent`, `amont-agent install`
   to add the Grep block; observe in a fresh session.

## Decision log

- 2026-10-11: the subagent fallback is "`agent_id` present, `agent_type` absent → silent". Captured payloads show `transcript_path` is the parent's in a subagent, so the planned `/subagents/` test could not work.
- 2026-10-11: a bare word must look like a symbol (an uppercase letter or `_`) unless a declaration keyword precedes it; `-i` and `$` paths are silent. The first 50-hit sample was 68% real symbol lookups, under the 85% bar; the second, 88%.
- 2026-10-11: `Evidence` for `symbol-grep-not-lsp` is 54.4 per 1,000 Bash calls, the backtester's unit; the plan's 43.8 was per 1,000 tool calls of any kind.
- 2026-10-11: shipped as v2.32.0 in this PR (Cargo bump and CHANGELOG section), as 2.31.0 did, not as `## Unreleased`.
- 2026-10-11: `whole-file-dump`'s LSP remedy does not check the agent: its `confirm` has no payload, and the agents measured without LSP (review panel) have no Bash. deliberate.

## Verification

| check | input | expected | actual |
|---|---|---|---|
| edit gate | Read lines 1–40 of a `.rs` file, then Edit a line outside that window | record whether Claude Code accepts the Edit; it fixes the "to edit" wording | `claude -p` (haiku): Read `offset 1, limit 10` of a 120-line `lib.rs`, then Edit line 101 → **accepted**, line 101 changed. Remedy says "to edit, Read the window you change". |
| unit | `lsp` module: extension table, `tools:` as comma line and as YAML list, plugin installPath lookup, no-.git dir, malformed `installed_plugins.json`, truncated settings | serves / silent as listed | 8 tests in `src/lsp.rs` pass, incl. `a_plugin_is_found_where_it_was_installed` (malformed JSON) and `the_most_specific_settings_file_decides` (truncated, empty). No-.git covered by the e2e row below. |
| unit | settings merge: user, project, local `enabledPlugins` | the most specific wins | `the_most_specific_settings_file_decides`: project `false` over user `true`, local `true` over user `false`, none → off. Pass. |
| unit | advisory text of each rule | reason + remedy at most 60 words | `the_advice_stays_under_sixty_words` in both rules. Pass. |
| fixture | 2–3 real subagent PreToolUse payloads | `agent_type` field name and values confirmed | 3 payloads captured live: main has neither field; Explore has `agent_id` + `agent_type: "Explore"`; a `--agents` custom agent has `agent_type: "narrow"`. `transcript_path` is the parent's, so the `/subagents/` fallback became `agent_id` without `agent_type` → silent. |
| corpus | `cargo test` corpus cases for both rules, incl. `rg`, `git grep` and long-form-flag cases, stop-list, silencers | every match fires, every nomatch silent | `corpus check` passes: `symbol-grep-not-lsp` 12 match / 14 nomatch, `code-read-not-lsp` 5 nomatch. |
| backtest | `amont-agent backtest symbol-grep-not-lsp` on W41 transcripts | rate near 43.8/1k; 50 hits hand-labelled, ≥ 85% real symbol lookups; false-positive rate reported | Backtester unit is per 1,000 **Bash** calls: weeks from 09-07 25.4, 46.8, 55.9, 59.2, 54.4. First sample: 34/50 real (68%) → shape test tightened. Second sample (fresh seed): **44/50 (88%)**; the 6 misses are env-var or column names searched as text. |
| timing | p95 of the Read hot path with `code-read-not-lsp` | under 5 ms per fire | Release build, 30–40 runs each: all LSP gates together < 1 ms (8.0 ms baseline, 8.4–8.6 ms when a late gate fails). A firing Read is ~23–25 ms; the extra ~14 ms is stance (a `git config` process) and journal, shared with every firing rule. |
| hook e2e | `tests/hook.rs`: whole Read of a 5 KB `.rs` in a git repo, fleet-lsp stub on PATH, plugin enabled | `additionalContext` with `code-read-not-lsp`, and not `read-unbounded-large` | Run with a 20 KB file, so the suppression of `read-unbounded-large` shows: advised, no `read-unbounded-large`, not a deny; silent the 2nd time in the session, advised in another session. Pass. |
| hook e2e | same Read of a persisted tool-result path | silent | `tool-results/x.rs` → no `code-read-not-lsp`. Pass. |
| hook e2e | same Read with `code-read-not-lsp` set to `observe`, 20 KB `.rs` | `read-unbounded-large` fires | Fires, remedy names `documentSymbol`, not `grep -n`. Pass. |
| hook e2e | same Read with `agent_type` naming a definition whose `tools:` lacks LSP | silent | `narrow` (`tools: Read, Grep`) silent; `agent_id` without type silent; Explore advised. Pass. |
| hook e2e | Grep tool payload `{"pattern":"fn agent_type","path":"<repo>"}` | advise `symbol-grep-not-lsp` | Advised once per pattern per session; `output_mode: count`, an alternation pattern and a `*.md` glob silent. Pass. |
| hook e2e | 3 KB `.rs`, a `.md`, a windowed Read, no fleet-lsp on PATH | silent each | Each silent; also plugin off and no `.git`. Pass. |
| install | `amont-agent install` on a settings copy with the old blocks | one new `Grep` block, old blocks unchanged, idempotent on rerun | `tests/settings_merge.rs`: PreToolUse blocks Bash, Read\|Edit\|Write\|MultiEdit, Grep, AskUserQuestion, ExitPlanMode; existing tests for idempotence and uninstall pass. |
| doctor | settings copy without the Grep block | doctor prints `missing: PreToolUse Grep` | `a_settings_file_without_the_grep_block_is_reported_by_name`: printed; after `install --write`, no `missing:`. Pass. |
| doctor | fleet-lsp stub on PATH, plugin on, then off | "LSP rules" line shows the path and the state, or the failing gate | `doctor_says_whether_the_lsp_rules_can_speak_here`: on → `LSP rules: fleet-lsp at …, plugin enabled`; off → `silent here … the fleet-lsp plugin is not enabled`. Pass. |
| live | after release+install, N ≥ 5 fresh `claude -p` runs asking to find `fn agent_type` in relais | the advisory appears; report how often the next call is LSP | v2.32.0 via brew, `install --write` (Grep block added), doctor: `LSP rules: fleet-lsp at /usr/local/bin/fleet-lsp, plugin enabled`. 5 runs (sonnet), "Find fn agent_type…": `symbol-grep-not-lsp` advised in **4/5**; after it, LSP was the next call in 2 and the call after next in 2 (one `find` between), then `findReferences`/`incomingCalls`. Run 5's `grep -E 'fn agen…\|…'` is an alternation: silent by design. The Grep tool is not available to a main session here (the call errors before any hook), so only subagents that list it reach that branch. A first batch with a prompt naming no symbol: windowed Reads and regex searches, silent by design; LSP first in 1/5. |

## Implementation review

- **approve-with-changes** (round 1, 89k tokens, 122 s; Delta, 101k, 20 s): round-1 findings 1, 3–6 fixed in e51495b; 2 and 7 recorded in this plan.
- Fixed: pure settings/plugin helpers with tests, `Told` instead of swallowed errors, `Finder` enum, quoted long flags, no-PATH and doctor tests.
- deliberate: `unrecorded` journal rows carry stance `-`, as the hook's `partial` rows already do (`hook.rs` `on_bash`), and `status` reads them.
- Delta 2 **approve** (106k, 19 s): Windows CI failed one doctor test (a `:`-joined PATH); fixed in 5f71192 (`EXE_SUFFIX`, `join_paths`).
- Windows CI then failed the Grep-tool test (path judged by spelling); fixed by asking the filesystem, `GrepScope { File, Dir, Missing }`. Delta 3 approve-with-changes → Delta 4 **approve** (114k, 10 s) on tree `3e3a678a`.
- Phase 2 done: v2.32.0 released (all release jobs green, 6 assets, tap formula, crates.io), installed, live row recorded.

### Outcome, one week after release

Observed after the push; these are not Verification rows. Rates per 1,000
tool calls, from the same scripts as Context:

- Whole-file code Reads: ≤ 11 (from 21.2).
- Bash symbol greps: ≤ 25 (from 43.8).
- LSP calls per 1,000 tool calls, and the LSP error rate: reported, with
  calls from v0.5.1 on as the baseline.

If the targets are not met, raising a rule to `deny` is proposed to the
person with these numbers.

<!-- panel: repos=amont-agent adds= reviewers=backend,lang:rust,tui,unix body-sha=646e2fb47d1b -->
