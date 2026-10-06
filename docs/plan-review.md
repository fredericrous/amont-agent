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
