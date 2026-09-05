# The assertions

A rule reads a command before it runs. An assertion checks what a command that
already reported success actually did.

The two halves are not the same job. `pipe-to-tail` can refuse
`git push … | tail -5` because the mistake is visible in the command string.
Nothing in a command string tells you that the push you just ran reported
`Everything up-to-date` about a branch you were not on.

| id | fires on | asks |
|---|---|---|
| `push-landed` | `git push` | is the branch on the remote, at the commit you have? |

## Only successful calls

Claude Code sends a failed tool call to `PostToolUseFailure`, an event this
crate ignores. So everything an assertion sees claimed to work — which is the
whole point. A command that returned an error is already in front of the model;
there is nothing invisible left to point out.

## An assertion cannot refuse

The tool has already run. A `deny` stance therefore speaks exactly like
`advise`, the same way it does at a session opening.

What an assertion *can* do is state a fact:

```
amont-agent/push-landed: `git push` exited 0, but origin/main is at d0765d4f9
while the local branch is at 8970c7ffb. The push did not land. Run the push
again on its own and read its output, then confirm with `git ls-remote`.
```

That is not advice to weigh. It is the remote's answer.

## What it refuses to judge

`push-landed` handles the unambiguous shapes and nothing else. A
`HEAD:refs/heads/other` refspec, a tag push, several refspecs at once,
`--delete`, `--mirror`, `--all`: each needs a different question asked of the
remote, and a confidently wrong accusation costs the channel its credibility —
which is the only thing the channel has.

The same goes for everything it cannot establish: a detached HEAD, a remote
given as a URL, a working directory that has gone, a remote that cannot be
reached without a password. All silence.

## It can never prompt

Hooks run with no controlling terminal, so a credential prompt does not fail —
it hangs, and it hangs the session rather than this process. Every child runs
with `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS`/`SSH_ASKPASS` disabled, ssh in
`BatchMode`, and a five-second deadline. A guard installed to make pushing safer
must never be the reason a push becomes impossible.

## Measuring

`examine` is pure, so the backtester replays it like any rule:

```console
$ amont-agent backtest push-landed --since 2026-07-06
push-landed   1007   38.7   routine   observe
```

Read that number correctly. For a rule, a firing is a mistake caught. For an
assertion, a firing is a *question asked* — 1,007 pushes in 29,758 Bash calls,
about one call in twenty-six paying for one `git ls-remote`. It speaks only when
the answer disagrees, which is far rarer.

How often a claim is actually broken is not backtestable at all: `verify`
touches the world, and the world has moved since those commands ran. That number
accumulates forward, in the journal:

```console
$ grep push-landed ~/.claude/amont-agent/journal.log
```

`held` is a push that landed, `broken` one that did not, `unverified` one this
crate declined to judge.
