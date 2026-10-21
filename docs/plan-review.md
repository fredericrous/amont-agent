# The review panel

Every plan passes a panel of expert reviewers before the person is asked to
approve it (ADR-0022, fleet rule `work.plan-review-panel`). The skill
`/plan-review` runs the panel. The `plan-review-panel` rule checks, at
`ExitPlanMode`, that it did.

## Who reviews

The panel follows the areas of every repository the plan names:

| area | reviewers (`plan-review-<role>` agents) |
|---|---|
| always | `language` for each language, with `lang=<l>` in its review block, and `backend` |
| ops | `platform` |
| a command line | `unix` and `tui` |
| large (500 or more files or own commits) | `po` and `architect` |
| an interface | `react`, `ui-design`, `ux-research` and `game-ux` |

Areas are read from each repository's default tree, never from keywords in
the plan. The tree is the first of `origin/HEAD`, `origin/main`,
`origin/master` and `HEAD` that exists, with no fetch. Each signal:

- **Languages:** `Cargo.toml`, `go.mod`, `package.json` or `tsconfig.json`,
  `pyproject.toml`.
- **Interface:** the `ui` trait in `.adr.yaml` `areas:`, or a
  `package.json` with a `dev` script or a runtime React or `@duro-app/ui`
  dependency.
- **Ops:** a kustomization, `Chart.yaml`, a `HelmRelease` or Flux
  `Kustomization`, or `*.tf`.
- **Command line:** the `cli` trait, `src/main.rs`, `[[bin]]`, a
  `package.json` `bin`, or `package main`.

Vendored, lock and `dist/` paths do not count. A fork's commits are counted
past its `upstream`, and a shallow clone is judged on files alone.

## What the plan carries

- A `## Review panel` section, at most 5 lines, under the H1.
- The full reviews under a final `## Full reviews (reference)`.
- A machine comment as its last line:

  ```
  <!-- panel: repos=amont-agent,decisions adds=ui reviewers=… body-sha=<12 hex> -->
  ```

`repos=` names each repository. A name resolves to
`git config amont.agent.plan-review.<name>.path` when that is set, for a
repository kept elsewhere; otherwise to `$AMONT_AGENT_PLAN_ROOT/<name>`, and
by default to `~/Developer/Perso/<name>`. `adds=` declares areas the plan
creates: `cli`, `ui`, `ops`, `large`, or `lang:<language>`. A repository
that does not exist yet needs a `lang:` entry.

## Which agents to launch

`amont-agent plan-panel <plan.md>` answers with the hook's own code:

```
repos=amont-agent areas=cli,lang:rust body=cf3f43c938c4 panel=full
plan-review-backend
plan-review-language --lang rust
plan-review-tui
plan-review-unix
```

`panel=delta` lists only the reviews needed since the plan's last accepted
body. `panel=current` lists none.

## Binding a review to the plan

`amont-agent plan-sha --block [--lang <l>] <plan.md>` prints the review
block that each reviewer's prompt carries:

```
<<<PLAN path=/Users/me/.claude/plans/p.md sha=<64 hex>>>>
```

The sha is that of what the **canonical body** says: the plan without its
front matter, its review section, its full reviews and its machine comment,
read as CommonMark. Every word, number and operator counts, code blocks and
code spans count exactly as written, and so do link destinations and the
kind of each block (list item, quote, heading, table cell, an ordered
list's start). Blank lines, line wrapping, indentation outside code, table
padding, list-marker and emphasis style and escapes do not. A landed copy in
`docs/plans/`, which gains front matter and may pass through a formatter
such as prettier, therefore hashes like the approved plan, unless the
formatter rewrites code. Writing the review results into the plan never
makes a review stale. Editing the body does.

The Markdown parser is `pulldown-cmark`, pinned exactly: a new version may
read a plan differently, so a bump is its own change, and a fixture test
fails when it re-hashes.

**Until the transition ends** (2.25 onward), a review or a baseline bound
to the byte sha that 2.24 and earlier computed still counts. The hook's
journal line for a pass starts `match=legacy` when it rested on one, and
`plan-sha --legacy` prints that sha, for a plan whose machine comment was
written before. Both go once the journal shows no `match=legacy` for
14 days.

**A delta in a new session.** A plan's baseline is found by its path, by
its body, and, last, by its H1 and `repos=`: the same plan presented from a
new session has a new file name, and after an edit only its title still
names it. Two plans with the same H1 and repositories therefore share a
baseline; backend still reviews the body. If the sha cannot be computed
(the parser panicked), the hook asks the person instead of passing.

A review counts only when Claude Code recorded it as completed:

- in the foreground, a result with `status: completed` and the agent's
  `agentType`;
- in the background, a task notification that Claude Code queued itself.

Notification text inside a tool result, or in a message, is never read.

The declared sha stops a skipped review and a review of an older body. It
does not stop a model that deliberately declares a sha for a body it did not
have reviewed.

## What the hook answers

| situation | answer |
|---|---|
| first presentation | the whole panel, each role bound to this plan's path; `backend` bound to this body |
| body changed since the last pass | `backend` plus every area new since then, bound to this body |
| same body, no new area | passes (a re-presentation, or a metadata-only write) |
| no machine comment, bad `repos=`, no review section | refused |
| no transcript, no plan path, git failed | the person is asked (`ask`) |
| refused twice already | the person is asked, with `UNREVIEWED: refused N×` |
| `⚠ unreviewed: …` in the review section | the person is asked |

A pass is remembered under `~/.claude/amont-agent/plan-review/`, which is
0700 with 0600 files, by plan path and by body. The same plan presented from
a new session, under a new file name, is therefore recognised.

The default stance is `deny`. Set `amont.agent.plan-review-panel.stance` to
`advise` to be told without being refused, or to `observe` to only journal.

## Recording what the panel did

`amont-agent plan-review record <plan.md> [--transcript <path>]` appends one
JSON row per completed plan-review reviewer of the plan to
`~/.claude/amont-agent/plan-review/outcomes.jsonl`, so the panel can later be
judged on its own record. Without `--transcript` it reads the transcript under
`~/.claude/projects` (modified in the last 7 days) that reviewed this plan's
path: the one that reviewed its current body first, then the newest. A long
session still being written to can have reviewed an older body days ago.

stdin is a JSON array of findings (empty stdin is `[]`):
`{"role", "severity": blocking|high|medium|low, "disposition":
accepted|rejected|decision, "section"?, "area"?}`. They are attached to the
rows this call writes for the highest round it writes, matched by role.

A row has: `schema` (`amont-agent-review-outcome-v1`), `recorded_at` (unix
seconds), `role`, `agent_id`, `model`, `tokens`, `seconds`, `round` (1-based,
in the order distinct bodies first appear), `body_sha`, `first_round_sha`,
`plan_path`, `plan` (`areas`, `repos`, `body_lines` of the body that round
reviewed; null when that body was never saved), with `first_round_sha` the
first body any call recorded for this plan path, across sessions, `verdict` (`rework`,
`approve-with-changes`, `approve` or null), `remit` (the areas the role is
asked for; `*` for backend) and `findings`.

Rows are deduped by agent id under an exclusive lock, so a second run, or two
at once, writes nothing twice. Each round written also saves the plan body
once as `plan-review/bodies/<sha>.md`, if the plan file still has that body
(otherwise a note on stderr); bodies older than 90 days are deleted. The
directories are 0700 and the files 0600.

Its status line (`wrote N rows (round R, plan <sha12>)`, `already recorded
round R of <sha12>; nothing written`, or `no rows written …` when every
reviewer lacked an agent id) goes to stderr; stdout stays empty.
`--transcript=<path>` works too, and `--` ends the options.

Exit codes: 0 when the ledger is up to date; 1 when no transcript names the
plan, a reviewer has no agent id (the other rows are still written) or the
ledger cannot be written; 2 on a usage error or malformed findings.

## Applying the reviewers' patches

A reviewer that proposes an edit ends its result with a `Patches:` section: a
fenced JSON array of `{"finding": n, "old_string": "...", "new_string": "..."}`,
one object per edit, `old_string` copied from the plan and unique in it.
`amont-agent plan-apply <plan.md>` applies them with no model. The accepted
findings come on stdin, `[{"role": "tui", "finding": 1}, ...]`; a finding may
carry `"patch": {"old_string", "new_string"}` to supply an edit the main
session wrote itself. Stdin on a terminal is a usage error.

The patches are read from the session transcript (`--transcript <path>`, or
else the one under `~/.claude/projects` that reviewed the plan), from the
latest completion of each role, launch or `SendMessage` resume, that reviewed
the plan body as it was before the apply. Each `old_string` must occur exactly
once and must not overlap another patch; an empty one is a usage error. A patch
that does not match is listed as `unmatched <role>#<n>: <reason>` (`absent`,
`absent (whitespace)`, `N occurrences`, `overlaps <role>#<n>`, `no patches
block`, `patches block is not JSON`), never guessed. The matched ones are
written once, atomically, to the file a symlink points to; if the plan changed
between the read and the write, nothing is written. A patch inside the front
matter, `## Review panel` or `## Full reviews` is applied and labelled
`outside the body`.

stdout is data: one line per patch, `applied tui#1 <section>` or `unmatched
tui#3: absent`, then `applied A, unmatched U, sha <sha12>`. When some patch is
unmatched, a `next:` line follows and the last thing on stdout is the block
the fallback subagent's prompt must start with:
`<<<APPLY path=<plan> sha=<sha12 of the body before the apply>>>>`.
Patch lines stay within 80 columns; the `next:` line and the block carry the
plan's path unclipped, since one is copied and the other is machine-read.
Diagnostics go to stderr. Every real run appends one
`amont-agent-review-merge-v1` row to `outcomes.jsonl` (`recorded_at`,
`plan_path`, `body_sha_before`, `body_sha_after`, `round`, `accepted`,
`applied`, `unmatched`, `mode: "command"`); merge rows are not review rows and
never count as a recorded reviewer.

| flag | effect |
|---|---|
| `--dry-run` | report what would change; write nothing, no row |
| `--check [--before <sha>]` | write nothing; a patch is verified when its `new_string` occurs once and its `old_string` is gone (a `new_string` that contains its `old_string` skips the absence test; a patch the apply step recorded as `overlaps` is verified on its `old_string` being gone alone, since the one merged edit that resolves two overlapping patches holds neither `new_string`). Patches resolve by the body before the apply: `--before` (12 to 64 hex), else the latest `command` row of this plan, else the current body |
| `--record-fallback` | after a subagent applied the unmatched edits: find its launch by the `<<<APPLY` block its prompt starts with, check the findings on stdin, and write one `mode: "subagent"` row (`agent_id`, `model`, `tokens`, `seconds`, `verified`); a rerun writes nothing |

| exit | meaning |
|---|---|
| 0 | every patch applied (or verified) |
| 3 | some patch unmatched or not verified; the matched ones are written |
| 1 | file I/O, the plan changed under the apply (nothing written), or no transcript holds the reviewer's result |
| 2 | usage error, including an empty `old_string` |

## Judging a seat: `plan-review stats`

```
amont-agent plan-review stats [--json] [--model <id>]
```

Reads `~/.claude/amont-agent/plan-review/outcomes.jsonl` and, for the
implementation review, `~/.claude/amont-agent/implementation-review-outcomes/outcomes.jsonl`,
through the one ledger reader in `review_outcomes.rs`; rows of another schema
(merge rows) are not reviews. It prints one row per (role, model): reviews,
findings by severity (`blocking/high/medium/low`), accepted findings by
severity, and accepted high-or-blocking findings per review. A finding is
accepted when its disposition is `accepted` (plan review) or `fixed`
(implementation review, role `implementation-review`). A row with 20 or more
reviews and under 0.05 accepted high-or-blocking findings per review ends in
`retire-candidate` (ADR-0042); retiring the seat stays the person's decision.
`--json` prints one array of objects instead, `--model` keeps one model. Exit
0, or 1 when a ledger cannot be read, 2 on a usage error.
