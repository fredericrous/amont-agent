# amont-agent

**Your agent just ran `git push … | tail -5`, the push was rejected, and the
command reported success.**

A pipeline's exit status is its **last** command's. `tail` succeeds at tailing
an error message, so a rejected push, a push killed by a timeout, and a push
that never left the machine all look identical — and the trimming throws the
error text away too. No git hook can catch that: the mistake is in the command
string and never reaches one.

`amont-agent` is a Claude Code `PreToolUse` hook that reads a shell command
before it runs, and can observe it, advise against it, or refuse it.

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
amont-agent doctor           # installed, runnable, and actually firing?
```

## It measures before it blocks

Each rule has a stance: `observe` records the firing and says nothing,
`advise` puts the reason into the model's context, `deny` refuses the call
with the reason and the remedy. Of the 38 rules, three ship as `deny` —
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

## Documentation

The [book](https://fredericrous.github.io/amont-agent/), versioned with the
code:

- [Installing](docs/install.md) — channels, the wiring, `doctor`, turning it off
- [Stances](docs/stances.md) and [configuration](docs/configuration.md)
- [The rules](docs/rules.md) and [the assertions](docs/assertions.md)
- [Measuring and graduating](docs/measuring.md) — the method
- [Preview approval](docs/preview.md) and [the review panel](docs/plan-review.md)
- [The session notice](docs/session-notice.md) — a stale checkout, told at session start
- [What it will not do](docs/refusals.md) and [the shell analysis](docs/analysis.md)

## Contributing

`make check` runs fmt, clippy `-D warnings` and the tests — exactly what CI
runs. One crate; `serde` and `serde_json` are its only dependencies, and only
for reading.

## Related

[amont](https://github.com/fredericrous/amont) — git hooks that catch a bad
commit before it exists, by the same author. Independent of this: no shared
code, and neither needs the other.

[attest](https://github.com/fredericrous/attest) — the CI end of the same
story: CI skips the gates a signed pre-push note says really ran. All three
share one conviction: trust what was verified, never what was reported.

## License

[MIT](LICENSE).
