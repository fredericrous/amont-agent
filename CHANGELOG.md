# Changelog

## v2.29.0

### Added

- **`publish-without-skill`: a release or a merge needs its skill.** A new
  rule, shipped as `advise` (ceiling `deny`). It fires on three commands:
  - a `git push` that publishes a `v*` tag (`--tags` and `--mirror` included);
  - `gh pr merge`;
  - an HTTP client sent to `/pulls/<n>/merge`.

  The rule looks for a call of the `tag-release` or `merge-when-green` skill
  (a Skill call, or the person typing `/tag-release`) in this turn or in the
  human turn before it. A skill called within that window passes silently.
  - **Reading the transcript.** It is read backwards from its end, and only
    as far as the second human prompt.
  - **Subagents.** A subagent's own transcript (`agent_transcript_path`) is
    read too.
  - **When it cannot tell, it never refuses.** That covers a missing,
    unreadable or truncated transcript, a turn longer than 16 MB, or a skill
    that is not installed.
  - **To refuse:** `git config --global amont.agent.publish-without-skill.stance deny`.
  - **Measured on 2026-10-07:** 8.4–28.3 uncovered publishes per 1000 Bash
    calls by week, falling. Count with `tools/skill-rate.py`.

### Changed

- **`release-tag-push` also matches `+refs/tags/v…` and `--mirror`.** It now
  shares its matcher with `publish-without-skill`. `--follow-tags` stays
  unmatched.

## v2.28.0

### Added

- **`plan-phases-open`: an agent no longer stops between plan phases.** A
  new `Stop` hook, shipped as `deny`: when the agent ends its turn while the
  `active` plan its branch carries (`docs/plans/`, changed since the
  merge-base with the remote default branch) still has an open phase that
  is not a `🧑 decision:`, the turn continues with that phase. It lets the
  turn end in plan mode, while background tasks run, on a last-message line
  starting with `WAITING: <reason>`, and after three continuations on one
  phase, when it tells the person instead. **Existing installs must re-run
  `amont-agent install --write`** to add the `Stop` entry; `doctor` now
  names the events an install lacks.

## v2.27.1

### Fixed

- **A delete-only push is not held under deny** (#66). `git push origin
  --delete <branch>…`, `-d` (also inside a cluster such as `-ud`), and a
  push whose every refspec is `:<ref>` publish nothing, so `push-preview`
  and `implementation-review` now pass them (`a delete publishes nothing`
  and `delete-only` in the journal) instead of holding them as unreadable.
  A push that deletes and publishes at once, `--delete` with a `src:dst`
  refspec, and `--prune` are still unreadable, and still held under deny.
- **A push from a removed session directory is judged, not waved
  through** (#67). Every confirmed rule used to decline as soon as the
  session's cwd was gone, so `cd /abs/repo && git push …` from a session
  left in a torn-down worktree passed every push gate. The test is now
  whether the matched clause's own directory exists, `cd`s followed. When
  it does not, `push-preview` and `implementation-review` hold the push
  under deny, because the shell then runs somewhere the hook cannot name
  (Claude Code resets it to the project root). Other rules still decline.
- **A firing hook call no longer spawns git once per key** (#70). Every
  stance lookup ran `git config` twice (`--global`, then `--system`), so a
  call firing one rule spawned six git processes and a Bash command firing
  several spawned more. Each scope's `amont.agent.*` keys are now read once
  per process with `--get-regexp`, both scopes in parallel. A boolean that
  is actually set is still normalised by git (`--type=bool`), and the scope
  rule is unchanged: only `--global` and `--system`, with their includes.
  Measured on a loaded macOS machine, release build, 100–200 runs each: a
  firing Read went from 111 to 38 ms p50, and a Bash command firing several
  rules from 398 to 38 ms p50 (p99 788 → 69 ms). The silent path is
  unchanged (~10 ms). One git spawn costs ~30 ms here under load, so the
  issue's 30 ms budget is not met on this machine.

## v2.27.0

### Added

- **`amont-agent rules --json`.** One array, one object per rule and per
  assertion, with `id`, `kind`, `default_stance` (what ships), `stance`
  (what is in force here), `max_stance`, `per_1000` and `measured`. The
  keys are an interface: the homebrew tap reads them. `rules` now refuses
  any other argument (exit 2); until now it ignored them, so `rules --json`
  printed the text table.

### Fixed

- **The brew caveat names every rule that refuses by default.** It said
  only `pipe-to-tail` refuses, false since `plan-review-panel` shipped at
  deny in 2.22.0. The release now writes the list from the published
  binary's `rules --json` (`scripts/bump-tap.py`), and from 2.27.0 the
  formula's `brew test` checks it against the brewed binary.

## v2.26.0

### Added

- **`lint-suppression-added`: an edit that adds a lint suppression is
  advised.** On an Edit, MultiEdit or Write the hook rebuilds the file
  before and after (a bounded read that never opens a FIFO) and compares
  suppression markers, not lines: `# type: ignore`, `# noqa`,
  `# pyright:` headers, `eslint-disable`, `@ts-ignore`, `#[allow]`/`#[expect]`,
  `//nolint`, and a lint configuration made looser (tsconfig, eslint,
  pyright, ruff, Cargo `[lints]`, `.golangci.yml`). Editing a line that
  keeps its suppression, or moving one, is silent. The advice points at
  `general.no-disabled-safety`; the journal records the marker kind and
  whether the file was read whole or only the fragments were seen.
  Ships `advise`, ceiling `deny`. `tools/suppression-rate.py` prints the
  weekly rate from transcripts.

## v2.25.0

### Changed

- **A plan's sha ignores formatting.** `plan-sha` and the
  `plan-review-panel` hook read the plan as CommonMark (`pulldown-cmark`,
  pinned exactly), so a formatter that adds blank lines, rewraps lines,
  pads tables or changes list-marker or emphasis style keeps the review
  bound. Every word, number and operator still counts, code blocks and
  spans count exactly as written, and so does the kind of each block
  (`a * b` ≠ a list item `* b`). On the 44 of 47 real plans that prettier
  changes, 36 keep their sha; the 8 others had code or text rewritten.
  Reviews and baselines bound to the old byte sha still count during a
  transition: the hook journals `match=legacy` when one is used, and
  `plan-sha --legacy` prints that sha. Both are removed once the journal
  shows no `match=legacy` for 14 days.
- **An edited plan in a new session is a delta.** Its baseline is now also
  found by its H1 and `repos=`, so backend plus the reviewers of any new
  area are asked for, not the whole panel.
- **A plan whose sha cannot be computed is asked about, not passed.** A
  panic in the Markdown parser goes to the person; `plan-sha` exits 1.

### Added

- **`read-unbounded-large`: a Read with no window over a large file is
  advised.** A Read with no `offset`/`limit` of a regular file over 16 KiB
  says what it costs (up to 2,000 lines in every later turn) and how to
  take the part that is wanted; `offset: 1` with a `limit` is the way
  through for a deliberate whole read. Ships `advise`, ceiling `deny`.
  Plans (`.claude/plans`, `docs/plans`), `.diff`/`.patch`, media and
  `tool-results/` are exempt, and a file the session already read is
  `file-reread`'s to answer. `tools/read-rate.py` prints the weekly rate
  from transcripts.


### Fixed

- **`persisted-output-dump` sees a Windows path.** It matched
  `/tool-results/` only, so on Windows a Read, or a `cat`/`head`, of a
  saved tool result, spelled `\tool-results\`, went unadvised. Either separator, or a mix
  of both, now counts.

- **The pass-file guard refuses writes, not reads.** 2.24.0 refused any
  Bash command naming `~/.claude/amont-agent/implementation-review/`,
  `ls` included (#61). It now refuses a redirect into the store or a
  writing program (`rm`, `cp`, `mv`, `tee`, `python`, `sed`, `chezmoi`,
  `git`, shells, …) with a word naming it, and lets `ls`, `cat`, `find`,
  `grep` and the other readers look at the record. The Write/Edit half is
  unchanged.

## v2.24.0

### Added

- **`implementation-review`: one reviewer reads the diff before the push.**
  Before the push of a branch that carries a plan, an `implementation-review`
  agent has read the diff against the plan and the active rules, and its
  verdict is bound to the **canonical tree id** of the commit — the sha256
  of `git ls-tree -r` without `docs/plans/`, 64 hex in any object format,
  read-only — so recording the review in the plan never makes it stale and
  a code change does. The rule (ADR-0022, `work.implementation-review`)
  resolves the push, takes its base from the remote's default branch
  (`<remote>/HEAD`, `main`, `master`; never the tracking ref, never `HEAD`),
  declines silently when `docs/plans/` did not change or the tree was
  reviewed, and otherwise says why — missing, stale (both trees named),
  `rework` with no delta, or unknown — with the next step. The verdict is
  read from the completed agent's result; a `rework` never passes. Ships
  `advise`; one journal line per push, `review=` and `verdict=` in the
  excerpt for the soak.
- **`amont-agent tree-sha [--block] [-C <dir>] [--] [<rev>]`** prints the
  canonical tree id, or the `<<<TREE repo=<name> sha=…>>>` block the
  reviewer's prompt carries, `<name>` being the directory that holds the
  common `.git` so a worktree named after its task still names its
  repository. Exit 0 / 1 / 2.
- **The person's overrule.** A `rework` that survives the delta goes to
  the person on a marked question (`[implementation-review <repo>@<sha64>]`,
  options exactly Overrule / Fix / Hold); the hook records it at
  `PreToolUse`, trusts the answer only without pre-filled `answers`, and on
  `Overrule` writes the pass file itself — only when the transcript shows
  the `rework` being overruled.
- **Only the hook writes a pass file.** A Write, Edit or MultiEdit under
  `~/.claude/amont-agent/implementation-review/`, or a Bash command naming
  it, is refused whatever the rule's stance, with `.` and `..` folded
  first.

### Changed

- **Bash and AskUserQuestion payloads carry `transcript_path`**, as
  `ExitPlanMode` already did, and `rules::Context` carries it to a
  `confirm`.
- **`Confirmed::YesSaying`**: a `confirm` that learned its reason by
  looking gives the reason to speak and the excerpt to journal, in place
  of the finding's reason and the command span.
- **`mine` breaks a covered-by tie by stance.** When several rules fire on
  every call of a shape — every push rule fires on every push — the one
  that ships loudest is named, not the alphabetically first.

## v2.23.1

### Fixed

- **`preview register --help` is not a registration.** 2.23.0 told the
  session that a chained `--help` call "did not bind"; help registers
  nothing, and binding now ignores it.

## v2.23.0

### Added

- **Mockup mode in `preview register`.** When the previewed branch commits
  a picked mockup's artboards (`*.dc.html` under `docs/mockups/<screen>`, or
  `git config amont.agent.preview.mockups`), the guide must show the
  screen's path, a `Viewport: <width>px · <theme>` line, the artboard
  beside the built screen as files next to it (`artboard.png` /
  `after.png`, also `mockup` / `live`, state pairs by suffix), and a
  `## Differences from the mockup` section (`None`, or `fixed:` /
  `deliberate: <reason>` bullets). The range starts at the merge base with
  the default branch, never the upstream, so a pushed branch stays in
  mockup mode; artboards with no resolvable base are a refusal. The
  refusal gives a paste-ready block; PNG widths that differ by more than a
  tenth warn. The page shows each pair full width, linked to full size,
  with a CSS-only overlay toggle. Prompted by application-landscape PR
  #302, whose preview was approved-asked while the build diverged from the
  picked artboard.
- **`push-preview` holds an unapproved mockup-mode commit** at `deny`
  instead of advising, through a new `Confirmed::YesAt` floor that is
  capped by the rule's `max_stance` and never raised from `observe`. Other
  UI pushes are still advised.
- **`register` prints `label` and `aliases`.** The label is the checkout's
  directory `@sha7`; the aliases are the main worktree's directory and the
  origin repository's name. A marked question may name any of them, so a
  question about `app@abc1234` approves a commit registered from the
  worktree `app-wt-x`.

### Fixed

- **Binding failures reach the session.** A `preview register` that did not
  bind (chained after another command, run in the background, output not
  matching HEAD) was recorded only in the journal; the session asked the
  person anyway and the approval bound nothing. The hook now says so right
  after the command, with the command to run alone. An `Approve` on a
  question that names none of a preview's labels says it approved nothing,
  and the registration stays pending for a corrected question instead of
  being dropped.
- **A guide heading may carry a note**: `## Try it (about 2 minutes)` names
  the Try it section. Other trailing words stay a different heading.
- **`rules` shows the stance in force.** It printed what each rule ships
  as, so a rule promoted to `deny` in `~/.gitconfig` still listed as
  `observe` — the one command you would run to ask whether a rule is armed
  said it was not. It now prints the resolved stance, with
  `(ships as …)` beside it only when a key on this machine moved it; the
  same for assertions. `check` and `status` already did this.
- **`corpus check` names a path that exists.** A disagreement line printed
  the corpus path the binary was built from, which outside a checkout is
  somebody else's disk; it now falls back to `<rule>.cases`.

### Documentation

- **The README is short again**, and the detail it carried lives in the book:
  the full rule table (all 32 rules, `forge-merge-by-hand` and
  `release-tag-push` included) in `docs/rules.md`, the `push-preflight`
  rationale beside it. The book's repository, edit and site URLs pointed at
  `fredericrous/amont`; they name `amont-agent` now, and `analysis.md` is in
  the table of contents.

## v2.22.1

### Changed

- **`plan-sha` and the `plan-review-panel` hook skip YAML front matter**
  (a `---` first line through the next `---`) and the blank lines after
  it. A plan gains front matter when it lands in `docs/plans/`, so the
  landed copy now hashes like the approved file the reviewers read.
  Landing can therefore be checked on the file it writes. Plans without
  front matter hash exactly as in 2.22.0, and stored baselines stay
  valid. A `---` first line with no closing `---` is still text.

## v2.22.0

### Added

- **`plan-review-panel`**, a rule on `ExitPlanMode` that ships as `deny`
  (ADR-0022, `work.plan-review-panel`). A plan is presented for approval
  only after its expert review panel has run. The panel is computed from
  the default tree of each repository in the plan's machine comment
  (`repos=`): one reviewer per language, backend, platform for ops, Unix
  and TUI for a CLI, PO and architect for 500 files or own commits, and 4
  interface roles for a UI. Areas the plan declares it adds (`adds=`) are
  added. A review binds when a `plan-review-<role>` agent launched with the
  plan's review block completed. Completion is read only from structured
  transcript fields, never from notification text echoed in tool output.
  A pass is remembered by plan path and by body, so a later change needs
  only its delta: backend plus the reviewers of any new area. The model
  can fix a missing comment, a missing section or missing reviews, so those
  are refused. After two refusals, and whenever the hook cannot check (no
  transcript, git failed), the person is asked instead. See
  `docs/plan-review.md`.
- **`amont-agent plan-sha [--short] [--block [--lang <l>]] [--] <file|->`**
  prints the sha256 of a plan's canonical body, which is the plan without
  its review section, its full reviews and its machine comment, so writing
  review results never changes it. `--block` prints the
  `<<<PLAN path=… sha=…>>>` line a reviewer's prompt carries. Exit 1 on an
  unreadable file, 2 on a usage error.
- **`amont-agent plan-panel <plan.md>`** prints the review agents a plan
  still needs, computed by the same code the hook judges with: the whole
  panel, the delta since its last accepted body, or nothing (`current`).
  The `/plan-review` skill launches exactly what it lists.
- **`Decision::Ask`**: a hook answer of `permissionDecision: "ask"`, which
  hands the call to the person whatever the permission mode.
- `install` adds a `PreToolUse` block for `ExitPlanMode`.

## v2.21.0

### Changed

- **`preview register` is guide-first** (fleet rule
  `work.preview-is-guided`). A bare "[preview id] Ship repo@sha?" gave the
  person nothing to decide with. `--guide <file.md>` is now required: a
  markdown guide outside the worktree with the H2 sections `Where we are`,
  `What you should see`, `Try it` (at least one numbered step and one
  http(s) URL), `Reference` and `Already checked`, emoji and case aside. A
  guide missing any of them is refused (exit 1) with exactly what is missing
  on stderr; no guide at all is a usage error (exit 2).
- **The guide is rendered to `index.html` beside it**: a self-contained page
  (inline CSS, light and dark, no script) titled `repo@sha` plus the first
  line of `Where we are`, with a large "Open the app" link, the sections in
  order, the guide's images inline and a `before`/`after` pair side by side
  under `Reference`. The converter is hand-rolled for a small markdown
  subset and escapes everything else; no new dependency.
- **The JSON gains `guide`, `page` and `page_url`** (`file://`). The hook
  still reads 2.20.0's shape, and `attestation` is still printed (equal to
  `guide`) for one release so an older hook binds a newer registration. The
  journal's `registered` line names the page.

### Added

- `preview register --open` opens the rendered page with the platform opener
  (`open`, `xdg-open`, `cmd /c start`), fire-and-forget; a failure to open
  never fails the command.

### Deprecated

- `--attestation` is read as `--guide` for this release and refused unless
  the file is a complete guide; stderr says so. It goes in the next.

## v2.20.0

### Fixed

Findings from the first day of the preview-approval soak (ADR-0023).

- **`cd <worktree> && amont-agent preview register …` binds.** It was
  journalled `unbound: not a standalone command`. Leading `cd <literal
  path>` clauses joined by `&&` are now accepted, and the repository is
  resolved through them; it must still be the printed repository at the
  printed `HEAD`. Every other chain stays unbound.
- **Interface changes are judged per package.** A push touching only
  duro-design-system's `packages/cli` was advised because the repository
  root has a `dev` script. A changed file now counts only when its nearest
  `package.json`, read from the pushed commit, has a `dev` script or a UI
  package (`react`, `react-dom`, `react-native`, `react-strict-dom`,
  `@duro-app/ui`) in `dependencies` or as a required peer.
- **Comment-only diffs are not interface changes.** application-landscape
  #301 changed only comments under `app/` and was advised. A `.ts`, `.tsx`,
  `.js`, `.jsx` or `.css` file whose every changed non-blank line is a
  comment no longer counts; any doubt still does.
- **Journal lines name the repository pushed.** `push-preview`,
  `push-published` and preview events were recorded under the session
  cwd's repository; they now name the push target's or the registration's.
- **Answer latency is measured by the hook.** `duration_ms` in the
  `AskUserQuestion` payload is not the person's answer time (a minutes-long
  approval was journalled `option,0s`). The latency is now PostToolUse time
  minus the PreToolUse time recorded for the question, journalled as
  `option,<secs>s,dur=<duration_ms>ms`.

## v2.19.0

### Changed

- **One dispatch for every judge.** The hook, `check`, `backtest`,
  `corpus check`, `backtest --compliance` and `mine` now reach the rules
  through a single `rules::evaluate`, instead of each deciding for itself
  what to do with a command the lexer could not read. Every existing rule
  keeps today's behaviour exactly — skipped on an unreadable command, as
  before — and every corpus replays unchanged. The seam exists for rules
  built on a real shell analysis, which carry their own account of what they
  could not read and so may judge such a command.
- **`check --dialect bash|zsh|unknown`.** A rule may depend on which shell
  runs the command. The hook reads it from `SHELL` as the existing zsh rules
  already did; `check` is told, and defaults to `unknown` — which is also all
  the backtester can say, since a transcript does not record the shell.

### Added

- **`request-fanout`**, the first rule built on a real shell analysis. It
  counts the explicit command-line transfers a command may and must make —
  per destination, through loops, function calls, `case` arms and client
  options such as `--retry`, `--paginate` and curl URL ranges — and advises
  when one destination may see more than 50. It never counts what it cannot
  read: an unbounded or unparsed region is reported beside the counts, never
  folded into them. A paced poll — a loop that sleeps at least ten seconds
  on every path back to its head, with a small burst between sleeps — is
  not fan-out and stays silent. Replayed over 30 days (57,361 calls) it
  fires on the two forms of the 2026-09-27 incident and nothing else. Ships
  `observe`; its ceiling is `advise`, because every bound is an estimate of
  what a program may do at run time.
- **`amont-agent analyze '<command>'`** prints the shell analysis of one
  command as JSON, per call site — what `tools/shell-oracle` checks against
  bash and zsh actually running it.
- **Shell analysis** (`src/analysis/`): a parser for a declared subset of
  bash and zsh, an abstract interpreter over it, and models of the network
  clients. The contract — inputs, assumptions, the subset, and what
  `Unknown` and `Incomplete` mean — is in `docs/analysis.md`.
- **Stance ceilings.** Every rule declares the loudest stance it may take.
  The cap is applied after every configured key, so neither
  `amont.agent.stance deny` nor the rule's own key can pass it, and
  `graduate` refuses to promote past it. Every existing rule's ceiling is
  `deny`, so nothing changes for them; `amont-agent rules` names a ceiling
  only where it is lower.

## v2.18.0

### Added

- **Preview approval (fleet ADR-0023): `push-preview`, `push-published`,
  `amont-agent preview register`.** A commit that changes a user interface is
  published only after the person approved that exact commit on localhost.
  The agent verifies first, registers the clean worktree's preview
  (`preview register --url … --attestation <file outside the worktree>`, bound
  to the session and prompt by the `PostToolUse` hook), and asks a marked
  question (`[preview <id>]`, options `Approve` / `Request changes` / `Hold`).
  Only the person's selection — read from `tool_response.answers` — or a typed
  `approve|ship|lgtm|looks good` as the very next prompt approves; `yes`, `go`,
  `ok` and `continue` do not. A question the model pre-answered through the
  tool's own `answers` input never approves, and under `deny` is refused.
  `push-preview` ships advising and reads a documented subset of push shapes
  (explicit remote and refspecs, `-C`, `cd`); anything else is journalled as
  unresolvable, and held under `deny` in a UI repository. `push-published`
  records, never speaks: published with or without approval, already present,
  or unverified — the numbers the soak review is read from. See
  `docs/preview.md`.
- **Open plans in the session notice (fleet ADR-0022).** A session opening in
  a repository with `docs/plans/` gets one line per `active` plan — title and
  first unchecked phase — and one per pointer file; a malformed `status` is
  reported, since nothing else reads plan front matter.

### Changed

- `install` registers three more hook targets: `UserPromptSubmit`,
  `PreToolUse:AskUserQuestion` and `PostToolUse:AskUserQuestion`. Run
  `amont-agent install --write` after upgrading; `doctor` reports the
  missing targets until you do.

## v2.17.0

### Added

- **`unsplit-expansion` — the zsh trap that never errors.** bash splits an
  unquoted `$var` on `IFS`; zsh does not, so `set -- $rp` sets `$1` to the
  whole string and `$2` to nothing, and `for f in $files` runs once with every
  path glued into one name. Nothing fails: the command runs with the wrong
  arguments and reports success — six PR merges on 2026-09-21 went to
  `…/repos/fredericrous/sre-agent 41/pulls//merge` (URL rejected), and a
  Prometheus query the same day fed an empty window into `increase(…[0s])`.
  The rule reads only the two idioms whose sole purpose is splitting — `set
  [--] $var` and `for x in $var` — and stays silent on `"$var"`, `$@`, `$*`,
  positionals, `$dir/x` and `$(cmd)` (zsh DOES split a command substitution).
  Ships observing.

## v2.16.0

### Added

- **`amont-agent mine` — the step before a rule exists.** `backtest` can only
  price a rule somebody already thought of. `mine` replays the same
  transcripts, groups every Bash call by command SHAPE — the parse with the
  branch, the path, the number and the commit message masked out, the
  program, its verbs and its flag set kept — and ranks the shapes that went
  wrong. Wrong means one of two things the transcript itself records: the
  tool result carried `is_error`, or a near-identical command followed within
  a few tool calls, which is what a model does when its first attempt did not
  land. Only the HEAD of a run of near-identical calls counts, because a
  polling loop is one decision repeated and not nineteen mistakes — without
  that, the top line of the real report was `echo waiting` at 1,088 calls and
  a rate of 0.99. Shapes an existing rule already fires on are listed apart
  as `covered by <rule>` rather than proposed. `--min-support` (5),
  `--min-rate` (0.3), `--window` (3), `--json`, and `--format cases`, which
  writes the same file `explain --format cases` does, so a shape mined today
  is a reviewable case today. Nothing mined reaches the hook: the rule that
  ships is still written by hand.

- **`amont-agent explain <rule> --sample N --rank novelty`.** The default
  sample is the first N matches the walk met, which is the oldest project,
  the oldest session and — because a habit repeats — very often N spellings
  of one command. Novelty picks the N least like each other and least like
  the cases already in `tests/corpus/<rule>.cases`, by greedy max-min over
  the shape distance. Deterministic, and a second review pass does not hand
  back the first pass's cases. Default behaviour is unchanged.

- **`amont-agent backtest --compliance` — what the advice buys.** `advise`
  costs context tokens every session forever and the backtest only prices the
  cost. For every firing this asks what the model did NEXT: when the next
  equivalent command — same shape, within `--window` (20) tool calls — no
  longer matches, the habit changed; when it matches again, the advice was
  ignored or never arrived; when nothing equivalent followed, the firing is
  unanswered and counts towards neither. Per model id and per week, because
  models differ and a pooled figure hides which one is listening. `observe`
  rules are reported beside them as the control — they said nothing, so their
  share is the rate the habit corrects on its own. `deny` rules are left out:
  a refused command never ran, so there is no next command. Measured over
  44,750 Bash calls: `glob-in-flag-value` 93% complied against
  `equals-separator`'s silent 72%, and `path-operand-missing` 6%, which is an
  advise that is not earning its tokens.

### Changed

- **The transcript reader can say what became of a call.** A second walk
  (`for_each_transcript`) buffers a transcript, joins each `tool_result` back
  to its `tool_use` by id, and hands the file's calls over in order with the
  outcome and the issuing model attached — none of which the streaming walk
  could see, because both live on later lines than the call itself. Result
  lines are read with substring scans rather than a JSON parse: a result
  carries the whole output of the command, which is where all the bytes in a
  transcript are, and the two facts wanted are an id and a boolean. The
  backtester's own walk is untouched.

## v2.15.0

### Changed

- **The inside of a command substitution is read.** `$(…)`, backticks and
  `<(…)` were blanked to spaces, so `$(git push | tail -1)` was no
  pipe-to-tail and `$(stat -f '%Sm' f)` — the example in that rule's own
  header — fired nothing. Their insides are now clauses of their own,
  appended after the line's and marked `nested` with the substitution's
  offset; every rule judges them as it would the bare command. Two things
  know a subshell is a subshell: a `cd` inside one moves only that
  substitution's clauses (`cwd_at`), and `X=$(cat f)` is not a dump into
  the tool result (`whole-file-dump`, `persisted-output-dump`,
  `file-reread`). An inside that cannot be read — `$(eval …)` — adds nothing,
  as before. Backtested over 38,611 calls: 7 of 29 totals move, all up and
  all by a few — `stat-bsd-format` 15 → 23, `forge-merge-by-hand` 306 →
  318 (merges captured into a variable), `glob-no-match` +18.

- **`stat-bsd-format` stays silent on the portable pair** `stat -c … ||
  stat -f …`: one spelling tried, then the other, is the form that works on
  either stat — and, now that substitutions are read, the first thing the
  rule met was `"$(stat -c %s "$f" 2>/dev/null || stat -f %z "$f")"`, which a
  deny would have refused.

- **`check` labels a finding with the stance in force**, not the shipped
  one — `[deny, ships as advise]` for a graduated rule — since a reader took
  the old `[advise]` for the verdict the hook would give.

## v2.14.0

### Fixed

- **`glob-no-match` fired on `if [[ $x == *pat* ]]`.** The `[[` and `case`
  exemption looked at the clause's first word, and after `if`, `while`,
  `until`, `elif` or `!` that word is the keyword — so the bare test was
  silent and the introduced one was advised, twice in one live session
  (2026-09-14). The exemption now steps over the keyword. `if grep -q x
  dir/*.rs` still fires: the keyword exempts a test, not an expansion.

- **`file-reread` on a background task's output file** — Read while still
  empty, then read again once the task had written to it — is the shape the
  size-and-mtime fingerprint below was missing live; pinned by a test of its
  own.

- **`worktree-remove-force` corpus topped up to 14 reviewed** (three dirty
  force-removals from the 30-day journal, two `--force`-on-another-verb
  negatives) so `graduate … --to deny` clears its 12-case gate.

- **A stance kept in a file your global config `include`s was invisible.**
  `git config --global --get` does not follow `include` or `includeIf` unless
  told to (measured on git 2.55), so a stance in the file a work identity or a
  personal one usually lives in read as unset, and the rule kept refusing. The
  reader now passes `--includes`; the scope stays what it was — the global and
  system files, and only what THEY name.

- **A Read was remembered before it happened.** The record was written on
  `PreToolUse`, even on the way to a refusal, so a Read that was refused or
  that failed on a path that is not there was on record, and the retry was
  told its contents were already in context. A Read, and a `cat`, are now
  remembered on `PostToolUse` — once the tool has reported it ran. This adds a
  `PostToolUse` entry for Read to the install; re-run
  `amont-agent install --write` (`doctor` says so).

- **`file-reread` trusted the session record over the file.** The Edit and
  Write tools were the only writes it knew about, so a file changed by
  `sed -i`, a formatter, `git checkout` or another session was still "unchanged
  since". A read is now recorded with the file's size and modification time,
  and a file that no longer matches is read again without comment.

- **`push-landed` asked `origin` about a push that went to a fork.** A `git
  push` that names no remote goes where `branch.<name>.pushRemote`, then
  `remote.pushDefault`, then `branch.<name>.remote` says; the assertion read
  only the last of those and accused a push that had landed. It now resolves
  the destination in git's own order.

## v2.13.0

### Added

- **`forge-status-stale-row`** — reducing a commit's append-only `/statuses`
  list by position. The endpoint returns every TRANSITION, newest first, one
  row per context per change, so it has to be deduped — and the obvious
  reduction, keep the first row seen, is wrong. A job that starts and is
  skipped inside one second emits `pending` and `success` with the SAME
  timestamp, and the API returns the pending row first, so that check reads
  `pending` for as long as anyone asks.

  The failure is silent and shaped like patience: an `until` loop on top of
  that reduction never exits, the run is green, the PR is mergeable, and the
  output prints `pending` — exactly what a not-yet-finished check prints. It is
  the mirror of `poll-blank-verdict`, which fires when a wait stops TOO EARLY
  because a blank read as a verdict; this one is a wait that never stops
  because a stale row read as the present.

  Measured over 35,165 calls and five weeks: 5 matches, all inside ONE day —
  0.8 per 1,000 that week, 0.0 in the four before. That shape is the point. The
  reduction had been written into a skill the day before as "keep the FIRST
  (newest) entry", so every later poll copied it, and one of them pinned a
  homelab deploy PR at `pending` while all 22 of its checks were green. A
  defect that arrives by being written down does not trend; it appears at full
  rate the moment the instruction lands. Ships `observe`.

## v2.12.0

### Added

- **`forge-merge-by-hand`** — merging a pull request by POSTing to the forge
  API. `POST /repos/{owner}/{repo}/pulls/{n}/merge` merges; it does not consult
  check runs, and answers `200` identically whether the run concluded success,
  concluded failure, or never started. The web UI greys the button out, the API
  does not, and neither does a shell. Silent by this crate's admission test: a
  merge onto red is indistinguishable at the call site from a merge onto green,
  so no correcting loop can form from the outcome — it is found later, in
  `main`, by someone else.

  Measured over 34,905 calls and five weeks, per 1,000: `0.3 → 4.7 → 4.9 →
  13.8 → 7.1`, 221 matches. That inverts the shapes this crate deliberately
  leaves alone. `--no-verify` fell 25.9 → 5.4 and `git add -A` 42.5 → 5.4 once
  they had consequences a loop could see; this one climbs roughly twentyfold,
  because improvising the procedure **works** — the merge succeeds, the session
  continues, and the shape gets repeated for every later merge rather than the
  instruction naming a skill being re-read.

- **`release-tag-push`** — pushing a version tag, which publishes. A `v*` tag
  drives the build-and-publish workflow, and that workflow reporting `success`
  means only that no step exited non-zero: the artefact can still be stale, or
  built from the wrong commit if the tag landed on the previous HEAD.
  `tag-after-commit` catches that when commit and tag are chained; typed as
  separate commands, nothing did. On an immutable registry the mistake cannot
  be withdrawn, only superseded.

  178 matches over 34,950 calls, per 1,000: `6.9, 4.6, 6.3, 3.8` — noisy, not
  climbing. Ships at `Observe`, and the reason is what it matches: pushing a
  version tag is not a mistake, it is the correct final step of a release, and
  this fires on every one of them. `Advise` would put a paragraph in front of
  an action that is usually right. Kept for cost-of-a-miss rather than
  frequency, with a real rate underneath it for a later graduation.

  The first rule aimed at skill bypass rather than at a wrong command, and the
  first to ship at `Advise` without serving time at `Observe`: the backtester
  supplies the baseline retroactively, which is what it is for, and `Advise` is
  the untried rung — this had never been said at the moment it happened, only
  written down somewhere read hours earlier. Not `Deny`, because an API merge
  is legitimate when the checks really were read first.

  Matches only the raw numbered endpoint, never `gh pr merge` — that is the
  documented last step of the procedure, so firing on it would fire on correct
  use. Requiring the `merge` child of a numbered pull request keeps `/pulls`,
  `/pulls/10` and `/commits/{sha}/status` silent.

## v2.11.0

### Changed

- **Opacity is per pipeline, not per line.** One `xargs` or `sh -c` anywhere
  used to make the whole command unreadable, so every rule went silent for
  every other clause on it. Measured over 33,774 real commands: 369 were
  opaque and 218 of those had a readable pipeline thrown away — the recurring
  shape being `git status -s | xargs git add && git commit … | tail -1`, whose
  second run is exactly what the only blocking rule exists for. The unit is
  the pipeline because `|` chains one command's output into the next, so a
  stage we cannot read makes that run unreadable; `&&`, `||` and `;` do not.
  Unreadable clauses are marked in place and never removed: dropping them
  would leave a clause's `next` still saying `Pipe` while the neighbour is now
  some other run's, inventing a `git push | tail` in the rule that refuses.
- **`eval`, `source` and `.` still hide the whole line.** They run in this
  shell and can move it, and every `confirm` that resolves a path depends on
  knowing where the command runs. Excluding them removes that entire class by
  construction rather than defending against it, at a cost of 44 commands.
- **A partly-read command never refuses.** Findings from one are capped at
  advise whatever stance the rule carries, and journalled under their real
  stance so the evidence accumulates. Total opacity would have let the command
  run; blocking on half a reading is the worst outcome available.
- **`backtest` prints what it could not read** — `opaque` was counted and
  never shown — and `check` names the unread run beneath the verdict.

### Fixed

- **Two rules re-derived what the lexer had already decided.** `sed-in-place`
  scanned every word for `-i`, so `env -i sed s/a/b/ f` found `env`'s flag and
  advised about a spelling nobody wrote. `kubectl-gitops` re-found the program
  by name, which lands on the flag's value in `sudo -u kubectl kubectl apply`,
  so an imperative write went unremarked. `program_index` is `pub(crate)` now:
  a rule that reads past the program asks rather than walks the words itself.

## v2.10.0

### Added

- **The file tier.** `install --write` now adds a `PreToolUse` entry for
  `Read|Edit|Write|MultiEdit` beside the Bash one (`doctor` says so until it
  is re-run), and the hook keeps one small record per session under
  `<config>/amont-agent/sessions/`: every Read, every `cat`-shaped dump, and
  every Edit or Write, by path. Nothing else in the crate remembers anything
  between calls; this is the narrowest state that answers one question.
- **`file-reread`**, advising: a Read — or a `cat`, `sed -n`, `head` — of a
  file this session already read, with nothing written to it since. Measured
  over 41,700 tool calls: 3,381 such re-reads in two sessions of every three,
  15% of every byte the tools returned. A write in between makes the re-read
  right, and the rule says nothing; a different `offset`/`limit` window is a
  different read. `examine` never fires — the backtester has no session to
  consult — so its evidence is the transcript measurement.
- **`whole-file-dump`**, advising: a file poured whole into the tool result
  (`cat FILE`, `sed -n '1,400p'`, `head -200`) where the Read tool would have
  given line numbers, a cap and a window. Files opened this way were 31.5% of
  all tool-result bytes. `confirm` looks at the file: a whole dump under 4 KB,
  a window under a hundred lines, a pipe or a redirect are all left alone.
- **`persisted-output-dump`**, advising: reading back whole a tool result the
  harness saved to a file for being too large — 31 of 83 were, within five
  calls — when `grep`, `head -c` or a windowed Read would take the part that
  was wanted.

### Changed

- **`foreground-poll` compares the loop's budget with the call's timeout.**
  `confirm` now reads the counter (`seq 1 36`, `{1..20}`, `-lt 40`), the
  deadline (`SECONDS+540`, `--timeout=180s`) and the `sleep`, and stays silent
  when the loop's own budget fits inside `tool_input.timeout` (two minutes
  when unset). What is left is a loop that can outlast its clock — the shape
  behind all 110 ten-minute kills measured, every one in the foreground. That
  is the precision a `deny` needs.
- The payload now carries `tool_input.timeout`, and a `Context` its
  `timeout_ms()`.

## v2.9.0

### Added

- **`path-operand-missing`**, advising. A read of a path that is not there —
  `grep -rn X app e2e docs` after `e2e` was renamed, `wc -l` over a file that
  moved, `cat` of a package not installed here. At a terminal that is the
  loudest failure there is; under the Bash tool it is not: `grep` is Claude
  Code's function over ugrep, where a missing path is a `warning:` in
  mid-stream while matches from the other operands print normally, and the
  coreutils print one line and carry on, with the chain's last clause holding
  the exit status. Measured over 32,710 calls: 206 such reads, 71% reported
  as success, the path never mentioned again in two of three. `examine`
  fires on a reader with a literal path operand; `confirm` asks the
  filesystem. Probes (`2>/dev/null`, `||`, `[ -f`), sequences that may have
  just created the path, and anything inside `ssh`/`kubectl exec` are left
  alone.
- **`stat-bsd-format`**, advising. `stat -f '%Sm' file` on GNU coreutils
  reads `-f` as `--file-system`, takes the format as a filename, and prints
  the filesystem report for the real file — six lines and exit 1, which a
  `$(stat -f …)` substitutes where a timestamp was wanted. The mirror of
  `sed-in-place`: `confirm` runs `stat --version`. Rare, and the last spelling
  on a GNU-first Mac that succeeds into the wrong answer. Known gap: a
  `stat` inside `$( )` is a substitution the lexer blanks, so the most
  common real shape is invisible to it.
- **The session notice names the grep shim.** Claude Code's shell snapshot
  runs `grep` as ugrep with `--ignore-files`, so a recursive grep skips every
  .gitignored path silently — 3,128 recursive greps measured, and
  `--no-ignore-files` used zero times. Undetectable per call (the shape is
  one call in ten), so it is said once at session start, only where a
  snapshot defines the function.

### Measured, not built

The GNU-versus-BSD hypothesis for `awk`, `xargs`, `tar`, `column`, `date`,
`base64` and the coreutils: 85 mismatches in 32,710 calls, 70 of them
`sed -i ''`, everything else 0 or 1. Claude Code's `find` is bfs, which
implements every GNU primary tried.

## v2.8.0

### Added

- **Three rules for the shell the Bash tool actually runs.** It is zsh, not
  the login shell, and two of zsh's defaults turn ordinary bash habits into
  silent failures. `glob-no-match` (advising, with a `confirm` that expands
  the pattern against the directory the clause runs in): an unquoted glob
  operand that matches nothing aborts the clause before it starts, the
  `2>/dev/null` beside it cannot hide the shell's own message, and a later
  clause carries the exit status — measured, 86% of 212 real cases came back
  reporting success, and in 170 of them files that did exist went unread too.
  `glob-in-flag-value` (advising): `--include=*.ts` is expanded, or fails,
  before grep sees it — 95% of 132 reported success. `equals-separator`
  (observing): a word beginning with `=` is a command lookup under zsh's
  `EQUALS`, so `echo ===` and `[ "$a" == "b" ]` fail and take every later
  clause with them — loud in 91% of 78 cases, which is why it only watches.
  All three confirm the tool's shell first (`tool_shell`) and stay silent for
  bash, which passes an unmatched glob through as text. This revisits the
  `fish-glob` rule deleted before 1.0: its "fails loudly" argument was made
  from command shape, and the shell's diagnostics say otherwise.

## v2.7.0

### Added

- **`stdin-hang`**, observing. A command that will read standard input with
  nothing on it — `cat > file`, a bare `python3`, `tee` outside a pipe, `sort`
  with no file — does not fail under the Bash tool: stdin is open and never
  closes, so it blocks silently until the tool's ten-minute clock moves it to
  the background, where it blocks some more. The crate's admission test, met
  exactly: nothing names the cause, so no correcting loop can form. Fed is a
  pipe, any `<` redirect, a heredoc, a here-string or a process substitution.
  Measured over 32,401 real commands: one firing, the `cat >` that cost its
  author ten minutes the night it was written, and no false positive once
  `--version`/`-v` were understood as exiting before reading.

### Fixed

- **The lexer reads `<<< word` as a here-string.** It was read as a heredoc
  with no terminator, which made the whole command opaque — every rule said
  "no opinion" on `bc <<< "1+1"` and `python3 <<< 'print(1)'`.
- **A process substitution after a redirect is the redirect's target.** In
  `grep x < <(cmd)` the `(` ended the clause and the `<` was lost, so the
  command read as fed by nothing. `cat <(sort a)` was the same shape.
- **A clause remembers that it carried a heredoc.** The operator, tag and body
  are all consumed by the lexer; `Simple::heredoc` now records that stdin was
  fed, which `stdin-hang` needs and nothing else could see.

## v2.6.0

### Added

- **`status` reads the journal.** It always promised to — *"every rule, its
  stance, and what it has seen"* — but every function in `journal.rs` wrote and
  none read; the only way to ask was `awk` on the file. A `seen, last 30 days`
  column now counts, per rule, what was denied, advised, watched, and how often
  `confirm` declined — with the leading reason. That last number is the one the
  observe-then-graduate loop runs on and the one nothing else can supply: the
  backtester replays `examine` against a world that has moved, so it never runs
  `confirm`. First read of the real journal: `push-preflight` 2 advised against
  143 unconfirmed, `foreground-poll` 24 against 120, `branch-force-delete` 0
  against 19 — rules whose precision lives entirely in `confirm`, now visible.
- **The journal records why `confirm` said no**, in the rule's own words, where
  it used to write the literal `skipped`. A rule declined a thousand times for
  `already in a linked worktree` is a rule watching a convention being followed,
  and that sentence is what tells a reader whether it is precise or merely quiet.
  Older records still count; they show no reason rather than an invented one.
- **`doctor` names the observing rules** beside the acting ones. A rule that
  ships `observe` is live and journaling from its first release, and a line that
  listed only what refuses read as if it were not installed at all.

### Fixed

- **`stale-base` no longer abandons a script at a bare `git`.** Its clause loop
  used `?` on `cmd.subcommand()`, so `git; git checkout -b feat/x` ended the scan
  before reaching the checkout. `let else` skips the clause instead.

- **The id column in `status`, `rules` and `corpus check` is now measured from
  the ids, not written down.** It was the literal `18` in four places.
  `worktree-remove-force` is 21 characters and pushed the later columns out of
  line; `worktree-isolation` is exactly 18 and ran into the next column with no
  space at all, which is how `worktree-isolationobserve` reached a release. Not
  only cosmetic: `tests/stance_scope.rs` reads a stance out of those columns by
  whitespace position, so a glued line hands back the wrong field — latent only
  because the fixtures ask about short ids. A test now walks every listing and
  asserts the first word of each row is an id the binary actually declares.

## v2.5.0

### Added

- **`worktree-isolation`**, observing. A working directory has one HEAD. When
  an agent and a person are both in the primary checkout, a branch created or a
  `--hard` reset by one moves the ground under the other: uncommitted edits ride
  onto a branch they were never meant for, or are reverted outright. Git prints
  nothing unusual — the checkout succeeds — so no correcting loop forms from the
  outcome; the first sign is a file that "went missing", found much later.
  `examine` fires on shape at 5.6 per thousand, flat across four full weeks of
  31,119 calls. `confirm` supplies the two facts that make it a finding: the
  command runs in the **primary** checkout (`--git-dir` equals
  `--git-common-dir` only there), and that repository **already has linked
  worktrees**, which is the evidence that someone is there to collide with.
  Most matched shapes are `cd <repo>-wt-<slug> && git checkout -b …` — the
  convention being followed — and `confirm` silences every one of them.
  Navigating back to the default branch, and `git worktree add -b` itself, are
  never matched: the second is the remedy, and a rule its own advice trips is
  unobeyable.

## v2.4.0

A guard that only ever read commands before they ran now also checks what a
command that reported success actually did. `git push` exiting 0 without the
push landing, `gh run watch --exit-status` returning 0 for a run that concluded
`failure`, a `v*` tag on stale HEAD: all of them report success, and none of
them leaves anything behind for a correcting loop to notice. The same session
that built this produced a fresh example of the class — a polling loop that read
`Could not resolve host` as a CI verdict and announced it as fact — which is the
other half of this release.


### Added

- **`poll-blank-verdict`** — a polling loop that stops on any value other than
  the one it names, so a failed lookup's empty string ends the wait and gets
  reported as the answer. Measured live on 2026-09-06: a laptop woke from a
  four-hour sleep, the loop resumed 3.6 seconds later before the resolver had
  settled, `curl` printed `Could not resolve host`, and the loop announced a CI
  state of "" as fact — twenty-seven seconds later the same name resolved in
  40 ms. It is `pipe-to-tail`'s failure in a different costume: the absence of
  an answer treated as one. 33 firings in 30,486 calls, all reviewed into the
  corpus; ships at `observe`, and the corpus already clears the graduation gate
  if you want `advise`.


Every rule so far reads a command before it runs. That catches a mistake you
can see in the command string, and misses the other half: a command that ran,
exited 0, and did not do the thing.

### Added

- **A second tier: assertions, on `PostToolUse`.** An assertion checks what a
  command that reported success actually did. The first one is `push-landed`:
  after `git push` exits 0, it asks the remote whether the branch is there, at
  the commit you have, and states the two SHAs when they differ. That is the
  2026-08-20 incident — several pushes reported exit 0 and had never left the
  machine — and denying `pipe-to-tail` removed one cause of it, not the class.
  `Everything up-to-date` is also exit 0.
- **`install` now writes three settings entries, not two.** The new one is
  `PostToolUse` on `Bash`. Re-run `amont-agent install --write` to add it;
  `doctor` warns until you do. **Nothing else changes if you don't** — the
  existing two entries keep working exactly as before.
- **`backtest` and `explain` accept an assertion id**, replaying its pure
  `examine` like any rule's. Read the rate correctly: for a rule a firing is a
  mistake caught, for an assertion it is a question asked. `push-landed` fires
  on about one Bash call in twenty-six — that is the cost of one
  `git ls-remote`, not the cost of speaking, and the new `routine` trend label
  says so rather than pretending a habit is failing to improve.
- **`docs/assertions.md`**, and `rules` lists assertions beside the rules.

### Notes

- **An assertion cannot refuse** — the tool has already run, so `deny` speaks
  exactly like `advise`, as it does at a session opening.
- **`PostToolUseFailure` is ignored on purpose.** A failed command is already
  in front of the model; there is nothing invisible left to point out.
- **Assertions ship at `observe`**, like everything else here: they journal and
  say nothing until you promote them with
  `git config --global amont.agent.push-landed.stance advise`.
- **A `verify` can never prompt.** `GIT_TERMINAL_PROMPT=0`, askpass disabled,
  ssh in `BatchMode`, five-second deadline. Hooks have no controlling terminal,
  so a credential prompt would hang the session rather than fail.

## v2.3.0

A stance is a statement by the person at the keyboard. Until now the
repository the agent was standing in could overrule it.

### Changed

- **Stances, and every `amont.agent.*` key, are read from `--global` and
  `--system` git config only.** Git's own search order ends at `--local` and
  `--worktree`, and the last file wins — so a `.git/config` outranked the
  answer you set, and `git config` is not a command any rule here refuses,
  which means the agent whose command was just denied could write one.
  `graduate` and `demote` already wrote `--global`; a stale `--local` key had
  been silently outranking them. **If you set a stance with a bare `git
  config` inside a repository, it is now ignored — re-set it with
  `--global`.**
- **`GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` are no longer stripped at
  startup.** The `GIT_*` sweep that removes `GIT_DIR` and friends — there so
  a session started inside a git hook cannot point our git commands at the
  wrong repository — was taking these two with it, and they say where config
  *lives* rather than which repository this is. Anyone keeping their git
  config somewhere other than `~/.gitconfig` had every stance ignored.
- **The journal is created `0600`, not `0644`**, and an existing `0644` one
  is narrowed in place the next time it is written. It holds command text,
  which the redactor already treats as carrying credentials; the mode should
  have said the same thing. A stricter mode is never widened.

### Fixed

- **A wrapper's own flags are no longer read as the program.** `nice -n 10
  git push`, `sudo -u root git push`, `timeout -k 5 90 git push` and `env -i
  git push` resolved to `-n`/`-u`/`-k`/`-i`, so every rule keyed on `git`
  went silent on the ordinary spelling of three of the six wrappers the
  reader exists to see past. The program's index now comes from the same walk
  as its name, so a program name appearing earlier as somebody else's
  argument (`sudo -u git git push`) can no longer displace it.
- **Redaction follows a credential into the next word.** `--password=x` was
  redacted and `--password x` was not, and neither was `Authorization: Bearer
  <jwt>`. Whatever closes the word — a quote, a comma — is kept attached, so
  the sample stays a command a reader can parse.
- **`uninstall` leaves an empty hook block the author wrote.** It dropped
  every block with an empty `hooks` array, which is not the same set as the
  blocks it emptied.
- **The `agents-md` notice drains amont's stderr while it runs** rather than
  after it exits. A child that filled the pipe buffer would have blocked in
  `write`, never exited, and been killed by the five-second budget — the
  notice going missing exactly when there was most to say.

### Added

- `AGENTS.md` and `CLAUDE.md` are now committed. They are generated by `amont
  agents-md` and had only ever existed in the author's working tree, so
  nobody who cloned this repository got them — and `amont agents-md --check`,
  which the session notice shells out to, had nothing to compare against.

## v2.2.0

Seven rules mined from the transcripts: nineteen thousand Bash calls across
forty-two sessions, sorted by what failed, was killed, or drew a correction.

### Added

- **`foreground-poll`**, advising. A polling loop (`until … do sleep 30;
  done`) or `gh run watch` in the foreground, where the Bash tool's clock
  kills it — 96 commands died at the ten-minute cap, sixteen hours of
  waiting for a kill. `confirm` reads `run_in_background` from the payload
  and stays silent for a call that is already detached.
- **`sed-in-place`**, advising. `sed -i ''` on GNU sed reads the empty
  string as the script (40 of 47 uses failed with `can't read s/…`); bare
  `-i` fails the same way on BSD. `confirm` asks `sed --version` and speaks
  only when the spelling and the sed disagree.
- **`kubectl-gitops`**, advising. An imperative `kubectl apply|create|patch|
  scale|delete …` from a repository that Flux or Argo reconciles: the
  controller reverts the change or the repository stops describing the
  cluster. Deleting a pod or a job is a restart, not drift, and stays
  silent; `confirm` looks for a reconciler's resources among the tracked
  YAML.
- **`tag-after-commit`**, advising. `git commit … && git tag vX` in one
  command: a refused commit leaves the tag on the previous one, and a
  tag-driven release publishes stale code under a new version — once, to
  npm, unrecoverably. Shape only.
- **`worktree-remove-force`**, advising. `--force` deletes whatever the
  worktree still holds; `confirm` runs `git status` inside it and speaks
  only when something would be lost.
- **`amend-pushed`**, advising. `git commit --amend` when a remote-tracking
  branch already contains HEAD.
- **`branch-force-delete`**, observing. `git branch -D` on a branch no
  remote has and the default branch does not contain.

### Changed

- The hook payload now carries `tool_input.run_in_background`, exposed to a
  rule's `confirm` as `Context::background`.

## v2.1.1

### Changed

- **`push-preflight`** now points at `amont rehearse --wait` (amont ≥ 1.28):
  it runs the push gate on a snapshot of `HEAD`, or follows the rehearsal a
  commit already started in the background, and stamps the tree. `amont run
  pre-push` remains the spelling on 1.27, and the remedy says so.

## v2.1.0

A new advisory rule for the moment a push is about to run a slow gate
inside its own connection.

### Added

- **`push-preflight`**, advising. A `git push` in a repository where amont
  runs a test gate at push time, on a tree that has not been rehearsed, is
  told to run `amont run pre-push` first. git opens its connection to the
  remote before `pre-push` and holds it idle for as long as the gate takes;
  a remote that closes idle sessions — Forgejo's own git timeout is six
  minutes — kills the push after the gate has already passed, and the
  failure reads as network. Measured 2026-09-04: three pushes in a row died
  that way, each paying the full suite, before a `--no-verify` retry of the
  already-attested tree went through — the bypass this rule exists to make
  unnecessary. amont ≥ 1.27 stamps the tree a passed gate ran against and
  `amont run pre-push` stamps `HEAD` with no connection open, so the push
  that follows skips the suite. `examine` fires on the shape of a push (not
  `--dry-run`, not `--no-verify`, not amont's own notes push); `confirm`
  stays silent unless amont guards the repository (an amont shim in
  `hooks/pre-push`), `amont list --json --stage pre-push` says a test gate
  runs, and `refs/notes/amont-gate` carries no `pre-push-*` token for
  `HEAD`'s tree.

### Removed

- `packaging/amont-agent.rb`, the seed used once to create the tap's formula.
  It has said `version "0.0.0"` with zero checksums ever since, while the
  real formula moved to 2.0.2 — a file that looks authoritative, is not, and
  drifts further with every release. Nothing referenced it.

  The tap is the single source for the formula, and `scripts/bump-tap.py`
  rewrites it on each release. amont keeps no seed either.

  **What is still missing, and is the real gap:** nothing verifies that the
  tap's formula can install what a release actually shipped. `publish-tap`
  runs `ruby -c`, which proves the file parses and nothing more. amont's own
  formula kept a `bin.install "amont-agent"` line for three releases after
  that binary left the archive, so `brew install` failed outright the whole
  time while every release went green — the checksums matched, the syntax
  was valid, and nobody ran brew. A stale copy in this repository would not
  have caught that; a post-publish `brew install` from the tap would.

## v2.0.2

### Fixed

- **`doctor` no longer accuses a live guard of being dead.** `SessionStart`
  writes the heartbeat once, at the beginning of a session, so any session
  open longer than the six-hour grace made the heartbeat age while the
  transcript kept being written — and `doctor` reported

  ```
  ✗ the guard has not run in 14h
  ```

  with the journal in the same directory, last written seconds earlier by a
  real `pipe-to-tail` denial. A long session is the normal case for this
  tool, so the check was accusing it of the one thing it was demonstrably not
  doing, and sending people to read debug logs for a hook that was working.

  The journal was already read for exactly this purpose and then never
  consulted at the verdict. It now is. Liveness takes the later of the
  heartbeat and the last firing, so the journal keeps the property its own
  comment claimed — it can confirm, never accuse: a genuinely quiet period
  leaves no entries and the heartbeat still decides.

- **`doctor` no longer writes to the journal it is inspecting.** The probe
  that proves the guard works feeds the real binary a command it must refuse,
  and that firing was recorded like any other. So every health check added a
  synthetic `pipe-to-tail` denial to the measurement — the same data `status`
  counts and the per-1000 evidence that gates `graduate` comes from. A rule
  looked more necessary the more often you asked whether the guard was
  healthy.

  It also made the liveness check above unfalsifiable once it started reading
  the journal, since the journal would always be seconds old by the time it
  was read. Both were found by the same test.

## v2.0.1

### Removed

- **There is no npm package, and there will not be one.** `amont-agent` was
  published to npm as part of v2.0.0 — or rather, five of its seven packages
  were, before npm's spam filter rejected the sixth by name. That block was
  worth listening to, because it stopped a package that should not have
  existed.

  `install` bakes the ABSOLUTE path of the running binary into
  `settings.json`, deliberately: a `PATH`-resolved command exits 127 into
  Claude Code's non-blocking bucket the moment `PATH` differs, which disables
  the guard with nothing to notice it. npm cannot supply a stable absolute
  path. Under `npx` the binary sits in npm's `_npx` cache, which npm garbage-
  collects. Under a project-local `npm i -D` it sits in one project's
  `node_modules`, while the guard it configures is machine-global — so
  `rm -rf node_modules` in a single repository would silently disable the
  guard for every session on the machine.

  amont's npm package exists for a reason that does not transfer: it is a
  per-repository tool, and `npm i -D amont` plus a `prepare` script makes the
  hooks travel with the repository. This is per-developer machine
  configuration, and nothing about it belongs to a project.

  The five platform packages published under v2.0.0 have been unpublished.
  The root package `amont-agent` was never published — the release workflow
  publishes platform packages before the package that depends on them, so
  the failure stopped short of creating a root package with a broken
  `optionalDependency`.

  **Install with Homebrew, cargo, the shell installer, or a release binary.**
  Each hands `install` a path that stays put.

## v2.0.0

`amont-agent` is now its own project. It was previously a third binary inside
the [amont](https://github.com/fredericrous/amont) release — bundled in the
tarball, in the `amont` npm package, and in amont's installers.

**Nothing about how the guard behaves has changed.** The hook's output is
byte-identical to 1.18.2 for every rule. Config keys are unchanged
(`amont.agent.*`, `$AMONT_AGENT_OFF`), the journal is still at
`~/.claude/amont-agent/journal.log`, and an existing `settings.json` entry
keeps working.

### If you had it from amont

Your installed copy still works and is not removed. amont 1.19.0 stops
shipping it, so to keep getting updates install it from here:

```sh
brew install fredericrous/tap/amont-agent
# or
curl -fsSL https://raw.githubusercontent.com/fredericrous/amont-agent/main/install/install.sh | sh
```

Then `rm ~/.local/bin/amont-agent` is safe once the new one is on `PATH`, and
`amont-agent doctor` will confirm which one Claude Code is actually running.

### Why the major bump

The version had to exceed 1.18.2 on crates.io regardless. `2.0.0` marks the
real break — it is no longer bundled and must be installed separately — and
keeps the two version streams independent, so `amont 1.19` and
`amont-agent 2.0` never look like they must match.

### Changed

- **No `amont-runtime` dependency.** Six small modules replace what this crate
  used to reach for. `cargo tree` is now `serde` and `serde_json` and nothing
  else — both only for *reading*.
- **A cloned repository can no longer weaken a stance.** amont's config reader
  lets a repository's committed policy `set` lines outrank system and global
  git config — correct for a hook manager, wrong for a guard, because it meant
  a cloned repository could set `amont.agent.<rule>.stance = observe` and
  disarm the guard on the machine of whoever cloned it. The reader here has no
  policy ladder. Stances answer to your own git config and to nothing a
  `git clone` can carry.
- **`settings.json` permissions are no longer widened.** The previous write
  path set mode `0644` unconditionally. An existing file now keeps the mode it
  had, and one created here starts at `0600` — it can hold MCP environment
  blocks, and those hold credentials.
- **The stale-`AGENTS.md` notice now shells out** to `amont agents-md --check`
  instead of linking amont's generator, and decides on amont's *stderr* rather
  than its exit code. `agents-md --check` returns 1 for a file it could not
  read as well as for a drifted one, so the exit code alone would announce
  staleness for a permissions error. With no `amont` on `PATH` the check is
  silent, which is the right answer for anyone who does not use amont.

### Fixed

- **The declared MSRV was not buildable.** Every published version up to 1.18.2
  claimed `rust-version = "1.74"`, inherited from amont's dependency-free
  commit path and — as that manifest's own comment predicted — long since
  drifted: `serde_json`'s `preserve_order` pulls `indexmap` → `hashbrown
  0.17.1`, which is edition 2024 and requires 1.85. Cargo 1.74 could not parse
  it at all. The floor is now **1.85.0**, measured (1.84 fails, 1.85 passes)
  and enforced by CI against the committed lockfile.

### Fixed (2)

- **A closed pipe no longer panics.** `amont-agent rules | head`,
  `--help | grep -q`, or any listing whose reader hangs up early printed a
  Rust backtrace — "failed printing to stdout: Broken pipe" — instead of
  dying from the signal like every other Unix filter. Rust ignores SIGPIPE at
  startup, so `println!` hits EPIPE and EPIPE is a panic.

  This is a bug the split introduced by omission: amont's `main` has restored
  SIGPIPE's default disposition since `amont list | head` panicked, and this
  crate left the workspace with the modules it imported rather than the ones
  it needed. The v2.0.0 release dry run caught it, from a smoke step doing
  `amont-agent --help | grep -q backtest` — which failed on exactly one of
  six build targets, because the output is small enough that whether the race
  fires depends on the machine.

### Added

- **Documentation.** `backtest` → `explain` → `corpus check` → `graduate` is a
  coherent measure-then-block workflow and was previously undocumented
  anywhere. It now has [a page of its own](docs/measuring.md), with the
  `pipe-to-tail` decision as the worked example.
- Windows is now in the test matrix alongside Linux and macOS.
