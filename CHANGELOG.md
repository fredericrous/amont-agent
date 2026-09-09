# Changelog

## Unreleased

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
