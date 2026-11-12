# shipped

`amont-agent shipped <plan.md> [--json]` says what shipped under a plan. It
is read-only and derives everything from git (decisions ADR-0036), plus `gh`
for the head of a GitHub squash.

## What it reads

For each repository in the plan's front matter `repos:` list (resolved as
`plan-panel` resolves them: git config `amont.agent.plan-review.<name>.path`,
else `<plan root>/<name>`, the plan root resolving exactly as plan review's
does, working-directory fallback included: see
[plan-review.md](plan-review.md)), it looks for `docs/plans/<the plan's file
name>` on `origin/main` (the plan or a pointer file). A repository without it
is reported as `no plan file`.

It fetches the provenance and attest notes refs of both name sets
(`refs/notes/greengate-{provenance,attest}`, `refs/notes/amont-{provenance,attest}`;
one `git ls-remote`, then one fetch of those origin lists) into local refs of
the same name. A ref origin does not list is skipped silently; an
`ls-remote` or fetch that fails is said on stderr, naming the refs, and the
command goes on with the notes it has. Gate stamps are never fetched. Nothing
else is written.

For each first-parent commit on `origin/main` that touched the file:

- **PR**: the last `(#n)` anywhere in the subject (GitHub's `... (#n)` and Forgejo's `Merge pull request '...' (#n) from ... into main`).
- **head**: the second parent of a merge commit; for a squash, `gh pr view <n>
  --json headRefOid` when `gh` is available and origin is github.com, else
  unknown.
- **release**: the first `v*` tag by version that contains the commit, else
  `not released`.

## Lookup order for the provenance record

The record is a `greengate-provenance-v1` (or legacy `amont-provenance-v1`)
payload followed by an armored SSH signature; one note may hold several blocks.
A block counts only under the header of its own name set, and its signature is
checked under that set's namespace. The lookup tries, in order:

1. the PR head commit, `refs/notes/greengate-provenance` first, then
   `refs/notes/amont-provenance`;
2. the same two refs on the merge commit's tree;
3. every note in those refs, read in one `git cat-file --batch` pass, for a
   block whose `patch` equals the merge commit's own patch id
   (the writer's own function, `implementation_review::branch_patch`, with the
   merge's first parent as base).

The object order wins: a legacy note on the head beats a green note on the
tree. Per object, the green ref comes first.

A block whose `commit` is neither reachable from an origin ref nor the merge's
PR head (its second parent, or `gh`'s `headRefOid`) is ignored; the PR head is
accepted even when its branch is gone from origin.

The plan a record binds is the changed `docs/plans/*.md` that is not a
`.reviews.md` sidecar and, among several, not a pointer (front matter
`canonical:`). In a repository holding only a pointer file, the record's `plan`
line binds the pointer's body-sha.

## Statuses

- `verified`: the signature verifies, with `ssh-keygen -Y verify -n` under the
  namespace of the record's name set (`greengate-provenance` or
  `amont-provenance`), for a principal of the committed
  `.github/allowed_signers` (or `.forgejo/allowed_signers`). The human row
  says `verified under <namespace>`; the JSON `status` stays `verified`.
- `signed, signer not allowed`: the signature is structurally valid
  (`ssh-keygen -Y check-novalidate`) but no listed principal verifies it.
- `signed, no allowed_signers`: valid signature, no allowed_signers file.
- `unsigned`: the block carries no signature.
- `bad signature`: the signature does not hold for the payload (tampered).
- `signature not checked`: `ssh-keygen` could not be run, or no scratch
  file could be written; nothing is said about the record.
- `no provenance`: nothing found.

Flags on a record: `plan line does not match tree` (its `plan` body sha
differs from the plan's body sha in the record's `tree`) and `reviewed an
earlier body` (it differs from the body sha on `origin/main`).

The `gates` line of the first block in `refs/notes/greengate-attest`, else
`refs/notes/amont-attest`, on the same object is shown as context, unverified.

## Output

One line per merge commit:

```
amont  #338  9f1c2ab  v1.55.2  review reviewed · preview not-required · plan 031240153fa4 · gates cargo-test clippy · verified under amont-provenance
```

`--json` prints one array of `{repo, pr, merge, head, release, provenance:
{status, review, preview, plan_body_sha, flags} | null, gates}` and nothing
else on stdout. Exit 0 on success, 2 on a usage error (a missing or unknown
argument, an unreadable plan), 1 when no repository could be read or the
answer could not be written (a reader that closed the pipe is not an error). Payload
files older than 30 days under `<config dir>/amont-agent/provenance/<repo>/`
are noted on stderr and never deleted.

## Budget

One `gh pr view` per GitHub squash PR, and none for a merge commit or a
non-GitHub origin. `AMONT_AGENT_GH` names another program in place of `gh`.
