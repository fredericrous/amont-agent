---
status: active
branch: feat/read-unbounded-large
repos: [amont-agent]
adrs: []
---
# `read-unbounded-large`: a Read with no window over a large file

## Review panel

👉 **Decide:** none. Approve if you accept advise-first with a deny ceiling; deliberate whole reads escape via `offset: 1` + `limit`.
📍 amont-agent · planned, nothing written · next: worktree `feat/read-unbounded-large`, plan as first commit. Panel: backend, lang:rust, tui, unix.
**Changed by review:** precedence keyed on whether the file was seen (survives `file-reread` on observe); named escape for deny; "KB on disk", quoted path, case-insensitive component-based exemptions.
**Verdicts:** round 1: 4 approve-with-changes. Round 2: backend approve. Carry-over (low): run the Verification-2 probes with `CLAUDE_CONFIG_DIR=<scratch>`.
📄 Full reviews: [2026-10-05-read-unbounded-large.reviews.md](2026-10-05-read-unbounded-large.reviews.md)

Repository: `amont-agent`. Lands as `docs/plans/2026-10-05-read-unbounded-large.md`, the
first commit of branch `feat/read-unbounded-large` (worktree-task).

## Context

`whole-file-dump` (2026-09-09) told the model to stop pouring files into the
context with `cat`/`sed -n` and to use the Read tool instead. It worked: Bash file
dumps fell from 31.5% of all tool-result bytes to 16%. Over the same period the
Read tool rose to 39% (61.9M of 160.2M chars, 30 days, 93,890 tool results). The
model followed the advice by switching to Read, but it still reads whole files:

| Read result | calls | chars | no `offset`/`limit` |
|---|---|---|---|
| > 16 KB | 812 | 23.1M (37% of Read, ~14% of every tool byte) | 584 |
| > 30 KB | 244 | 10.8M | 198 |
| > 50 KB | 81 | 4.8M | 77 |

Weekly rate of unbounded Reads over 16 KB, per 1000 tool calls, W36→W41: 7.5, 2.8,
6.8, 6.2, 7.8, 11.1. Flat to rising, so not correcting on its own. By extension:
`.rs` 263, `.md` 185, `.tsx` 37, `.py` 26, `.diff`/`.patch` 29.

111 of the 584 are plan files under `~/.claude/plans/`. The plan-review panel
reads those whole, and that is the job, not waste. The same is true of a plan
under `docs/plans/`. These are exempt.

Nothing guards the Read tool's size today. `file-reread` only asks whether the
file was read before, and `persisted-output-dump` only covers `tool-results/`.
Bash output is already capped at 30K by the harness (30-day max 29,636), so
copying tokenjuice and compacting output after the fact would save 2–4% at best.
Advice before the read is where the bytes are.

## The rule

`src/rules/read_unbounded_large.rs`, the same shape as `file_reread.rs`: a
`RULE` const whose `examine` returns `None`, because the backtester sees no Read
calls and the rule lives in the hook's file path. Plus `pub fn` helpers that the
hook calls.

- `id: "read-unbounded-large"`, `default_stance: Advise`, `max_stance: Deny`.
  Advise because that is how every file-tier rule ships, and `measuring.md`
  says deny is earned from the journal, not argued. Deny is the
  graduation path once the journal shows the advice is obeyed or ignored.
  Deny can be the ceiling because there is a way through: a deliberate whole
  read passes `offset: 1` with a `limit` that covers the file. Any `offset` or
  `limit` makes the window something other than `"full"` (`payload.rs:289-290`),
  so the rule stays silent. The remedy names this escape and a test pins it
  (backend, tui).
- `evidence: per_1000: 4.7` (584 minus 111 plan reads minus 29 diff/patch reads,
  over 93,890 calls), `measured: "2026-10-05"`, `trend: Trend::Flat(6)`. The
  comment records the rising tail (W40 7.8, W41 11.1), which `Trend` has no
  variant for. While implementing, recount `per_1000` with the committed
  query (see Docs), applying exactly what `applies` exempts.
- `const LARGE: u64 = 16 * 1024;`. Below this the advice costs more than the
  read. The Read median is 3.4 KB and p90 is 14.5 KB, so 16 KB catches the tail
  and not the routine read. It is also tokenjuice's pass-through threshold for
  Claude Code.
- `pub fn applies(path: &Path, window: &str) -> Option<u64>`: `Some(bytes)` when
  all of these hold:
  - `window == "full"` (the spelling `payload.rs:289` already uses for no `offset`/`limit`);
  - the path is not exempt (see below);
  - `std::fs::metadata(path)` is a regular file with `len() > LARGE`. A failed
    stat is silence, which is the module's rule for every check. The module
    doc says `applies` only stats the file and never opens it. `metadata`
    does not block, and `is_file()` rejects a FIFO or a device, so neither can
    hang the hook (unix).
- Exempt, with a reason in a comment next to each:
  - **Media the Read tool renders, not text:** `png jpg jpeg gif webp pdf ipynb`.
    Matched on `Path::extension()` with `eq_ignore_ascii_case`, so `X.PNG` is
    exempt too. Their size in bytes does not say what lands in the context.
  - **Documents whose job is to be read whole:** a `plans` component directly
    under a `.claude` or `docs` component (matched with `Path::components()`, not
    substrings), or a `.diff`/`.patch` extension (case-insensitive).
    Plan-review and implementation-review agents are handed these to read in
    full. Advising them to window would push them to review half the plan.
  - `tool-results/` paths (`persisted_output_dump::is_persisted`).
    `persisted-output-dump` owns those.
- `pub fn phrase(shown: &str, bytes: u64) -> (String, String)`, a mechanism
  sentence in the voice of the existing rules. `{kb}` is `bytes.div_ceil(1024)`
  (MSRV 1.85 allows it), so 16,385 bytes reads "17 KB", never "16 KB … over 16 KB":
  - reason: "`{path}` is {kb} KB on disk and is being Read with no window: up
    to 2,000 lines of it land in the context, and every later turn of this
    session carries them again. Unbounded Reads over 16 KB were 37% of the Read
    tool's bytes measured." "On disk" is deliberate. Read stops at 2,000 lines by
    default, so the size on disk can overstate what loads (unix, backend).
    The 37% was measured from actual result sizes, so it stands.
  - remedy: "Find the part you need first with the Grep tool (or
    `grep -n '<anchor>' '{path}' | head`), then Read that window with
    `offset`/`limit`. To read more on purpose, pass `offset: 1` with a `limit`
    covering the lines you need: the whole file only when that is the job."
    The wording matters. A `limit` past 2,000 lifts the Read tool's default
    cap, so "cover the file" would push the model toward a costlier read
    than the one it was advised against (backend r2). The path is single-quoted with an embedded
    `'` escaped (tui), so a path with a space survives being copied.

## Wiring (`src/hook.rs`, `on_file`, ~line 466)

Three reviewers found the same bug in the first sketch. Gating on
`reread_verdict(..).is_none()` is wrong: that function also returns `None`
when `file-reread` is on observe (`hook.rs:559-561`), after journalling
`watched`. The new rule would then speak on a re-read and write a second
journal row. Precedence has to come from whether the file was *seen*, not
from whether a message came back.

- `reread_verdict` returns a small struct, `Reread { seen: bool, text:
  Option<String> }`. `seen` is `true` when `last_read` matched the window rule at
  `hook.rs:539`. Both callers (`on_bash` at 252 and `on_file` at 481) read
  `.text` exactly as before.
- `on_file` binds `let persisted = op.window == "full" &&
  is_persisted(&shown);` once at the top, and the persisted block uses it. The
  order of the existing messages does not change: persisted, then reread. The
  new rule is evaluated **last**:

```rust
if !persisted && !reread.seen {
    if let Some(bytes) = rules::read_unbounded_large::applies(&op.path, &op.window) {
        let rule = &rules::read_unbounded_large::RULE;
        let stance = crate::stance::resolve(rule);
        let (reason, remedy) = rules::read_unbounded_large::phrase(&shown, bytes);
        let text = decision::phrase(rule.id, &reason, &remedy);
        note_file(rule, stance, op, &shown);
        match stance {
            Stance::Deny => deny.push(text),
            Stance::Advise => advise.push(text),
            Stance::Observe => {}
        }
    }
}
```

Precedence is this rule's own choice, not an invariant of the file tier. The
`cat` path already sends both `file-reread` and `whole-file-dump`
(`tests/hook.rs:741-749`). This rule stays quiet on a re-read because
`file-reread`'s remedy already says to use `offset`/`limit`, and it stays quiet
whatever stance `file-reread` is on. `note_file` (`hook.rs:565`) records the
journal entry, so `amont-agent status` and `graduate` see the rule without new
plumbing. Register the rule in `rules::RULES` and `pub mod`
(`src/rules/mod.rs`) next to `file_reread`.

The `cat` path (`on_bash`) is not touched. `whole-file-dump` already covers it,
with its own 4 KB threshold.

## Docs

- `docs/rules.md`: a row after `file-reread`: `read-unbounded-large` | `advise` |
  a Read with no `offset`/`limit` of a file over 16 KB, where the whole file
  lands in the context and every later turn carries it; plan files and diffs a
  reviewer is handed are exempt.
- `whole-file-dump` is not changed. Its remedy already says "Read tool, with
  `offset`/`limit`" (`whole_file_dump.rs:59`). The model is ignoring that half
  of the advice, which is what this rule addresses.
- `docs/measuring.md`: a short section, "The Read tier from transcripts", with
  the weekly query committed as `tools/read-rate.py` (the scratchpad `wk.py`,
  cleaned up). For each ISO week it prints tool calls, unbounded Reads over
  16 KB after the same exemptions `applies` makes, per 1000, and characters. The
  soak and the recount of `per_1000` both run it (backend: a scratchpad script
  does not outlive the session).
- `CHANGELOG.md` entry. Version bump and release through `tag-release` afterwards,
  as its own step.
- `amont agents-md` in the worktree, staged into the same commit if it changes.

## Tests

Unit tests (`read_unbounded_large.rs`, `#[cfg(test)]`). Files go under
`std::env::temp_dir()` in a unique subdirectory per test, removed afterwards,
as `ReadFixture::new` does. No new dev-dependency (no `tempfile`):
- `a_large_file_read_whole_applies`: 20,480 bytes, `.rs`, window `full` → `Some(20480)`;
- `a_window_is_silent`: the same file, `0:200` and `1:5000` → `None`;
- `the_threshold_is_exclusive`: 16,384 bytes → `None`; 16,385 → `Some`, and its
  `phrase` says "17 KB";
- `nothing_to_stat_is_silent`: a missing path, a directory, and (`#[cfg(unix)]`, made
  with `std::process::Command::new("mkfifo")`) a FIFO → `None`, with no hang;
- `exempt_paths_are_silent`: 20 KB files at `…/.claude/plans/x.md`,
  `…/docs/plans/x.md`, `x.diff`, `X.PATCH`, `x.png`, `X.PNG`,
  `…/tool-results/x.txt` → `None`. Also `…/myplans/x.md` → `Some`, so a
  substring is not enough to exempt a path;
- `the_remedy_quotes_the_path`: `phrase("/a b/it's.rs", …)` contains
  `'/a b/it'\''s.rs'`.

Integration tests (`tests/hook.rs`). Add `ReadFixture::with_size(name, bytes)`;
`new` keeps its 6,000 bytes:
- `a_large_whole_read_is_advised`: the first unbounded Read of a 20 KB file says
  `read-unbounded-large`, and `decision()` is `None` (advise, not deny);
- `a_windowed_or_deliberate_whole_read_is_silent`: the same with `,"limit":200`,
  and with `,"offset":1,"limit":<the fixture's line count>`. Neither stdout
  contains `read-unbounded-large`;
- `a_small_first_read_stays_silent`: the 6,000-byte fixture is silent on a first
  read, so `a_file_read_twice_is_advised_and_an_edit_between_resets_it` still holds;
- `a_reread_says_file_reread_only`: the second whole Read of the 20 KB file contains
  `file-reread` and not `read-unbounded-large`;
- `an_observed_reread_still_suppresses`: add
  `ReadFixture::file_event_with_global(…, cfg)`, routed through the existing
  `send_with_global` (`tests/hook.rs:951-978`; plain `send` does not pin
  `GIT_CONFIG_GLOBAL`, so it would read the developer's own `~/.gitconfig`).
  Pass `cfg = "[amont \"agent.file-reread\"]\n\tstance = observe"`. Both sends
  share `home()`, so the session state carries over. A second whole Read
  prints neither rule. The journal rows **for this test's `session_id`** are
  one `file-reread watched` and no `read-unbounded-large` (the journal is
  per-thread `home()`, so the test asserts on its own session's rows, not on a
  total).

## Verification

1. `make fmt` then `make check` (`lint` = fmt-check + clippy `-D warnings`,
   `test` = cargo test) in the worktree. Expected: all of the tests named above
   pass, with zero clippy warnings. Before pushing, compare against the CI
   commands in `AGENTS.md`.
2. Live, on the release build (`cargo build --release`). Three probes, each
   with its **own fresh `session_id`** so `file-reread` cannot confound them
   (tui). Each checks `rc=0`, an empty stderr (`2>err; test ! -s err`), and
   that `read-unbounded-large` is present or absent in stdout:
   - `<worktree>/src/hook.rs` (28.6 KB), no window → present;
   - the same with `"limit":100` → absent;
   - `~/.claude/plans/tingly-pondering-eclipse.md` → absent (exempt).
3. `amont-agent rules` lists `read-unbounded-large  advise (max deny)`.
4. After release and install, in one real session: an unbounded Read of a
   file over 16 KB shows the advice. `amont-agent status` then shows a row
   shaped like the existing `file-reread` one (`file-reread  advise  advise
   81.1/1000 measured 2026-09-09  112 advised`), i.e.
   `read-unbounded-large  advise  advise  4.7/1000 measured 2026-10-05  1 advised`.
5. Soak: run `tools/read-rate.py` two weeks after release. Compare the
   weekly per-1000 rate against W40 7.8 / W41 11.1, and from the journal,
   the share of `advised` rows followed by a windowed Read or Grep of the
   same path within 3 file operations. If neither falls, that is the case
   for `deny`, made from the journal.

<!-- panel: repos=amont-agent reviewers=backend,lang:rust,tui,unix body-sha=9f0ee6b68bf1 -->
