---
status: active
branch: feat/deny-agents-scope
repos: [amont-agent, fleet-lsp]
adrs: []
---
# Plan: fix fleet-lsp's call-order error, deny symbol-grep-not-lsp for haiku workers

## Review panel
📄 Full reviews: [2026-11-09-call-order-error-haiku-deny.reviews.md](2026-11-09-call-order-error-haiku-deny.reviews.md)
👉 **Decide:** none — approve if the haiku deny stays refuse-once (a firmer nudge); making it a real block waits for one week of data.
📍 fleet-lsp, amont-agent · plan reviewed, no code yet · next: classify the 9 errors, then a worktree per repo. Panel: backend (opus), lang:rust (opus), unix (opus), tui (opus); codex skipped (sends the plan outside).
**Changed by review:** A1 saves both warm-start failures; the deny keyed per agent with an Unrecorded fallback; `backtest` check replaced by a hook test plus a live deny.
**Verdicts:** 4 approve-with-changes in rounds 1 and 2, backend approve on the final body; 4 findings carried into implementation.

## Picture

```
 today                               after this plan
 -----                               ---------------
 lookup --> LSP                      lookup --> LSP
              |                                   |
   no repository chosen yet           "no python project in <repo>"
              |                       (the real reason, fleet-lsp 0.6.1)
              v                                   |
         agent greps (advised,                    v
         not blocked)                   agent stops, or opens a file

 lookup --> grep                     lookup --> grep
              |                                   |
          advised once                  denyAgents match: refused once
              |                                   |
              v                                   v
         grep runs                      agent uses LSP or repeats it
```

A symbol lookup used to fall back to grep after an unhelpful LSP error, or to
run grep after one piece of advice; after this plan the error names its real
reason and a haiku worker's first grep per pattern is refused (the identical retry runs: a firmer nudge, not a block).

## Context
The per-agent-type measurement (window 10-29 onward, event-time) showed:
- 19 LSP errors after 0.5.6. Gone-root errors are fixed (11 before, 0 after).
- **"no repository chosen yet"** is the largest open cause (9 in the window, 7 from `relais:relais-reviewer-fable-medium`). fleet-lsp raises it in `src/workspace.rs:435` (`refusal::no_document`, `src/route.rs:266`) when a URI-less request (workspaceSymbol) arrives and `recent` is empty. `recent` is seeded only by `warm_start` (`workspace.rs:385`), which logs a failure and discards its text. Logs show two real causes: the session root is not a git repo (`~/Developer/Perso`), or the session repo has no project of that language (python in a rust repo). The second cause gets a misleading "open a file first" message and agents retry.
- `relais:relais-worker-haiku-default` meets the deny gate (grep-for-symbol 41/1k, LSP error share 5%, 2% without the 11-01 tsserver SIGABRT incident). Other types do not.
- `symbol-grep-not-lsp` has stance per rule only (`stance::resolve(&Rule)`, `crates/amont-agent-guard/src/stance.rs:69`), so "deny for haiku workers only" needs a per-agent-type scope. The ceiling is already `deny`.

Outcome: URI-less refusals say the real reason, the guidance note says the call order, and `symbol-grep-not-lsp` denies for named agent types, set to the haiku worker.

## Phases

### Part A: fleet-lsp v0.6.1 (repo fredericrous/fleet-lsp)
Start with a read-only classification: for the 9 "no repository chosen" errors, read each session's `cwd` from its transcript and tell non-repo cwd from repo-without-language. This sizes the fix and goes in the PR description. A1 changes both causes: a non-repo cwd then gets `no_repository` ("<dir> is in no git repository; fix: open a file inside a repository"), a repo without the language gets `nothing`.

- **A1.** `workspace.rs`: keep the last `warm_start` failure text (`warm_failure: Option<String>`, set in the two `Err` arms of `warm_start`; never cleared, since it is read only while `recent` is empty and nothing is gone). In `request` (`workspace.rs:433-436`) use it as the fallback before `refusal::no_document`: `None => self.warm_failure.clone().unwrap_or_else(|| refusal::no_document(..))`. A python call in a rust repo then gets "no python project found in <repo>; fix: none", which tells the agent to stop. `no_document` stays for a session with no warm start at all.
- **A2.** `plugins/fleet-lsp/hooks/code-intelligence.sh`: change the fourth bullet (file line 18, "Each repository you query...") so it says to open a file (documentSymbol/hover) before workspaceSymbol when the session did not start in a repo. Keep the index at 6 lines or fewer (`check-plugin.sh` caps it).
- **Tests** in `tests/relay.rs`, with `FALSIFY:` comments per repo convention: (1) a refused session repository answers a URI-less `workspace/symbol` with the warm-start text, not the generic one (reuse `a_refused_session_repository_still_initializes`, `relay.rs:1691`); (2) a session outside any repo gets the `no_repository` text naming the session dir, and `no_document` remains only when no warm start ran; (3) a later `didOpen` of a good repo routes the request (no FALSIFY: `didOpen` seeds `recent`, workspace.rs:475). Tests (1) and (2) each fail with its hunk reverted.
- `CHANGELOG.md`, release v0.6.1 via the `tag-release` skill (tap test block unchanged, so no `homebrew-tap` edit), then `brew upgrade fleet-lsp` and `/plugin update fleet-lsp` (the person's step), `fleet-lsp doctor`.

### Part B: per-agent-type stance in amont-agent (repo fredericrous/amont-agent, minor release)
- **Key:** `amont.agent.<rule-id>.denyAgents`, multi-valued, global/system config only (as the stance keys: a repository cannot lower it). Each value is an exact agent type or a prefix ending in `*` (`relais:relais-worker-haiku-*`). An empty or valueless entry, a bare `*`, or a `*` anywhere but the end is ignored with one stderr warning via `complain`; unit-tested.
- `gitconfig.rs`: add `values(key) -> Vec<String>` over the existing per-scope snapshot, which already holds every repeated value (`lookup` at `gitconfig.rs:221` keeps only the last). Precedence, stated once: the `--global` and `--system` lists are merged; a scope git refuses (`Snapshot::Refused`) adds no values and is reported once via `complain`.
- `stance.rs`: add `resolve_for(rule: &Rule, agent: &lsp::Agent) -> Stance`. It calls `resolve(rule)`, and raises only `Advise` to `Deny` when the agent is `Named(t)` and `t` matches a `denyAgents` value; the result is clamped by `rule.max_stance`. `Observe` (switched off, shadow arm, configured observe) is never raised.
- Deny text: a Deny gets its own remedy in `symbol_grep_not_lsp::phrase`: "Refused for this agent type. Ask LSP: workspaceSymbol, then findReferences. If LSP refused this lookup, run the same search again: it runs." The existing once-per-pattern telling (`first_telling`, `hook.rs:1846`) stays, so this deny is a refuse-once (unlike the Read rule's deny, which always fires, `hook.rs:1241-1246`): a firmer nudge, not a block. Test it in `tests/hook.rs` (deny text, then the identical retry runs). Key the telling per agent at both call sites, `grep_gate` (`hook.rs:1804`, Bash) and `on_grep` (`hook.rs:1847`), adding `@<agent_id>` to the `grep <pattern>` key only when `resolve_for` raised the stance to Deny (as the Read rule keys at `hook.rs:1249`), so a parent session already advised on a pattern does not stop a worker's deny while the advise path keeps its key byte for byte; test that case, and that a non-denied subagent stays silent after its parent was told. When `first_telling` hits `Told::Unrecorded` (`hook.rs:1954`) on a Deny, fall back to Advise, journalled `unrecorded` (as `Once::Unrecorded`, `hook.rs:1881-1883`): otherwise every retry is refused and the remedy's promise is false; test it. A Grep-tool retry leaves no journal line (`hook.rs:1855`), so the retry share in the remeasure comes from transcripts.
- Call sites: `src/hook.rs:496` (Bash loop, `bash.agent`) and `src/hook.rs:1845` (`on_grep`, `op.agent`), `symbol-grep-not-lsp` only (at :496 a branch on `rule.id == sg::RULE.id` inside the shared loop). Every other rule keeps `resolve`.
- **Tests:** unit in `stance.rs` (matching, prefix, non-named agents, ceiling clamp, Observe never raised); `tests/hook.rs` also checks that a denied haiku grep still passes `grep_gate` (`hook.rs:518`); `tests/stance_scope.rs` (a repository `.git/config` cannot set `denyAgents`; a global value does); `tests/hook.rs` (a Named haiku-worker grep is denied, main and another type stay advised); update `docs/configuration.md` and `docs/stances.md`.
- Release as the next minor (`tag-release`); the amont-agent binary installs per its normal path.
- **Rollout:** `git config --global --get-all amont.agent.symbol-grep-not-lsp.denyAgents | grep -qxF 'relais:relais-worker-haiku-*' || git config --global --add amont.agent.symbol-grep-not-lsp.denyAgents 'relais:relais-worker-haiku-*'` on this laptop (idempotent). Rollback: `git config --global --fixed-value --unset-all amont.agent.symbol-grep-not-lsp.denyAgents 'relais:relais-worker-haiku-*'` (plain `--unset` exits 5 on repeated values and reads the pattern as a regex). Patch the dotfiles source so it is not drifted (the laptop is the source; patch the template, never `apply` over it). Confirm the payload's `agent_type` for a haiku worker is exactly `relais:relais-worker-haiku-default` (the transcript meta says so) with one live deny.

### Sequencing
A and B are independent repos; each is one branch and one PR in its own worktree (`worktree-task`). B's rollout config waits for B's release and for A's v0.6.1 to be installed, so the deny does not land on top of the old messages.

## Verification
- fleet-lsp: the three relay tests fail with their hunk reverted and pass with it applied; live: a python `workspaceSymbol` in a rust-only repo returns the "no python project" text; doctor healthy.
- amont-agent: `tests/hook.rs`: Grep payload, agent_type `relais:relais-worker-haiku-default` → deny; same payload with no agent_type or `relais:relais-worker-sonnet-default` → advise. Live: one haiku-worker relais run whose grep for a symbol → journal line with stance deny. (`backtest` uses `default_stance`, `backtest.rs:68-71`, and cannot see agent types.)
- After release, remeasure with `/tmp/lsp-measure/count_post_events.py` over one week: haiku-worker grep-for-symbol falls under 25/1k (retried greps counted; also report the share of refusals followed by an identical retry; if the target is missed, the decision for the person is whether to skip `first_telling` for `denyAgents` matches, making it a real block) with an error share at or under 5% (n >= 100 calls: about 125 a week, so one error is about 0.8 points); fable-reviewer `no_document` errors 7 -> 1 or fewer. If the haiku error share is above 7% over at least 100 calls, roll back: `git config --global --fixed-value --unset-all amont.agent.symbol-grep-not-lsp.denyAgents 'relais:relais-worker-haiku-*'`, and remove the line from the dotfiles template.

### Observed, Part B (amont-agent)
- Real `amont-agent hook` binary, scoped global config with `denyAgents = relais:relais-worker-haiku-*`, a Grep payload for a symbol: main and `relais-worker-sonnet-medium` are advised (additionalContext); the haiku worker is denied with "Refused for this agent type. Ask LSP: workspaceSymbol…"; the identical haiku retry in the same session passes (empty output, exit 0).
- `cargo test` guard unit tests: 761 passed. `tests/stance_scope.rs`: 17 passed, including a repository `.git/config` cannot set `denyAgents` and a global value does. `tests/hook.rs`: the four `a_denied_worker_*` tests pass (refused once and the retry runs; refused after its parent was advised; shell grep; unrecordable telling advised), and the symbol-grep family passes. Three `a_plan_lookup_*_is_cut_at_the_budget` tests failed once at load 30 and passed alone (3 passed, 6.8 s at load 16): the load flake their own comment describes.
- The live haiku-worker check (the payload's exact `agent_type`, a journal line with stance deny) needs the released binary and moves to the post-release remeasure below.
- Part A's observations are recorded in the fleet-lsp pointer plan.

## Decision log
- 2026-11-10 — the Deny remedy is a constant (`DENY_REMEDY`) chosen by the callers with `is_denial_by_agent(stance, source)`, not part of `phrase`, because the choice depends on where the stance came from, which `phrase` does not see.
- 2026-11-10 — `GrepOp` carries `agent` and `agent_id`, as `Bash` does; folding the id into `Agent::Named` is follow-up debt (rust profile).
- 2026-11-10 — deliberate: `grep_gate` returns an empty pattern for an observing stance, and no test yet shows that a plain configured `deny` speaks the ordinary remedy. The first is safe because `final_stance` never raises Observe, so `grep_telling` returns before keying on it; switch to an `Option` if that coupling changes. Both are follow-ups, to avoid a third review pass over latent-only risks.
- 2026-11-10 — the Part B relais run was interrupted at its 1200 s wall time under load 93-130 after writing 551 lines; salvaged by hand (docs, verification, commit) and the decision recorded as `salvaged`. Part A was blocked twice by contention (baseline, then admission wait) and is done by hand.

## Implementation review
- implementation-review: round 1 approve-with-changes (two blocking: swallowed lock error, `pats` name); delta approve-with-changes, all resolved, plan record written here (91k, 64 s; 40k, 49 s).
- backend: round 1 approve-with-changes (record the refusal after `final_stance`); delta all four resolved, two lows deliberate (28k, 29 s).
- lang:rust: round 1 approve-with-changes (carry the raise as a type); delta all five resolved, one low deliberate (45k, 49 s).
- unix: round 1 approve-with-changes; delta approve, empty-pattern low deliberate (58k, 45 s; 25k, 12 s).

## Non-goals
Denying for other types (sonnet workers fail the error gate; fable reviewers fail it too; Explore/general-purpose/fork never call LSP, cause still unknown); the tsserver SIGABRT (one incident, 11-01).

## Code review
- fleet-lsp: backend, lang:rust, unix (tui left out: one refusal sentence changes, no layout)
- amont-agent: backend, lang:rust, unix (tui left out: no output layout change, one config key)
- dotfiles: unix (one global git config line in the template)

<!-- panel: repos=fleet-lsp,amont-agent reviewers=backend,lang:rust,tui,unix body-sha=c08d95a21edf -->

