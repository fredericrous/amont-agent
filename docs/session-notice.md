# The session notice

There is a mistake no command-level rule can catch, because no command is
wrong: a session opens in a checkout last pulled on Tuesday, the model reads
the tree it is given, and builds a feature that landed on `origin/main` on
Wednesday. The work is correct against the code it can see.

So at `SessionStart` the guard does the one thing the model cannot do for
itself.

## The stale checkout

It refreshes `origin/main` — one branch, no tags, killed at five seconds,
skipped when `FETCH_HEAD` is under ten minutes old so a burst of sessions
shares one round-trip — and if `HEAD` is behind, says so:

```
amont-agent/stale-base: this checkout of amont-agent (branch main) is 8
commits behind origin/main; newest there: d3b2ed5 chore(release): 2.1.0
(3 days ago). Work that seems missing here may already exist on
origin/main — `git log HEAD..origin/main --oneline` lists it — and a
branch or worktree started from HEAD inherits the gap; one started from
origin/main does not.
```

**It never pulls.** `git pull` rewrites the working tree under whoever is
using it, and a per-task worktree exists precisely so that nobody does that.
Moving `refs/remotes/origin/*` is safe in every worktree at once; moving `HEAD`
is not.

If the fetch fails or times out, the notice is computed against the last
successful fetch and says so. When the checkout is up to date, or it is not a
repository, or there is no remote, it says nothing.

The `stale-base` rule is the same fact at the moment it is about to be
inherited: `git worktree add`, `git checkout -b` or `git switch -c` from `HEAD`
or a local branch, while that start point is behind. The remote form
(`… -b feat/x origin/main`) is the remedy and never fires.

```sh
git config --global amont.agent.stale-base.stance observe   # measure, say nothing
git config --global amont.agent.fetch false                 # never touch the network
git config checkout.defaultRemote forgejo                   # measure against another remote
```

`checkout.defaultRemote` is git's own key for "which remote is the remote", and
a repository mid-migration — `origin` a mirror going stale, a second remote
carrying the truth — sets it once for both git and the guard. With two remotes
and no preference the guard says nothing rather than guess.

## The stale guidance block

The same moment is when an agent reads `AGENTS.md` and believes it, and follows
it for the whole session. A block generated two releases ago can be wrong
before any command runs.

This one is **entirely optional and entirely about
[amont](https://github.com/fredericrous/amont)**. If — and only if — this
repository carries an `<!-- amont:start -->` block and `amont` is on your
`PATH`, the guard asks amont the question amont already answers:

```sh
amont agents-md --check
```

and reports drift. Two file reads decide whether to spawn anything at all, so
a repository with no such block costs no process. **No `amont` on `PATH` means
this says nothing** — which is the right answer for anyone who does not use it.

It reads amont's *stderr*, not its exit code, and that is not fussiness:
`agents-md --check` exits `1` for a file it could not read as well as for a
drifted one, so the exit code alone would announce staleness for a permissions
error. A session-opening notice that cries wolf is worse than one that
occasionally says nothing.

```sh
git config --global amont.agent.agentsMdNotice false   # silence this half
```
