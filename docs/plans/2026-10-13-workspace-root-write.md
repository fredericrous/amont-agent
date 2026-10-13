---
status: active
branch: feat/workspace-root-write
repos: [amont-agent]
adrs: []
---

# amont-agent: deny writes into the workspace root, point to the current workspace

## Review panel

👉 **Decide:** none. Approve if refusing writes into `~/Developer/Perso`, with the remedy naming the linked worktree this session works in (or its scratch directory), is the guard you want.
📍 amont-agent · plan only · next: worktree + rule; remedy prefers a swept place. Panel: backend (opus-5-5), lang:rust (opus-5-5), tui (opus-5-5), unix (opus-5-5).
**Changed by review:** a replay gate before Deny (Advise otherwise); `~/` expanded, zsh/`>&file` forms, `.git` files, temp dirs never roots; `Context.command` so an Analysis rule can confirm.
📄 Full reviews: [2026-10-13-workspace-root-write.reviews.md](2026-10-13-workspace-root-write.reviews.md)
**Verdicts:** 4 approve-with-changes in round 1; backend approve on the final body after your delta (worktree-only remedy: check-ignore gate, submodules excluded, session scratch fallback); 2 rejected; 3 carried into implementation.

## Context

`~/Developer/Perso` is a workspace root: it is not a git repository, but it
contains about 100 of them. The Bash tool's cwd keeps drifting back to it, and
commands like `cmd > sb-dev.log 2>&1` then leave scratch files there. Today it
holds 128 loose files, 86 of them `*.log`/`*.err`. These files belong to no
repository: git never shows them, no sweep removes them, and nothing says which
task wrote them.

The new rule refuses an output redirect or `tee` target that resolves directly
into such a root. The refusal names the linked worktree the session is working in, or its scratch directory, so
the fix is a one-word path change. It sits next to `path-operand-missing`, uses
the same split (a pure `examine`, and a `confirm` that looks at the filesystem
through `Context::cwd_at`) and is phrased the same way. It is a separate rule
because the mechanism is different: a file that gets created, not a file that is
missing.

## Behaviour

- **Rule id:** `workspace-root-write`, with `default_stance: Deny` and
  `max_stance: Deny`.
- **Shape (`examine`, pure):** it uses `Examine::Analysis`, because only the
  analysis frontend reads `&>`/`&>>` (`src/analysis/frontend/parse.rs:1128`).
  The legacy lexer turns `&>` into a background `&`.
  - It flags write redirects (`>`, `>>`, `>|`, `N>`, `N>>`, `&>`, `&>>`) whose
    target is a literal word, on `SimpleCmd.redirects` and on
    `Cmd::Redirected` (so `{ …; } > x.log` and `done > x.log` count).
  - It flags the operands of `tee` (non-flag words, `sudo tee` included). The
    model for this is `stdin_hang.rs`.
  - It also flags the zsh forms `>!`, `>>!` and `<>`, and `>&file` /
    `>& file` (a file target in bash and zsh).
  - It skips `/dev/*`, fd duplications to a digit or `-` (`>&2`, `2>&1`,
    `>&-`) and unknown words. A word is known only when every part is
    `Lit`/`SingleQuoted` and it has no `$`, backtick, glob or `{`. This is
    a test on `analysis::ir::Word`, with the same meaning as
    `unknowable()` in `path_operand_missing.rs`.
  - An unquoted leading `~/` or bare `~` is resolved against `$HOME`,
    because agents write `> ~/Developer/Perso/x.log`. `~user` and a quoted
    `~` stay unknown.
- **Fact (`confirm`):**
  - `Context` gains `command: &'a str` (set from `bash.command` in `hook::confirmed`, empty in tests that need none). `Finding.span` covers the target word only; `confirm` re-reads that span through `analysis::analyze` to get the unquoted target. This is the first `Examine::Analysis` rule with a `confirm`.
  - `dir = parent(resolve(ctx.cwd_at(span.start), target))`.
  - `dir` is canonicalized first, so symlinks (such as `/tmp`) resolve
    before the `.git` walk.
  - It fires only when `dir` is a workspace root: no ancestor of `dir` (itself
    included) has `.git`, and at least two immediate children of `dir` do.
    `.git` counts as a file or a directory (worktrees, submodules). The scan
    stops at the second hit.
  - A path into a subdirectory such as `foo/bar.log` does not fire, and neither
    does a target anywhere inside a repository.
  - `std::env::temp_dir()`, `$CLAUDE_CODE_TMPDIR` and `/tmp` (canonicalised)
    are never roots, even when they hold worktrees: the remedy points there.
  - Anything else returns `No(..)` and is journalled as `unconfirmed`.
- **Pointing to the current workspace:** `confirm` reads `Context.transcript`
  backward, with a bounded tail of about 2 MB.
  - It looks for the latest tool input path (`file_path`, `path` or
    `notebook_path` on Read/Edit/Write/Grep/LSP) or Bash `git -C <p>` / `cd <p>`
    that lies in a git working tree. It keeps that path only when the tree is
    a linked worktree: `.git` is a file at its top whose `gitdir:` line ends in `/worktrees/<name>` (a submodule's points at `.git/modules/<name>` and is not one), wherever it lives: in the
    root (`homelab-wt-x`), under `.claude/worktrees/`, or in `/tmp`. Linked
    worktrees are what worktree-sweep removes, but only clean ones (`git status --porcelain` empty). So the worktree is named only when `git -C <wt> check-ignore -q <name>` succeeds: an untracked log would hold the sweep forever. A primary checkout (`.git` is a directory) is never named, because
    nothing sweeps it and the person works there.
  - When the session touched no linked worktree, the remedy names
    the session's scratch directory `${CLAUDE_CODE_TMPDIR:-/tmp}/claude-<uid>/<slug>/<session_id>/` only (absolute, built from the payload's `session_id` and `transcript_path`). `worktree-sweep --scratch` sweeps those UUID directories and nothing else in `/tmp`.
  - It returns `YesSaying { reason, remedy }`.
  - **Reason:** "`sb-dev.log` would land in `~/Developer/Perso`, a workspace
    root that is in no repository: git never shows the file and nothing sweeps
    it (128 files there today)." The count is read live and stays cheap
    because it uses `read_dir` with no recursion. It counts non-directory
    entries only.
  - **Remedy:** "This session works in the worktree `/Users/fredericrous/Developer/Perso/homelab-wt-x`: write
    `/Users/fredericrous/Developer/Perso/homelab-wt-x/sb-dev.log` (absolute, never `~`: quoted, it does not expand), or `<session scratch dir>/sb-dev.log`
    for a scratch log."
  - If the transcript finds no linked worktree, or cannot be read, the remedy
    names only the scratch directory.
  - The remedy ends: "If the file belongs in the root on purpose, ask the
    person." `docs/rules.md` names the demotion command.
  - The transcript is read only after the root fact is established. The
    tail read drops its first, possibly partial, line. The latest match by
    position wins. The fixture tests a cut line, two linked worktrees (latest wins), a primary checkout touched last (skipped) and a submodule (skipped).
  - Any I/O error returns `No`, and the command passes. The check costs at
    most one `read_dir` of the root plus one 2 MB read, and only when the
    shape matches.
- **Demotion:** one command, documented in the module comment the same way as
  `pipe-to-tail`:
  `git config --global amont.agent.workspace-root-write.stance observe`.

## Files

- `src/rules/workspace_root_write.rs` (new).
  - Hoist `resolve` out of `src/rules/path_operand_missing.rs` into
    `src/rules/mod.rs` as a `pub(crate)` helper. `unknowable` takes the legacy
    `shell::Word`, so this rule gets its own known-word test on
    `analysis::ir::Word` (see Shape).
  - Read the transcript with the existing reader in `src/transcript.rs` if one
    fits a single file's tail. Otherwise use a small backward line scan, as in
    `implementation_review::reviews`.
- `src/rules/mod.rs`: add `pub mod` and the `RULES` entry.
- `tests/corpus/workspace-root-write.cases`, plus its entry in the `EMBEDDED`
  table in `src/corpus.rs`.
- `docs/rules.md`: add a table row, and a paragraph under "Why only three of
  them deny".
- `CHANGELOG.md`: add the rule under `## Unreleased` / `### Added`.
- `Cargo.toml`: bump to 2.38.0.
- `docs/plans/2026-10-13-workspace-root-write.md`: this plan, which lands as the
  first commit.

## Evidence

The `per_1000` figure comes from running
`amont-agent backtest --rule workspace-root-write --since 2026-09-01` once the
rule is built. The backtester never runs `confirm` (`src/rules/mod.rs:158`), so that figure is the rate of redirects to a literal path, not the rate of writes into a root. Record it as such, and give the 86 files on disk as the measurement in `measured`. If the Phase 3 gate passes, then as
with `pipe-to-tail`, the rule ships at Deny straight away rather than walking
the observe → advise ladder. The 86 files already on disk are the evidence the
ladder would collect, and the refusal carries a one-word fix.

## Phases

- [x] 1. Worktree `feat/workspace-root-write` from `origin/main` (using
  `worktree-task`), with the plan as the first commit.
- [x] 2. Implement the rule, the helpers hoist and the tests.
  - Unit tests: `examine` shapes, including `&>`, `2>`, `tee -a`, `sudo tee`,
    `> /dev/null`, `2>&1`, `>&file`, `>!`, `cat <<EOF > x.log`, `$VAR.log`,
    `> ~/Developer/Perso/x.log`, `> "~/x.log"` (unknown, skipped),
    `{ a; b; } > x.log`, `> sub/x.log` (which still fires as a shape and is
    rejected by `confirm`).
  - `confirm` against a tempdir fixture: a root with two git children (one
    `.git` a file), a repo cwd, a nested path, a `cd repo && … > x.log` chain
    resolved through `cwd_at`, a symlinked root, and a temp dir with two git
    children that does not fire.
  - The transcript pick on a fixture JSONL.
- [x] 3. Backtest, then fill in `Evidence`. Gate: replay the built rule,
  `confirm` included, over the shape hits of the last 30 days that ran with
  the workspace root as cwd: take the hits from `backtest --json` and feed each one as a PreToolUse payload to the built binary with `cwd` set to the root, as in the end-to-end check. List each hit and its verdict in `Evidence`. If any hit is a file meant to stay in the root
  (not a scratch log or output), ship at Advise instead of Deny. Update the
  docs and CHANGELOG, then run `make check` and `make lint`.
- [ ] 4. Implementation review, a PR, then merge on green
  (`merge-when-green`).
- [ ] 5. Release 2.38.0 (`tag-release`), `brew upgrade amont-agent`, then
  observe it live. After one week, count the root's loose `*.log` and `*.err` files (86 now; it must not grow) and read the deny and `unconfirmed` counts from the journal.

## Decision log

- 2026-10-13: Phase 2 ran through relais (run-65db850448ff2-14756). Its reviewer was unavailable; the candidate passed `make check`, and was salvaged with one edit. The no-worktree remedy no longer adds "or in the repository the task is about", because a primary checkout is never named.
- 2026-10-13: the version bump moves to the release PR (`chore(release): 2.38.0`), the convention of every earlier release.
- 2026-10-13: the Phase 3 replay showed that `$HOME` qualifies as a root (`~/apisix-workshop` and `~/emsdk` are clones), so `>> ~/.netrc` would be refused. `$HOME` is now excluded like the temp directories, with a test.

## Phase 3 results

- Backtest since 2026-09-07: 9,856 shape hits in 83,733 calls (117.7 per 1000, flat over 6 weeks). The backtester does not run `confirm`.
- Replay of 3,357 distinct commands recorded with `~/Developer/Perso` as cwd, through the built binary, `confirm` included: 22 denied, 3,335 passed.
  - 18 `*.tmp.mjs` probes, `scratch-relay.log` (from `../../`): scratch, correctly refused.
  - `tsconfig.build.json` and `.env` (one each): artifacts of the replay. The transcript records the session's directory, not the shell's live cwd, and neither file exists in the root. Live payloads carry the real cwd.
  - `.aval19-dir`, written to `~`: the `$HOME` false positive, fixed (Decision log).
- Gate: no file meant to stay in the root was refused, so the rule ships at Deny.
- End to end on the built binary, in a tempdir root with two git children and this session's transcript: `echo x > probe.log` and `make &> build.log` were denied, naming this worktree (it ignores `*.log`). `tee -a notes.txt` was denied, naming the session scratch directory (`notes.txt` is not ignored). `cd a && … > probe.log`, `> /tmp/probe.log` and `$(cd a && echo x > y.log)` passed.

## Verification

- `make check` (fmt, clippy `-D warnings` with and without `test-hooks`, all
  tests) and `make lint` (`test_bump_tap.py`). The tap caveat picks the rule up
  automatically, because it is deny by default.
- **End to end on the built binary:** feed PreToolUse Bash payloads to
  `amont-agent` with `cwd` set to a tempdir workspace root.
  - `echo x > probe.log` is denied: a transcript whose last path is a linked worktree ignoring `*.log` names that worktree; a primary checkout, a submodule or a non-ignoring worktree touched last names the session scratch directory only.
  - `cd repoA && echo x > probe.log` and `echo x > /tmp/probe.log` pass.
- **Live check after install, in this session:**
  - `echo x > ~/Developer/Perso/probe.log` is refused and names this session's scratch directory (it touched no linked worktree).
  - `echo x > ~/Developer/Perso/amont-agent/probe.log` passes. Remove that
    probe file afterwards.

<!-- panel: repos=amont-agent reviewers=backend,lang:rust,tui,unix body-sha=d3a6781d2312 -->
