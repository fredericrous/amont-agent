# Measuring and graduating

This is the part that makes the rest defensible. Every rule's stance is a
claim about your own behaviour, and the claim is checked against your own
transcripts rather than asserted.

The loop is: **backtest → explain → review → graduate.**

## 1. Backtest — what would this have cost me?

`backtest` replays your Claude Code transcripts through the rules and reports
firings per 1,000 tool calls per week, so a rule's cost is a number rather
than an impression.

```sh
amont-agent backtest --since 2026-07-06
amont-agent backtest --rule pipe-to-tail --json
amont-agent backtest --transcripts ~/.claude/projects   # where they live
```

A weekly series is the thing to read, not a total. A habit that is halving on
its own does not need a `deny`; the model is already correcting. A flat line
over weeks is a habit that will not correct itself, and that is what promotion
is for.

## 2. Explain — look at the actual matches

A rate is only trustworthy if the matches behind it are real. `explain` prints
every match for one rule so you can read them.

```sh
amont-agent explain pipe-to-tail
amont-agent explain pipe-to-tail --sample 20
```

## 3. Review — turn matches into reviewed judgements

Precision is kept as a **corpus of judgements**, not as a metric, because a
metric charts a regression and a test prevents one.

```sh
amont-agent explain pipe-to-tail --format cases >> tests/corpus/pipe-to-tail.cases
$EDITOR tests/corpus/pipe-to-tail.cases    # each `?` becomes match or nomatch
amont-agent corpus check                   # and this runs in the test suite
```

Include the cases that should **not** match. A corpus of positives alone
measures recall and says nothing about how often the rule is wrong, which is
the number that decides whether it can be allowed to refuse anything.

## 4. Graduate — promote on the evidence

```sh
amont-agent graduate bare-stash-pop --to advise
amont-agent graduate bare-stash-pop --to deny
```

Promotion is gated on the corpus: a rule cannot be promoted past a corpus that
does not support it.

## Demotion is not gated at all

```sh
amont-agent demote bare-stash-pop
```

No questions, no evidence required, effective on the next command. This
asymmetry is deliberate. A guard that is hard to back out of is one people
uninstall instead of demoting — and uninstalling takes every rule with it,
including the ones that were working.
