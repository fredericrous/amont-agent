# The implementation review

Before the push of a branch that carries a plan, one independent reviewer
has read the diff against that plan and the repository's active rules, and
its verdict is recorded in the plan (ADR-0022, fleet rule
`work.implementation-review`). The worktree-task skill runs the review
(step F4b). The `implementation-review` rule checks, at `git push`, that it
happened for the tree being pushed.

The plan step has a panel; the quality loop has one reviewer. A diff with
its surrounding code is larger than a plan and arrives on every push, and
the reviewer's job is narrower: findings only, never an edit, never a
waiver.

## What the reviewer reads

Its prompt carries, in order: the review block, `Round 1.` or `Delta.`, a
brief the session builds once, and its output contract. The brief is the
landed plan's path and Verification section, the diff against the default
branch without `docs/plans/` (its stat when it is large), the repository's
active constraints (`aval rules --level constraint`), and the plan's
Non-goals. It checks plan conformance, that every recorded verification is
an observable check, violated constraints (rule id and `file:line`), tests
that cannot fail, and scope past the phases or the Non-goals.

Its result begins with the verdict, exactly, because the hook reads it:

```
Verdict: approve | approve-with-changes | rework
Findings:
1. [blocking|high|medium|low] <problem>. Evidence: <file:line>. Rule: <id or —>. Edit: <concrete change>.
Would still check by hand: <one to three things>
```

## What a review binds to

The **canonical tree id**: the sha256 of `git ls-tree -r` of the commit
with every entry under `docs/plans/` left out, 64 hex in any object format,
computed read-only. Recording the review in the plan therefore never makes
it stale; a code change does.

```sh
amont-agent tree-sha [--block] [-C <dir>] [--] [<rev>]
```

`--block` prints the block the reviewer's prompt carries:

```
<<<TREE repo=<name> sha=<64 hex>>>>
```

`<name>` is the basename of the directory holding the repository's common
`.git`, so a worktree checked out as `amont-agent-wt-x` says `amont-agent`.
Exit 0; 1 outside a repository or for a rev that names no tree (the reason
on stderr); 2 on a usage error.

## What the hook does at a push

1. Resolves the push (`crate::push_target`): the repository and the commit
   each refspec publishes. A dry run, a push to `main`/`master`, and a
   push of tags only are not its business.
2. **No plan on the branch**: `docs/plans/` unchanged between
   `merge-base(<pushed>, <base>)` and the pushed commit, where `<base>` is
   the first of `<remote>/HEAD`, `<remote>/main`, `<remote>/master` that
   exists, never the branch's own tracking ref and never `HEAD`. The rule
   declines, silently.
3. Computes the canonical tree of the pushed commit.
4. Looks for a binding: a completed `implementation-review` agent in this
   session's transcript whose block names this repository and tree, or a
   pass remembered under `~/.claude/amont-agent/implementation-review/by-tree/`.
   Completion is read from structured fields only, as the review panel
   reads it; a notification echoed inside a tool result never counts.
5. **Reads the verdict.** A review that said `rework` and got no later
   delta on the same tree is not a pass.
6. On the first transcript binding that passes, writes the pass file
   (0700 directory, 0600 file), so a new session pushing the same tree
   passes without a transcript.

| situation | `advise` (ships) | `deny` |
|---|---|---|
| reviewed this tree, verdict not `rework` | silent | passes |
| no plan on the branch | silent | silent |
| reviewed an older tree | note, names both trees | held, names both trees |
| verdict `rework`, no delta | note | held |
| never reviewed | note | held |
| transcript unreadable, git failed, no default branch known | note | held |
| a push shape this guard cannot read (`--all`, `--mirror`, …) | journalled, passes | held |

Every note ends with the next step:

```
amont-agent/implementation-review: no implementation review of amont-agent tree 5085edae1d66 is in this session. worktree-task F4b: `amont-agent tree-sha --block`, then launch the implementation-review agent …
```

## The person's overrule

A `rework` that survives the delta goes to the person on a marked
question: its text starts with `[implementation-review <repo>@<sha64>]`
and its options are exactly `Overrule`, `Fix`, `Hold`. As with the preview
approval, the hook records the question before it runs and trusts the
answer only when that record exists and the question did not arrive with
`answers` pre-filled. On `Overrule`, and only when this session's
transcript shows a completed review of that tree that said `rework`, the
hook writes the pass file with `verdict: overruled`. An overrule that
bound nothing says so in the model's context.

## Only the hook writes a pass file

Under `deny` a pass file is a trust boundary. A Write, Edit or MultiEdit
under `~/.claude/amont-agent/implementation-review/` is refused, with `.`
and `..` folded before the comparison; a Bash command whose words or
redirect targets name that directory is refused too, whatever the rule's
stance. The Bash half is best-effort by nature: a parser cannot see inside
`python -c`.

## What the journal records

One line per push, under the rule's id, in the buckets `amont-agent
status` counts: a declined push as `unconfirmed` with the reason
(`reviewed`, `overruled`, `no-plan`, `dry-run`, `no-branch`,
`unresolvable`), a spoken one as `advised`, `denied` or `watched`. The
excerpt of a spoken line is structured:

```
tree=<12 hex> review=<stale|rework|missing|unknown> verdict=<approve|approve-with-changes|rework|->
```

Away from a push: `overruled`, `answered`, `unbound`, `unknown`,
`prefilled`, `unrecorded` for the marked question, and `denied` for a
refused write. The soak reads them with:

```sh
grep -E ' implementation-review ' ~/.claude/amont-agent/journal.log
```

## Stance

Ships `advise`. `git config --global amont.agent.implementation-review.stance
deny` holds a push whose review is missing, stale, `rework` or unknown;
`observe` journals and says nothing. The 2026-10-06 soak review, alongside
`push-preview`, decides between `deny`, `advise` and retiring the gate from
the journal's `review=` counts and the first real cycles' finding quality.
