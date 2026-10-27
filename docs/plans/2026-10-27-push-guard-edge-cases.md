---
status: active
branch: fix/push-guard-edge-cases
repos: amont-agent
adrs: [ADR-0042]
---
# Plan: push guard edge cases

## Review panel

⚠ unreviewed: the person asked for these two fixes directly ("I want the 2 push guard edge cases fixed", 2026-10-27); each is small and bounded, and the implementation review still runs before the push.

## Context

Seen on 2026-10-27, while setting up the holdouts store
(`fredericrous/holdouts`, a new Forgejo repository):

1. **A shell variable in `git -C`.** The command
   `S=~/.local/share/store; git -C $S switch -c x && git -C $S push -u origin x`
   was held by `implementation-review` with "the push cannot be read: not a
   git repository". `push_target::read` refuses any expanded `-C` word
   (`a word comes from a substitution`), and the session cwd was not a
   repository, so the reason named the wrong cause. Holding is right, but
   agents write `VAR=path; git -C $VAR …` for long paths all the time, and
   the refusal sent the session to the wrong fix. `Context::cwd_at` has the
   same gap for `cd $VAR`: it falls back to the session cwd and asks the
   wrong repository.
2. **`amont-agent check` is not what the hook decides.** `run_check`
   (`src/main.rs`) calls `rules::evaluate`, which runs each rule's
   `examine` only. A rule with a `confirm` step (`push-preview`,
   `implementation-review`, `push-preflight`) is printed as if it fired,
   with its stance. On the holdouts store, a repository with no interface
   and no plan, `check` printed `push-preview [deny]` and
   `implementation-review [deny]`; the hook itself confirms neither there.
   A session that tests a command with `check` reads a verdict the hook
   would never give.

## Goal

The push guard reads a `-C` (and `cd`) directory written as a variable
that the same command line assigned to a literal, and says exactly why when
it cannot. `amont-agent check` reports what the hook would decide in the
directory it is run from.

## Non-goals

- Reading variables from the environment, from a substitution, or from an
  earlier tool call. Only a literal assigned on the same command line is
  read; everything else stays unresolvable.
- Changing what any rule decides. Only how a command is read, and what
  `check` reports.

## Behaviour

**Phase 1: variables in `-C` and `cd`.**
- In a command line, a clause made only of assignments (`S=/a/b`,
  `S='/a b'`, `S="$HOME/x"` with `$HOME` alone expanded), connected by `;`
  or `&&` before a `git -C $S …`, `git -C "$S" …`, `git -C ${S} …` or
  `cd $S`, gives that word its literal value. The last assignment before
  the clause wins. An assignment inside a substitution or after the clause
  does not count, and neither does a value that itself holds another
  expansion (other than a leading `$HOME`) or a `~` that is not leading.
- `$HOME/...` and `${HOME}/...` in `-C` and `cd` resolve against `HOME`,
  as `~/` already does.
- A `-C` or `cd` word that is still an expansion makes the push
  `Unresolvable` with the shape `the -C directory is a shell variable this
  guard cannot resolve (assign it a literal on the same line, or write the
  path)`, or `… cd directory …` for `cd`. It is never `not a git
  repository`.
- `push_target::read` stays pure: it takes the resolved word values from
  the caller (`Context`), which owns the clauses.

**Phase 2: `amont-agent check` decides as the hook does.**
- `check` takes `--cwd <dir>` (default: the process's own directory) and
  runs each fired rule's `confirm` in a `Context` built for that directory,
  the way `src/hook.rs` does.
- A rule whose `confirm` says no is not printed as firing. With
  `--explain`, it is listed as `not confirmed: <why>`.
- A rule whose `confirm` says yes at another stance prints that stance.
- `check`'s output and exit status are otherwise unchanged, so existing
  callers and the backtester keep working. The backtester keeps calling
  `evaluate` directly.

## Phases

- [x] Phase 1 — `push_target` and `cwd_at` read same-line literal variables; the new unresolvable shape.
- [x] Phase 2 — `amont-agent check` runs `confirm` in a context for `--cwd`.
- [ ] Release amont-agent so the installed hook carries both.

## Decision log

- 2026-10-27 — Edge case 2 was first misdiagnosed. The hook itself did not ask for a preview or a review on the holdouts push; `amont-agent check` printed shape matches as verdicts. The fix moved to `check`.
- 2026-10-27 — The relais successor for phase 1's review fixes was accepted with an unchanged candidate (2e2b541, the same as its predecessor), all criteria marked met by profile checks. I made the fixes by hand and recorded it with relais (`feedback corrected`).
- 2026-10-27 — The panel was not run: the person asked for these fixes
  directly, and the implementation review covers the code. The plan
  records that.

## Verification

Observed 2026-10-27 on `fix/push-guard-edge-cases` with the built binary (`./target/debug/amont-agent check --explain`) and the test suites.
- Phase 1: `S=<relais path>; git -C $S push origin HEAD:feat/x`, from `--cwd /tmp` → read as relais → observed: `push-preview not confirmed: not a repository with a user interface`, `implementation-review not confirmed: nothing-new`, and `push-preflight not confirmed: no test gate runs at push time here`. All three asked relais, not `/tmp`. The first run used `feat/x`, which doesn't exist; the held push was then the unresolvable refspec, not the variable.
- Phase 1: `git -C $UNSET push -u origin feat/x` → observed: `the -C directory is a shell variable this guard cannot resolve (assign it a literal on the same line, or write the path)`, never `not a git repository`.
- Phase 1: `S=/x; cd $S && git push origin feat/x` → `cwd_at` gives `/x` (test `a_cd_to_a_variable_set_on_the_line_is_followed`), and `directory_for` gives `/b` for `S=/b; cd $S` (table `a_push_runs_in_its_cd_and_c_directories_with_same_line_variables_read`, which also covers `-C`, `"$S"`, `$NOPE` and `cd $NOPE`).
- Found while verifying, and fixed: `push-preflight` ignored `git -C` entirely, literal or variable. It now uses `push_target::directory_for`, so `git -C <relais> push` from `/tmp` reads relais's own pre-push gate.
- Phase 2: `check --cwd <holdouts store> --explain 'git push -u origin docs/store-url'` (no interface, no plan) → observed `no rule fires`, with `push-preview not confirmed: not a repository with a user interface` and `implementation-review not confirmed: nothing-new` listed under `--explain`. Test `tests/check_confirm.rs`, run against the real binary in a temp repository, covers push-preview not firing, its `not confirmed:` line, and an unconfirmed rule (pipe-to-tail) still printing.
- Phase 2: the precision suite passes from a fresh single-worktree clone (`git worktree list` = 1): `cargo test --test precision --test check_confirm`, ok, 55 s.
- The second Phase 2 row (a repository with an unapproved interface change) is not run separately; `push-preview`'s own confirm tests cover that branch, and `check` calls the same `confirmed()`.

## Outcome

(Filled when the plan closes.)

## Implementation review

- Round 1: approve-with-changes (71k tokens, 118 s). Fixed in the next commit:
  - variables changed by `+=`, `unset`, `read`, `export`/`declare`/`local`/`typeset` or a `for` loop are no longer read as their earlier literal;
  - push-preflight falls back to the clause directory except for the shell-variable shapes;
  - the bare-stash-pop and stale-base remedy checks run in a temp repo;
  - `final_stance` takes no bool;
  - `ctx` and `w` are renamed.
- The relais worker also changed the lexer to read `do`/`then`/`else` as clause openers. That made every push rule judge pushes inside loop and `if` bodies, which this plan's non-goals rule out, so I reverted it. The `for` row is valid bash with a prior literal; a mutation of the `for` handling turns it red. Whether pushes inside `do … done` should be judged is a separate question for its own change.
- Delta: approve-with-changes (94k tokens, 77 s). All five round-1 findings resolved. New finding A (high): an assignment or `unset` behind a shell keyword (`then S=/c`, `do unset S`) left the old literal. Fixed conservatively: a clause that opens or continues a compound command clears every variable read so far, and any `NAME=` word in a clause that is not assignment-only clears that name. `eval`, `source` and `.` are already refused by the lexer ('no opinion'). A mutation of the clear-all path turns the table red. Known over-caution: `env S=/c cmd` refuses rather than misreads.
- Fresh round 1 on the delta fix: approve-with-changes (61k tokens, 83 s). Fixed:
  - the `cd` target is renamed `cd_target`;
  - a compound command now clears every name not assigned again, `HOME` included, and a mutation of that turns the `HOME` row red;
  - only a clause's command word counts as a compound keyword, so `git add .` resolves (new row).
