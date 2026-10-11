---
status: active
branch: feat/attestations-sweep
repos: [amont-agent, dotfiles]
adrs: [ADR-0022]
---

# Attestations are swept, except what a plan cites

## Review panel

👉 **Decide:** none — approve if 30 days, plus whatever a plan cites, plus a 7-day trash, is the right retention for evidence.
📍 amont-agent, dotfiles · plan reviewed · next: `src/attestations.rs` and the subcommand, v2.34.0; dry check 2026-10-27. Panel: backend, rust, tui, unix.
**Changed by review:** any scan error removes nothing; removal goes to a timed trash first; one locked run at a time, `--type=path` roots.
**Verdicts:** round 1 four approve-with-changes; round 2 backend approve-with-changes on the final body; 4 findings carried into implementation.
📄 Full reviews: [2026-10-11-attestations-swept-except-cited.reviews.md](2026-10-11-attestations-swept-except-cited.reviews.md)

## Context

`~/.claude/amont-agent/attestations/` is where the worktree-task skill puts
evidence (F3 records, F7 preview screenshots and `guide.md`, and the
`index.html` page `amont-agent preview register` renders beside a guide),
and where sessions drop ad-hoc measurement output. Nothing ever removes
anything from it. Measured 2026-10-11:

- 121 directories, **71 MB**, all written since 2026-09-28 (about 5 MB a
  day, about 1.8 GB a year). 71 are named by a commit sha, 50 by a slug.
  Many sha names are the full 40 hex characters.
- The two largest (11.7 MB each, `v2-size-diff-*`) are raw JSON dumps; the
  rest are screenshots (411 png, 65 jpg), guides and logs.
- **7 directories are cited by landed plans** in 5 repositories
  (`amont-agent`, `application-landscape`, `happier`, `relais`,
  `activity-vision`), e.g. `attestations/d78826f/` in
  `2026-10-07-publish-without-skill.md`. A plain age sweep would leave those
  plans pointing at nothing.
- Finding the cited ones is cheap: a text scan of the 379 `docs/plans/*.md`
  under `~/Developer/Perso/*` (5.6 MB) for `attestations/<name>` takes
  0.18 s.

The disk ran full on 2026-10-09 (239 MB free), so an unbounded store under
`~/.claude` is worth bounding now, while the decision of what to keep is
still small.

## Approach

### What is kept

A direct child of `attestations/` is **kept** when any of these holds:

1. **Recent:** its newest entry (the directory and every file under it,
   by mtime) is younger than **30 days**. That covers an open PR, a preview
   awaiting an answer (registrations live 24 h) and a plan being written.
   A future mtime or an unreadable mtime counts as recent.
2. **Cited:** its name is cited (see below) in a Markdown file under
   `docs/` of a known repository, or in `~/.claude/plans/*.md`.
3. **Pinned:** it contains a file named `.keep`.

Everything else is **removed**. A child that is not a directory, or is a
symlink, is left alone and never followed. Names starting with `.` are never
candidates (that keeps `.trash-*` out of the rules above).

### What a citation is

A citation is the text `attestations/<name>` where `<name>` is the maximal
run of `[A-Za-z0-9._-]` after the slash, with a trailing `.` stripped (a
sentence end). A following `/`, end of line or any other character ends it.
So `attestations/x/guide.md` cites `x`, and `attestations/x.` cites `x`. It
is plain `str::find`: no regex, no new dependency. The full path
`~/.claude/amont-agent/attestations/x` contains the same text and counts.

Slug names match exactly (`abc` does not keep `abc1234`). A cited name of at
least 7 hex characters also keeps a directory whose name is 40 hex and starts
with it, because plans cite short shas.

### Where the repositories are

The same roots `plan_review` already resolves (`src/plan_review.rs`
`root()`, `ENV_ROOT`): every directory directly under the plan root
(`$AMONT_AGENT_PLAN_ROOT`, else `~/Developer/Perso`) that has `docs/`,
including task worktrees (they sit there as siblings), plus every
`amont.agent.plan-review.<name>.path` from global git config (dotfiles).
The scan reads every `docs/**/*.md` of each root, not only `docs/plans/`,
plus `~/.claude/plans/*.md`.

No network. One subprocess of its own, `git config --global --type=path
--get-regexp '^amont\.agent\.plan-review\..*\.path$'`, at most once a day
(`--type=path` expands the `~` in `path = ~/.local/share/chezmoi`, as
`plan_review` already does). Not the `gitconfig` snapshot, which reads
`--system` too and returns no keys when git does not run: here git not
running, or any exit other than 0 and 1, is a scan failure (below); exit 1
with empty output is git saying no key matched: no configured roots.

One run at a time: a run starts by creating `attestations.sweeping` with
`create_new`; only the holder checks the stamp and scans, and removes the
file when done. A lock older than an hour is a crashed run: it is taken
over by renaming it to `attestations.sweeping.stale-<pid>` (only one rename
can win), and only the winner then creates the lock; the others back off.
`--apply` takes the same lock, and exits 1 with `a sweep is running` when
another run holds it. A scan failure stops removal of directories, not the purge of
`.trash-*` entries, which were already decided.

### When the scan fails

Any read error during the scan stops **all** removal for that run: a
configured root missing (including an
`amont.agent.plan-review.<name>.path`), a `docs/` directory or a Markdown
file unreadable, `~/.claude/plans` unreadable, or the `git config` call
failing. The dry run still lists, marking every would-be removal
`held (scan failed: <path>)`. The run journals `attestations-scan-failed
<path>`. Any error stops removal; there is no tolerated case.

### Filesystem rules

- `symlink_metadata` throughout. Symlinks are never followed, for recency or
  for removal.
- `remove_dir_all` does not follow nested symlinks (fixed since Rust 1.58.1,
  CVE-2022-21658; MSRV is 1.85).
- Removal tolerates NotFound (another process got there first).

### Removal goes through trash

A directory to remove is first renamed to
`attestations/.trash-<unix-secs>-<name>` (same filesystem, atomic). The time
is in the name because a rename does not change the moved directory's own
mtime; it also keeps two trashings of one name apart. The sweep deletes a
`.trash-*` entry once the time in its name is 7 days old (an entry whose
name carries no readable time is left alone). That gives a 7-day undo:
`mv .trash-<secs>-<name> <name>`.

### When it runs

- `amont-agent attestations` (new subcommand): one row per directory:
  verdict, name, age, size, reason (reason last), with `remove` rows sorted
  last. Reasons read `recent 3d`, `cited by happier/docs/plans/…`, `.keep`,
  or `held (scan failed: <path>)`. **Dry by default**; `--apply` removes.
  - On a terminal: columns aligned with widths measured from the data (the
    `src/ui.rs` pattern).
  - When stdout is not a terminal: tab-separated, same column order, no
    colour.
  - Totals and the copyable hint `nothing removed; amont-agent attestations
    --apply removes <n> (<MB>)` go to stderr.
  - Names and cited paths pass through `ui::sanitize` (names are
    untrusted). Ages use the `doctor` `ago` format (`41d`). MB means 10^6
    bytes.
  - Exit codes: 0 when the dry run or `--apply` finishes cleanly. 1 when the
    scan fails (message `amont-agent: <path>: <io error>; nothing removed`,
    and `--apply` removes nothing) or any removal fails (each named on stderr
    as `✗ <name>: <io error>` via `ui::error_sign`; the rest are still
    processed and journalled). 2 for an unknown flag, as in the rest of the
    binary.
- At `SessionStart`, beside `session_state::sweep()`,
  `preview::sweep()` and `plan_phases::sweep()` (`src/hook.rs` `decide`):
  `attestations::sweep()` applies, **at most once a day**. The stamp file
  `attestations.swept` sits beside the journal; its mtime is the clock. The
  lock holder touches the stamp (created, or mtime set) **before** the scan,
  so a failed scan still advances it and there is no rescan every
  session (the lock above, not the stamp, keeps two sessions from both
  scanning); `doctor` reports the failure. Every failure is silent at the
  hook, as in the other sweeps.
- One journal line per removed directory: rule `-`, outcome
  `attestation-swept`, excerpt `<name> <bytes>`; and one per run with the
  totals (`attestations-kept <n> <bytes>`), so a reader of the journal can
  see what went.

### Seen elsewhere

- `doctor` prints one line: `attestations: <n> directories, <MB>, oldest
  <age>; <k> cited; swept <age> ago`. It is a WARN when the last run
  journaled `attestations-scan-failed` (the detail names the path and the
  next command, `amont-agent attestations`) or when the stamp is older than 2
  days.
- `docs/preview.md` and the worktree-task skill's F3/F7 text gain one
  sentence: evidence under `attestations/` lasts 30 days unless a plan
  cites it or it holds a `.keep`; a removed directory sits in `.trash-<secs>-<name>`
  for 7 days.

### Expected effect today

The oldest directory is 13 days old, so the first run removes nothing; the
first removals land around 2026-10-28 and move to `.trash-*` first, then are
deleted 7 days later. At the measured rate the store then holds about 150 MB
(30 days) plus what plans cite.

## Non-goals

- Moving cited evidence into the repositories (it is kept out of the
  worktree on purpose: binary and outside the reviewed tree).
- Reading PR bodies on GitHub or Forgejo for citations: 30 days covers an
  open PR; a merged PR's evidence that matters is cited by its plan.
- A size cap. Age plus citations bounds it; a cap would remove recent
  evidence of open work first.
- Sweeping other stores (`review-briefs/`, `scratch/`, `reports/`), each
  under 3 MB.
- Making the 30 days configurable.
- A restore command: `mv .trash-<secs>-<name> <name>` is the undo.

## Phases

1. ✅ `src/attestations.rs` (keep rules, citation matcher, scan, sweep, trash),
   the subcommand, the SessionStart call with its daily stamp, journal
   lines, the doctor line and its warn, docs (`docs/preview.md`,
   `CHANGELOG.md`), tests, v2.34.0 — one PR. Tests pass `now` in and age
   files with `File::set_modified`; e2e tests set a scratch
   `CLAUDE_CONFIG_DIR`/`HOME` and `AMONT_AGENT_PLAN_ROOT` and never touch the
   real `~/.claude`.
2. Release (`tag-release`), `brew upgrade amont-agent`; run
   `amont-agent attestations` (dry) on the real store and check the 7 cited
   directories show as kept; one `SessionStart` writes the stamp and the
   `attestations-kept` journal line. The worktree-task skill sentence
   (dotfiles, F3/F7) lands with it.
3. On 2026-10-27, before the first real removals (about 10-28), run
   `amont-agent attestations` (dry) on the real store and check every
   `remove` line against `grep -r attestations/<name>` of every repo, and
   against its 7-character prefix for a 40-hex name; nothing cited may be in it.

## Verification

| check | input | expected |
|---|---|---|
| unit | a scratch store: `old` (40 d), `fresh` (2 d), `cited` (40 d, named in a plan), `pinned` (40 d, `.keep`), `deep` (40 d dir, one file inside touched 2 d ago), a file `loose.txt`, a symlink `link` → outside | removed (to `.trash-<secs>-old`): `old` only; `deep` kept as recent; `loose.txt` and `link` untouched, link target untouched |
| unit | plan root missing | nothing removed, `attestations-scan-failed <path>` |
| unit (cfg(unix)) | an unreadable plan file (chmod 000) | nothing removed, exit 1 |
| unit | `amont.agent.plan-review.dotfiles.path` pointing at a missing path | cited-by-dotfiles directories kept, nothing removed, scan-failed journaled |
| unit | `amont.agent.plan-review.dotfiles.path` pointing outside the plan root, present | its `docs/` scanned |
| unit | citation cases: `attestations/x/guide.md`, `attestations/x.` (trailing period), a name with `.` (`v1.2`), the full path `~/.claude/amont-agent/attestations/x`, `attestations/m0-duro-web` at a line end | each keeps the exact name |
| unit | short sha: plan cites `d78826f`, store has `d78826f` + 40 hex | the 40-hex directory kept |
| unit | plan cites `abc`, store has `abc1234` | `abc1234` not kept |
| unit | `~/.claude/plans/x.md` citing `new-thing` (40 d) | kept |
| unit | citation in `docs/notes/y.md` (not under `docs/plans/`) | kept |
| unit (cfg(unix)) | a symlink inside a stale dir pointing outside | the target untouched after removal |
| unit | a directory with a future mtime | kept |
| unit | trash: a 40-day directory removed, then sweeps 1 day and 8 days later (`now` passed in) | appears as `.trash-<secs>-old`; still there after the 1-day sweep; deleted by the 8-day one; a `.trash-*` is never a candidate; a `.trash-x` with no time is left alone |
| hook e2e | `SessionStart` twice in a row on a store with one 40-day directory | removed on the first; the second does not scan (stamp younger than a day) |
| hook e2e | two concurrent `SessionStart`s (`& wait`) | one `attestations-kept` line |
| doctor | the scratch store | `attestations: 5 directories, …, oldest 40d; 1 cited; swept 0d ago` |
| doctor | the last run journaled `attestations-scan-failed`; separately, a stamp older than 2 days | WARN in both; the first names the path and `amont-agent attestations` |
| subcommand | `amont-agent attestations` on the scratch store | lists verdict, name, age, size, reason with `remove` rows last; removes nothing; totals and hint on stderr; exit 0 |
| subcommand | `--apply` | removes `old` (to trash); exit 0 |
| subcommand | scan failure; unknown flag | exit 1 with `amont-agent: <path>: <io error>; nothing removed`; exit 2 |
| subcommand | `amont-agent attestations \| head -1` | exit 0, no broken-pipe error on stderr |
| unit | `attestations.sweeping` 10 min old | no scan, nothing removed |
| unit | `attestations.sweeping` 2 h old | taken over; the run proceeds |
| e2e | 20 concurrent `SessionStart`s on a store with a stale stamp | exactly one `attestations-kept` journal line |
| unit | `git config --get-regexp` exit 1, empty output (no `plan-review.*.path` keys) | not a scan failure; the sweep proceeds |
| unit | a scratch `HOME` whose global config has `amont.agent.plan-review.dots.path = ~/dots`, `~/dots/docs/plans/x.md` citing `by-dots` (40 d) | `~/dots` scanned, `by-dots` kept, no scan failure |
| e2e | a 2 h stale lock and 20 concurrent `SessionStart`s | exactly one `attestations-kept` line |
| subcommand | `--apply` while `attestations.sweeping` is held (10 min old) | `a sweep is running`, nothing removed, exit 1 |
| unit | a scan failure plus a `.trash-<secs>-x` 8 days old | no directory removed; the trash entry purged |
| subcommand (cfg(unix)) | `--apply` with one directory chmod 000 | the others removed, `✗ <name>: <io error>` on stderr, exit 1 |
| subcommand | a name containing `\x1b` | printed sanitised |
| subcommand | piped to a file | tab-separated, same column order, no colour |
| subcommand | widest line on the real store, on a terminal | at most 80 columns |
| timing | `SessionStart` with a stamp younger than a day: hyperfine against 2.33.0 | within 1 ms |
| timing | first `SessionStart` of the day on the real store (121 directories, 34 roots; first run, `sudo purge` not required) | under 300 ms including the git call; if over, the sweep moves out of `SessionStart` into the subcommand only |
| live | after release, `amont-agent attestations` on the real store | 7 cited directories `keep (cited by …)`, nothing `remove` today |
| live | 2026-10-27 dry run | every `remove` line absent from `grep -r attestations/<name>` over every repo, and from a grep of the 7-character prefix of each 40-hex name |

## Verification record (Phase 1, before the push)

| check | actual |
|---|---|
| unit / e2e / subcommand / doctor rows | `tests/attestations.rs` (15) and the unit tests in `src/attestations.rs`: all green in `make check`. ✅ |
| stale lock, 20 sessions | 50 rounds per run. With the lock alone, round 24 of one run journaled two `attestations-kept`; with the day claim (Decision log), 8 runs = 400 rounds, one each. ✅ |
| real store, dry run (branch build) | 119 directories, 71.9 MB, every row `keep` (the oldest is 12 days), exit 0; `doctor`: `attestations: 119 directories, 71.9 MB, oldest 12d; 7 cited; never swept` — the 7 found by hand. ✅ |
| real git config | snapshot, whole suite, diff: unchanged. ✅ (see Decision log) |
| timing | dry run on the real store at load average ~100: 0.4 s warm, 1.8 s cold; over budget, so the sweep is detached (Decision log). SessionStart interleaved, 15 runs each, load ~140: 2.33.1 p50 ~243 ms, branch with a stale stamp (spawns) ~255 ms, branch with a fresh stamp equal to the baseline. ✅ |
| live | Phase 2, after the release. |

## Decision log

- Detached sweep (asked 2026-10-11, the person chose it over the plan's subcommand-only fallback): the daily sweep measured 0.4–1.8 s, so `SessionStart` does one stat and, when the stamp is a day old, spawns `amont-agent attestations --daily` in its own process group and returns; the child takes the lock, re-checks the stamp, sweeps and journals as before.
- A cited directory under 30 days shows `recent`, the first reason that holds; `doctor`'s `cited` count is how the 7 citations were checked live.
- During implementation, an intermediate test wrote `amont.agent.plan-review.dots.path = ~/dots` into the real `~/.config/git/config` (git's `--global` followed the real `XDG_CONFIG_HOME`). It was removed by hand; the final tests set `HOME` and `XDG_CONFIG_HOME` to scratch, and a snapshot/diff around the whole suite shows the real config unchanged.
- `a_session_opening_on_a_stale_guidance_block_is_told` (existing, `tests/hook.rs`, 5 s budget for the `amont` stub) fails under load: the whole `hook` test binary failed 1/8 on this branch and 1/8 on `origin/main`, interleaved at load average ~150. Not this change; `make check` was re-run when it hit.
- Once a day is decided by `attestations.swept.<day>`, created with `create_new` after the lock: the 50-round stale-lock test caught two holders (a takeover that renames a live lock back can overwrite a lock a third process created in between). The claim is the decider; the lock still keeps a manual `--apply` and a daily run apart, best effort.
- deliberate: the scan-failure message reads `; no directory removed`, not the body's `; nothing removed`: the trash purge still runs after a failed scan (final plan-review round, carried).
- CI on the first push (#95): Ubuntu, the stale-lock e2e found a lock left behind with no owner: a takeover that renamed a live lock back could resurrect it after its holder had finished, blocking every run for an hour. The takeover no longer renames back: it drops its stale copy and backs off; the lock is advisory and the day claim keeps the daily run single. Windows: every unit test that ages a directory failed, because `File::open` on a directory needs `FILE_FLAG_BACKUP_SEMANTICS` there; both test helpers now open directories that way. A Windows-target `clippy` also caught `lsp::tests::fixture` (from #91) as dead code there; it is `#[cfg(unix)]` like its one caller.
- Rebased on #94 (`plan-review record`): CHANGELOG v2.34.0 carries both entries; the usage text and the subcommand table have both commands.
- Implementation via relais (run-65d8c5c05fb82-3aa1, budget_exhausted after its worker finished: baseline 5.4 min plus worker 17 min over the 20-minute wall clock); the candidate was applied and verified here, then the detached sweep, 50 lock rounds, the test wait and the version bump were added by hand.

## Implementation review

- approve (Delta, round 2), after approve-with-changes in round 1 (80k, 51 s; Deltas 84k, 11 s and 85k, 6 s).
- Fixed: symlink test no longer tied to a date, purge failures reported (✗, exit 1), help first and `--daily` documented, pure git-exit mapping tested, root guard on the chmod test.
- deliberate: `; no directory removed` (see Decision log).

<!-- panel: repos=amont-agent,dotfiles adds= reviewers=backend,lang:rust,tui,unix body-sha=148d0ee68826 -->
