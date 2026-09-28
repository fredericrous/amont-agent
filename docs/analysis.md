# Shell analysis

`src/analysis/` answers one question about a single Bash command, before it
runs: which network transfers it may make, and which it must, with the
evidence for each count. `request-fanout` is its first consumer. The analysis
decides nothing; the rules do.

## Layers

| layer | module | job |
|---|---|---|
| frontend | `analysis::frontend` | source → crate-owned IR, spans on every node |
| semantics | `analysis::interp`, `state`, `domain` | abstract interpretation: state, control flow, exit statuses, counts |
| command models | `analysis::models` | a client's arguments → its explicit transfers |
| policy | `rules::request_fanout` | whether the result warrants advice, and the words |

Everything in `analysis` is pure. The dialect is an input; nothing reads the
environment, the filesystem or the network. The legacy lexer
(`src/shell.rs`) is untouched and still serves every other rule.

## Inputs

- **The command text.**
- **The dialect** — `bash`, `zsh` or `unknown`. The hook reads it from
  `SHELL`; `check --dialect` sets it; the backtester always uses `unknown`,
  because transcripts do not record the shell. Where bash and zsh differ, an
  `unknown` dialect gets the join of both readings — never one of them
  silently.

Dialect differences that are modelled: whether the last element of a
pipeline runs in the current shell (zsh yes; bash only under
`shopt -s lastpipe`); word-splitting of unquoted parameters (bash splits,
zsh does not); an unmatched glob in a URL-shaped word (bash passes it
through; zsh refuses the command).

## Assumptions

Every "established" count holds under these, and the rule's message lists
them:

- the shell is not killed from outside;
- redirections succeed;
- `errexit` and `pipefail` are off unless the command sets them (then they are
  modelled);
- no aliases are defined;
- inherited environment variables are unknown;
- the command has no positional arguments of its own.

## What is counted

**Explicit command-line transfers**: the requests a client's arguments ask
for. Redirects, authentication hops, a client's own retries and pagination,
and defaults from `~/.curlrc` are named as possible extras and never counted.

A count is an interval: at least `lower`, at most `upper`, where `upper` is a
number, *saturated* (too large to represent), *uncapped* (positive evidence
that nothing bounds it: a loop following next-page links, `--paginate`,
`wget -r`, `while true` with no way out), or *unknown* (no evidence either
way). Zero annihilates: code that provably never runs makes no transfers,
whatever it would have done.

A finite upper bound on a loop comes only from the **counter pattern**: a
literal initial value, a comparison as the condition's final command, exactly
one unconditional `+1` of the counter in the body, and no other write to it —
including from a function the body calls, or from anything unmodelled — and
no `continue` that could skip the increment.

## The supported subset

Sequences, `&&`/`||`, pipelines and `!`, `if`/`elif`/`else`, `case` with `;;`,
`;&` and `;;&`, `for`, `for ((…))`, `while`, `until`, `{ }`, `( )`, `(( ))`,
`[[ ]]`, `[ ]`/`test`, function definitions in both forms, `local`/`typeset`,
`break`/`continue` with levels, `return`, `exit`, `set -e`,
`set -o pipefail`, `shopt -s lastpipe`; `$( )` and backticks, `$v`, `${v}`,
positional parameters, `$@`/`$*`, the `${v:-x}` family (not evaluated: its
value is unknown), every quoting form, unquoted brace expansion.

## Unknown and Incomplete

- **Unsupported** constructs — `eval`, `source`, `trap`, `xargs`,
  `parallel`, a command name built at run time, a program that runs another
  program — may do anything: every variable becomes unknown, the command may
  exit or never end, and the region is reported as activity the analysis
  could not follow. It never erases a known count elsewhere in the command.
- **Incomplete** means a resource limit ended the analysis (nesting depth,
  IR size, interpreter steps, call depth, recursion). What was found before
  it is kept; nothing after it is known.
- Cardinalities — `{1..999999999}`, `seq`, curl URL ranges — are computed
  arithmetically and never expanded.

A rule built on the analysis never fires on an unknown count alone.
