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

- [ ] Phase 1 — `push_target` and `cwd_at` read same-line literal variables; the new unresolvable shape.
- [ ] Phase 2 — `amont-agent check` runs `confirm` in a context for `--cwd`.
- [ ] Release amont-agent so the installed hook carries both.

## Decision log

- 2026-10-27 — The panel was not run: the person asked for these fixes
  directly, and the implementation review covers the code. The plan
  records that.

## Verification

- Phase 1: `amont-agent check --cwd <a repo> 'S=<repo path>; git -C $S push -u origin feat/x'`
  → the push is read as the repository at `<repo path>`, not as
  unresolvable → (to fill).
- Phase 1: `… 'git -C $UNSET push -u origin feat/x'` → the shape names a
  shell variable, never `not a git repository` → (to fill).
- Phase 1: `S=/x; cd $S && git push origin feat/x` → `cwd_at` gives `/x`
  → (to fill).
- Phase 2: `amont-agent check --cwd ~/.local/share/relais-holdouts-store-pending 'git push -u origin feat/x'`
  (a repository with no interface and no plan in the branch) → neither
  `push-preview` nor `implementation-review` is printed as firing →
  (to fill).
- Phase 2: the same command with `--cwd` set to a repository with a `dev`
  script and an unapproved interface change → `push-preview` is printed →
  (to fill).

## Outcome

(Filled when the plan closes.)
