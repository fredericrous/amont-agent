# The board: session activity

Every session records which repository, worktree and branch it edits, so that
another session can tell whether somebody else is working in the same place.
`work-overlap` reads it, and `amont-agent board` lists it (see below).

## The activity file

One file per session, `<journal dir>/sessions/<full session id>.activity`
(the journal dir is `<config dir>/amont-agent`, `~/.claude/amont-agent` by
default). Each line is

```text
<ts> <repo_identity> <worktree_root> <branch>
```

whitespace-separated, where

- `ts` is epoch seconds;
- `repo_identity` is the git common dir, shared by every worktree of a
  repository (`/r/amont-agent/.git`);
- `worktree_root` is the worktree's top directory;
- `branch` is read from `HEAD`, or `detached`.

A space, tab, newline or `%` inside a field is percent-encoded (`%20`, `%09`,
`%0A`, `%25`), so a path with a space still splits into four fields.

The place is resolved without running a process: the hook walks up to `.git`,
follows a `gitdir:` file and its `commondir`, and reads `HEAD`.

## Which calls record

A call records when its `PreToolUse` payload is

- an Edit, MultiEdit, Write or NotebookEdit: the place is the one of the
  file's parent directory;
- a Bash command that runs `git commit`, `push`, `tag`, `worktree add`,
  `checkout -b` or `switch -c`: the place is the one of `git -C <dir>`, else
  of a leading `cd <dir> &&` or a subshell `(cd <dir> && ...)`, else of the
  payload's cwd.

A Read, Grep or Glob, and any other Bash command, records nothing. A target
outside any git repository records nothing. The payload's cwd is never the
place of a file tool.

## Last seen, and what is live

A line is appended only when the place differs from the file's last line.
Every other recorded call only moves the file's modification time. **The
modification time is the session's last-seen time**, not `ts`.

Only the latest line of a session is live: a session that worked in
repository A and then moved to B is not in A any more, whatever its earlier
lines say. A session is live in that place while its file's modification time
is less than the window old.

`work-overlap` looks for peers in two passes, so a write that meets nobody
spawns nothing. The first pass reads only files modified within the last week
(the sweep's horizon; older files are skipped from their metadata), and only
their last line. Only when one of them is in the same repository is the
window read from git config, and the second pass keeps the live ones.

A changed-place append to a file longer than 256 KiB first trims it to the
newest complete lines that fit in 128 KiB, the new line included (a new line
that alone exceeds 128 KiB is kept alone); a call that finds the same place
never trims. The trim writes `<file>.tmp.<pid>` (mode 0600) and renames it
over the file, so a reader sees the old or the new file whole. A line another
process appends during a trim may be lost. The session sweep deletes it after
a week. The record is written after the decision and never changes it; a
failure to write, and a trim that fails (the line is then appended to the
untrimmed file), are each journalled once per session and then ignored.

## `work-overlap`

Advise, ceiling advise. On a session's first Edit, MultiEdit, Write or
NotebookEdit in a repository, it advises once when another session's latest
line names the same repository and that session is live. The window is 2 h
and is set with `git config --global amont.agent.work-overlap.window`
(`90m`, `3h`, `2d`; a bare number is seconds). The message names the other
session, its worktree, its branch, the title of the active plan its branch
added (the first active file under `docs/plans` that the worktree's `HEAD`
adds relative to the repository's default branch, read with bounded git calls
only when the rule fires; none for a peer on the default branch) and how long
ago it was last seen. The plan lookups of all named peers (at most three)
share one 2 s budget, and a peer reached after it is named without a plan.
The message then points to `amont-agent board --repo <repo
name>`, then to a message to that session (SendMessage) or to the person. It
is silent when the only matching activity is the session's own, and fires
when another live session works in the very same worktree.

The once-per-repository telling is used up only when a live peer is found:
a session whose first write meets nobody is told on a later write, once a
peer appears. Under `observe` the journalled firing counts as the telling,
so the journal holds one line per session and repository.

## `amont-agent board`

```text
amont-agent board [--repo <name>...] [--since <dur>] [--fetch] [--json]
```

Read-only. It lists, per repository, the work in flight and what landed
recently, derived from git and the activity files on every call. It reads the
activity files through the same reader as `work-overlap` and never writes
them.

Repositories: those named with `--repo` (resolved like plan-review `repos=`
names: `git config amont.agent.plan-review.<name>.path`, else
`<plan root>/<name>`), else the working directory's repository, else every git
repository directly under the plan root.

**In flight**: each worktree of `git worktree list --porcelain` (the main
checkout on the default branch with no session is not listed), and each local
or `origin/` branch not merged into the default branch that is live. A branch
is live when its tip commit is newer than `--since` (default 48h), when it is
checked out in a worktree, or when a session's latest activity line names it.
Plans are read (`git diff`, `git show`) only for live branches: a stale
unmerged branch, with or without a plan, costs no git call beyond the single
`for-each-ref` listing and is not listed. Each entry shows:

- the path (if checked out) and the branch; a branch checked out in a worktree
  whose change is already on the default branch is marked
  `(merged: git worktree remove <path>)`;
- the title of the active plan: a plan file under `docs/plans` that the branch
  adds relative to the default branch and whose `status` is `active`. A plan
  whose `repos:` names several repositories is listed under each of them: in
  another repository's block it appears as a branch-only entry labelled
  `<repository>:<branch>` (in branch-name order), found by scanning the other
  repositories directly under the plan root (primary checkouts only: a linked
  worktree directory is never scanned, and a repository is scanned once),
  unless that repository carries the same file itself.
  This scan runs when one repository is selected (`--repo`, or the cwd);
  listing the whole root skips it, since every repository is listed anyway.
  For such a repository a branch is live when its tip is within `--since` or
  it is checked out; its sessions are not read here (ask `--repo` for that
  repository). It costs one `git for-each-ref` call (default branch, branch
  tips and checked-out branches), so a sibling with no live branch costs
  exactly one git call; a live branch adds one merged-ness listing, then
  plans for it;
- `programme <slug> ticket <id>` for a `prog/<slug>/<id>` branch;
- the sessions whose latest activity line names that worktree, with the full
  session id and the age of the activity file's modification time.

**Merged**: a live candidate branch (a worktree's branch, or a live branch
with no worktree) is merged when its whole change from the merge-base
(`git diff <merge-base> <branch>`) is not empty and its `git patch-id
--stable` equals the patch-id of one non-merge commit on the default branch
after the merge-base. The default branch's patch-ids are built once per
repository per call, from the oldest merge-base among its candidates
(`git log -p --no-merges <oldest-merge-base>..<default>` into `git patch-id
--stable`); each candidate then costs a merge-base and one diff. Every diff
and log runs with `--no-ext-diff --no-textconv --no-color`, and nothing is
written to the repository. A merged branch with no worktree is left out; one
in a worktree stays listed with the label above. This is how a squash merge
reads as merged although the branch's commits are not ancestors of the
default branch. Known limits:

- Without `--fetch`, a branch merged after the last fetch reads as in flight.
- A squash whose content changed while merging (a conflict resolution), or one
  merged after the default branch changed lines within 3 lines of the branch's
  change, is not recognised and stays listed until it leaves the `--since`
  window.

**Just landed** (`landed` in the JSON): `git log --first-parent <default>
--since <dur>`, with short sha, subject and age. `--fetch` first runs
`git fetch --prune origin -q` on the selected repositories, up to 4 at once,
each within 60 s, and every fetch finishes before any report is built; a
failure is one stderr line starting `<repo>:`, in the order of the selected
repositories. When stderr is a terminal a `fetching <n> repositories...` line
comes first.

A repository with nothing in flight and nothing landed prints nothing unless
it was named with `--repo`. `--json` prints an array of `{repo, path,
worktrees: [{path, branch, merged, plans, programme_ticket, sessions: [{id,
last_seen_secs}]}], landed: [{sha, subject, age_secs}]}`; `merged` is a boolean; `plans` is an array of titles (empty when none); `path` is null for a
branch with no worktree, `programme_ticket` is `<slug>/<id>` or null. Exit 0
unless the command itself fails; an unreadable repository is reported on
stderr and skipped.
