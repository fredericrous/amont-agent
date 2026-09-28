# Preview approval

`push-preview` and `push-published` carry ADR-0023 (`work.preview-approval`,
fleet decision corpus): a commit that changes a user interface is published
only after the person approved **that commit**, seen on localhost.

The rule exists because interface regressions kept reaching pull requests and
deployed sites after the agent had checked its own screen. The person's eyes
are the check that does not share the agent's blind spots, and localhost is
where rejecting a screen costs one sentence instead of a fix pull request and
a release.

## The flow

1. **Verify.** The agent drives the changed route in a real browser, checks the
   console and network, and takes a screenshot. It writes that down in a file
   **outside the worktree** — `~/.claude/amont-agent/attestations/<sha>/` by
   convention — because a record inside it would dirty the commit it
   describes.
2. **Serve and register.** From the clean worktree, at the commit to be
   pushed:

   ```sh
   amont-agent preview register --url http://localhost:5173/settings \
     --attestation ~/.claude/amont-agent/attestations/abc1234/verify.md
   ```

   Run it as its own foreground command. It refuses a dirty tree, a
   non-commit `HEAD` and an attestation inside the worktree, and prints one
   JSON object. The `PostToolUse` hook binds that output to the session and
   to the person's current prompt. A registration run chained, piped or in
   the background is not bound; the journal says why.
3. **Ask, in the same turn.** One `AskUserQuestion` whose text carries
   `[preview <id>]` and lists every `repo@sha` it approves, with options
   exactly `Approve`, `Request changes`, `Hold` — no "(Recommended)"
   suffix.
4. **Push after `Approve`.** The approval covers that commit; a new commit,
   amend or rebase needs a new preview.

## What approves

| the person… | result |
|---|---|
| picks `Approve` on the marked question | the listed commits are approved |
| picks `Request changes` or `Hold` | the previews are dropped |
| does not answer (a timeout) | still pending |
| types `approve`, `ship`, `lgtm` or `looks good` as the next prompt, after a marked question was asked | approved |
| types anything else next — including `yes`, `go`, `ok`, `continue` | the previews lapse |

Unmarked questions are ignored, so a release question answered "Approve"
approves no preview, even in the same turn. Another session's answers never
touch this session's previews. Registrations and approvals lapse after a day,
checked whenever they are read.

`AskUserQuestion` accepts `answers` in its input, so a model could pre-answer
its own question. The `PreToolUse` hook records, per `tool_use_id`, whether a
marked question arrived pre-answered; such a question never marks its
previews as asked (so no answer to it can approve), the `PostToolUse` side
checks the same record again, and under `deny` the question is refused before
it runs.

## When the rule speaks

`push-preview` fires on a push only when all of these hold:

- the repository has a user interface: a `dev` script in `package.json` at
  the root or in `web/`, or `git config amont.agent.push-preview.ui true`
  (`false` opts a repository out);
- a pushed branch carries a file under `app/`, `src/` or `web/`, or a
  `.tsx`, `.jsx`, `.css` or `.html` file;
- that commit has no approval.

A push to `main` or `master` is left to amont's `branch-protect`.

### Supported push shapes

v1 reads an explicit remote plus explicit refspecs — `git push -u origin
feat/x`, `git push origin HEAD:feat/x`, several refspecs — with `-C <dir>`
and a leading `cd` honoured. A destination under `refs/tags/` is excluded;
one under `refs/heads/` is judged, including a tag's commit pushed onto a
branch.

Everything else is **unresolvable** and journalled with its shape: a bare
`git push`, a remote with no refspec (git's `push.default` would decide),
`--all`, `--mirror`, `--tags`, deletes, glob refspecs, a URL as the remote,
a word from a substitution, an unknown flag. Under `advise` an unresolvable
push passes with a journal note; under `deny` a push to a UI repository is
held — `--all` is not the way around the gate. The shapes the journal
collects decide what the next version reads.

## What `push-published` records

It never speaks. Before a push to a UI repository the hook remembers where
each branch destination stood (keyed by `tool_use_id`); after it, one
`git ls-remote` per destination, and one journal line:

| outcome | meaning |
|---|---|
| `published-approved` | the remote now holds an approved UI commit it did not hold before |
| `published-unapproved` | the same, without an approval — the gap `advise` leaves |
| `published-no-ui` | published, nothing a person could preview |
| `already-present` | the remote held the commit before the push |
| `unverified` | the remote does not hold it, or could not be asked |

## What this is not

A registration is the agent's attestation of worktree, commit and URL.
Nothing here proves the browser served that commit or that the person
looked. It covers pushes made through Claude Code, not pushes typed in a
shell, and it is a workflow aid, not a Git enforcement boundary.

## Reading the soak

```sh
grep -E 'push-(preview|published)' ~/.claude/amont-agent/journal.log
```

The rule's own lines (`registered`, `replaced`, `approved`, `dropped`,
`unanswered`, `expired`, `prefilled`, `unbound`) carry the answer latency on
approvals and drops (`option,4s`). A median under about ten seconds after a
week says the approval is a rubber stamp; ADR-0023 then retires the gate and
keeps the verification.
