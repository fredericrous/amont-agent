# The board: session activity

Every session records which repository, worktree and branch it edits, so that
another session can tell whether somebody else is working in the same place.
`work-overlap` reads it; `amont-agent board --repo <repo>` lists it.

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
