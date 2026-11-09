---
status: done
branch: feat/chain-with-and
repos: [amont-agent]
adrs: []
---
# A step that can fail is not joined to a consequential step by `;`

## Review panel

👉 **Decide:** (1) `xargs … ; git commit` stays unmatched, since xargs pipelines are opaque (recommended), or A may be opaque when only xargs causes it; (2) rename the id to `semicolon-after-fallible`, which names the hazard, or keep `chain-with-and`.
📍 amont-agent · plan only, no code yet · next: Phase 1, the rule and its unit tests. Panel: backend (opus-5-5), rust (opus-5-5), unix (opus-5-5), tui (opus-5-5); codex: skipped, plan mode allows no side-effect commands.
**Changed by review:** xargs dropped from the fallible set; B must be readable and not nested; newline, `set -e` and keyword clauses handled; prevents/evidence/paths specified.
**Verdicts:** round 1: 4 approve-with-changes; round 2: 4 approve-with-changes; backend delta: approve. 8 medium or low findings carried into implementation.
📄 Full reviews: [2026-11-09-a-step-that-can-fail.reviews.md](2026-11-09-a-step-that-can-fail.reviews.md)

## Picture

```
  clause A --(;)--> clause B        flagged
  can fail          consequential

  A: python/python3 -c, a file, or - with a heredoc; sed -i; perl -i;
     git add; cargo fmt; cargo build; make; a test or build runner
  B: git commit/tag/push, amont land, gh pr merge, kubectl apply/delete,
     a test run (only when A is an edit: python, sed -i, perl -i, fmt)

  not flagged: A is echo, printf, cd, pwd, ls, sleep, true, export, cat,
     tail, git status/diff/log/fetch; a clause that is opaque (xargs
     pipelines are always opaque); a nested clause from $(...); a clause
     starting with if/then/else/do/while/{ ; a test run followed by a
     test run; a pair after an earlier top-level set -e; && || or &
     between the two
```

The rule fires only on the left-to-right pair A then B in the first two
boxes, joined by `;` or by a newline (both lex as the same connector), and
never on a pair joined by `&&`, `||` or `&`.

## Context

Twice on 2026-10-22 a failed step passed silently because `;` joined it to the next one: a Python edit failed its own `assert`, then `; cargo test` ran on the unedited file and passed; `xargs git add` failed, then `; git commit` committed only the deletions. The lesson is a memory note (`feedback_chain_dependent_steps_with_and`), which no rule enforces. amont-agent has no rule on `;` between dependent steps: `tag-after-commit` ignores the connector and only matches `commit ; tag` by accident.

Outcome: a new rule `chain-with-and` in amont-agent that flags the narrow shape, ships at `observe`, and graduates to `advise` on the corpus gate. The memory note then points at the rule.

## Behaviour

- `examine` takes the `Parsed` from `shell::lex` (`shell.rs:505`). For each readable clause `A` (`opaque.is_none()`, not `nested`) whose `next` is `Some(Connector::Semi)` (`shell.rs:51`), look at the following clause `B`, which must also be readable and carry the same `nested` value as `A`: nested clauses are appended after the line's own (`shell.rs:25-27`, `:932`), so after a trailing `;` or newline the next element is not always the next command. Fire when `A` is in the fallible set and `B` is in the consequential set (sets in the picture), matched with `program()`, `subcommand()` and `has_flag` (`shell.rs:1066-1293`).
- A newline lexes as the same `Semi` connector (`shell.rs:827-829`), so multi-line commands are in scope. The backtest reports `;` and newline firings separately.
- `xargs` is not in the fallible set: every pipeline holding it is opaque (`INDIRECT_FORKING`, `shell.rs:468`, `:954-957`), so a rule that reads only readable clauses cannot see it. The `xargs … ; git commit` incident goes in the corpus as a `nomatch` case with a note saying why; the add incident is matched as `git add -A ; git commit`.
- A clause whose first word is in `CLEARS_ALL` (`shell.rs:307-310`: `if`, `then`, `elif`, `else`, `fi`, `do`, `done`, `while`, `until`, `case`, `esac`, `function`, `}`, `(`, `)`, `eval`, `source`, `.`) or is `!` is neither A nor B. A `;` puts `then`, `do`, `else` and `fi` first in their clause (`shell.rs:318-319`), so the first command of a body shares its clause with the keyword and is never A: `if x; then make; git push; fi` gives no finding (unit test), while `then sed -i s/a/b/ f; make; git push; fi` fires on the later pair `make ; git push` (unit test). An `if` or `while` condition is never A for the same reason. This is a known gap, accepted to keep loop bodies out of scope.
- A test run followed by a test run never fires (running both is deliberate). A pair after an earlier top-level `set -e` or `set -o errexit` clause does not fire.
- `A | tail ; B` is not flagged: the clause before `;` is `tail`.
- Finding: `reason` = "`;` or a newline runs `<B program>` even if `<A program>` failed, so it acts on the state the failed step left: a test passes on an unedited file, a commit takes half the files."; `remedy` = "Join dependent steps with `&&`, or run them as separate calls and check each exit status." Span is `A.at..B.end`, as in `tag_after_commit.rs`.
- `prevents` (under 140 characters): "A failed edit, add or test followed by `;` into a commit, push or test run that acts on the wrong state"; `irreversible: false`; `evidence` is the backtest's per_1000 and date (harness.part-names-its-failure).
- Stance: `default_stance: Observe`, `max_stance: Advise` (a heuristic on the command's own shape must not deny). `commit ; tag` keeps firing in `tag-after-commit`; here `git commit` is not in the fallible set, so the two rules never report the same pair.

## Phases

1. **The rule and its unit tests.** Model on `tag_after_commit.rs:20-82`. Unit tests include `echo $(git push); make;` (no finding), a newline pair, a heredoc `python3 - <<EOF` then `cargo test`, and `set -e; sed -i s/a/b/ f; git commit -m x` (no finding).
   - add `crates/amont-agent-guard/src/rules/chain_with_and.rs` (with a `#[cfg(test)]` module)
   - edit `crates/amont-agent-guard/src/rules/mod.rs` (`pub mod chain_with_and;`, alphabetical)
   - edit `src/rule_table.rs` (`RULES` entry, next to `tag_after_commit::RULE` at :29)
2. **The corpus, then the backtest.** At least 8 `match` (the Python incident verbatim, `git add -A ; git commit`, newline and heredoc variants) and at least 8 `nomatch` (the `xargs` incident with its note, `echo x ; git commit`, `cd d ; git push`, `a && b`, `git status ; git commit`, `cargo test -p a ; cargo test -p b`, `python x.py | tail -3 ; git commit`, the `set -e` case, an opaque A, a nested `$(…)`). Seeded with `amont-agent explain chain-with-and --format cases`, labelled by hand. Gate: 12 reviewed, 4 negatives, 0 disagreements (`graduate.rs:31-47`). Before the `RULES` entry ships, `amont-agent backtest` over the local transcripts: firings per 1000 calls split by `;` and newline; above 5 per 1000, narrow the sets first. The number goes into `evidence.per_1000` and the PR.
   - add `crates/amont-agent-guard/tests/corpus/chain-with-and.cases`
   - edit `crates/amont-agent-guard/src/corpus.rs` (`EMBEDDED` tuple)
3. **Docs.**
   - edit `docs/rules.md` (row: `chain-with-and` | `observe` | an edit, add or test joined by `;` to a commit, push or test that runs even when it fails)
   - `tests/rules_json.rs` unchanged: its list (`:198-212`) holds the irreversible ids only, and this rule is `irreversible: false`

## Verification

- `cargo test -p amont-agent-guard`: the table tests (`every_rule_has_an_embedded_corpus`, `rule_ids_are_lowercase_kebab`, `no_rule_ships_above_its_ceiling`, `prevents` under 140 characters) and the rule's unit tests pass.
- `amont-agent corpus check` lists `chain-with-and` with its reviewed counts and precision 100%.
- Piloted run: feed `git add -A ; git commit` and the Python-then-`cargo test` incident to the built `amont-agent hook` with a scratch state dir. At `observe` the stance prints nothing (`src/hook.rs:315`), so check the journal for a `watched` row, then rerun with `git config amont.agent.chain-with-and.stance advise` and read the advise text. The same commands joined with `&&` produce none.
- `amont-agent rules` at 80 columns shows the new row without wrapping.

## Non-goals

- No general `set -e` or `pipefail` detection beyond the top-level `set -e` skip; no rule on `||`.
- No change to `pipe-to-tail` or `tag-after-commit`.
- Not graduating past `advise`; graduation to `advise` waits for the corpus gate and a week of observe rows.
- The memory note stays; once the rule is at advise its MEMORY.md index line is dropped.

## Code review

- amont-agent: backend, lang:rust, unix (tui left out: the only output is one finding sentence)

## Verification record

| Check | Input | Expected | Actual |
|---|---|---|---|
| `make check` | the branch tree | fmt, clippy, tests green | pass (relais run, then re-run by hand after one clippy fix and the review fixes) |
| `amont-agent corpus check` | `chain-with-and.cases` | 12+ reviewed, 4+ negatives, 0 disagreements | 29 reviewed (16 negative), precision 100% |
| piloted hook at `advise` | `git add -A ; git commit -m x` | one finding | one finding, reason names `git add` / `git commit` |
| piloted hook at `advise` | `git add -A && git commit -m x` | none | none |
| piloted hook at `advise` | `python3 -c "assert 0" ; cargo test` | one finding | one finding (beside `heavy-build-unqueued`) |
| `amont-agent backtest --rule chain-with-and --json` | 81,433 local calls | under 5 per 1000 | 261 firings, 3.2 per 1000; 244 of them multi-line commands (a heredoc edit then a test or build), 17 single-line; measured 2026-11-09 |
| `amont-agent rules` | built binary | row at `observe`, `3.2/1000`, `(max advise)` | as expected |

## Decision log

- 2026-11-09: the plan went to implementation with `xargs` unmatched and the id `chain-with-and` (the 👉 defaults).
- 2026-11-09: relais run 1 was refused (a check name the profile does not define), run 2 needed a trust grant (the remote moved to greenpiste), run 3 ended `budget_exhausted` with a candidate that failed one clippy lint. The candidate was applied by hand from the run's preserved patch and the lint fixed (`next_back`).
- 2026-11-09: `setopt`/`unsetopt` errexit is read as well as `set -e`, which the plan's Behaviour does not name. It only narrows firings.
- 2026-11-09: the `Evidence.trend` is `Rare`; the 3.2 per 1000 is the backtest above.

## Implementation review

- **implementation-review**: round 1 approve-with-changes (fixed: holds-until marker, opaque-A corpus case, comment wrap; per_1000 source recorded above); delta approve-with-changes, low only.
- **backend**: round 1 approve-with-changes (fixed: holds-until, `set -o errexit` test, dry-run and `gh pr view` exclusions, step names); delta approve-with-changes, low only.
- **lang:rust**: round 1 approve-with-changes (fixed: step names, comment); delta low only. `deliberate:` `Option<bool>` for errexit, duplicated operand filters, `gh` arm keyed apart from the other sets: refactors, carried.
- **unix**: round 1 approve-with-changes (fixed: `setopt errexit`); delta low only. `deliberate:` `setopt no_err_exit` is not read as turning errexit off (after `set -e` the pair stays silent; the rarer reverse fires), and a python script run counts as an edit; both only change firings in the safe direction and the observe week will show how often.

<!-- panel: repos=amont-agent adds= reviewers=backend,lang:rust,unix,tui body-sha=f4fa4379374a -->
