# amont-agent

A guard that reads a shell command before Claude Code runs it, and can
observe it, advise against it, or refuse it.

It exists because of a class of mistake no git hook can see, since the mistake
is in the command string and never reaches a hook at all:

```sh
git push origin main 2>&1 | tail -5
```

A pipeline's exit status is the status of its **last** command. `tail` succeeds
at tailing an error message, so a rejected push, a push killed by a timeout,
and a push that never left the machine all report success — and the trimming
discards the error text too, so the failure is silent in both channels.

`amont-agent` is a `PreToolUse` hook. It is a single binary, it is opt-in, and
it phones nothing home.

## The shape of it

Three stances, and the middle one is the point:

| stance | effect |
|---|---|
| `observe` | records the firing and says nothing at all |
| `advise` | puts the reason into the model's context; refuses nothing |
| `deny` | refuses the tool call, with the reason and the remedy |

Only one rule ships as `deny`. The rest ship as `observe`, and a rule is
promoted only once your own transcripts say it should be — see
[measuring and graduating](measuring.md).

## Start here

```sh
amont-agent install          # prints the settings block, writes nothing
amont-agent install --write  # merges it into ~/.claude/settings.json
amont-agent doctor           # is it installed, runnable, and actually firing?
amont-agent status           # every rule, its stance, and what it has seen
```

## Relationship to amont

[amont](https://github.com/fredericrous/amont) is a git-hook manager by the
same author. The two are independent: neither needs the other, they share no
code, and `cargo tree` here shows `serde` and `serde_json` and nothing else.

They meet in exactly one place, and it is optional. If amont is installed and
this repository carries its generated `AGENTS.md` block, a session opening on
a stale block is told so — see [the session notice](session-notice.md). With
no `amont` on `PATH`, that check says nothing.
