---
status: active
branch: feat/block-copy-recognition
repos: [amont-agent, greenharness-skills, grh]
adrs: [ADR-0067]
---
# amont-agent recognises a copy of a reviewed block

## Review panel
📄 Full reviews: [2026-11-09-amont-agent-recognises-a-copy.reviews.md](2026-11-09-amont-agent-recognises-a-copy.reviews.md)
👉 **Decide:** `block reinstate`, a way to undo a revocation, goes beyond ADR-0067 (the po's note). I recommend dropping it: a revocation stays final, a corrected block has a new fingerprint, and a mistaken revocation only costs full reviews for that block; dropping removes one verb, one guard rule and one known limit. The plan as written keeps it; say so and I apply a one-seat delta.
📍 amont-agent, greenharness-skills, grh · planned, nothing built · next: Phase 1, the fingerprint and the `block` subcommand. Panel: architect, backend, rust, po, tui, unix (all opus), codex.
**Changed by review:** registration moves into `hold` and needs each profile's explicit approve; revocation is per fingerprint, undone only by `block reinstate`, which a guard refuses to an agent; the pin check leaves a latest-check record the hook requires.
**Verdicts:** architect and unix approve, backend, rust, po and tui approve-with-changes on the final body, after two rounds and two deltas; Codex's four highs applied; no blocker left; about 30 medium or low findings carried into implementation.

## Picture
```
 branch ──► block fingerprint ──► registry (by-block/ in the hook's store)
   │          sha256 of the +/- lines      ▲ written only by the hook, after
   │                                       │ every profile approved the tree
   ├─ plan-panel --code-review -C/--base ──► "none — copy of fp:<12 hex>"
   ├─ review-brief ──► pin check (gh release view at the step's pin)
   └─ push hook ──► recompute the fingerprint
                      hit : implementation-review alone
                      miss: the full declared profiles, no question asked
```

The picture shows where a copy is recognised: the fingerprint of a branch feeds
the proposal and the brief, and the push hook alone registers a block and
rechecks the match, so a stated copy is verified and never trusted.

## Context

ADR-0067 (`work.reviewed-block-copy`, decisions#183), the person's choices of
2026-11-09, programme `proportional-ship` tickets t13 to t16. On 2026-11-08 a
fleet change copied two reviewed CI blocks into 13 repositories; they got
implementation-review alone because the agent noticed the copies, diffed them by
hand and the person approved twice. The ADR makes the harness recognise a copy.
Facts from t14 and t15, read at amont-agent `1724ac7` (3.27.0):

- A `none — <reason>` line already parses (`code_review::parse_line`) and only
  drops the profile seats; implementation-review stays required for every push.
  So a `copy of` line parses on any version and an old hook trusts it unverified.
- `declaration_of` only parses plans. `hold` returns early when the declared
  profile list is empty (`:1495`), which is where a `none` line lands. And
  `ProfileGate::standing` judges one profile per call (`hold` loops over them),
  so it cannot know that every profile approved a tree.
- `plan-panel --code-review` reads the plan only, never the branch.
- The hook's pass store is `journal::dir()/implementation-review`
  (`implementation_review.rs:478`); the write guard already covers any new
  directory under it. `atomic::write_atomic` swaps a whole file in one rename and
  takes no lock, so a read-modify-write loses concurrent updates.
- `amont_agent_guard::git::bytes_in` takes no environment and discards stderr,
  and production `git diff` calls do not isolate the user's git config; the
  fingerprint must, or it differs between machines without an error.
- Nothing calls `gh` yet. Tools are called through `run_bounded` with an
  `AMONT_AGENT_<TOOL>` override and degrade to a notice.
- The skills repo's `scripts/check.sh` forbids `ADR-NNNN`, `work.<key>`,
  `/Users/` and `fredericrous/decisions` inside `parts/`, so skill text says
  what the rule does and never cites the ADR.

## Phases

Order matters. The hook (phase 2) ships before the proposal (phase 3), so a
`copy of` line is never produced where nothing verifies it, and the hook treats a
`copy of` line as a miss whenever it does not enforce (advise stance or off);
`plan-panel` never prints one in that case either. Phase 5 merges only after the
person has installed 3.28.0 and `amont-agent --version` shows it: a 3.27.0 hook
trusts a `copy of` line as free-text `none`. One amont-agent pull request
carries phases 1 to 4 as separate commits. amont-agent is still named
amont-agent in code; the rename to greenpiste is the gate-guardrails map's later
step, and the registry sits under `journal::dir()` so it moves with it.

### Phase 1: the fingerprint
- `crates/amont-agent-guard/src/git.rs`: a new `bytes_in_env` (as `bytes_in`, plus
  environment pairs, and stderr captured for the error text).
- `crates/amont-agent-process/src/block_fingerprint.rs` (new), declared in
  `crates/amont-agent-process/src/lib.rs`:
  - `fingerprint(repo, merge_base, head) -> Result<Option<Fingerprint>, FpError>`.
    `Ok(None)` means a binary file in the diff and nothing else; a git failure is
    `Err`, which the hook journals and treats as a miss. It runs `git diff
    --binary --no-ext-diff --no-textconv --no-color --no-renames
    --no-indent-heuristic --no-relative --src-prefix=a/ --dst-prefix=b/ -U0
    --diff-algorithm=myers <merge-base> <head> -- ':(top)' ':(top,exclude)docs/plans'`
    through `bytes_in_env` with `GIT_CONFIG_GLOBAL=/dev/null`,
    `GIT_CONFIG_NOSYSTEM=1`, `LC_ALL=C` and `GIT_CONFIG_COUNT` pairs, so neither
    the repository's `.git/config` nor a subdirectory `-C` changes the text.
  - `fingerprint_of_diff(&[u8]) -> Option<Fingerprint>` is the pure part, on
    bytes with no lossy decode. A state machine over `diff --git`, the headers up
    to the first `@@`, and the hunk lengths of `@@ -a,b +c,d`, so a removed line
    `-- x` is a content line and not a header. It keeps the `+`/`-` lines with
    their sign, in order, including empty added or deleted lines (a blank line
    keeps its sign and can matter inside a multiline string); drops paths,
    headers, `@@` lines, context and `\ No newline` lines; removes the block's common leading
    indentation (the byte-wise minimum over non-empty lines after the sign; a tab
    is not a space); strips CR and trailing ASCII space and tab; and takes the
    sha256 of the lines joined by `\n`. The block id shown is `fp:` plus the
    first 12 hex characters. The model for the `-U0` walk is `notes_patch`
    (`implementation_review.rs:251-265`).
- `src/main.rs`: a `block` subcommand with verbs `sha [-C <dir>] [--base <ref>]`
  (stdout `<64 hex>\tfp:<12 hex>`), `list`, `revoke` and `reinstate` (phase 2). `Sub` enum,
  `SUBCOMMANDS` (31 becomes 32), the `USAGE` entry and a `BLOCK_USAGE` const, in
  the style of `run_tree_sha`, flags with values as `--base <ref>` (the default is
  the merge base with `origin/HEAD`). Exits: 0 ok; 1 no fingerprint (a binary diff
  prints nothing on stdout and `amont-agent: block: the diff has a binary file
  (<path>); a binary change is never a copy` on stderr; git failed), unknown or
  ambiguous fp, store unwritable; 2 usage; notices on stderr only.
- `tests/block.rs` (new) with fixtures under `tests/fixtures/block/`: the diffs
  of adr-lint#53 (`1499a39`), two of its copies (attest, decisions, which has
  the step under another name), kb-vision#59 (`69c7642`) and one of its copies
  (sre-agent), and three new shapes (amont, duro-operator, customer-vision).
  Expected: copies match their pilot and each other; no new shape matches. The
  near misses of t14: a pin changed as context, CRLF and another indent of the
  whole block match; an extra line (a blank added or deleted line too), a
  swapped line, a changed comment, a pin
  changed inside the change and one line moved to another indent miss. Also: a
  non-UTF-8 byte difference changes the hash; a content line `--- x` and a
  missing newline at end of file are handled; the same fingerprint with
  `diff.algorithm`, `diff.noprefix`, `diff.indentHeuristic`, `diff.relative` and
  `core.quotepath` set in the repository's config or in `~/.gitconfig`, and with
  `-C` pointing into a subdirectory.

### Phase 2: the registry and the hook
- `crates/amont-agent-process/src/implementation_review.rs`:
  - The registry, under `store()` beside `by-tree/` (`pass_file`, `:488`), is
    made of files that are written once, so concurrent hooks lose no write.
    Four kinds, all mode 0600, under `by-block/<fp>/`:
    - `<rules12>-<agents12>/entry.json`, one generation per rules sha
      `<<<RULES sha>>>` and agent digest (prefixes), so a rules or agent change
      lets a new pilot register again: the fingerprint, the profile roles that
      approved, the full rules sha, a digest of the profile agent files read
      through `crates/amont-agent-process/src/agents.rs`, the source branch and
      content id. Registering over an existing generation (`AlreadyExists`) is
      success.
    - `copies/<sha256(repo)[..16]>-<commit>.json`, one per recorded copy (the
      repository name inside the JSON).
    - `revoked`, at the fingerprint level and not per generation: it holds the
      reason, and a revoked fingerprint never matches, whatever generation exists
      or is created later, until the person runs `block reinstate`. A generation
      that a concurrent registration creates after a revoke is ignored.
    - `pins/<content_id>.json`, the latest pin check (phase 3), keyed by the
      hook's own `content_id` (`implementation_review.rs:1191`) and replaced
      atomically with `write_atomic`: the last check wins, so a later failure
      overwrites an earlier pass; it also notes whether `AMONT_AGENT_GH` was set,
      and the hook refuses such a record unless its own environment sets the
      override too (a real hook does not).
    The immutable kinds are staged as `.tmp-<pid>-<name>` (a leftover of that name
    removed first, opened with `create_new`, fsynced) and then
    `std::fs::hard_link`ed to their name, which fails atomically on an existing
    name; the staged name is removed after the link whatever its result, readers
    skip `.tmp-*`, and an unparseable file reads as absent. An entry whose stored
    fingerprint differs from its directory name is ignored, and only names of
    exactly 64 hex characters are read.
  - Registration happens in `hold` (`:1466`), after the per-profile loop
    (`:1519`), with its own eligibility check over the latest bound transcript
    review of every required profile: each must say `Verdict::Approve` for this
    exact content id. `Standing::Passed` is not enough, because it also covers
    approve-with-changes and remembered passes; the transcript evidence is read
    even when `standing` returned a cached pass, so approvals collected across
    several hook attempts still register, once. The declaration must be the full
    `code_roles` set for the repository's file classes. Never inside `standing`,
    which sees one profile; never for a pass carried by patch or by an earlier
    tree, an overrule, or a plan that narrowed its profiles with a reason; never
    for a revoked fingerprint.
  - `declaration_of` (`:1128`) only records a `Declared::NoReview` reason that
    starts `copy of fp:`. `hold` verifies it before the `profiles.is_empty()`
    early return (`:1495`): it recomputes the fingerprint (the line's 12-hex
    prefix must equal it) and requires the fingerprint not to be revoked, the
    generation for today's rules sha and agent digest to exist and cover every
    role the repository's file classes would need, and the latest
    `pins/<content_id>.json` for the pushed tree to be a pass. When no
    generation matches today's rules or agents but an older one exists, the cause
    is `rules changed since registration` or `agent <a> changed since
    registration`; with none at all, `no entry`. A hit requires no profiles,
    journals `profiles-copy-matched` and writes the copy file. Anything else is a miss: the declared profiles become the full
    `code_roles` set, journalled `profiles-copy-missed`, and `hold` returns
    `profile-missing` naming the agents and the cause (`no entry for fp:…`,
    `revoked: <because>`, `role <r> not covered`, `rules changed since
    registration`, `agent <a> changed since registration`, `no passed pin check; run amont-agent review-brief -C <dir>`,
    `binary diff`, `git failed`). No question to the person. A free-text
    `none — …` reason is untouched, `plan_only` still short-circuits first
    (`:1471`), and the digest still hashes the sentence.
- `src/main.rs`, `block list [--json]` (one TSV row per entry, no header:
  `fp:<12 hex>\tactive|revoked\t<roles, comma-separated>\t<copy count>\t<source
  repo>`; `--json` carries the full copies lists) and `block revoke <fp> --because
  <text>` (`<fp>` is `fp:<12 hex>`, a unique prefix, or the 64 hex, validated
  before it joins a path; writes the fingerprint's `revoked` file, staged and
  hard-linked like every registry file, so a second revoke exits 0 and prints the
  copy list again, which catches a copy written during the first; prints every
  recorded copy, pushed and not necessarily merged, one `<repo>\t<branch>\t<commit>`
  line each on stdout, for a re-check; `block reinstate <fp>` removes the
  `revoked` file and only the person runs it, in their own shell: a new guard rule
  under `crates/amont-agent-guard/src/rules/` refuses an agent's Bash call that
  runs `amont-agent block reinstate`, since `HOOK_ONLY_DIRS` (`:2142`) guards
  file writes and not commands, with a test in `tests/implementation_review.rs`;
  an unknown or ambiguous fp exits 1 with `amont-agent: block: no entry fp:<id>;
  see amont-agent block list`). The CLI may write the store; an agent's own Bash
  write is still refused by the existing guard (`HOOK_ONLY_DIRS`, `:2142`).
- `tests/implementation_review.rs` in the style of `World` (`:68`) and the
  `none —` case at `:2877-2894`: a pilot approved by all profiles registers only
  after the last approve (one hook run before it, one after); a copy with a pin
  record and no profiles passes; a copy after a rules change, an agent change, a
  revocation, or with no pin record is a miss with its named cause; under the
  advise stance a `copy of` line is a miss; a registration is refused after an
  overrule, a narrowed plan, a carried pass, an approve-with-changes and a
  missing profile; approvals arriving across successive hook attempts register
  once; a pass-then-failure of the pin check at the same tree is a miss; a
  generation created after a revoke is ignored, and `block reinstate` makes the
  block match again; 16 parallel hook runs lose no copy file and a revoke in the
  middle stays revoked; the
  existing carry tests (`:3457-3830`) and `only_the_hook_writes_a_pass_file`
  (`:954`) pass unchanged.

### Phase 3: the proposal and the pin check
- `crates/amont-agent-process/src/code_review.rs`: `NoReviewReason::CopyOf`
  (`:190`, `Display` at `:202`) printing `copy of fp:<12 hex>`.
- `src/main.rs`, `run_plan_panel` (`:1871`): optional `-C <dir>` and `--base
  <ref>` (the base defaults to the merge base with `origin/HEAD`), parsed as flags
  with values in the style of `run_tree_sha`; either flag without `--code-review`
  is a usage error (exit 2), and the usage text says so. Given the branch, the
  proposal computes the fingerprint, reads the registry, and prints
  `<repo>\tnone\tcopy of fp:…` for a registered, unrevoked block whose pin check
  passed and only while the implementation-review hook enforces; anything else
  prints the profile proposal as today. `plan-panel` runs the pin check itself,
  with the same function as `review-brief` but without writing a record (the
  record belongs to a pushed tree, and writing it is `review-brief`'s job); when
  the hook does not enforce it prints one stderr line `amont-agent: plan-panel:
  the implementation-review hook does not enforce; the profiles are proposed`. When a registered block failed its pin
  check, one stderr line `amont-agent: plan-panel: fp:<id> is a registered copy
  but the pin check failed: <why>; the profiles are proposed` (with `gh` absent,
  `<why>` is `gh not found; install it or set AMONT_AGENT_GH`). Without the branch
  the output does not change.
- `crates/amont-agent-process/src/review_brief.rs`, `build` (`:449`): the pin
  check, for a branch whose fingerprint is registered. The block carries one
  marker line, `# release-pin: <owner/repo> <tag-template> <asset-template>
  <ENV_KEY>`, in the added lines (the person's choice of 2026-11-09). The check
  reads the pin `<ENV_KEY>` from the same file at HEAD, expands the two templates,
  and runs `gh release view <tag> --repo <owner/repo> --json assets` through
  `run_bounded` (`:183`) with a 20 s deadline and an `AMONT_AGENT_GH` override.
  The asset must be in the list. The result is a "Pin check" section of the brief
  and a notice on stderr. It writes `by-block/<fp>/pins/<content_id>.json` for
  every check, a pass or a failure, replacing the previous one atomically (the
  last check wins), using the same path function as the hook so writer and reader
  cannot differ. A missing `gh`, a failed call, a timeout, a missing marker or a
  missing asset all write a failure, so the match does not count and the profiles
  are required, even when an earlier check of the same tree passed. The hook
  itself does not call the network.
- `tests/plan_review.rs` (the `--code-review` cases from `:1308`) and
  `tests/review_brief.rs` (fakes by env, `fake_adr_lint` at `:301`): a fake `gh`
  that has the asset, lacks it, fails, times out and is absent; `plan-panel`
  prints no `copy of` line while the hook does not enforce; `-C` without
  `--code-review` exits 2; the help text of `block` and `plan-panel` stays within
  80 columns.

### Phase 4: ledger, docs, changelog
- `crates/amont-agent-process/src/impl_review_outcomes.rs` and
  `crates/amont-agent-process/src/review_outcomes.rs`: tag a review as copy or
  not, set when the hook journalled `profiles-copy-matched`, and
  `plan-review stats` counts copies and defects found in copies, so the rule can
  be revisited at the first code defect found in a copy or at 30 registered
  copies. Read both files first; `tests/outcomes_stats.rs` and
  `tests/review_outcomes.rs` carry the style.
- `docs/implementation-review.md` (profiles section, `:395-415`), the journal
  outcome names (`profiles-copy-matched`, `profiles-copy-missed`), `docs/plan-review.md`
  (`--code-review`, `:405-421`), `README.md` (optional tools: `gh` joins the list)
  and `CHANGELOG.md` (an `## Unreleased` entry).
- Release 3.28.0 through the `tag-release` skill: a `chore(release)` pull
  request that edits `CHANGELOG.md`, `Cargo.toml` and `Cargo.lock`, then the
  tag. Installing the new binary on the laptop is the person's, as for earlier
  releases.

### Phase 5: the skills and the pin
Merges only after the person has installed 3.28.0; the text applies only when
`amont-agent --version` is 3.28.0 or later, and on an older binary the agent
declares the profiles.
- `parts/workflow/skills/plan-review/SKILL.md` (`:82-105`): the proposal also
  prints `copy of fp:<id>` when given the branch with `-C` and `--base`; the hook
  rechecks it; a miss means the declared profiles, with its named cause and no
  question.
- `parts/workflow/skills/worktree-task/SKILL.md` (`:124-126`, `:217-232`,
  `:260-262`, `:278-280`) and `parts/workflow/skills/worktree-task/plan-template.md`
  (`:28-33`, `:102-114`): the pointer line is copied verbatim and the hook
  rechecks it; the brief carries the pin-check result; a pilot's block carries
  the `# release-pin:` marker line.
- `parts/workflow/agents/implementation-review.md` and
  `parts/workflow/agents/plan-review-backend.md` (`:69-73`): a `copy of` line is
  the hook's to verify, not a reason the reviewer judges; a failed or missing pin
  check in the brief is a blocking finding for implementation-review.
- No `ADR-NNNN`, `work.<key>`, `/Users/` or `fredericrous/decisions` in any file
  under `parts/` (`scripts/check.sh`). One pull request in the skills
  repository, then a `v*` tag, then, once the laptop runs amont-agent 3.28.0, a
  grh pull request that bumps `catalog.toml` (`skills_version`,
  `skills_sha256`) with a `CHANGELOG.md` entry.

## Verification

- Phase 1, input: the fixtures above through `amont-agent block sha` and the
  library, expected: copies of the adr-lint pilot share one fingerprint, copies
  of the kb-vision pilot share another, no new shape matches either, and the
  near-miss table holds; the same diff with each hostile git setting, and from a
  subdirectory, gives the same fingerprint; a diff with a binary file prints
  nothing and exits 1.
- Phase 2, input: a throwaway repository driven by the hook tests, expected:
  registration, hit, every named miss cause, the advise-stance miss, revocation,
  refusal cases and the 16-run race as listed; then the whole `cargo test` once on
  the final tree.
- Phase 3, input: `plan-panel --code-review -C <dir> --base <ref>` on a branch
  that is a registered copy with a fake `gh` that has the asset, expected
  `<repo>\tnone\tcopy of fp:…` and a pin record; with the asset missing, `gh`
  absent or timed out, the profiles and the stderr line.
- Phase 4, input: a journal with one `profiles-copy-matched` push and one ordinary
  push, expected: `plan-review stats` shows 1 copy, 0 defects in copies, and the
  ordinary review untagged.
- A real run, once, on the final binary: build a branch in a scratch clone from
  the merged adr-lint#53 diff, with the `# release-pin:` marker line added, at a
  different pin; register the pilot through a hook run with every profile
  approved, then push the copy and read the journal for `profiles-copy-matched`
  and the brief's pin-check section.
- Phase 5, `scripts/check.sh` exits 0 and the skills CI passes on Ubuntu and
  macOS; `grh install workflow` on the pinned catalog shows the new text.

## Measures after release

- Over 14 days from installation: the `profiles-copy-matched` and
  `profiles-copy-missed` counts in the journal; median review tokens of reviews
  tagged copy against pilots in the ledger (target: about 100k less per copy, an
  estimate to confirm against the ledger's profile-review rows); pin records made
  through the `AMONT_AGENT_GH` override and refused by the hook outside tests
  (target: 0).
- Rules churn: the number of times the RULES sha changed in the 30 days before
  installation, recorded in the pull request; a weekly change would empty the
  registry each week and say the rules sha binding is too coarse.
- Roll back (revoke every entry, release a fix): any code defect found in a copy,
  or any copy-matched push whose brief shows a failed pin check.

## Non-goals

- Seeding the registry with adr-lint#53 and kb-vision#59. Only the hook
  registers, so the registry starts empty and the first pilot after this ships
  registers itself. Their blocks also lack the marker line.
- A shared CI template that would remove the copies, and a lint that refuses the
  web route (the programme's out-of-scope entries).
- The greenpiste rename. Nothing here depends on the binary's name.
- Running the pin check in the hook, or in each repository's CI.
- Installing the released amont-agent and reinstalling the relais plugin on the
  laptop.
- A known limit, narrowed but not closed: `AMONT_AGENT_GH`, like the other tool
  overrides, is a test seam, and an agent that sets it to a fake could make a
  pin check pass. The pin record notes the override and the hook refuses such a
  record outside tests, which closes the environment route; a pin record
  written by hand through the CLI is still evidence for the reviewer, not a
  proof against a deliberate bypass.

## Code review
- amont-agent: backend, lang:rust, unix, tui (the new `block` subcommand's TSV and JSON output, exit codes and usage text, and the proposal line, are user-facing output; architect left out: ADR-0067 already decided the structure)
- greenharness-skills: none — markdown skill and agent text only, checked by `scripts/check.sh`
- grh: none — a pin bump in `catalog.toml` and a changelog line

<!-- panel: repos=amont-agent,greenharness-skills,grh adds=contract reviewers=architect,backend,lang:rust,po,tui,unix,codex body-sha=dfc422a7a1f7 -->
