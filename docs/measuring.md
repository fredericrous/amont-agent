# Measuring and graduating

This is the part that makes the rest defensible. Every rule's stance is a
claim about your own behaviour, and the claim is checked against your own
transcripts rather than asserted.

The loop is: **mine → backtest → explain → review → compliance → graduate.**

## 0. Mine — what is going wrong that no rule names?

Every step below prices a rule somebody already thought of. `mine` is the
step before that: it groups the transcripts by command *shape* and ranks the
shapes that went wrong.

```sh
amont-agent mine --since 2026-08-15
amont-agent mine --min-support 10 --min-rate 0.4
amont-agent mine --format cases >> tests/corpus/<new-rule>.cases
```

A **shape** is the parsed command with its literals masked — the branch, the
path, the line number and the commit message taken out, the program, the
verbs and the flag set kept:

```text
git push origin feat/mine 2>&1 | tail -5  ─┐
git push origin fix/lint  2>&1 | tail -20 ─┴→ git push origin <word> 2>&1 | tail -5
```

A call counts as gone wrong when either of two things the transcript records
is true:

- **failed** — the tool result carried `is_error`: a non-zero exit, a refused
  permission, a run the harness killed;
- **corrected** — a near-identical command followed within a few tool calls.
  A model that re-issues the same shape is a model whose first attempt did
  not land, and that is the interesting half: the mistakes worth a rule are
  the ones nothing reports.

Only the HEAD of a run of near-identical calls counts as a correction. A
polling loop is one decision repeated twenty times, not nineteen mistakes —
before that rule existed, the top line of the real report was `echo waiting`.

**`bad` and `rate` are suspicion, not verdict.** A shape re-run for good
reasons carries a high rate and names no mistake at all. That is why every
row prints its samples and why the next step is a person reading them.

Shapes an existing rule already fires on are listed apart, `covered by
<rule>`, rather than proposed — and a covered shape that is still going wrong
is a rule that is observing when it should be advising.

What `mine` does **not** do is write the rule. Nothing learned here reaches
the hook: the path is `mine → write the rule by hand → cases → corpus check
→ graduate`, and what ships is the hand-written rule with its reason, its
remedy and its reviewed corpus. A guard that refused a command because a
clustering run found it suspicious could not explain itself to the person
whose work it just refused.

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
amont-agent explain pipe-to-tail --sample 20 --rank novelty
```

`--rank novelty` changes WHICH twenty. The default is the first twenty the
walk met — the oldest project, the oldest session, and, because a habit
repeats, very often twenty spellings of one command. Novelty picks the
twenty least like each other and least like the cases already in
`tests/corpus/<rule>.cases`, by greedy max-min over the shape distance, so
an hour of labelling buys as much of the precision estimate as an hour can.
It is deterministic: the same transcripts and the same corpus pick the same
cases, and a second pass does not hand back the first pass's.

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

## 4. Compliance — is the advice worth its tokens?

`advise` buys its place in the model's context with tokens, every session,
forever. The backtest says what that costs. This says what it buys.

```sh
amont-agent backtest --compliance
amont-agent backtest --compliance --rule glob-in-flag-value --json
amont-agent backtest --compliance --window 40
```

For every firing it looks at what the model did **next**. If the next
equivalent command — same shape, within the window — no longer matches the
rule, the habit changed (`complied`). If it matches again, the advice was
read and ignored, or never reached the model (`ignored`). If nothing
equivalent followed, the firing is unanswered and counts towards neither.

Three things make the number readable:

- **Per model.** Models differ, and a pooled figure hides which one is
  listening. `claude-opus-5` and `claude-fable-5` do not answer the same way
  to the same sentence.
- **`observe` rules are the control.** They said nothing, so their share is
  the rate the habit corrects on its own. Advice is worth its tokens only
  where the advised share beats the silent one — the same argument the
  stance ladder rests on, measured instead of assumed.
- **`deny` rules are left out.** A refused command never ran, so there is no
  next command to compare it against.

What it cannot see: a transcript records what the model ran, not whether the
hook spoke. A firing here means "the rule as it stands today would fire on
this command", replayed. Where a rule has been widened since, or was
observing then and advises now, the number is a reconstruction — and it is
still the only evidence there is.

## 5. Graduate — promote on the evidence

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
