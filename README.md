# amont-agent

**Your agent just ran `git push … | tail -5`, the push was rejected, and the
command reported success.**

A pipeline's exit status is its **last** command's. `tail` succeeds at tailing
an error message — so a rejected push, a push killed by a timeout, and a push
that never left the machine all look identical, and the trimming throws the
error text away too. The failure is silent in both channels, and the model
reads its own impatience as a green tick.

No git hook can catch that. The mistake is in the command string and never
reaches one.

`amont-agent` is a Claude Code `PreToolUse` hook that reads a shell command
before it runs, and can observe it, advise against it, or refuse it.

[![CI](https://github.com/fredericrous/amont-agent/actions/workflows/ci.yaml/badge.svg)](https://github.com/fredericrous/amont-agent/actions/workflows/ci.yaml)
[![License](https://img.shields.io/github/license/fredericrous/amont-agent)](LICENSE)

```console
$ amont-agent check 'git push origin main 2>&1 | tail -5'
pipe-to-tail [deny]
  `git push` pipes into `tail`, so the pipeline reports tail's exit status,
  not git push's. A failed, rejected or timed-out run reads as success, and
  the trimming discards the error text as well.
  → Run `git push` on its own and read its output afterwards. Then verify the
    effect rather than the exit code.
  ▸ git push origin main 2>&1 | tail -5
```

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/fredericrous/amont-agent/main/install/install.sh | sh
```

Then wire it in — a separate, deliberate step, because a program that can
refuse your agent's commands should not add itself to your settings as a side
effect of you downloading it:

```sh
amont-agent install          # prints the settings block, writes nothing
amont-agent install --write  # merges it into ~/.claude/settings.json
amont-agent doctor           # installed, runnable, and actually firing?
```

Also: `brew install fredericrous/tap/amont-agent` · `cargo install amont-agent`
· [prebuilt binaries](https://github.com/fredericrous/amont-agent/releases/latest)
for Linux (gnu/musl, x86_64 and aarch64), macOS (Intel and Apple silicon) and
Windows.

## It measures before it blocks

Five of the six rules ship as `observe` — they record and say nothing at all.
That is not timidity, it is the method:

| stance | effect |
|---|---|
| `observe` | records the firing and says nothing |
| `advise` | puts the reason into the model's context; refuses nothing |
| `deny` | refuses the tool call, with the reason and the remedy |

`observe` and `advise` are not two ways of saying "not blocking yet".
`additionalContext` enters the model's context and changes its behaviour,
which contaminates the rate the observation exists to measure. **A rule that
talks is intervening.**

So a rule is promoted from your own transcripts, not from an argument:

```sh
amont-agent backtest --since 2026-07-06         # firings per 1,000 tool calls, weekly
amont-agent explain pipe-to-tail --format cases >> tests/corpus/pipe-to-tail.cases
$EDITOR tests/corpus/pipe-to-tail.cases          # each `?` becomes match or nomatch
amont-agent corpus check                         # and this runs in the test suite
amont-agent graduate pipe-to-tail --to deny
```

`pipe-to-tail` is the only rule that blocks, and it blocks because seven
consecutive weeks of measurement showed no downward trend while every other
habit halved. A habit the model is already correcting does not need a `deny`.

Demotion is not gated at all — `amont-agent demote <rule>`, no questions. A
guard that is hard to back out of is one people uninstall instead of demoting,
and uninstalling takes every rule with it.

[The full method](docs/measuring.md).

## The rules

| rule | ships as | what it catches |
|---|---|---|
| `pipe-to-tail` | `deny` | a mutating command whose status is swallowed by a pipe |
| `bare-stash-pop` | `observe` | `git stash pop` with no ref, where `refs/stash` is shared across worktrees |
| `gh-pr-merge-auto` | `observe` | `--auto` on a repo with no required checks, which merges immediately |
| `no-verify` | `observe` | turning the whole commit gate off rather than one check |
| `git-add-broad` | `observe` | staging the tree instead of the change |
| `stale-base` | `advise` | a branch or worktree started from a checkout the remote has moved past |

`stale-base` advises from the start because it refuses nothing and names a
failure no correcting loop can see: **nothing fails when you build on stale
code.** The work is correct against the code it can see, and the conflict
arrives later, from somewhere else. So a session opening in a checkout the
remote has moved past is told — after a five-second, one-branch fetch that
never pulls. [Why it never pulls](docs/session-notice.md).

## What it will not do

- **It never emits `allow`.** Approving everything it has no objection to would
  switch off the permission system it was installed beside. Silence is how it
  says "no objection".
- **Every failure path is silence.** An unreadable payload, an unknown event, a
  command it cannot parse, a rule that panics — all exit 0 having written
  nothing. A hook that fails toward *refusing* gets deleted from
  `settings.json`, which switches off every rule at once; one that fails toward
  silence loses a single firing.
- **It does not judge what it cannot read.** Heredocs without terminators,
  `eval`, `sh -c`, unbalanced quotes — opaque never fires.
- **It does not phone home.** No telemetry, no update checks. Every firing is
  journalled to `~/.claude/amont-agent/journal.log`, redacted, and it only
  counts — nothing in it may participate in a decision.
- **A cloned repository cannot change a stance.** Promotion power lives in your
  own git config, never in a committed file.

[The reasoning in full](docs/refusals.md).

## Turning it down

```sh
git config --global amont.agent.pipe-to-tail.stance observe   # one rule
git config --global amont.agent.stance observe                # all of them
AMONT_AGENT_OFF=1                                             # this shell
amont-agent uninstall --write                                 # remove the entries
```

## Documentation

[Installing](docs/install.md) · [Stances](docs/stances.md) ·
[The rules](docs/rules.md) ·
[Measuring and graduating](docs/measuring.md) ·
[The session notice](docs/session-notice.md) ·
[Configuration](docs/configuration.md) ·
[What it will not do](docs/refusals.md)

## Contributing

```sh
make check    # fmt, clippy -D warnings, tests — exactly what CI runs
```

One crate, `serde` and `serde_json` its only dependencies, and both only for
*reading* — Claude Code's payload and your `settings.json`, two shapes defined
by somebody else. What this binary writes is emitted by a hand-rolled escaper,
so the reading and the writing share no representation.

## Related

[amont](https://github.com/fredericrous/amont) — git hooks that catch a bad
commit before it exists, by the same author. Independent of this: no shared
code, and neither needs the other. They meet in one optional place, described
in [the session notice](docs/session-notice.md).

## License

[MIT](LICENSE).
