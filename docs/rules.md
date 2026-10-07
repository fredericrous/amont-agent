# The rules

| rule | ships as | what it catches |
|---|---|---|
| `pipe-to-tail` | `deny` | a mutating command whose status is swallowed by a pipe |
| `bare-stash-pop` | `observe` | `git stash pop` with no ref, where `refs/stash` is shared across worktrees |
| `gh-pr-merge-auto` | `observe` | `--auto` on a repository with no required checks, which merges immediately |
| `forge-merge-by-hand` | `advise` | merging a pull request by POSTing to the forge's merge endpoint, which answers `200` whether the checks passed, failed or never started |
| `forge-status-stale-row` | `observe` | keeping the first row of a commit's append-only `/statuses` list, so a stale `pending` reads as the present and the wait never ends |
| `no-verify` | `observe` | turning the whole commit gate off rather than one check |
| `git-add-broad` | `observe` | staging the tree instead of the change |
| `stale-base` | `advise` | a branch or worktree started from a checkout the remote has moved past |
| `push-preflight` | `advise` | a `git push` whose slow pre-push test gate has not been rehearsed with `amont rehearse --wait` |
| `push-preview` | `advise` | a push that would publish interface changes no approved localhost preview covers ([preview approval](preview.md)) |
| `plan-review-panel` | `deny` | a plan presented at `ExitPlanMode` before its expert review panel ran ([the review panel](plan-review.md)) |
| `plan-phases-open` | `deny` | a turn ending while the plan this branch carries still has an open phase that is not a `🧑 decision:` — the agent is sent on to it |
| `implementation-review` | `advise` | a push of a branch that carries a plan, whose diff no independent reviewer has read for the tree being pushed ([the implementation review](implementation-review.md)) |
| `foreground-poll` | `advise` | a polling loop or `gh run watch` in the foreground, where the tool's ten-minute clock will kill it one poll short |
| `sed-in-place` | `advise` | `sed -i` spelled for the other sed (`-i ''` on GNU, bare `-i` on BSD) |
| `kubectl-gitops` | `advise` | an imperative `kubectl` write in a repository Flux or Argo reconciles |
| `tag-after-commit` | `advise` | `git tag` chained onto a `git commit` that a hook may have refused |
| `release-tag-push` | `observe` | pushing a `v*` tag, which publishes: the tag may name the wrong commit, and a green workflow does not mean the artefact is right |
| `worktree-remove-force` | `advise` | `git worktree remove --force` on a worktree that still holds uncommitted work |
| `amend-pushed` | `advise` | `git commit --amend` on a commit the remote already has |
| `branch-force-delete` | `observe` | `git branch -D` on a branch whose commits are on no remote and not merged |
| `poll-blank-verdict` | `observe` | a wait that stops on any value but the one it names, so a failed lookup's empty string reads as the answer |
| `worktree-isolation` | `observe` | a branch created, or `git reset --hard`, in the primary checkout of a repository that already has linked worktrees |
| `stdin-hang` | `observe` | a command that will read standard input with nothing on it — `cat > file`, a bare interpreter, `tee` outside a pipe — which blocks silently until the tool's clock runs out |
| `glob-in-flag-value` | `advise` | an unquoted glob inside a flag value (`--include=*.ts`), which zsh expands — or fails on — before the program sees it |
| `glob-no-match` | `advise` | an unquoted glob operand that matches nothing, which under zsh aborts the clause before it starts while a later clause reports success |
| `equals-separator` | `observe` | a bare word beginning with `=` (`echo ===`, `[ x == y ]`), which zsh reads as a command lookup and whose failure aborts the whole command list |
| `unsplit-expansion` | `observe` | an unquoted `$var` under `set --` or `for … in`, which zsh leaves as one word: the positionals or the loop get the whole string and the command runs wrong while reporting success |
| `path-operand-missing` | `advise` | a read of a path that is not there — under the grep shim a warning in mid-stream, for the coreutils one line and carry on — which the chain then reports as success |
| `stat-bsd-format` | `advise` | `stat -f '%…'` on GNU stat (or `-c` on BSD), which prints a filesystem report where a timestamp was wanted |
| `whole-file-dump` | `advise` | a file poured whole into the tool result by `cat`/`sed -n`/`head` — 31% of all result bytes measured — where the Read tool would have windowed it |
| `persisted-output-dump` | `advise` | reading back whole a tool result the harness saved to a file for being too large, paying for it twice |
| `file-reread` | `advise` | a Read, or a `cat`, of a file this session already has in context and that is unchanged on disk since — answered from the session's own record, not the command |
| `read-unbounded-large` | `advise` (ceiling `deny`) | a Read with no `offset`/`limit` of a file over 16 KB, where the whole file lands in the context and every later turn carries it; plan files and diffs a reviewer is handed are exempt |
| `lint-suppression-added` | `advise` (ceiling `deny`) | an Edit, MultiEdit or Write that adds a lint suppression (`# type: ignore`, `# noqa`, `eslint-disable`, `@ts-ignore`, `#[allow]`, `//nolint`) or loosens a lint configuration (tsconfig, eslint, pyright, ruff, Cargo `[lints]`, golangci), where `general.no-disabled-safety` says to fix the finding instead; the file is compared before and after by marker, not by line |
| `request-fanout` | `observe` (ceiling `advise`) | one command that may make more than 50 explicit network transfers to one destination — a loop following `Link: next`, `gh api --paginate`, a curl URL range — counted by the shell analysis ([analysis.md](analysis.md)) with the loops and calls that multiply them |

Two more checks run after a command rather than before it, on `PostToolUse`:
`push-landed` and `push-published` verify what a push that reported success
actually did. See [the assertions](assertions.md).

`amont-agent rules` prints this with each rule's measured firing rate, and
with the stance in force on this machine rather than the shipped one: a rule
you promoted in `~/.gitconfig` reads `deny (ships as observe)`, and a rule
with a ceiling names it, `(max advise)`.

## Why only three of them deny

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
