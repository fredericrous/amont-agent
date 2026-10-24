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

The **content id**: the sha256 of `git ls-tree -r -z` of the commit with
every entry under `docs/plans/` left out, 64 hex in any object format,
computed read-only (attest SPEC, "Content id"). Recording the review in the
plan, or adding a plan, therefore never makes it stale; a code change does.
It is not attest's canonical tree, canon(T), which counts whether a plan
file exists; `amont-agent tree-sha` prints the content id.

A pass also holds for another tree with the same **branch patch**, when the
repository's rule files are unchanged (ADR-0034,
`work.implementation-review-binding`). The patch is `git diff --binary
<merge-base> <pushed> -- . ':!docs/plans'` hashed by `git patch-id
--verbatim`: whitespace counts, index lines and hunk line numbers do not.
The rules are the entries of `AGENTS.md`, `CLAUDE.md`, `.adr.yaml`, `.adr/`
and `.claude/` in the pushed tree. So a rebase onto a moved `main` that
does not touch the patch or the rules keeps the review. A rebase that
resolved a conflict, changed a line of the patch's context, or brought new
rules does not. A person's overrule carries over on the same terms, once
the overruled tree itself has been pushed (the push files it under its
patch); journalled `overruled-by-patch`. When the patch cannot be computed,
the held message says why.

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
3. Computes the content id of the pushed commit.
4. Looks for a binding: a completed `implementation-review` agent in this
   session's transcript whose block names this repository and tree, or a
   pass remembered under `~/.claude/amont-agent/implementation-review/by-tree/`.
   Completion is read from structured fields only, as the review panel
   reads it; a notification echoed inside a tool result never counts. A
   reviewer **resumed** with `SendMessage` on a new block counts as a
   review of that block ([below](#a-resumed-review)).
5. **Reads the verdict.** A review that said `rework` and got no later
   delta on the same tree is not a pass.
6. On the first transcript binding that passes, writes the pass file
   (0700 directory, 0600 file), so a new session pushing the same tree
   passes without a transcript. The pass is also filed under `by-patch/`
   with the patch, the rules fingerprint and the tree.
7. **No binding for this tree**: looks the branch patch up under
   `by-patch/`. A pass there whose rules fingerprint matches is copied to
   `by-tree/` for this tree (`"how": "patch <old tree>"`), and the push
   passes as `reviewed-by-patch`. A patch or fingerprint that cannot be
   computed is a miss.

| situation | `advise` (ships) | `deny` |
|---|---|---|
| reviewed this tree, verdict not `rework` | silent | passes |
| reviewed the same branch patch, rules unchanged | silent | passes |
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

## A resumed review

The delta (F4b.6) may resume the same reviewer with `SendMessage`, giving
it the new block, instead of launching a fresh one. Each round is a review
of its own, read from the shape a real session recorded
(`tests/fixtures/sendmessage-resume.jsonl`, 2026-10-08):

1. the launch's result carries `toolUseResult.agentId`;
2. a `SendMessage` whose `input.to` is that agent id, and whose result
   carries `toolUseResult.resumedAgentId` = the same id, starts a round:
   its prompt is `input.message` (the block is read from it) and its id is
   the SendMessage's own;
3. the round completes only on a structured task notification whose
   `<tool-use-id>` is that SendMessage's id and whose `<task-id>` is the
   agent id. The launch's own result is never overwritten.

A message to an agent that was not launched as `implementation-review`, a
round whose notification names another task, or a notification typed or
echoed rather than queued by Claude Code does not count. Only this rule
reads resumes: the plan-review panel counts fresh launches alone.

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
and `..` folded before the comparison; a Bash command that would write
there — a redirect into it, or `rm`, `cp`, `tee`, `python`, `chezmoi` and
the like with a word naming it — is refused too, whatever the rule's
stance. Reading the record (`ls`, `cat`, `find`, `grep`) is allowed. The
Bash half is best-effort by nature: a parser cannot see inside `python -c`
beyond its words.

`~/.claude/amont-agent/provenance/` is guarded the same way (below).

## The provenance record

When the hook passes a push that carries a plan, it writes the unsigned
`amont-provenance-v1` payload to
`~/.claude/amont-agent/provenance/<repo>/<pushed sha>`. The format is attest
SPEC `amont-provenance-v1`; the decision is ADR-0036. One datum per line:
`amont-provenance-v1`, `commit`, `tree` (the full tree), `patch` (only when
the branch patch could be computed), `plan <path> <body sha>` (the plan
under `docs/plans/` changed on the branch, read from the pushed tree),
`review <reviewed|by-patch|overruled> <content id the review read>`,
`preview <approved|evidence|not-required>` and `producer amont-agent
<version>`. There is no `canon` line: attest's canonical tree is a tree fact
amont-agent does not own, and the signer, amont, computes it and inserts it
before signing. A payload that carries one is refused by the signer.

It is written on `reviewed`, `reviewed-by-patch`, `overruled`,
`overruled-by-patch` and on a transcript pass; never for a held push or a push
declined as no-plan, no-branch, dry-run or nothing-new. A write that fails is
journalled `provenance-write-failed` and never changes the judgment; a write
that lands is journalled `provenance-written`. The record is written once
per repository and commit: a re-push of the same commit finds the file and
keeps it (the first pass's origin stays), journalled `provenance-kept`.
The `preview` datum is the preview gate's own decision: `evidence` when the
plan's evidence stood, `approved` when the person's approval did,
`not-required` for no interface change. While the preview has not stood
(the gate holds the push), no record is written, journalled
`provenance-deferred`: the push that passes writes it. Like the pass files, the
directory is hook-only: Write, Edit and Bash writes are refused.

## What the journal records

Every push writes its decision line, under the rule's id, in the buckets
`amont-agent status` counts: a declined push as `unconfirmed` with the reason
(`reviewed`, `reviewed-by-patch`, `overruled`, `overruled-by-patch`,
`no-plan`, `dry-run`, `no-branch`, `unresolvable`), a spoken one as `advised`, `denied` or `watched`. The
excerpt of a spoken line is structured:

```
tree=<12 hex> review=<stale|rework|missing|unknown> verdict=<approve|approve-with-changes|rework|->
```

A push that passed may write a second line. Its review is compared with the
rules as they are now, advise-only and never holding the push; when they
differ, a `rules-checked` line is journalled. `status` counts it in no bucket,
because its outcome, `rules-checked`, is a note beside a decision, not one.
Its excerpt is
`tree=<12 hex> review=<rules-missing|rules-stale|rules-unknown>`:
`rules-missing` when the pass stored the sha of no rules line, `rules-stale`
when the rules changed since, `rules-unknown` when the pass predates the
stored sha or `aval` could not answer.

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

## Recording what a review found

```
echo '[{"severity":"high","disposition":"fixed","section":"Verification"}]' |
  amont-agent implementation-review record <plan.md> --verdict approve-with-changes
```

Appends one `amont-agent-impl-review-outcome-v1` row per invocation to
`~/.claude/amont-agent/implementation-review-outcomes/outcomes.jsonl` (outside
the hook-only store): `recorded_at`,
`tree` (the content id of HEAD, as `tree-sha`), `round` (one more than the rows
already kept for the plan: a per-plan count, unlike the plan-review ledger's
`round`), `plan_path`, `model` (the `resolvedModel` of the completed
`implementation-review` agent in the newest transcript of the working
directory's session, empty when none), `verdict` and
`findings`. Each finding on stdin is `{severity: blocking|high|medium|low,
disposition: fixed|deliberate|overruled, section?}` and is rebuilt from those
keys alone; `--verdict` is
`approve`, `approve-with-changes` or `rework`. A bad severity, disposition or
verdict exits 2 and appends nothing; an unreadable stdin exits 1. When a pass
is remembered for the tree and its verdict differs from `--verdict` (a remembered
`overruled` counts as agreeing with `rework`), a warning
goes to stderr. The command writes only `outcomes.jsonl`
and its lock file, never a pass file. `amont-agent plan-review stats` reads
this ledger and reports it as role `implementation-review`, `fixed` counting as
accepted.
