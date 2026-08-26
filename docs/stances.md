# Stances

A rule does one of three things, and the middle one is the point:

| stance | effect |
|---|---|
| `observe` | records the firing and says nothing at all |
| `advise` | puts the reason into the model's context; refuses nothing |
| `deny` | refuses the tool call, with the reason and the remedy |

`observe` and `advise` are not two ways of saying "not blocking yet".
`additionalContext` enters the model's context and therefore changes its
behaviour, which contaminates the rate the observation exists to measure. **A
rule that talks is intervening.** That is why the two are named differently
and why `backtest` numbers from an `advise` rule are not comparable with the
numbers that justified promoting it.

## Changing one

Takes effect on the next command; nothing to restart.

```sh
git config --global amont.agent.pipe-to-tail.stance observe
git config --global amont.agent.stance observe          # every rule
```

The ladder, most specific first:

```
rule.default_stance  <  amont.agent.stance  <  amont.agent.<id>.stance
```

then clamped to `observe` if the guard is switched off.

## Why git config and not a committed file

Promotion power stays on the machine, with the person. A rule that a committed
file could promote to `deny` would mean cloning a repository hands it the
power to refuse your shell commands.

This is enforced twice, because one enforcement was not enough. amont's config
reader lets a repository's committed policy `set` lines outrank system and
global git config — right for a hook manager, wrong for a guard — so this
project reads git config itself, without that ladder. A stance answers to your
own git config and to nothing a `git clone` can carry.
