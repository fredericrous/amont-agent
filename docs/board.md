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

The file is cut back to its newest 2000 lines when it passes 4000, and the
session sweep deletes it after a week. The record is written after the
decision and never changes it; a failure to write is journalled once per
session and then ignored.

## `work-overlap`

Advise, ceiling advise. On a session's first Edit, MultiEdit, Write or
NotebookEdit in a repository, it advises once when another session's latest
line names the same repository and that session is live. The window is 2 h
and is set with `git config --global amont.agent.work-overlap.window`
(`90m`, `3h`, `2d`; a bare number is seconds). The message names the other
session, its worktree, its branch and how long ago it was last seen, and
points to `amont-agent board --repo <repo name>`, then to a message to that
session (SendMessage) or to the person. It is silent when the only matching
activity is the session's own, and fires when another live session works in
the very same worktree.

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

- the path (if checked out) and the branch;
- the title of the active plan: a plan file under `docs/plans` that the branch
  adds relative to the default branch and whose `status` is `active`. A plan
  whose `repos:` names several repositories is listed under each of them: in
  another repository's block it appears as a branch-only entry labelled
  `<repository>:<branch>`, found by scanning the other repositories directly
  under the plan root, unless that repository carries the same file itself.
  For such a repository a branch is live when its tip is within `--since` or
  it is checked out; its sessions are not read here (ask `--repo` for that
  repository). It costs the default-branch probes, one `for-each-ref` and one
  `worktree list`, then plans only for its live branches;
- `programme <slug> ticket <id>` for a `prog/<slug>/<id>` branch;
- the sessions whose latest activity line names that worktree, with the full
  session id and the age of the activity file's modification time.

**Just landed**: `git log --first-parent <default> --since <dur>`, with short
sha, subject and age. `--fetch` first runs `git fetch origin -q` on the
selected repositories (a failure is one line on stderr).

A repository with nothing in flight and nothing landed prints nothing unless
it was named with `--repo`. `--json` prints an array of `{repo, path,
worktrees: [{path, branch, plans, programme_ticket, sessions: [{id,
last_seen_secs}]}], landed: [{sha, subject, age_secs}]}`; `plans` is an array of titles (empty when none); `path` is null for a
branch with no worktree, `programme_ticket` is `<slug>/<id>` or null. Exit 0
unless the command itself fails; an unreadable repository is reported on
stderr and skipped.
