---
status: active
branch: feat/screenshot-in-main-session
repos: [amont-agent]
adrs: []
---
# Screenshots belong in a subagent

## Review panel
👉 **Decide:** none — approve if a once-per-session advice is enough to move main-session screenshots into `browser-check`.
📍 amont-agent · plan reviewed, nothing built · next: worktree + Phase 1. Panel: backend, lang:rust, tui, unix (opus), codex (gpt-6-astra).
**Changed by review:** exact advice text without `filePath`; paired silent→advise tests; TARGETS after ExitPlanMode.
**Verdicts:** round 1 4× approve-with-changes; round 2 2× approve, 2× approve-with-changes; codex approve-with-changes; 7 carried into implementation.
📄 Full reviews: [2026-11-12-screenshots-belong-in-a-subagent.reviews.md](2026-11-12-screenshots-belong-in-a-subagent.reviews.md)

## Picture
```
Claude Code: PreToolUse(take_screenshot)
        |
        v
payload.rs --> Event::PreScreenshot
        |
        v
hook.rs on_screenshot
   agent_id present? ------- yes --> silent
   filePath set? ----------- yes --> silent
   stance observe? --------- yes --> journal, silent
   told this session? ------ yes --> silent
        | no
        v
journal + Decision::Advise ("delegate to browser-check")
```
The picture shows the four silencers a main-session screenshot passes
before the one advice is emitted.

## Goal
When the main conversation calls `mcp__chrome-devtools__take_screenshot`
without `filePath`, greenpiste advises it once per session to delegate the
check to the `browser-check` subagent, so the image (≈126 KB each, 368 of them
in past transcripts) does not land in the main context. Subagents and saved-to-file
screenshots are left alone.

## Non-goals
- No deny: advise only (ceiling `advise`). The rule is prose in worktree-task F2
  and CLAUDE.md; this makes it visible, not enforced.
- No other chrome-devtools tools (`take_snapshot`, `list_network_requests`):
  measure after this ships before widening.
- No change to the ios-simulator screenshot tool (55 calls, ≈60 KB) for now.
- No change to how `browser-check` itself works.
- A `filePath` screenshot later opened with Read still enters context; this
  rule does not catch that, so the advice does not offer `filePath` as a remedy.

## Code review
- amont-agent: backend, lang:rust, unix (tui left out: no command-line output changes; the advice is hook JSON read by the model)

## Behaviour
PreToolUse with `tool_name == "mcp__chrome-devtools__take_screenshot"`:

| payload | result |
|---|---|
| no `agent_id` (main), no `filePath` | advise, first time this session: additionalContext is exactly `amont-agent/screenshot-in-main-session: a screenshot here puts the image (~126 KB) into the main context. Delegate the check: Agent tool with subagent_type "browser-check", giving the URL and what to verify.` |
| same, a second time in the session | silent (once per session, `first_telling` keyed `RULE.id`) |
| no `agent_id`, `filePath` non-empty | silent: the image is saved, not attached |
| `agent_id` present (any value, invalid included) | silent: already a subagent |
| stance resolved to `observe` | journalled, silent |
| stance resolved to `deny` | advises (ceiling `advise`; `Stance::Advise \| Stance::Deny => Decision::Advise`, as `guidance_written`) |

Install: `amont-agent install --write` adds one PreToolUse block with matcher
`mcp__chrome-devtools__take_screenshot`; re-running it is idempotent (existing
`settings::ensure`).

## Phases
- [ ] Phase 1 — rule + payload + dispatch + install target:
  `crates/amont-agent-guard/src/rules/screenshot_in_main_session.rs`,
  `crates/amont-agent-guard/src/rules/mod.rs`, `src/rule_table.rs` (rule added; `the_table_holds_every_rule_in_its_place` 51→52 and its comment),
  `crates/amont-agent-guard/src/payload.rs` (journal excerpt `take_screenshot @main`, no URL or path; new `Event::PreScreenshot` with
  `agent_id`, `file_path`, `session`, `cwd`, `permission_mode`; arm beside the
  Grep arm at ~546), `src/hook.rs` (arm beside `Event::PreGrep` at ~177; handler
  modelled on `on_grep` ~1906 but using `stance::resolve(&RULE)`, not
  `resolve_for`), `crates/amont-agent-guard/src/settings.rs` (TARGETS entry
  with a comment, as Grep's, placed after `("PreToolUse", Some("ExitPlanMode"))` so existing `pre[i]` index asserts keep their matchers),
  `crates/amont-agent-guard/src/corpus.rs` +
  `crates/amont-agent-guard/tests/corpus/screenshot-in-main-session.cases`
  (header + `nomatch` lines, as `code-read-not-lsp.cases`).
- [ ] Phase 2 — tests: `tests/hook.rs` (main/no filePath → advise once;
  second call silent; filePath → silent; `agent_id:"w1"` → silent;
  `agent_id: 5` → silent; each silent case is followed, in the same session,
  by a main no-filePath call asserted to advise, so the test fails if the arm
  is missing or the silent call spent the once-per-session advice; stance
  `observe` via global config → silent), `tests/settings_merge.rs` (block counts 6→7 and 5→6,
  index asserts re-checked).
- [ ] Phase 3 — docs + release notes: `docs/rules.md` (row: `advise` (ceiling
  `advise`)), `CHANGELOG.md` (Unreleased → Added). Release 3.32.0 via
  `tag-release`; the install is the person's.

## Decision log
- 2026-11-12 — Advise, not deny, because the person chose "build it now" for an
  advisory hook and the written rule is new; a deny waits on a count.
- 2026-11-12 — Silent when `filePath` is set, because the MCP then saves the
  image instead of attaching it (its tool description), so nothing reaches context.
- 2026-11-12 — `agent_id` of any value counts as a subagent (payload maps a
  non-string to `Some("")`), so a malformed id never nags a subagent.

## Verification
- Phase 1–2 (each run with a scratch `CLAUDE_CONFIG_DIR` and a fresh
  `session_id`, so the real journal and told-state are untouched): the release
  binary fed real PreToolUse JSON on stdin (main, no
  filePath) → stdout `hookSpecificOutput.additionalContext` equals the
  Behaviour-table string byte for byte (the Phase 2 test asserts the same
  string) → <observed>. Same JSON with `"agent_id":"w1"` → empty
  stdout, exit 0 → <observed>. With `"filePath":"x.png"` → empty → <observed>. The first JSON again, same
  session → empty stdout → <observed>.
  Every case: exit 0 and empty stderr; the advise stdout parses with `jq -e .`
  → <observed>.
  Would fail if: the arm is missing (NotOurs → empty for every case).
- Install: `amont-agent install --write` against a scratch `CLAUDE_CONFIG_DIR`
  copy of the real `settings.json`, run twice → exactly one new block with the
  matcher, second run no diff → <observed>.
- Live (after the person's install): one main-session screenshot in a real
  Claude Code session shows the advice; one via `browser-check` does not →
  <observed>. This also proves Claude Code honours a literal `mcp__…` matcher.
- 7 days after install: main-session `take_screenshot` calls per session vs
  the 368-call baseline, and `Read` of `*.png` within 3 calls of a `filePath`
  screenshot → <observed>. This is the count the deny waits on.

## Outcome

<!-- panel: repos=amont-agent reviewers=backend,lang:rust,tui,unix,codex body-sha=ee2b41c07011 -->
