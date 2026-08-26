# The rules

| rule | ships as | what it catches |
|---|---|---|
| `pipe-to-tail` | `deny` | a mutating command whose status is swallowed by a pipe |
| `bare-stash-pop` | `observe` | `git stash pop` with no ref, where `refs/stash` is shared across worktrees |
| `gh-pr-merge-auto` | `observe` | `--auto` on a repository with no required checks, which merges immediately |
| `no-verify` | `observe` | turning the whole commit gate off rather than one check |
| `git-add-broad` | `observe` | staging the tree instead of the change |
| `stale-base` | `advise` | a branch or worktree started from a checkout the remote has moved past |

`amont-agent rules` prints this with each rule's measured firing rate.

## Why only one of them denies

`pipe-to-tail` blocks because seven consecutive weeks of measurement showed no
downward trend while every other habit halved. That is the bar: a rule earns
`deny` from your own transcripts, not from an argument about how bad the
mistake is. See [measuring and graduating](measuring.md).

`stale-base` advises from the start because it refuses nothing, speaks only
after measuring a real gap, and names a failure no correcting loop can see —
**nothing fails when you build on stale code.** The work is correct against
the code it can see, and the conflict arrives later, from somewhere else.

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
