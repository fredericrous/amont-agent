---
status: active
branch: feat/publish-without-skill
repos: [amont-agent]
adrs: [ADR-0008, ADR-0022]
---

# publish-without-skill: a tag push or a PR merge needs its skill first

## Review panel

👉 **Decide:** whether, after Phase 4 measures the rate, to set this rule to deny right away (`amont.agent.publish-without-skill.stance deny`) or let it soak at advise first.
📍 amont-agent · plan reviewed, nothing built · next: Phase 0 transcript capture. Panel: backend, lang:rust, tui, unix.
**Changed by review:**
- The window is now the current turn plus one human reply, and accepts a slash command.
- A missing transcript or a missing `SKILL.md` now advises instead of refusing, through a new `Confirmed` ceiling.
- `--follow-tags` was dropped, and the rule no longer needs a working directory.

📄 Full reviews: [2026-10-07-publish-without-skill.reviews.md](2026-10-07-publish-without-skill.reviews.md)
**Verdicts:** 4 approve-with-changes in round 1, and backend approve-with-changes twice in round 2. The four final low/medium notes are listed in Full reviews and are carried into the implementation.

## Goal

On 2026-10-07 a session cut relais v0.10.0 by hand. It merged the PRs and pushed the tag from memory, without calling the `merge-when-green` or `tag-release` skill. Nothing broke that time. But working from memory skips steps, for example the explicit `conclusion` check, the artifact check and the sweep. CLAUDE.md and the memory `feedback_invoke_release_skills_not_memory` both say to call the skill, and the model still didn't.

Two existing rules see these commands, and neither checks whether the skill ran:
- `release-tag-push` (observe) watches version-tag pushes.
- `forge-merge-by-hand` (advise) warns on a curl merge, and its remedy names the skill.

This plan adds a gate built like `implementation-review`. A publishing command is refused unless the skill it needs was called recently, in the window defined in Behaviour 2.

## Non-goals

- It does not prove that the skill's steps were followed. It only proves that the skill was loaded when it was needed.
- It does not change the skills' text, or the `gh-pr-merge-auto` and `tag-after-commit` rules.
- It does not guess who owns a repo. The rule looks at the command only.
- It does not match `--follow-tags` or `push.followTags`. Whether those publish a `v*` tag cannot be seen from the command, and matching them would refuse ordinary branch pushes.

## Behaviour

1. **What is matched.** The matching moves into a new shared module, `src/publish_cmd.rs`. Its `classify(&Parsed) -> Option<Publish{kind, excerpt}>` reuses the existing helpers:

   | Matched command | Required skill | Reused helper |
   |---|---|---|
   | A `git push` that publishes a version tag: `vX…`, `refs/tags/vX…`, either with a leading `+`, `--tags`, or `--mirror`. Not `--delete`, not a dry run. | `tag-release` | `release_tag_push::is_version_tag` and its `examine` |
   | `gh pr merge …`, with or without `--auto` | `merge-when-green` | the operand check in `gh_pr_merge_auto.rs` |
   | An HTTP client sending `/pulls/<n>/merge` | `merge-when-green` | `forge_merge_by_hand::{is_pr_merge_path, is_http_client}` |

   `release-tag-push` moves to `classify` too. Two changes in what it matches are deliberate and come with corpus lines: it now strips a leading `+` and matches `--mirror`. Nothing else it matches changes.

2. **How long a skill call stays valid.** A skill counts while the command runs either in the turn where the skill was called, or in the human turn right after it. That allows one human prompt in between, for example an answer to the skill's "confirm?" question, or a "status?" sent part-way through a poll.

   What starts a new turn is a human prompt: a `user` entry that is neither of these:
   - a tool result, an AskUserQuestion answer, a task-notification, a queued command, a wakeup or a compaction summary;
   - the skill-body entry that Claude Code writes right after a Skill `tool_use`.

   Phase 0 captures the exact shape of each of these entries.

   The window is measured in human prompts, not in time. A skill called before a long poll still covers its own merge hours later, including across wakeups and Stop-hook continuations. Phase 4 reports how old the skill call is at each publish, so a time cap can be added later if the data shows one is needed.

3. **What counts as the skill having run.** Any of these:
   - an assistant `tool_use` entry with `name == "Skill"` and `input.skill` equal to the required skill, or to `<plugin>:<skill>`;
   - a human prompt that runs the skill as a slash command (`/tag-release`). Phase 0 captures that shape.

   Only structured fields count, never text echoed in a tool result. This is the same anti-forgery stance as `completed_agents` (`src/plan_review.rs:20-25`).

4. **How the transcript is scanned.** One forward pass, in the style of `completed_agents` (`src/plan_review.rs:707-718`):
   - A substring check comes first, and only lines containing `"Skill"`, `"type":"user"` or the slash-command marker are parsed.
   - The pass keeps the index of the last two human prompts and of the most recent Skill call for each skill.
   - A half-written last line is ignored.
   - Budget: under 50 ms at p99 on a transcript of 50 MB or more. The scan only runs when a command matches, about 21 times per 1,000 commands (`release_tag_push.rs:66` gives 6.9, and `forge_merge_by_hand.rs:80` gives 13.8).

5. **What the rule decides.**
   - **Skill seen inside the window:** `Confirmed::No`. The command runs, and nothing is printed.
   - **Skill not seen:** the command is refused. The reason names the skill, for example "call the merge-when-green skill (Skill tool), then re-run; ci.gates-the-merge". The remedy also gives the person a way out, worded like `implementation-review` (`src/rules/implementation_review.rs:46-50`): "If the skill does not apply here, ask the person; they can run `git config --global amont.agent.publish-without-skill.stance observe`."
   - **Not knowable:** the rule advises and never denies. This covers a transcript that is missing, unreadable or truncated, and a required skill whose `SKILL.md` cannot be found (under `$CLAUDE_CONFIG_DIR` or `~/.claude`, in `skills/` or a plugin's skills folder).

     Making this work needs a new piece: a stance ceiling on `Confirmed`. Today `YesAt` and `YesSaying.floor` can only raise the stance (`src/rules/mod.rs:157-175`). The plan adds `ceiling: Option<Stance>` to `YesSaying` and applies it in `hook.rs` after the partly-read cap (`:311-315`).
   - **Journal:** every finding records one reason tag, `no-skill`, `stale-turn` or `unknown:<why>`, the way `src/journal.rs:578-581` does.

6. **Stance.** The rule ships with `default_stance = Advise` and `max_stance = Deny`, the fleet pattern.
   - This machine has no global `amont.agent.stance`, only per-rule keys (checked 2026-10-07 with `git config --get-regexp`). The journal shows `release-tag-push` observing and `forge-merge-by-hand` advising.
   - So the rule advises until `git config --global amont.agent.publish-without-skill.stance deny` is set, and the two related rules keep their stances.
   - Rollback is the same key set to `observe`.
   - It is not one of the missing-cwd push gates (`hook.rs:740-743`). It needs no directory, so a second id list next to that one, "rules that do not need a working directory", lets `confirmed()` skip the `is_dir` precheck (`hook.rs:703-705`) for it. No new field on `Rule`.

7. **Citations.** In the module doc and the reason text: `ci.gates-the-merge`, and `ADR-0008 release.trigger`.

## Phases

- [x] Phase 0 — **Capture the real shapes.** No code in this phase. From transcripts under `~/.claude/projects`, record:
  - the assistant `Skill` `tool_use` and its result, both plain and plugin-qualified;
  - the skill-body `user` entry that follows a Skill call;
  - a typed `/tag-release` prompt;
  - an ordinary human prompt, next to an AskUserQuestion answer, a task-notification, a wakeup and a compaction summary;
  - the `transcript_path` a subagent's Bash payload carries, in both directions:
    - the parent calls the skill and the subagent publishes;
    - the subagent calls the skill itself and then merges. Its tool calls go to its own transcript file.

    This decides which file `confirm` reads, and whether it also reads the parent's. The default is that a skill called in either file counts, inside the window.
  - a Stop-hook continuation entry and a `/loop` wakeup entry.
  - the stance keys: `git config --get-regexp 'amont\.agent\..*stance'`.

  Then time a stub forward scan on the largest local transcript, 10 runs, p50 and p99. Write all the findings in the decision log.
- [x] Phase 1 — **The shared pieces.**
  - `src/publish_cmd.rs` holds `classify`. `release_tag_push`, `forge_merge_by_hand` and `gh_pr_merge_auto` switch to it. Existing corpus lines stay green, and new lines cover the `+` and `--mirror` cases.
  - `skill_window(reader, skill) -> Result<Window, Unknown>` lives next to `completed_agents` in `src/plan_review.rs`, or in `src/transcript.rs`, whichever reads better. It returns one of: in the current turn, in the previous turn, stale, or unknown.
  - The `ceiling` field on `Confirmed`, applied in `hook.rs`. The list of rules that need no working directory, in `hook.rs`.
  - Unit tests: forged text, a task-notification inside the turn, the skill-body entry, a slash command, the plugin-qualified name, one intervening human prompt (still valid), two intervening prompts (stale), and a truncated last line.
- [x] Phase 2 — **The rule.**
  - `src/rules/publish_without_skill.rs` holds a pure `examine` (`classify`) and a `confirm` that reads `ctx.transcript`, checks that `SKILL.md` exists, and calls `skill_window`.
  - Wiring: `src/rules/mod.rs` (the `pub mod` line and `RULES`), the `src/corpus.rs` `include_str!`, and `tests/corpus/publish-without-skill.cases`.
- [ ] Phase 3 — **End-to-end tests.** `tests/publish_without_skill.rs`, using the `World`/`Transcript` builder pattern from `tests/implementation_review.rs:281-380`, with this rule's own stance key set to `deny` and the other rules at their defaults, as on this machine. Each case and its expected result:

  | Case | Expected |
  |---|---|
  | No skill call | deny, reason `no-skill` |
  | Skill called in the current turn | pass |
  | Skill called, then one human reply | pass |
  | Skill called, then two human prompts | deny, reason `stale-turn` |
  | Task-notification inside the turn | pass |
  | `/tag-release` typed by the person | pass |
  | Transcript missing, unreadable, or ending in a truncated line | advise |
  | `SKILL.md` missing | advise |
  | Working directory missing, skill called | pass |
  | Command only partly read | advise |
  | Rule set to observe | silent |
  | The subagent's own transcript holds the skill call | pass |

  Three more cases:
  - `gh pr merge --auto` is still denied by `gh-pr-merge-auto`.
  - A curl merge made without the skill: the output is this refusal alone, because the hook drops advice when there is a refusal (`hook.rs:336-341`). The skill name and the next step fit in the first 80 columns.
  - The same merge with this rule at advise: both pieces of advice show, this rule's first.
- [ ] Phase 4 — **Evidence and docs.**
  - The backtester never runs `confirm` (`src/rules/mod.rs:153-155`), so the measure comes from a one-off pass in `tools/`. It runs `classify` and then `skill_window` over local transcripts and reports, per 1,000 Bash calls:
    - matched commands;
    - refusals without the skill;
    - refusals that the one-reply allowance avoided;
    - unknowns;
    - the age of the skill call at each publish, p50 and max.

    The 12 `release-tag-push` and `forge-merge-by-hand` lines already in `~/.claude/amont-agent/journal.log` serve as a cross-check, including the relais v0.10.0 tag push from 2026-10-07.

    These numbers fill `Evidence{per_1000, measured, trend}`.
  - Docs: a row in `docs/rules.md`, the README and `docs/index.md` counts, a `docs/measuring.md` entry and a CHANGELOG entry.
  - Nothing changes in the deny list in `tests/rules_json.rs` or in the caveat built by `scripts/bump-tap.py`.
  - Run `amont agents-md` and stage the result.

## Decision log

- 2026-10-07 — One new rule, not extensions of `release-tag-push` and `forge-merge-by-hand`. Those two judge the command itself. This rule judges what came before it, with its own rollback and evidence. The overlap is acceptable, and its combined text is pinned by a test.
- 2026-10-07 — The turn boundary comes from the transcript, not from `prompt_id` records. Recording `prompt_id` would need a Skill PostToolUse hook plus state, and the transcript already holds the answer.
- 2026-10-07 (review) — The window is the current turn plus one human turn. The skills ask the person to confirm, and people send "status?" during a poll; scoping to the current turn only would refuse the skill's own merge.
- 2026-10-07 (review) — When the answer is unknown, the rule advises, enforced by a new `Confirmed` ceiling. This differs from `implementation-review`'s unknown path, which is denied under deny. An unreadable transcript must never block a release.
- 2026-10-07 (review) — The rule is not a missing-cwd gate. A list of ids next to the gates list lets `confirm` run without a directory. `--follow-tags` is out of scope.
- 2026-10-07 (review) — A round-2 blocker assumed a global `amont.agent.stance deny`. Checking the machine showed only per-rule keys, so the two related rules keep their observe and advise stances and are not touched. Denying is an explicit per-rule key, which the person sets.
- 2026-10-07 (review) — The window has no time limit, and Phase 4 measures the age of the skill call before any cap is added.
- 2026-10-07 (Phase 0) — Shapes captured from this machine's transcripts:
  - A model Skill call is an assistant `tool_use` with `name: "Skill"` and `input.skill`. It is followed by a `tool_result` (`toolUseResult: {success, commandName}`) and a skill-body `user` entry with `isMeta` and `turnCompanion` set. Neither of those has an `origin`.
  - A typed skill is a `user` entry with `origin.kind: "human"` whose string content starts `<command-message>tag-release</command-message>\n<command-name>/tag-release</command-name>`.
  - Human prompts carry `origin.kind: "human"`.
  - Task notifications carry `origin.kind: "task-notification"`, plugin messages `"plugin"`, `/loop` wakeups `isMeta` with no origin, and compaction summaries `isCompactSummary`.
  - A task notification opens a new `promptId`, so `promptId` is not a turn. The boundary is `origin.kind == "human"`.
- 2026-10-07 (Phase 0) — In a subagent, the hook's `transcript_path` is the parent's file, and `agent_transcript_path` is the subagent's own (`<session>/subagents/agent-<id>.jsonl`, per the Claude Code hooks reference). The subagent's file has no human prompt. `confirm` reads both files and takes the nearer call, so a skill called by the parent within the window, or by the subagent itself, both count.
- 2026-10-07 (Phase 1) — The scan lives in its own module, `src/skill_window.rs`, rather than in `plan_review.rs` (1,928 lines) or the backtest scanner `transcript.rs`.
  - A typed slash command counts only when the prompt starts with Claude Code's `<command-` tags. Prose that quotes the tag is not a call.
  - Any unparseable last line counts as unknown, not only one that mentions a skill: where a line was cut cannot be known.

## Verification

- CI's commands, run locally: `make check` (fmt, clippy `-D warnings`, test), `amont-agent corpus check`, and `tests/rules_json.rs`. Expected: all pass. `actual:`
- Timing: the Phase 4 tool on the largest local transcript, 10 runs. Expected: p99 under 50 ms. `actual:`
- Live run in a scratch repo with a throwaway remote, with a local build installed through `install --write`, and `amont.agent.publish-without-skill.stance deny` set. Fill in an `actual:` for each step:

  | # | Action | Expected |
  |---|---|---|
  | 1 | `git push --dry-run origin v0.0.1` | silent |
  | 2 | `git push origin v0.0.1` with no skill call | denied, names `tag-release`, journal `no-skill` |
  | 3 | Call the `tag-release` skill, then push | allowed |
  | 4 | Two new human prompts, then push again | denied, `stale-turn` |
  | 5 | `gh pr merge` on a throwaway GitHub PR, without and then with `merge-when-green` | denied, then allowed |
  | 6 | Forgejo curl `/pulls/<n>/merge` on a throwaway PR, without and then with the skill | denied, then allowed |
  | 7 | Type `/merge-when-green` myself, then merge | allowed |
  | 8 | A subagent pushes a tag after the parent called the skill | the result Phase 0 decided |
  | 9 | Step 2 with `transcript_path` unreadable | advised, not denied |
  | 10 | Step 3 from a deleted working directory | allowed |
  | 11 | A subagent calls `merge-when-green` itself, then merges | allowed |

## Implementation review

(Filled in by worktree-task F4b.)

## Outcome

(Filled in after merge and release.)

<!-- panel: repos=amont-agent reviewers=backend,lang:rust,tui,unix body-sha=0fc2867b7638 -->
