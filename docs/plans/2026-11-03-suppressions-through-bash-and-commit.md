---
status: active
branch: feat/suppressions-gate
repos: [amont-agent, decisions, every repo with an amont.conf except happier]
adrs: [decisions:ADR-0011]
---
# Suppressions: through Bash, and at commit

## Review panel
👉 **Decide:** none — approve if a reasonless-only commit gate plus a post-Bash check closes the Bash and non-Claude-Code routes.
📍 amont-agent, decisions, 25 fleet repos · plan reviewed · next: a worktree, Phases 1–3, release, decisions PR, then fleet. Panel: backend, lang:rust, tui, unix (claude-opus-5-5), codex.
**Changed by review:** `GIT_INDEX_FILE` kept so `commit -a` is checked; the override is command-scoped only and its every channel is refused; the pack scope is `*` (amont refuses `tsconfig*.json`).
**Verdicts:** round 1 4× approve-with-changes; round 2 backend approve; codex rework (1 blocking, 2 high applied; backend approve-with-changes on them). 3 carried into implementation.
📄 Full reviews: [2026-11-03-suppressions-through-bash-and-commit.reviews.md](2026-11-03-suppressions-through-bash-and-commit.reviews.md)

## Context

The `lint-suppression-added` rule (2026-10-06, deny on this machine since about 10-25) refuses an Edit or Write that adds a lint suppression. Since the deny, 15 such Edit/Write calls were attempted and 15 were refused. While it only advised (10-06 → 10-19), 32 got through.

The original plan made Bash writes a non-goal: "the transcripts show these edits go through Edit and Write". Since the deny, that has stopped being true.
- **grh, 2026-10-29.** After clippy failed with "never used", the session ran `sed -i 's/^mod exit;$/#[allow(dead_code)]\nmod exit;/' src/main.rs`. The line landed in commit `7952647`.
  - In the same session, a subagent had been denied three times trying the same thing through Edit.
  - Files written with `cat > src/….rs <<EOF` brought in `#[allow(clippy::too_many_arguments)]`.
- **happier, 2026-10-25.** A reasonless `exhaustive-deps` disable landed in `50d34efa35`.
- **homelab, 2026-11-03.** `# shellcheck disable=SC1091/SC2086` landed. These carry reasons, but the scanner never looks at shell files.
- **Coverage.** The hook only fires inside Claude Code. Commits from web sessions, from a relais runner that is not Claude Code, or from the person are not covered at all.

The person chose two layers ("1", 2026-11-03):
- **After a Bash call:** compare the repo's working tree with its index.
- **At commit:** an amont pre-commit gate compares each staged file with HEAD.

Both reuse the rule's existing scanner.

Gate policy, as the person decided:
- A specific marker with a reason on its line passes.
- A bare marker blocks.
- A blanket marker blocks.
- A loosened lint configuration blocks.

The person commits those last cases themselves with `git -c amont.agent.suppressions=allow commit`, and the hook refuses that override from agents.

Rollout, as the person decided: ship an `amont.pack`, and adopt it in every repo with an `amont.conf` except happier, a fork under heavy refactoring.

## Non-goals

- **The Edit/Write rule does not change.** It still denies every added marker, with or without a reason. The reasoned exception is the commit gate's.
- **No judgement of reason text.** A reason is any non-empty text in the tool's own reason position.
- **Bash commands whose text names no marker** are not caught after the call. An example is `python3 /tmp/fix.py` running a script written earlier. The commit gate catches them.
- **No shellcheck inside YAML `run:` blocks.**
- **No new forms beyond shell.** No `biome-ignore` or `oxlint`: the fleet uses neither (checked when the scanner is extended).
- **Disguised workarounds are left to review.** An example is `cast(...)` replacing a `# type: ignore`.
- **NotebookEdit stays out**, as before.

## Behaviour

### Shared: what a change adds (`crates/amont-agent-guard/src/rules/lint_suppression_added.rs`)

- **`Shell` file kind.** It covers `.sh`, `.bash`, `.zsh`, and files without an extension whose first line is a `sh`/`bash`/`zsh` shebang.
  - The marker is `# shellcheck disable=SC1091,SC2086`, one marker per code.
  - `worth_rebuilding` keyword: `shellcheck`.
- **`net_added(changes: &[(old_path, before, new_path, after)]) -> Vec<(path, Hit)>`.** Today's comparison works on one file. Classify each side using its own pathname and content, then sum marker counts over every file in the change; the key leaves the path out. Added and deleted sides are absent, not classified using the opposite side's pathname. Report additions at `new_path`. The single-file wrapper supplies the same path for both sides.
  - A marker moved from one file to another cancels out. That covers trade-agents `45f3dc7a`, where 26 `# type: ignore[arg-type]` lines moved between packages.
  - Hits are reported in path order, then line order.
  - Known approximation: removing a `noqa: E501` in one file lets another file add one. The key includes whether the hit is reasoned, so removing a reasoned marker never cancels an added bare one.
  - `examine_change` stays as the single-file case of `net_added`.
- **`reasoned(&Hit, line) -> bool`.** A marker passes only in its tool's own reason form:

| Form | Reason position |
|---|---|
| `eslint-disable*` with rule names | ` -- <text>` |
| `@ts-expect-error` | text after the tag |
| `#[allow(..)]` / `#[expect(..)]` | `reason = "<non-empty>"` |
| `noqa: CODE`, `type: ignore[code]`, `pyright: ignore[rule]` | ` -- <text>`, or a following `# <text>` |
| `//nolint:linter` | ` // <text>` |
| `# shellcheck disable=SCnnnn` | a following ` # <text>` |

- **Never reasoned, so always blocking:**
  - a marker with no code: `/* eslint-disable */`, `# noqa`, `# type: ignore`, `//nolint`;
  - `@ts-ignore` and `@ts-nocheck`;
  - file-level `# ruff: noqa` and `# pyright: basic/off/reportX=none`;
  - every config `Setting`.
- **Python port.** `tools/suppression-rate.py` keeps in step: shell kind, `net_added` and `reasoned`. `tests/fixtures/suppressions.txt` gains tagged rows for each, so the Rust unit test and `--self-test` agree.

### Layer 1: after a Bash call (`src/hook.rs`, `on_post_bash`)

1. **Prefilter.** It runs from both `on_post_bash` and `on_post_bash_failure` (a failing `sed -i … && cargo clippy` still wrote the file). It runs before any process starts, with no git: the lowercased command must name a marker keyword, meaning the union of the `worth_rebuilding` lists plus `shellcheck`, or a config filename from `file_kind`. Otherwise the path stays as today, one lex and no processes.
   - The prefilter also skips detached calls and `Parsed::Opaque` commands.
2. **Repo.** `rev-parse --show-toplevel` in `bash.cwd`. Then `status --porcelain=v1 -z --untracked-files=all`.
   - Both use `git::stdout_bounded` / `bytes_bounded` under one shared 1 s deadline, like `ADR_LINT_DEADLINE`.
   - Only examined file kinds are kept, at most 50 files, each under the rule's `MAX_READ`.
   - The "before" text is the index blob, read with one new `git::cat_file_batch_bounded` (`:path`), built on `bytes_bounded` so the 1 s deadline covers it too; `cat_file_batch` (via `bytes_in`) has no deadline. An untracked file has an empty "before". The "after" text is the working tree.
3. **`net_added` over those files.** Hits already told in this session are dropped. They are recorded in `session_state` as (repo, path, marker key, ordinal), so each hit is told once. A suppression left uncommitted from earlier work is told once too, in wording that does not claim this command typed it.
4. **Stance** comes from the same `lint-suppression-added` rule. The journal row has mode `bash`.
   - Observe is silent.
   - Advise and deny return `Decision::Assert`, the PostToolUse block reply:

     `amont-agent/lint-suppression-added: the working tree of <repo> now holds N lint suppression(s) the index does not:`, then one `  <path>:<line>: <marker>` line per hit, then `REMEDY_MARKER` or `REMEDY_CONFIG`.

     The hook cannot tell which command typed a hit, so the text does not claim that this one did.
5. **Fail open.** A timeout, a git failure or a cap overflow is journaled as `bash-slow`, `bash-git` or `bash-cap`, and the hook stays silent. The commit gate is the backstop.

### Layer 2: `amont-agent suppressions --staged` (new subcommand, `src/main.rs`, `src/suppressions.rs`)

1. **Read the staged change.** `git diff --cached --name-status -z -M --diff-filter=ACMRD`, retaining both rename paths and keeping entries where either side is an examined kind. Classify each side independently, including content-based shell detection. A deleted file has an empty "after", so its markers count as removed and a package moved as delete+add cancels like a rename. Test a `.txt` containing a bare Python suppression renamed to `.py` (blocks), the reverse rename (passes), and a same-kind move (cancels).
   - "before" is `HEAD:<old path>`, empty for an added file or on an unborn HEAD.
   - "after" is `:<path>`, the staged blob, never the working tree.
   - `HEAD:<old path>` is requested only for M, R and D entries. A requested name that git reports missing exits 2, and a test pins it.
   - Sizes come first, from one `cat-file --batch-check`. A blob over 4 MiB is never read: it is named on stderr, and the pass line ends `, S skipped (over 4 MiB)`. Before reading any remaining content, sum the sizes of every requested before/after blob, counting repeated requests. A total over 64 MiB exits 2 with `suppressions: scan budget exceeded (64 MiB); scan incomplete` on stderr. Otherwise read the batch with an enforced output-byte ceiling covering payloads and protocol headers; an overflow or incomplete response exits 2. Test many individually sub-4-MiB blobs just below and above the cumulative budget, verify rejection precedes content reads, and record peak RSS.
   - `GIT_INDEX_FILE`: git hands the hook a temporary index on `commit -a`, `commit <path>` and `--only`. `main()` scrubs that variable (`src/main.rs:339`), so `suppressions` captures it before the scrub and passes it to its git children.
2. **Judge.** `net_added`, then:
   - a reasoned hit is listed as `kept (reasoned)` and passes;
   - any other hit blocks.
3. **Output** (`guidance.gate-output-is-failures`). A pass is one line: `suppressions: N files, 0 added (K kept with a reason)`. The pass line and the hit lines go to stdout. Skips, git errors and the override notice go to stderr. A failure prints:
   - every blocking hit, never cut off at the hook's `SHOWN` of 3, one per line as `path:line: <marker>: <fix>`. `<fix>` is the reason form from the table (`add reason = "…"`, `add -- <why>`), or `blanket, never passes`. A test pins 4 or more hits, all printed;
   - the agent's next step: `fix the cause, or stop and tell the person`;
   - last, under `for the person only (an agent's attempt is refused):`, `git -c amont.agent.suppressions=allow commit`.

   Exit codes:
   - 0: clean, or only reasoned hits;
   - 1: a blocking hit;
   - 2: a usage error or a git failure (fail closed). stderr: `suppressions: <git command> failed: <first stderr line>`. The override is read first, so `allow` still lets the person through.
4. **Override.** Only a command-scoped `allow` counts: the gate reads it from `GIT_CONFIG_PARAMETERS` or `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_<n>`. A value in repo or global config is ignored and named on stderr. A command-scoped `allow` prints what it lets through and exits 0. A `git -c` override reaches the gate through `GIT_CONFIG_PARAMETERS`, which `main()`'s env scrub (`src/main.rs:336`) leaves alone. A test pins this.
5. **Pack.** A new `amont.pack` at the amont-agent root:
   ```
   # amont-agent: the lint suppressions a commit adds (general.no-disabled-safety)
   pre-commit  suppressions  *  block  amont-agent suppressions --staged
   ```
   - It blocks from day one: the gate judges only what a commit adds, so no repo has a backlog.
   - Scope `*` uses amont's supported grammar; the gate filters examined kinds internally, including extensionless shell scripts identified by their staged shebang. Add an integration test that vendors this exact pack into a scratch repo, verifies the declaration is runnable, and commits bare suppressions in a source file, an extensionless shell script, and a config-only change. Record the no-relevant-files gate latency.
   - Rollback: the gate follows the `lint-suppression-added` stance.
     - Advise is the rule's default, so it applies on every machine with no stance set, relais runners included. Advise and deny both exit 1 on a blocking hit.
     - Observe prints its hits and exits 0, so one stance change stops the gate on a machine without 25 PRs.
     - A test pins all three stances, including an empty git config (advise, so exit 1).
   - Where `amont-agent` is not on PATH, amont reports the command as a non-blocking gap (`amont/docs/custom-checks.md:192-203`). Phase 6 checks that every machine that commits has it, the relais runners included.

### Layer 2b: the override stays the person's (new rule `suppressions-override`)

- **What it refuses.** A PreToolUse Bash rule that denies an agent command when either of these appears in a `git` invocation or in the environment:
  - `-c amont.agent.suppressions=…`, `amont.agent.suppressions` set through `git config`, or `GIT_CONFIG_KEY_<n>=amont.agent.suppressions` or `GIT_CONFIG_PARAMETERS` carrying it in the environment;
  - `hook.skip` naming `suppressions` or `pre-commit-suppressions`;
  - an `amont.severity` setting that lowers `pre-commit-suppressions` to `warn`;
  - a write of `amont.agent.lint-suppression-added.stance` (the rollback switch), by `git config` or `GIT_CONFIG_*`.

  An agent editing a config file with Edit/Write is not covered: it is left to review and journaled as a known gap.
- **Stance.** `default_stance: Deny`, `max_stance: Deny`. The switch exists for the person only (ADR-0011: loosening "needs … the person's agreement"), so it cannot soak in observe mode.
- **Deny text.** `amont-agent/suppressions-override: this override is the person's (general.no-disabled-safety). Fix the finding, or stop and tell the person which hits the gate printed.`
- **Not refused.** `hook.skip=pre-commit` as a whole stays allowed: the person approved it for config-only commits, and it is a different decision.

## Phases

1. **Scanner** (amont-agent).
   - Shell kind, `net_added`, `reasoned`.
   - Unit tests for every table row and for moves cancelling.
   - Python port, fixture rows, `--self-test` green.
2. **Commit gate.**
   - The `suppressions --staged` subcommand: bump `SUBCOMMANDS`, add usage and `--help`.
   - `tests/suppressions.rs` with scratch repos, covering:
     - added, reasoned, moved, renamed, unborn HEAD;
     - staged vs working tree differing;
     - `git commit -a` and `git commit <path>` (temporary index in `GIT_INDEX_FILE`);
     - a deleted file whose markers moved to an added one;
     - a missing `HEAD:<path>` answer (exit 2);
     - a 50 MiB staged `.js` (not read; peak memory recorded);
     - the `-c` override through a real `git commit` with an amont-style hook;
     - exit codes.
   - `amont.pack`; amont-agent's own `amont.conf` vendors it only after the Phase 7 decisions PR has merged (ADR-0044: no blocking lint before the rule body states it).
3. **After-Bash path and the override rule.**
   - `on_post_bash` prefilter → status → `net_added` → once-per-session → Assert.
   - The `suppressions-override` rule.
   - `tests/hook.rs` cases:
     - the grh `sed -i` shape is asserted;
     - `cat > x.rs <<EOF` with `#[allow]` is asserted;
     - a told hit is not told twice;
     - a command naming no keyword spawns nothing (checked with a fake `git` on PATH that records calls);
     - observe stance;
     - a timeout fails open, including a fake `git` that sleeps in `cat-file`;
     - the override is denied through each channel: `-c`, `git config`, `GIT_CONFIG_KEY_<n>`, `hook.skip=…suppressions`, an `amont.severity` lowering, and a stance write; `hook.skip=pre-commit` is allowed.
4. **Backtest and cost.**
   - Transcripts: the share of Bash calls passing the prefilter, and the post-Bash p50/p95 on the largest worktrees (trade-agents, relais).
   - Git: replay `suppressions --staged` on every non-merge commit since 2026-10-25 in the fleet repos (each commit's parent tree against its tree) and compare with the expected table under Verification.
   - Record the false positives and fix them before release.
5. **Release.**
   - Prerequisite: the producer's separate minor release, amont >= 1.70.0, is released before any consumer deployment or pack adoption (Phases 5 and 6); this plan does not claim it released or installed.
   - Through the `tag-release` skill: CHANGELOG, minor bump, brew upgrade, `amont-agent doctor`.
   - Live check in a scratch worktree: the post-Bash Assert fires on a `sed -i` adding `#[allow(dead_code)]`, the commit is blocked, and `-c amont.agent.suppressions=allow` from an agent is denied.
6. **Fleet adoption**, after the decisions PR from Phase 7 has merged. Every repo with an `amont.conf`, except happier.
   - Before enabling the pack, on every machine that commits, relais runners included: check `amont --version` is >= 1.70.0 AND `amont-agent` is available on PATH at the released version.
   - The repos: activity-vision, adr-lint, amont, application-landscape, authelia-oidc-operator, aval, cluster-vision, customer-vision, ddns-updater-operator, decisions, duro-app, duro-design-system, duro-lexical-multi, fleet-lsp, grh, homelab, homelab-preview-operator, kb-vision, relais, social-planner, sre-agent, stealth-fetch, ticket-vision, trade-agents, website-builder.
   - Per repo:
     1. A worktree from `origin/main`: `amont add github:fredericrous/amont-agent`, then `amont agents-md`.
     2. Commit only the explicit paths. The pack commit is config-only, so it uses the approved `git -c hook.skip=pre-commit` where a repo's own pre-commit hook needs it.
     3. PR, then merge on green.
     4. `amont trust` on the live checkout after it fast-forwards.
   - duro-operator: left alone while its live checkout holds uncommitted work. Recorded, not forced.
7. **Close.**
   - decisions, `docs/principles/clean-code.md`, one PR with a pack refresh where it is vendored. This lands BEFORE Phase 6: ADR-0044 (`guidance.code-rules-checked-by-fit`) lets a lint block a commit only where the rule body states it.
     - `general.no-disabled-safety`'s **Checked by:** gains `amont-agent suppressions --staged` for an added bare or blanket marker and a loosened config, at commit.
     - `general.linters-police-suppressions`'s last sentence names that gate beside the agent hook.
   - The amont-agent plan record and the original plan's non-goal amended to point here.
   - Memory note; measure DUE 2026-11-17.

## Code review

- amont-agent: backend, lang:rust, unix (tui left out: the one new subcommand prints `path:line: marker` lines in the style `adr-lint check` and `amont-agent check` already use, with no interactive output)
- decisions: none — one line of markdown
- fleet repos (Phase 6): none — vendored `amont.conf` pack block and regenerated AGENTS.md only

## Decision log

- 2026-11-03: the person chose both layers (option 1), "reasonless only" at commit, and the whole fleet except happier.
- 2026-11-03: Layer 1 compares the working tree with HEAD, not the index (deviation from the reviewed text). Found in the fails-first probe: `sed -i … && git add` stages the suppression, so an index comparison saw nothing. Its message reads "… its last commit does not:". Test `a_suppression_staged_by_the_same_command_is_still_asserted`.
- 2026-11-03: the Rust scanner reads an attribute through its closing `]`, across lines (0f82017). Phase 1 read `#[expect(\n lint,\n reason = "…"\n)]` line by line as a bare `expect` with no code, so the commit gate would have blocked every rustfmt-wrapped reasoned one; relais's review missed it.
- 2026-11-03: `suppressions.rs` joins the CLI-verb modules the stdout guard lists (`tests/hook.rs`), beside `open.rs`; the worker had routed around the guard with `write_fmt` alone.
- 2026-11-05: the person requested the amont index-handoff fix. The amont-owning session implements the producer; this branch opts into its absolute `AMONT_INDEX_FILE` without restoring Git's repository environment globally. An unset or empty hint retains the direct-hook fallback. Invalid hints follow the existing typed-error, person-override and observe paths. Consumer commit `61a49e252cb880b0fef54748eaec53148f1e3ae3` came through relais; the orchestrator approved its verified candidate with one nonblocking CHANGELOG wrapping nit, not a new human approval.

## Verification

| Check | Expected | Actual |
| --- | --- | --- |
| fails first | the new hook and gate tests run against 3.16.0: every assert and block case fails; the allow cases pass trivially | 3.16.0 has no `suppressions` verb (exit 2) and its hook is silent after the grh `sed -i`; the branch blocks it (exit 1) and asserts `main.rs:1: #[allow(dead_code)]`. The staged case failed on the branch's first (index) comparison and passes after 0782050 |
| grh `7952647` replay | blocked: `#[allow(dead_code)]` at `src/main.rs` | blocked: `src/main.rs:10` and `:13` `#[allow(dead_code)]: add reason = "…"`; grh `9c19b22` and `76fefe4` also blocked on `src/fetch.rs:117` `too_many_arguments` |
| happier `50d34efa35` replay | blocked, reasonless `exhaustive-deps` (replay only; happier does not adopt) | blocked: `useSessionDetailsPaneSync.ts:49` `eslint-disable-next-line react-hooks/exhaustive-deps: add -- <why>` |
| trade-agents `45f3dc7a` replay | its 26 moved `type: ignore[arg-type]` cancel. Any surplus is reported, and each one is checked against the parent tree | the 26 moved `type: ignore[arg-type]` cancel; 1 surplus blocked, a NEW `# pyright: reportArgumentType=none` header (parent has 2 such headers, the commit 3). True positive; it is named at `test_worker_execute.py:19`, the first surplus occurrence in path order (the plan's known approximation), while the added line is in `test_registry_store.py` |
| duro-design-system `66b1905e` replay | the reasoned `@ts-expect-error` lines are kept | passed (the reasoned `@ts-expect-error` lines kept) |
| homelab `d6bef10ed` replay | the shellcheck disables with ` # reason` are kept; the bare `# shellcheck disable=SC2086` in the workflow YAML is out of scope (YAML) | passed (reasoned shellcheck disables kept; YAML not examined) |
| relais `6f9fbee9` replay | the `#[expect(..., reason=…)]` conversions are kept, or cancel against the removed `#[allow]`s | passed (wrapped `#[expect(…, reason = …)]` kept; this needed 0f82017) |
| no-keyword Bash | the fake `git` records 0 calls | `a_command_naming_no_keyword_spawns_no_git`: the fake `git` records 0 calls |
| cost | the prefilter-pass share and post-Bash p95 are recorded; on a pass, p95 is under 300 ms on trade-agents. If it is over, the person decides before release | prefilter passes 2,394 of 38,037 Bash calls since 10-20 (6.3%). On a pass, 20 runs on a fresh worktree under a loaded machine: trade-agents (1,289 files) p50 232 ms, p95 266 ms; relais (596 files) p50 253 ms, p95 301 ms; no keyword: 13 ms and 11 ms. Under the 300 ms bar on trade-agents |
| suite | `cargo test --workspace` exits 0 and lists the new hook and suppressions tests; clippy `-D warnings` exits 0; `tools/suppression-rate.py --self-test` reports 0 rows that disagree with the Rust test | the commit gate (`make check`: cargo test --workspace, clippy -D warnings, fmt, the suppression-rate self-test, 26 checks) passed on every commit; `--self-test`: 100 samples agree |
| live | the Phase 5 scratch check, observed after brew upgrade | Phase 5, after release: not in this PR |
| fleet | `amont list --json` in each adopted live checkout lists `pre-commit-suppressions` at block, trusted | Phase 6: not in this PR |
| measure, 2026-11-17 | the `net.py`-style git scan over the fleet since release shows 0 reasonless net-new markers landed; the post-Bash journal shows fires and whether the next call removed the marker | DUE 2026-11-17 |
| replay, whole fleet | known true positives only; moves and reasoned forms pass; no scan errors or identified false positives | Original replay (before the final binary): 7,376 non-merge commits since 2026-10-25 across 27 repositories: 7,371 pass, 5 block (above), 0 errors; 0 false positives. New default-branch-only replay (also before the final binary): 675 non-root changes in 27 repos: 672 pass, 3 known true-positive blocks, 0 scan errors; 1 root skipped; 205 s |
| review fixes | decisions PR merged; review-fix commit recorded | decisions PR https://github.com/fredericrous/decisions/pull/152 merged (72189bfb33e121af87c1cd7a4d9e374419b010f6). Review-fix commit 525bd598bddbedeec72d064ab787c57b072d975c. The orchestrator's salvage decision is recorded as made by the orchestrator; no new human approval is claimed |
| make check at 525bd59 | the gate tests and workspace checks pass | tests/suppressions.rs 18 passed; tests/suppressions_after_bash.rs 10 passed; tests/suppressions_override.rs 9 passed; tests/rules_json.rs 5 passed; full workspace tests, fmt and clippy passed (run-65f887bf44ce7-d523/logs/attempt1-cmd0.log); pre-commit passed at 525bd59. Python `--self-test` passed again after that commit |
| real integration, installed amont 1.69.0 | exact local pack: source `x.rs`, extensionless `bin/x` and a tsconfig-only commit each blocked | Observed with the installed amont 1.69.0 and the exact local pack (not the unpublished remote `amont add`): all three commits blocked, exit 1, HEAD unchanged, the hit stdout visible in the gate logs. Historical release blocker (superseded by the branch-binary probe below): a subdirectory `git commit -a` unexpectedly succeeded and the gate printed 0 files. Follow-up diagnosis reproduced it in primary and linked worktrees: amont's external-check `execute` calls `strip_git_env` (`amont-runtime/src/manifest.rs:989`, `hooks/common.rs:689`), removing `GIT_INDEX_FILE`; a wrapper observed `IDX=unset`, so the gate read the ordinary index rather than git's temporary one. A handwritten hook calling the same binary blocks. Repeating the faulty check also reuses its false clean stamp. Evidence: run-65f89214c1245-ca85/attempt-2-result.txt; that inspection's requested full-suite rerun failed three child-reaping timing tests, whose focused rerun passed, so its overall outcome is failed, not accepted. The source-level fix and stale-stamp upgrade probe below supersede this blocker. The installed runner has not yet been upgraded, and no release or fleet adoption is claimed |
| measurements, earlier recovered debug binary (not the final binary) | cost and memory bounds recorded | Evidence directory `attestations/suppressions-2026-11-05`. Docs-only, 20 scans: median 58.6 ms, p95 66.5 ms. 50 MiB skip: RSS 3.89 MiB. 60 MiB total: RSS 130.90 MiB. 66 MiB: exits 2 before reading content, RSS 3.90 MiB. These are earlier measurements, superseded for the current source by the row below |
| index-handoff consumer | precedence, fallback, invalid hints, override and full checks pass | `61a49e2`: `tests/suppressions.rs` 23 passed; full `make check` exited 0 on the final candidate (`run-65f89e2e5d604-12574/logs/attempt2-cmd0.log`), and the commit's 26 checks passed. The orchestrator approved the native review's sole remaining finding as a nonblocking cosmetic CHANGELOG wrapping nit; no check was waived. This decision produces an accepted outcome with no receipt file, not a native automatic-accept receipt |
| real index handoff, both branch binaries | exact unmarked pack judges the index being committed in primary and linked worktrees | Passed with consumer `61a49e2` and the amont producer branch's debug binary, not installed releases: source, extensionless shell and config-only bare additions blocked; subdirectory `commit -a` blocked in both worktree types, HEAD unchanged. `commit <path>` and `--only` bad path blocked in both (4); reasoned `--only` passed (2); `--only README.md` correctly excluded an unrelated staged bare suppression (2). Evidence: `attestations/suppressions-2026-11-05/final-handoff.uh8bOB/{probe-results,commit-paths}.json` |
| old false-clean stamp, same-path binary upgrade | a pass recorded by the broken boundary cannot bypass the corrected consumer | Old amont 1.69.0 plus the old scanner accepted the bad temporary-index tree; a repeated attempt printed “passed on this exact tree earlier”. Atomically replacing both binaries at those same fixture paths then rejected the same tree, exit 1 at `x.rs:1`, HEAD unchanged, no cache reuse. Before/after binary sizes, mtimes and SHA-256s are recorded. Evidence: `attestations/suppressions-2026-11-05/index-upgrade-0y0ub7za/{before,old-stamp-reuse,after}.json` |
| current source debug binary, resource probe | record latency and memory; reject over-budget scans before content reads | Consumer `61a49e2`, same real-pack probe above: docs-only 20 scans p50 53.9 ms / p95 63.1 ms; 50 MiB script skipped, RSS 3.50 MiB; 60 MiB total passed, RSS 155.07 MiB; 66 MiB exited 2, RSS 3.91 MiB. These are current-source debug-binary measurements, not published release-artifact measurements |

<!-- panel: repos=amont-agent,decisions reviewers=backend,lang:rust,tui,unix,codex body-sha=81f46293160c -->
