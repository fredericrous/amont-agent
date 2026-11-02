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

After the `review` line, one `code-review <role> <how> <content id>` line
records each code-review profile the plan declares for the repository, in
declaration order: `how` is `reviewed`, `by-patch`, `carried` or `overruled`,
and the content id is the tree that profile's review read. A push with no
declared profile writes no such line. `amont-agent doctor` (and `doctor
--json`) reports one gap for each code-review agent file
(`code-review-<role>.md`, `code-review-language.md`, `interface-review.md`)
that no agents directory carries, and says nothing when all eight are there.

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

A push that passed writes a second line. Its review is compared with the
rules as they are now, advise-only and never holding the push, and a
`rules-checked` line is journalled. `status` counts it in no bucket, because
its outcome, `rules-checked`, is a note beside a decision, not one. Its
excerpt is `tree=<12 hex> review=<rules-current|rules-missing|rules-stale|rules-unknown>`:
`rules-current` when the review read the rules as they are now (the count the
rollout reads before the check may deny), `rules-missing` when its prompt had
no rules line, `rules-stale` when the rules changed since, `rules-unknown`
when the pass predates the stored sha or `aval` could not answer.

A third line, `candidates-checked`, records what the review did with the
candidates adr-lint listed. Its excerpt is `tree=<12 hex> review=<...>`:
`candidates-ruled n=<n>` when every listed id has a ruling,
`candidates-undisposed n=<k>/<n>` when `k` of `n` do not,
`candidates-unlisted` when the reviewer's prompt had no block,
`candidates-unknown` when the pass predates the stored candidates, and
`candidates-unavailable` when it had the unavailable marker. It never changes
the judgment, and `status` counts it in no bucket.

## Candidates and rulings

`amont-agent review-brief` runs `adr-lint candidates --base <merge-base>
--json --max 40` in the repository (`adr-lint` from PATH, or the program
named by `AMONT_AGENT_ADR_LINT`; a bounded wait; stdin from /dev/null). A
candidate is a line adr-lint marked on a line the branch adds, for the
reviewer to rule on; adr-lint owns the document (`adr-lint.candidates/1`)
and the ruling record (`adr-lint.ruling/1`). The reader refuses any other
`format` token and ignores unknown fields.

Right after the RULES line the brief prints a block to paste verbatim into
the reviewer prompt:

```
<<<CANDIDATES sha=<sha256 of adr-lint's stdout> n=<count>>>>
<id> <rule> <pattern_id> <path>:<line>
<<<END CANDIDATES>>>
```

and a `## Candidates` section in the body with each candidate's id, place,
rule, pattern message and text, the per-rule overflow counts, and the
instruction for the result. When adr-lint is missing, fails or prints
another format, the block is `<<<CANDIDATES unavailable>>>`, the section
names the cause, stderr carries one line, and the brief still exits 0.

The reviewer's result carries a `Candidates:` block with one line per listed
id: `<id> violation: <finding>` or `<id> fine: <one clause quoted from the
rule body>`. An id is the full 64 hex or a unique prefix of at least 12.

On a passing review (the transcript, the tree memory and the patch memory
alike; the pass file keeps what the prompt listed and the result ruled) the
push check journals the `candidates-checked` line above and appends, for each
ruling, one `adr-lint.ruling/1` line to
`~/.claude/amont-agent/rulings/<repo>.jsonl` (created 0600 in a 0700
directory, appended to and never rewritten): `format`, `review` (the
reviewer's agent id), `tool` (`amont-agent`), `repo`, `tree` (the content
id), `candidate`, `rule`, `pattern_id`, `pattern` (the message from the
brief's section, the rule id when unknown), `ruling`, `clause` and `at`
(RFC 3339 UTC). A retried push of the same review appends no second record
for the same review and candidate. `adr-lint ruling-record --check <file>`
validates the file.

A repository with no `.adr.yaml` gets a brief with no RULES line and a
`# Code rules` section saying that no decided code rules apply (stderr says
so in one line); `rules-sha` there still exits 3.
A repository with `.adr.yaml` where aval is not installed (`AMONT_AGENT_AVAL`
unset and no `aval` on PATH, not even a non-executable file or a dangling
symlink) also gets the brief, with no RULES line and a `# Code rules` section
saying the decided code rules were not read; stderr says so in one line. The
`review-brief` exit codes: 0 the brief was printed (aval not installed
included), 1 a repository error, 3 an installed aval failed or
`AMONT_AGENT_AVAL` points nowhere. A row of `aval relevant`
without a string `key` is an aval error (exit 3) naming the row.

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

## Per-profile briefs

`amont-agent review-brief --profile <role>` (`backend`, `platform`, `unix`,
`tui`, `react`, `ui-design`, `architect` or `lang:<x>`) adds a
`Profile: <role>` line after the TREE, RULES and CANDIDATES lines and, after
the code rules, a `## Judge rules for <role>` section: the full text of each
active code rule whose `**Checked by:**` classes are all `judge` and whose
`**Reviewed by:**` line names the role (`language` matches any `lang:<x>`).
A `Reviewed by:` name outside that set makes the command exit 1 naming the
rule and the name; an unknown `--profile` exits 2. Without `--profile` the
brief is unchanged except for one line in the plan section, `Declared code
review for <repo>: <profiles|none — reason|not declared>`, read from the
plan (the canonical file of a pointer plan, under the plan root).

## The declared profiles at a push

When the plan declares code-review profiles for the repository, the push
needs a pass for each of them on the pushed tree, beside the implementation
review (ADR-0052). The hook reads the declaration from the plan(s) changed
under `docs/plans/` between the merge base and the pushed commit, as that
commit holds them; a pointer plan (`canonical: <repo>:<path>`) is read
through its canonical file under the plan root. A canonical file or a
`## Code review` section that cannot be read is held under `deny` and
journalled (`profiles-unreadable`) otherwise. A plan with no section, or a
repository line `none — …`, requires the implementation review alone; a
plan with no section journals `undeclared-profiles`.

A completed launch counts for a profile only when its agent is the one the
profile maps to (`lang:<x>` to `code-review-language`, `ui-design` to
`interface-review`, any other role to `code-review-<role>`) and its prompt
carries exactly one `Profile: <role>` line naming that profile, in its
header. Two lines, or a line naming another profile, count for nothing. The
header is every line before the first that opens the body: a Markdown heading
(`#` to `######` then a space), a code fence (three or more backticks or
tildes), or an unfenced diff (`diff --git`, `@@`). So an inlined brief, which
starts with `# Implementation review brief`, and an inlined diff never add a
line. The launch's own `Profile:` line must come before them: a prompt that
opens with the inlined brief and carries its line only there counts for
nothing. The review block (`<<<TREE …>>>`) a profile review binds is read
from the same header, so a block inside an inlined brief or diff binds no
tree (3.11.1). A resumed round
(`SendMessage`) inherits its launch's profile and may omit the line; one that
names another profile counts for nothing. A profile review never satisfies
the implementation review, nor the reverse. A profile's verdict is read like
the implementation review's: `rework` is not a pass.

A pass is a completed review in this session's transcript whose `TREE`
block names this repository and the pushed content id, or a remembered pass:
`by-profile/<slug>/by-tree/<repo>/<sha>.json`, and
`by-profile/<slug>/by-patch/<repo>/<patch>.json` for a tree with the same
branch patch and rule files (`lang:rust` is `lang-rust`). The implementation
review's own `by-tree/` files are unchanged. A missing profile is refused:

```
missing code review of <repo> tree <12 hex>: <agent> [--lang <x>], … (declared in <plan path>). Launch them on this tree, or narrow ## Code review with a reason.
```

The person's Overrule of a profile's `rework` uses
`[implementation-review <repo>@<sha64> profile=<role>]` and writes that
profile's pass only.

### A profile's `approve` carries to a fix

After a fix the implementation review always reads the new tree, and a
profile whose verdict was `rework` or `approve-with-changes` reviews again.
A profile that said `approve` keeps its pass on a later pushed tree when the
hook can check it (`work.code-review-delta`, ADR-0052). A profile pass
records the commit the review read (the newest commit of
`git rev-list <pushed> ^<merge-base>` whose content id is the reviewed tree),
the plan that declared the profile, and a digest of this repository's parsed
`## Code review` declaration. When a declared profile has no pass on the
pushed tree, and the implementation review has passed it, the hook accepts the
pass of an earlier commit C only when all hold:

- its verdict is `approve` (not `approve-with-changes`, `rework` or
  `overruled`);
- C is in `git rev-list <pushed> ^<merge-base>`: an ancestor of the pushed
  commit and strictly after the current merge-base, so a rebased branch (the
  old commit is no longer an ancestor) and a review from a plan already merged
  carry nothing; a rebase keeps a pass only by patch, as before;
- its plan path and declaration digest equal the current ones;
- the pass has all three fields; one written without them never carries.

The approval may also come from the session itself, with no push ever
judged at its tree (3.12.0). The hook reads the transcript before any filed
pass. It takes this profile's latest completed review, in launch order, of a
tree of a commit in `git rev-list <pushed> ^<merge-base>` other than the
pushed one:

- if that review is not `approve`, nothing carries, not even a filed pass:
  a later `rework` of an earlier tree outranks an older pass;
- if it is `approve`, it carries when every branch commit whose content id
  is the reviewed tree holds the current plan path and declaration digest,
  read at that commit from the plans it changed since the merge-base. A
  content id leaves `docs/plans/` out, so a plan-only commit shares its
  tree: each such commit is one the review may have read. A declaration
  that differs or cannot be read there carries nothing, and never holds the
  push as unknown;
- a pointer plan records no revision of its canonical plan, so what it
  declared then cannot be proven: it gets no session carry, and filed passes
  still may.

The carry writes a pass for the pushed tree (`how` is `carried-from <12 hex
of the reviewed content id>`) and journals `profile-carried` with
`carried-from <12 hex>` and the `git diff --shortstat C <pushed>`. Otherwise
the profile is missing and must review the new tree.

The implementation review's own block is read from the prompt's header too
(3.12.0), as a profile's is: a `<<<TREE …>>>` block inside an inlined brief
or diff binds nothing, so the prompt's TREE line comes first.
