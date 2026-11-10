# Full reviews: call-order error, haiku deny

## Full reviews (reference)
- **unix**, round 1: approve-with-changes (28k, 33 s). Merged global and system lists; idempotent rollout and exact rollback; ignore empty or bare `*` values. Round 2: all resolved; low (rollback exit 5 means already absent) carried.
- **backend**, round 1: approve-with-changes (39k, 63 s), 7 findings (A1 vs test (2); dead clear; `backtest` blind to agent type; verification not observable; numeric trigger; dotfiles review line; `grep_gate` test). Round 2: approve-with-changes (28k, 20 s), refuse-once must run through the plan. Re-run: 3 findings (32k, 60 s), Unrecorded telling on a Deny, wrong Bash site, Grep-tool retry not journalled. Final body: **approve** (29k, 21 s).
- **lang:rust**, round 1: approve-with-changes (43k, 63 s), 5 findings. Round 2: approve-with-changes (23k, 17 s), all 5 resolved; `no_repository` "new" rejected (`workspace.rs:672`); git >= 2.37 note and retry count carried.
- **tui**, round 1: approve-with-changes (37k, 82 s), 4 findings (A1 vs test (2); deny remedy text; file line 18; rollback). Round 2: approve-with-changes (33k, 55 s), all resolved; per-agent keying applied, false clause fixed.
- **codex**: skipped, it sends the plan to an outside service with the sandbox off; the person has not asked for it.
- **Carried into implementation:** `first_telling` needs a third result for Unrecorded; a Named agent with no `agent_id` needs a defined key (journal `unrecorded`, as `refuse_once_at`, `hook.rs:1913-1923`); rollback `|| [ $? -eq 5 ]`; git >= 2.37 for `--fixed-value`.
