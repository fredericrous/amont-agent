---
status: active
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

1. Rules, `src/lsp.rs`, Grep payload + matcher, remedies, tests, docs — one PR.
2. Release (`tag-release`), `brew upgrade amont-agent`, `amont-agent install`
   to add the Grep block; observe in a fresh session.

## Verification

| check | input | expected |
|---|---|---|
| edit gate | Read lines 1–40 of a `.rs` file, then Edit a line outside that window | record whether Claude Code accepts the Edit; it fixes the "to edit" wording |
| unit | `lsp` module: extension table, `tools:` as comma line and as YAML list, plugin installPath lookup, no-.git dir, malformed `installed_plugins.json`, truncated settings | serves / silent as listed |
| unit | settings merge: user, project, local `enabledPlugins` | the most specific wins |
| unit | advisory text of each rule | reason + remedy at most 60 words |
| fixture | 2–3 real subagent PreToolUse payloads | `agent_type` field name and values confirmed |
| corpus | `cargo test` corpus cases for both rules, incl. `rg`, `git grep` and long-form-flag cases, stop-list, silencers | every match fires, every nomatch silent |
| backtest | `amont-agent backtest symbol-grep-not-lsp` on W41 transcripts | rate near 43.8/1k; 50 hits hand-labelled, ≥ 85% real symbol lookups; false-positive rate reported |
| timing | p95 of the Read hot path with `code-read-not-lsp` | under 5 ms per fire |
| hook e2e | `tests/hook.rs`: whole Read of a 5 KB `.rs` in a git repo, fleet-lsp stub on PATH, plugin enabled | `additionalContext` with `code-read-not-lsp`, and not `read-unbounded-large` |
| hook e2e | same Read of a persisted tool-result path | silent |
| hook e2e | same Read with `code-read-not-lsp` set to `observe`, 20 KB `.rs` | `read-unbounded-large` fires |
| hook e2e | same Read with `agent_type` naming a definition whose `tools:` lacks LSP | silent |
| hook e2e | Grep tool payload `{"pattern":"fn agent_type","path":"<repo>"}` | advise `symbol-grep-not-lsp` |
| hook e2e | 3 KB `.rs`, a `.md`, a windowed Read, no fleet-lsp on PATH | silent each |
| install | `amont-agent install` on a settings copy with the old blocks | one new `Grep` block, old blocks unchanged, idempotent on rerun |
| doctor | settings copy without the Grep block | doctor prints `missing: PreToolUse Grep` |
| doctor | fleet-lsp stub on PATH, plugin on, then off | "LSP rules" line shows the path and the state, or the failing gate |
| live | after release+install, N ≥ 5 fresh `claude -p` runs asking to find `fn agent_type` in relais | the advisory appears; report how often the next call is LSP |

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
