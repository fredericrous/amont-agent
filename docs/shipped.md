# shipped

`amont-agent shipped <plan.md> [--json]` says what shipped under a plan. It
is read-only and derives everything from git (decisions ADR-0036), plus `gh`
for the head of a GitHub squash.

## What it reads

For each repository in the plan's front matter `repos:` list (resolved as
`plan-panel` resolves them: git config `amont.agent.plan-review.<name>.path`,
else `~/Developer/Perso/<name>`), it looks for `docs/plans/<the plan's file
name>` on `origin/main` (the plan or a pointer file). A repository without it
is reported as `no plan file`.

It fetches `refs/notes/amont-provenance` and `refs/notes/amont-attest` from
origin into local refs of the same name. A failed fetch is said once and the
command goes on with the notes it has. Nothing else is written.

For each first-parent commit on `origin/main` that touched the file:

- **PR**: a trailing `(#n)` in the subject.
- **head**: the second parent of a merge commit; for a squash, `gh pr view <n>
  --json headRefOid` when `gh` is available and origin is github.com, else
  unknown.
- **release**: the first `v*` tag by version that contains the commit, else
  `not released`.

## Lookup order for the provenance record

The record is an `amont-provenance-v1` payload followed by an armored SSH
signature; one note may hold several blocks. The lookup tries, in order:

1. `refs/notes/amont-provenance` on the head commit;
2. the same ref on the merge commit's tree;
3. every note in that ref, read in one `git cat-file --batch` pass, for a
   block whose `patch` equals the merge commit's own patch id
   (`git diff --binary <first parent> <merge> -- . ':!docs/plans' | git
   patch-id --verbatim`).

A block whose `commit` is not reachable from an origin ref is ignored.

## Statuses

- `verified`: the signature verifies, with `ssh-keygen -Y verify -n
  amont-provenance`, for a principal of the committed
  `.github/allowed_signers` (or `.forgejo/allowed_signers`).
- `signed, signer not allowed`: no listed principal verifies it.
- `no provenance`: nothing found.

Flags on a record: `plan line does not match tree` (its `plan` body sha
differs from the plan's body sha in the record's `tree`) and `reviewed an
earlier body` (it differs from the body sha on `origin/main`).

The `gates` line of the first block in `refs/notes/amont-attest` on the same
object is shown as context, unverified.

## Output

One line per merge commit:

```
amont  #338  9f1c2ab  v1.55.2  review reviewed · preview not-required · plan 031240153fa4 · gates cargo-test clippy · verified
```

`--json` prints one array of `{repo, pr, merge, head, release, provenance:
{status, review, preview, plan_body_sha, flags} | null, gates}` and nothing
else on stdout. Exit 0 on success, 2 on a usage error (a missing or unknown
argument, an unreadable plan), 1 when no repository could be read. Payload
files older than 30 days under `<config dir>/amont-agent/provenance/<repo>/`
are noted on stderr and never deleted.

## Budget

One `gh pr view` per GitHub squash PR, and none for a merge commit or a
non-GitHub origin. `AMONT_AGENT_GH` names another program in place of `gh`.
