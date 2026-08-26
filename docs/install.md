# Installing

The binary first, then the wiring. They are separate acts on purpose: a
program that can refuse the commands your agent runs should not install itself
into your `settings.json` as a side effect of you fetching it.

## The binary

```sh
curl -fsSL https://raw.githubusercontent.com/fredericrous/amont-agent/main/install/install.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/fredericrous/amont-agent/main/install/install.ps1 | iex
```

Either one downloads a release binary, verifies it against the published
`SHA256SUMS`, and puts it in `~/.local/bin`. Neither wires anything in.

Or: `brew install fredericrous/tap/amont-agent`, `cargo install amont-agent`,
`npx amont-agent`, or a binary straight from
[Releases](https://github.com/fredericrous/amont-agent/releases/latest).

Under npm, node start-up is paid on every hook call. `amont-agent install`
writes the path of a real binary, so if you care about the ~30ms, install one
of the other ways.

## The wiring

```sh
amont-agent install          # prints the settings block, writes nothing
amont-agent install --write  # merges it into ~/.claude/settings.json
```

`install` refuses to guess. It will not patch a `settings.json` it could not
parse, and it will not write one whose formatting it cannot reproduce — a diff
full of reformatting hides the one line it added — so it prints the block and
changes nothing unless `--reformat` says otherwise. `uninstall` removes exactly
what it wrote and leaves everything else byte-identical.

```sh
amont-agent install --write --project   # .claude/settings.json instead
amont-agent install --write --local     # .claude/settings.local.json
```

Two entries are written: the guard on `PreToolUse`, and a `SessionStart` entry
that leaves a heartbeat and states where the checkout stands against the
remote. Without the heartbeat, `doctor` cannot tell "nothing fired this week"
from "the guard has been dead since Tuesday".

Your `settings.json` keeps the mode it already had, and a file created here
starts at `0600`: it can hold MCP environment blocks, and those hold
credentials.

## Knowing it is alive

Claude Code hooks fail open quietly: a command that cannot be resolved exits
127, which is a *non-blocking* status, and nothing tells you. `doctor` exits
non-zero when the guard is inert, so it can run from cron:

```
✓ installed in /Users/you/.claude/settings.json
✓ amont-agent 2.0.0 at /Users/you/.local/bin/amont-agent
✓ a refused command produces a valid decision document
✓ last ran 4m ago
✓ acting on pipe-to-tail
```

## Turning it off

```sh
AMONT_AGENT_OFF=1                                # this shell only
git config --global amont.agent.enabled false    # everywhere
amont-agent uninstall --write                    # remove the settings entries
```

Demoting one rule is almost always the better move than switching the guard
off — see [stances](stances.md). A guard that is hard to back out of is one
people uninstall instead of demoting, and uninstalling takes every rule with
it.
