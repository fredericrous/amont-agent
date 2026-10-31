# amont-agent

**Your agent just ran `git push … | tail -5`, the push was rejected, and the
command reported success.**

A pipeline's exit status is its **last** command's. `tail` succeeds at tailing
an error message, so a rejected push, a push killed by a timeout, and a push
that never left the machine all look identical — and the trimming throws the
error text away too. No git hook can catch that: the mistake is in the command
string and never reaches one.

`amont-agent` is a Claude Code hook that reads an agent's tool calls before
they run — a shell command first of all — and can observe one, advise against
it, or refuse it. The same guard holds the work-process verdicts a tool call
waits on: a plan's review panel before `ExitPlanMode`, the implementation
review and the preview approval before a push.

[![CI](https://github.com/fredericrous/amont-agent/actions/workflows/ci.yaml/badge.svg)](https://github.com/fredericrous/amont-agent/actions/workflows/ci.yaml)
[![License](https://img.shields.io/github/license/fredericrous/amont-agent)](LICENSE)

```console
$ amont-agent check 'git push origin main 2>&1 | tail -5'
pipe-to-tail [deny]
  `git push` pipes into `tail`, so the pipeline reports tail's exit status, not git push's. A failed, rejected or timed-out run reads as success, and the trimming discards the error text as well.
  → Run `git push` on its own and read its output afterwards. Then verify the effect rather than the exit code.
  ▸ git push origin main 2>&1 | tail -5
```

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/fredericrous/amont-agent/main/install/install.sh | sh
```

Or `brew install fredericrous/tap/amont-agent`, `cargo install amont-agent`,
the PowerShell installer, or a
[prebuilt binary](https://github.com/fredericrous/amont-agent/releases/latest).
Then wire it in — a separate step, because a program that can refuse your
agent's commands should not add itself to your settings as a side effect of
you downloading it:

```sh
amont-agent install          # prints the settings block, writes nothing
amont-agent install --write  # merges it into ~/.claude/settings.json
amont-agent doctor           # installed, runnable, firing? plan root in use
```

## It measures before it blocks

Each rule has a stance: `observe` records the firing and says nothing,
`advise` puts the reason into the model's context, `deny` refuses the call
with the reason and the remedy. Of the 40 rules, three ship as `deny` —
`pipe-to-tail`; `plan-review-panel`, which refuses a plan at
`ExitPlanMode` before its review panel ran; and `plan-phases-open`, which
sends an agent on when it stops with a plan phase still open. A rule is promoted from your own
transcripts, not from an argument: `mine`, `backtest`, `explain`,
`corpus check`, `backtest --compliance`, then `graduate`. Demoting is one
command with no questions. `amont-agent rules` lists every rule with the
stance in force on your machine and its measured rate.

It never emits `allow`, every failure path is silence, it does not phone home,
and no repository can change a stance — stances live in your `--global` git
config only:

```sh
git config --global amont.agent.pipe-to-tail.stance observe   # one rule
AMONT_AGENT_OFF=1                                             # this shell
```

## Runs alone

`aval`, `adr-lint` and `amont` are optional. Without `aval`, `review-brief`
prints the brief with a section saying the decided code rules were not read
and exits 0 (`rules-sha` still exits 3, and the push check journals the
rules as unknown). Without `adr-lint`, the brief marks its candidates
unavailable and still exits 0. Without `amont`, the guard rules and the
review commands run unchanged; amont's own commit and push gates are then
simply not in play.

## Documentation

The [book](https://fredericrous.github.io/amont-agent/), versioned with the
code:

- [Installing](docs/install.md) — channels, the wiring, `doctor`, turning it off
- [Stances](docs/stances.md) and [configuration](docs/configuration.md)
- [The rules](docs/rules.md) and [the assertions](docs/assertions.md)
- [Measuring and graduating](docs/measuring.md) — the method
- [Preview approval](docs/preview.md) and [the review panel](docs/plan-review.md)
- [What shipped under a plan](docs/shipped.md)
- [The board: work in flight](docs/board.md)
- [The session notice](docs/session-notice.md) — a stale checkout, told at session start
- [What it will not do](docs/refusals.md) and [the shell analysis](docs/analysis.md)

## Contributing

`make check` runs fmt, clippy `-D warnings` and the tests across the
workspace — exactly what CI runs. Three crates, so each changes for one reason
and the compiler keeps them apart (decisions ADR-0039):

| Crate | Holds | Pages |
|---|---|---|
| [`amont-agent-guard`](crates/amont-agent-guard) | shell analysis, the rules and their stances, the assertions, the journal, `quiet` | [rules](docs/rules.md), [assertions](docs/assertions.md), [stances](docs/stances.md), [analysis](docs/analysis.md), [measuring](docs/measuring.md), [quiet](docs/quiet.md) |
| [`amont-agent-process`](crates/amont-agent-process) | the plan-review panel, the implementation review, preview approvals, provenance, `shipped` | [plan review](docs/plan-review.md), [implementation review](docs/implementation-review.md), [preview](docs/preview.md), [shipped](docs/shipped.md) |
| `amont-agent` (the root) | the CLI and the hook's dispatch: the composition root that wires both | [install](docs/install.md), [configuration](docs/configuration.md), [session notice](docs/session-notice.md) |

The guard never names the work process or the binary; the work process never
names the binary. Dependencies are `serde`/`serde_json` (for reading),
`pulldown-cmark` (a plan's sha) and `amont-quiet` (the quiet-run entry), each
argued in its crate's manifest.

## Related

[amont](https://github.com/fredericrous/amont) — git hooks that catch a bad
commit before it exists, by the same author. Neither needs the other at run
time; the one shared code is the quiet-run entry, linked from amont's
published `amont-quiet` crate.

[attest](https://github.com/fredericrous/attest) — the CI end of the same
story: CI skips the gates a signed pre-push note says really ran. All three
share one conviction: trust what was verified, never what was reported.

## License

[MIT](LICENSE).
