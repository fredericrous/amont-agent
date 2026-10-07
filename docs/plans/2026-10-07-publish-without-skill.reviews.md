# publish-without-skill — full reviews


**backend, round 1:** approve-with-changes (40k tokens, 52 s).
- high: the skill-body entry and a typed slash command broke the turn boundary (applied).
- high: `--follow-tags` was new behaviour, not reuse (applied, now a non-goal).
- medium: a question asked by the skill pushed the publish into the next turn (applied: the one-reply window).
- medium: no rate and no latency budget (applied).
- medium: verification could not be observed, and there were no reason tags in the journal (applied).
- low: the missing-cwd hold was wrong for merges (applied).
- low: the stance question was moot (superseded; see round 2).

**lang:rust, round 1:** approve-with-changes (50k tokens, 50 s).
- blocking: falling back to advise could not be implemented without a ceiling on `Confirmed` (applied).
- high: the missing-cwd gate gave a misleading refusal (applied).
- medium: `--follow-tags` (applied).
- medium: the backtester never runs `confirm` (applied: a one-off tool in `tools/`).
- medium: a reply from the person mid-skill (applied).
- low: the backwards read had no implementation (applied: a forward scan).

**tui, round 1:** approve-with-changes (40k tokens, 41 s).
- high: a missing `SKILL.md` must fall back to advise (applied).
- medium: the refusal must give the person the rollback (applied).
- medium: a "status?" message mid-poll (applied).
- low: pin the combined text with `forge-merge-by-hand` (applied).

**unix, round 1:** approve-with-changes (39k tokens, 43 s).
- high: the rule should not be a missing-cwd gate (applied).
- high: `--follow-tags` (applied).
- medium: the forward scan and its budget (applied).
- low: `+refs/tags` and `--mirror` (applied).
- low: a truncated last line (applied).

**backend, round 2:** approve-with-changes (44k tokens, 84 s).
- blocking: the global deny stance would make the related rules refuse every match. Resolved: the machine has no global key, and the journal confirms it.
- high: a subagent calling the skill itself (applied).
- medium: the window has no time limit (applied: stated as such, and measured).
- low: the rate is about 21 per 1,000 (applied).
- low: an id list rather than a `Rule` field (applied).

**backend, final body:** approve-with-changes (40k tokens, 38 s). A–E resolved. Four notes are carried into the implementation; the body is left as is so the reviews stay bound to it:
1. Insert the rule into `RULES` just before `forge_merge_by_hand::RULE` (`src/rules/mod.rs:417-418`), so its advice comes first.
2. Carry `ceiling` through the hook's `Confirmation { floor, said }` (`src/hook.rs:673-676,715-722`) and apply it with `.min()` after `:311-315`.
3. The `gh pr merge --auto` test without the skill expects two refusals, `gh-pr-merge-auto`'s first. With the skill called, it expects `gh-pr-merge-auto`'s alone.
4. Phase 4 reports refusals per unique command per session as well as raw counts. One v0.10.51 push was journaled 3 times in 18 s.
