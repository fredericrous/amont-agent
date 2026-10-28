# The rules

| rule | ships as | what it catches |
|---|---|---|
| `pipe-to-tail` | `deny` | a mutating command whose status is swallowed by a pipe |
| `bare-stash-pop` | `observe` | `git stash pop` with no ref, where `refs/stash` is shared across worktrees |
| `cd-carries-over` | `advise` | a Bash call that runs in a directory an earlier call of its own parallel batch moved the shell to with a top-level `cd` or `pushd` (the shell is no longer where the batch's first call ran, and the harness does not announce a carry between siblings); use `(cd X && …)`, `git -C X` or absolute paths |
| `gh-pr-merge-auto` | `observe` | `--auto` on a repository with no required checks, which merges immediately |
| `publish-without-skill` | `advise` | a `v*` tag pushed or a pull request merged with no call of the `tag-release` / `merge-when-green` skill in this turn or the one before, so the procedure ran from memory |
| `forge-merge-by-hand` | `advise` | merging a pull request by POSTing to the forge's merge endpoint, which answers `200` whether the checks passed, failed or never started |
| `no-verify` | `observe` | turning the whole commit gate off rather than one check |
| `git-add-broad` | `observe` | staging the tree instead of the change |
| `stale-base` | `advise` | a branch or worktree started from a checkout the remote has moved past |
| `push-preflight` | `advise` | a `git push` whose slow pre-push test gate has not been rehearsed with `amont rehearse --wait` |
| `push-preview` | `advise` | a push that would publish interface changes no approved localhost preview covers, unless the plan the person approved declares the evidence is enough ([preview approval](preview.md)) |
| `plan-review-panel` | `deny` | a plan presented at `ExitPlanMode` before its expert review panel ran ([the review panel](plan-review.md)) |
| `plan-phases-open` | `deny` | a turn ending while the plan this branch carries still has an open phase that is not a `🧑 decision:` — the agent is sent on to it |
| `implementation-review` | `advise` | a push of a branch that carries a plan, whose diff no independent reviewer has read for the tree being pushed ([the implementation review](implementation-review.md)) |
| `foreground-poll` | `advise` | a polling loop or `gh run watch` in the foreground, where the tool's ten-minute clock will kill it one poll short |
| `unbounded-background-push` | `advise` | a `git push` sent to the background with its output kept in a file and no `timeout`, so the pre-push gate inside it can run for as long as it likes with nothing ending it or reporting it |
| `gui-open-unbounded` | `advise` | a macOS app launch (`open -a <App>`, `open <x>.app`) with no `timeout`, sent in the background; silent on platforms where `open` is not a launcher: measured to hang twice on 2026-10-14, and the chain after it never ran |
| `sed-in-place` | `advise` | `sed -i` spelled for the other sed (`-i ''` on GNU, bare `-i` on BSD) |
| `kubectl-gitops` | `advise` | an imperative `kubectl` write in a repository Flux or Argo reconciles |
| `tag-after-commit` | `advise` | `git tag` chained onto a `git commit` that a hook may have refused |
| `release-tag-push` | `observe` | pushing a `v*` tag, which publishes: the tag may name the wrong commit, and a green workflow does not mean the artefact is right |
| `worktree-remove-force` | `advise` | `git worktree remove --force` on a worktree that still holds uncommitted work |
| `amend-pushed` | `advise` | `git commit --amend` on a commit the remote already has |
| `branch-force-delete` | `observe` | `git branch -D` on a branch whose commits are on no remote and not merged |
| `worktree-isolation` | `observe` | a branch created, or `git reset --hard`, in the primary checkout of a repository that already has linked worktrees |
| `stdin-hang` | `observe` | a command that will read standard input with nothing on it — `cat > file`, a bare interpreter, `tee` outside a pipe — which blocks silently until the tool's clock runs out |
| `glob-in-flag-value` | `advise` | an unquoted glob inside a flag value (`--include=*.ts`), which zsh expands — or fails on — before the program sees it |
| `glob-no-match` | `advise` | an unquoted glob operand that matches nothing, which under zsh aborts the clause before it starts while a later clause reports success |
| `equals-separator` | `observe` | a bare word beginning with `=` (`echo ===`, `[ x == y ]`), which zsh reads as a command lookup and whose failure aborts the whole command list |
| `unsplit-expansion` | `observe` | an unquoted `$var` under `set --` or `for … in`, which zsh leaves as one word: the positionals or the loop get the whole string and the command runs wrong while reporting success |
| `path-operand-missing` | `advise` | a read of a path that is not there — under the grep shim a warning in mid-stream, for the coreutils one line and carry on — which the chain then reports as success |
| `workspace-root-write` | `deny` | an output redirect or `tee` target that lands directly in a workspace root (a directory in no git tree that holds several repositories), where the file belongs to no repository, is never shown by git and is never swept; the refusal names the linked worktree this session works in, or its scratch directory |
| `stat-bsd-format` | `advise` | `stat -f '%…'` on GNU stat (or `-c` on BSD), which prints a filesystem report where a timestamp was wanted |
| `whole-file-dump` | `advise` | a file poured whole into the tool result by `cat`/`sed -n`/`head` — 31% of all result bytes measured — where the Read tool would have windowed it |
| `persisted-output-dump` | `advise` | reading back whole a tool result the harness saved to a file for being too large, paying for it twice |
| `file-reread` | `advise` | a Read, or a `cat`, of a file this session already has in context and that is unchanged on disk since — answered from the session's own record (one per agent, exact windows, certain or uncertain; a compaction clears the main one), not the command |
| `read-unbounded-large` | `advise` (ceiling `deny`) | a Read with no `offset`/`limit` of a file over 16 KB, where the whole file lands in the context and every later turn carries it; plan files and diffs a reviewer is handed are exempt |
| `code-read-not-lsp` | `advise` (ceiling `deny`) | a Read with no `offset`/`limit` of a code file over 4 KB (`.rs .go .py .ts/.js`) that fleet-lsp serves, where `documentSymbol` and `goToDefinition` answer in a few lines; once per file per agent per session (every time at `deny`, until LSP is asked), and `read-unbounded-large` stays quiet while it speaks. A Read of a file the same agent already asked LSP about (a `PostToolUse` LSP hook records each call, and the files its answer names, per session and agent; an `Error performing` answer counts) gets no LSP advice and is journalled `lsp-first` (`read-unbounded-large` still speaks past 16 KB); at `deny` the refusal names `documentSymbol` and says the Read passes once LSP was asked or if LSP errors. Silent without `fleet-lsp` on `PATH`, with the plugin off, outside a git working tree, for an agent whose tools lack `LSP`, and on files `read-unbounded-large` exempts |
| `symbol-grep-not-lsp` | `advise` (ceiling `deny`) | `grep`/`rg`/`git grep`, or the Grep tool, searching code for an identifier-shaped pattern (camelCase, PascalCase, snake_case, or after `fn`/`def`/`class`…), where `workspaceSymbol` and `findReferences` find the definition and its uses; once per pattern per session. Silent on a pipe filter, `-c`, `-v`, `-i`, `-f`, several `-e`, comment markers (`TODO`), text files, `$VAR` paths, and in the same places as `code-read-not-lsp` |
| `lint-suppression-added` | `advise` (ceiling `deny`) | an Edit, MultiEdit or Write that adds a lint suppression (`# type: ignore`, `# noqa`, `eslint-disable`, `@ts-ignore`, `#[allow]`, `//nolint`) or loosens a lint configuration (tsconfig, eslint, pyright, ruff, Cargo `[lints]`, golangci), where `general.no-disabled-safety` says to fix the finding instead; the file is compared before and after by marker, not by line |
| `agent-guidance-written` | `advise` (ceiling `advise`) | an Edit, MultiEdit or Write to agent guidance (`SKILL.md`, `CLAUDE.md`, `CLAUDE.md.tmpl`, `AGENTS.md`, `MEMORY.md`, `.claude/agents/*.md`, `dot_claude/agents/*.md`), where the `guidance.*` and `prose.*` rules apply (ADR-0026, ADR-0032) and `aval relevant --path <file>` lists them; told once per session and file (per agent), later writes are journalled only. Other memory files are exempt |
| `agent-guidance-shape` | `advise` (ceiling `advise`) | a write that leaves a `SKILL.md` without `disable-model-invocation: true` and without a one-line `description:` that says when to use it — a `Use <word> …` or `whenever …` clause (`guidance.pointer-names-when`), or grows a `CLAUDE.md`, `CLAUDE.md.tmpl`, `AGENTS.md` or `MEMORY.md` past 8,192 bytes (`guidance.always-on-is-an-index`; a ratchet: shrinking or equal size passes). Judged on the rebuilt whole file only; a fragment is journalled and silent; told once per session and file |
| `adr-lint-written` | `advise` (ceiling `advise`) | an Edit, MultiEdit or Write (a `PostToolUse`, once it has succeeded) of a `.rs`, `.ts`, `.tsx`, `.go` or `.py` file in a repository that vendors adr-lint's `check` row (decisions ADR-0044). The repository is the git root of the FILE's directory, not the payload cwd; the gate is open only when `amont list --json` run there prints `amont-list-v1` listing the check id `pre-commit-adr-lint-check`, trusted or not (amont lists an untrusted row with no `command`), asked once per session and repository, with a 5 s deadline. It then runs `adr-lint check --json -- <path>` (`AMONT_AGENT_ADR_LINT` names another program), killed at 1 s, and advises each finding as `path:line:col: rule-id: message` with one `aval rule <id>` line per rule and how to waive (`adr-lint: allow <rule-id> -- <reason>`). Journal: each finding as `rule path text` (`advised`, or `watched` under `observe`, which says nothing); `adr-lint-slow` on each kill; `adr-lint-unavailable`, `adr-lint-exit-2`, `adr-lint-exit-3`, `adr-lint-exit-other`, `adr-lint-unparsed`, `amont-list-failed` and `amont-list-unexpected` once per session, never shown to the agent |
| `test-output-trimmed` | `advise` (ceiling `deny`) | a test, lint or typecheck runner (`cargo test`, `pytest`, `npm run test`, `tsc`, …) piped into `tail`/`head`/`grep` while `pipefail` is not definitely on, so a failing run exits 0 and the trimming can cut the failure off. The shell analysis tracks `pipefail` as yes/no/maybe, including zsh `setopt`/`unsetopt` and `set -o nopipefail`; only a definite yes exempts. Evidence: 33.7 per 1000 calls (2,790 of 82,857, `amont-agent backtest --rule test-output-trimmed --since 2026-09-11`, measured 2026-10-11; weekly 20.1 to 53.4, flat 4 weeks) |

Two more checks run after a command rather than before it, on `PostToolUse`:
`push-landed` and `push-published` verify what a push that reported success
actually did. See [the assertions](assertions.md).

`amont-agent rules` prints this with each rule's measured firing rate, and
with the stance in force on this machine rather than the shipped one: a rule
you promoted in `~/.gitconfig` reads `deny (ships as observe)`, and a rule
with a ceiling names it, `(max advise)`.

## Why only four of them deny

`pipe-to-tail` blocks because seven consecutive weeks of measurement showed no
downward trend while every other habit halved. That is the bar: a rule earns
`deny` from your own transcripts, not from an argument about how bad the
mistake is. A habit the model is already correcting does not need a `deny`.
See [measuring and graduating](measuring.md).

`plan-review-panel` is the other kind of `deny`, and the person's choice
rather than a measurement: it fires on `ExitPlanMode`, not on a shell command,
and checks a fact — whether the review panel the plan calls for ran on the plan
being presented — naming the missing roles when it did not. After two refusals
of the same plan, or whenever it cannot check, it hands the call to the person
as `ask` instead. See [the review panel](plan-review.md).

`plan-phases-open` is the third, also the person's choice. It fires on
`Stop`, the moment the agent ends its turn. ADR-0022 makes a plan one branch
and one pull request with its phases as commits, so a turn that ends with
a phase still open is a stall, not a handoff. The rule reads only the
`active` plans changed on the current branch (against the merge-base with
the remote default branch), never one already on main, and sends the agent
on to the first open `- [ ] ` under `## Phases`. It is silent when that
phase is a `🧑 decision:`, in plan mode, while background tasks are still
running, and when the agent's last message has a line starting with
`WAITING: <reason>` (a preview awaiting approval, a deferred phase, a
blocker). After three continuations on the same phase in one session it lets
the turn end and tells the person, with a note only they see; their next
prompt resets the count. Anything it cannot establish — no repository, a git
failure, an unreadable plan, a session id it will not use as a file name —
lets the turn end. To turn it off without uninstalling:
`git config --global amont.agent.plan-phases-open.stance observe`.

`workspace-root-write` is the fourth, and ships at `deny` straight away
instead of walking the observe, advise ladder. The evidence is on disk, not in a
rate: 86 `*.log` and `*.err` files in `~/Developer/Perso`, none of them in a
repository, none swept, none traceable to a task, and the refusal carries a
one-word fix. `examine` reads only the shape (a write redirect or a `tee`
operand with a literal target); `confirm` establishes the fact, that the
directory the clause runs in, after its `cd`s, is in no git tree and has at
least two children with a `.git`. The system temp directories are never roots.
The remedy names the linked worktree the session touched last, but only when
`git check-ignore` says a file of that name is ignored there (an untracked log
would hold `worktree-sweep` forever), and otherwise the session's scratch
directory, which `worktree-sweep --scratch` clears. To demote it to a record:
`git config --global amont.agent.workspace-root-write.stance observe`.

`stale-base` advises from the start because it refuses nothing, speaks only
after measuring a real gap, and names a failure no correcting loop can see —
**nothing fails when you build on stale code.** The work is correct against
the code it can see, and the conflict arrives later, from somewhere else. So a
session opening in a checkout the remote has moved past is told — see
[the session notice](session-notice.md) for why it fetches and never pulls.

`push-preflight` advises for the same reason. git opens its connection to the
remote *before* it runs `pre-push` and holds it idle for as long as the test
gate takes; a remote that closes idle sessions kills the push after the gate
has already passed, and the model reads "the network" where the cause was the
gate's placement. With amont ≥ 1.28, `amont rehearse --wait` runs the same
gate on a snapshot of `HEAD` with no connection open — or follows the
rehearsal `amont.rehearseOnCommit` already started — and stamps the tree, so
the push that follows skips the suite (`amont run pre-push` on 1.27). The rule
speaks only when `confirm` finds all three facts: amont guards this
repository's pushes, a test gate would run for this push, and `HEAD`'s tree
carries no stamp yet.

## Shape, then the world

Most rules after the first handful came out of the transcripts the same way —
tens of thousands of Bash calls, sorted by what failed, was killed, or drew a
correction (see [`mine`](measuring.md#0-mine--what-is-going-wrong-that-no-rule-names)).
Each fires on shape and, where the fact lives in the world, confirms it first:
whether the call already runs in the background, which `sed` is on `PATH`,
whether the repository holds a Flux or Argo resource, whether the worktree is
dirty, whether the remote has the commit, whether any other branch has the
commits. A rule that fires on shape alone, like `tag-after-commit`, names a
failure every command in the chain reports as success.

## `pipe-to-tail` in full

```sh
git push origin main 2>&1 | tail -5
```

A pipeline's exit status is its **last** command's. `tail` succeeds at tailing
an error message, so a rejected push, a push killed by a timeout, and a push
that never left the machine all report success — and the trimming discards the
error text, so the failure is silent in both channels.

The remedy the rule prints is not "don't use tail": it is to run the mutating
command on its own, read its output afterwards, and then verify the *effect*
(`git ls-remote origin refs/heads/<branch>`) rather than the exit code.

`set -o pipefail` first, or writing to a file and tailing the file, are also
accepted — the rule fires on the shape, and the reason names all three ways
out.

## Asking about one command

```sh
amont-agent check 'git push | tail -1'
```

No stdin, no session, no journal entry — just the rules over one string, with
whatever they would have said.
