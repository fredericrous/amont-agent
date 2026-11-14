---
status: active
branch: feat/greenpiste-ships
repos: [amont-agent, grh, greenharness-skills, decisions]
adrs: [ADR-0064, ADR-0068]
---
# amont-agent ships as greenpiste

## Review panel

👉 **Decide:** none — approve if greenpiste 4.0.0 ships under its own name, with `amont-agent` as an alias copy (installers, brew, and grh through a declared `bin_aliases`) until the census.
📍 amont-agent, grh, greenharness-skills, decisions · nothing built · next: Phase 1 (the names module). Panel: backend, lang:python, lang:rust, platform (opus-5-5), codex.
**Changed by review:** rules that match a command accept both names, so a Deny rule cannot be skipped through the alias. grh migrates hooks and receipts on an upgrade and places the alias, declared in the manifest. The release builds both archive sets, runs a dry run, a standalone `install.sh` check and a brew upgrade through the rename.
📄 Full reviews: [2026-11-14-amont-agent-ships-as-greenpiste.reviews.md](2026-11-14-amont-agent-ships-as-greenpiste.reviews.md)
**Verdicts:** round 1 approve-with-changes 4; round 2 approve 2, approve-with-changes 2; Codex approve-with-changes (3 high, applied); final binds approve 4; 0 rework; 3 rejected (duplicates or a note); 7 carried into implementation.

## Picture

```
 today (3.34.x)                     after this plan (4.0.0)
 --------------                     -----------------------
 crates  amont-agent                greenpiste
         amont-agent-guard          greenpiste-guard
         amont-agent-process        greenpiste-process
                                    (old crates frozen at 3.34.x)
 binary  amont-agent                greenpiste
                                    amont-agent: an alias (a copy)
 brew    fredericrous/tap/          .../greenpiste; amont-agent renamed
         amont-agent                to it (formula_renames.json)
 assets  amont-agent-<v>-<t>        greenpiste-<v>-<t>, the old name too
                                    until the census, greenharness.toml
 hooks   settings.json runs         `greenpiste install --write` rewrites
         /usr/local/bin/amont-agent the 15 entries; the old ones still
                                    run through the alias meanwhile
 says    "amont-agent/<rule>: …"    "greenpiste/<rule>: …" (names module)
 grh     bin = "amont-agent" 3.23.1 bin = "greenpiste" 4.0.0, pinned
 reads   AMONT_AGENT_*, amont.agent.*, ~/.claude/amont-agent/
                                    unchanged here: plan 3b-2
```

The tool is installed, published and spoken of as greenpiste; every name it
reads from disk, git config or the environment stays until plan 3b-2, and the
hook entries already written keep working through the alias.

## Context

Programme gate-guardrails (`decisions/docs/programmes/gate-guardrails`),
build step 3b. greengate went through the same step as plan 3a-1
(`amont/docs/plans/2026-11-09-amont-ships-as-greengate.md`), then 3a-2 (data
names) and 3a-3 (signed notes). The person's end state (2026-11-12): "no
amont, only green*". greenpiste's step splits the same way: this plan,
3b-1, renames what is installed, published and printed; 3b-2 renames what
it reads and writes (env `AMONT_AGENT_*`, git config `amont.agent.*`,
`~/.claude/amont-agent/`, the review-outcome schema strings relais reads,
the provenance directory greengate reads).

An inventory (2026-11-14) found: three crates, all published; no names
module (about 61 source files print the name, `settings.rs:51` holds
`BIN`); hook entries matched by command basename (`settings.rs:184-190`
`is_ours`); 15 hook entries on this machine baked to
`/usr/local/bin/amont-agent`; grh's catalog at `bin = "amont-agent"`
3.23.1, and grh fetching `{bin}-{version}-{target}.tar.gz`
(`grh/src/fetch.rs:62`); relais mapping its `amont_agent` key to the binary
`amont-agent` (`relais/crates/relais/src/tooling.rs:76-80`); no shipped
`greenharness.toml`; crates.io names `greenpiste`, `greenpiste-guard`,
`greenpiste-process` free (HTTP 404).

## Goal

greenpiste 4.0.0 is installed, published and named greenpiste: crates,
binary, archives, Homebrew formula, messages and the shipped manifest;
`amont-agent` keeps working as an alias copy until the programme's census,
and grh pins greenpiste 4.0.0 by its new name.

## Non-goals

- Every name the program reads or writes (env `AMONT_AGENT_*`, git config
  `amont.agent.*`, `~/.claude/amont-agent/` and its files, the
  `amont-agent-*-v1` schema strings, temp-file names, the `amont_agent`
  key in relais's config): plan 3b-2. Messages that name those keys keep
  naming them.
- relais's `default_bin` and greengate's text that names amont-agent: they
  keep working through the alias and move with 3b-2, which touches both
  repositories anyway.
- Rewriting this machine's `~/.claude/settings.json`: the person runs
  `greenpiste install --write` after installing 4.0.0.
- `~/.claude/CLAUDE.md` and the vendored packs in other repositories: the
  person's file, and the fleet sweep (step 4).
- Renaming the local `amont-agent/` checkout: after worktrees drain.


## Code review
- amont-agent: backend, lang:rust, lang:python, platform, unix, tui, architect
- grh: backend, lang:rust, unix (lang:python left out: no Python file; platform left out: no workflow changed; tui left out: one recommend string; architect left out: a catalog entry in the existing format)
- greenharness-skills: none — markdown only
- decisions: none — markdown only

## Behaviour

- **One names module.** `amont-agent-guard` gains `names.rs`: the product
  and binary name (`greenpiste`), the alias (`amont-agent`), the installer
  URL and the formula name. Every message, usage text, error prefix
  (`greenpiste: …`), hook decision prefix (`greenpiste/<rule>: …`,
  `decision.rs:198-200`), `--version` line and command hint is built from
  it. `settings.rs`'s `BIN` and the scattered constants
  (`candidates_block.rs:32`, `shape.rs:84`, `page_open.rs`,
  `block_reinstate.rs`, `test_output_trimmed.rs`, `activity.rs`,
  `quiet.rs`, `preview.rs`, `attestations.rs`) read it. Every rule that
  matches a command's program (`block_reinstate.rs:28`, `page_open.rs:458,502`,
  `preview.rs:1573`, `test_output_trimmed.rs:106`, `shape.rs:84`,
  `quiet.rs:296`) accepts both `greenpiste` and the alias through
  `names::is_ours_program`, so a Deny rule is not bypassed through the alias. A test fails on
  `\bamont-agent\b` anywhere inside a string literal outside `names.rs`
  and an explicit list of the 3b-2 names (`amont-agent/` config paths,
  `amont-agent-*-v1`, `AMONT_AGENT_`, `amont.agent.`, the `"tool":"amont-agent"`
  record field of `quiet.rs:296`, the `amont-agent-shim-` temp names of
  `shim.rs:59,74`); the test scans `tests/` too, and the `amont-agent/grep-shim:`
  message of `shim.rs:31` moves to `names`.
- **Crates.** Published as `greenpiste`, `greenpiste-guard` and
  `greenpiste-process` at 4.0.0, guard then process then the binary crate;
  directories `crates/greenpiste-{guard,process}`, Rust paths
  `greenpiste_guard::…`; `[workspace.dependencies]` takes the new keys at
  `=4.0.0`. The `amont-agent*` crates are not republished nor yanked
  (as for amont); the README says they were renamed. The crates.io token's
  `publish-new` scope and a pattern covering `greenpiste*` are checked
  before the tag.
- **The binary and its alias.** The binary is `greenpiste`. `install.sh`,
  `install.ps1`, `make install` and the formula also place `amont-agent`
  beside it as a copy, written to a temporary name and renamed over the
  target, so the 15 entries baked to `/usr/local/bin/amont-agent` and
  every caller of `amont-agent` (grh's old pin, relais, skills) keep
  working; an alias invocation prints no warning. `cargo install
  greenpiste` places no alias (as for greengate).
  grh's manifest format gains a declared `[tool] bin_aliases` list
  (`grh/src/manifest.rs`, whose `Tool` denies unknown fields, and
  `docs/extension-manifest.md`, with a parse test); greenpiste's
  `greenharness.toml` names `bin_aliases = ["amont-agent"]`, so grh holds no
  tool name in its code (ADR-0064).
  grh's placement path must also create each declared alias from the verified
  greenpiste bytes, using temporary-file-plus-rename placement, and track
  its path and hash in the receipt. Refresh a grh-owned old executable
  into that copy; preserve foreign binaries and report any collision.
  Keep alias ownership through binary migration, and include it in grh's
  uninstall handling after integrations are removed. Test fresh placement,
  upgrade over a recorded old binary, interrupted-install recovery and
  uninstall; both installed names must answer 4.0.0 and compare identical.
- **Hook entries move once.** `install --write` writes entries for the
  running binary's absolute path; `is_ours` recognises both `greenpiste`
  and `amont-agent` basenames (`.exe` trimmed), so `greenpiste install
  --write` replaces the old entries instead of adding a second set, and
  `uninstall --write` removes either. `doctor` reports entries still
  pointing at `amont-agent` as one line naming the fix.
- **Release assets.** `greenpiste-<v>-<target>` archives hold `greenpiste`;
  until the census, each release also builds `amont-agent-<v>-<target>`
  holding a regular file `amont-agent`, because older `install.sh` and
  grh pins build that name. All go in `SHA256SUMS`. The packaged-binary
  smoke test runs both binaries (`.exe` on Windows), `cmp` proves the two
  are identical, and `verify-brew` and the musl download step read
  `greenpiste-` names.
- **The shipped manifest.** `greenharness.toml` at the root (grh's v2
  format, `bin = "greenpiste"`); `scripts/release-manifest.py`, copied from
  greengate's, writes `[tool.release]` sha256s into the uploaded copy; the
  version guard checks `[extension] version` against the tag; a re-run never
  replaces a released manifest: Publish's rerun path drops `gh release upload --clobber` (`release.yaml:303`) for greengate's check-or-upload (`amont/.github/workflows/release.yaml:469-529`).
- **Standalone install check.** A `standalone` job in `release.yaml`
  (listed in Publish's `needs`) runs `install.sh` with a local-archive
  override (`GREENPISTE_ARCHIVE_DIR`, checksum verified) on a clean runner
  with no grh, and a post-publish run of the same script covers the real
  download URL; it runs `greenpiste --version`, `greenpiste rules --json`
  and `amont-agent --version` through the alias.
- **Homebrew, after the assets.** `publish-tap` copies
  `packaging/homebrew/greenpiste.rb` to `Formula/greenpiste.rb`, then runs `bump-tap.py` on it (as greengate does, `amont/.github/workflows/release.yaml:886-898`), adds
  `"amont-agent": "greenpiste"` to `formula_renames.json` and removes
  `Formula/amont-agent.rb` in one tap commit; `verify-brew` installs
  `fredericrous/tap/greenpiste` and checks `greenpiste --version` and `amont-agent --version`; a second step installs `amont-agent` from the tap's parent commit, moves the tap to HEAD, runs `brew upgrade`, and checks both names answer 4.0.0.
  `bump-tap.py` reads `greenpiste-` entries, skips `amont-agent-` ones in a SHA256SUMS holding both (tested), accepts either url prefix in
  a formula, writes `greenpiste-`. `release-manifest.py`'s `REPO` and tarball prefix become `greenharness/greenpiste` and `greenpiste-`.
- **Its own pack and config.** `greengate.pack`'s line becomes
  `greenpiste suppressions --staged`; consumers that vendor it take it at
  the fleet sweep (the alias keeps their copy working). `adr-lint.toml`
  names `--bin greenpiste`.
- **grh.** `catalog.toml`'s greenpiste `[[part]]` block is replaced by one
  `[[pinned]]` entry for the shipped manifest at v4.0.0 (as greengate's),
  the recommend lines moving into `greenharness.toml` and naming greenpiste; `tests/cli.rs` and
  `scripts/e2e.sh` use the new binary name; `install.rs`'s `binary()` treats a
  recorded binary whose file name is not `part.bin()` as stale and fetches
  the new one (a `tests/cli.rs` case seeds a receipt with a placed
  `amont-agent`). When a grh-owned ToolInstall's recorded executable
  differs from the selected binary, refresh that integration even when
  its status is Present, and replace its recorded install and undo
  arguments with the new absolute path after successful verification.
  Leave adopted integrations under their existing ownership policy and
  report the explicit migration command. Test a receipt containing both
  BinaryPlaced and ToolInstall with a complete old-name hook set: upgrade
  must rewrite the hooks and receipt commands, a second install must be
  idempotent, and uninstall must remove the migrated hooks. Released as
  a grh minor.
- **Skills.** `greenharness-skills` sources say `greenpiste <cmd>` where
  they say `amont-agent <cmd>` today (39 files), released as a skills
  minor; the installed copies follow at the person's `grh` update.
- **Rollback.** Published names cannot be withdrawn; the way back is
  `install.sh` pinned to 3.34.0 (its `amont-agent-3.34.0-*` assets stay),
  a tap commit restoring `Formula/amont-agent.rb` (the rename alone
  resolves to 4.0.0), and grh's previous release.

## Phases

- [x] Phase 1 — the names module and every message through it:
  `amont-agent/crates/amont-agent-guard/src/names.rs`,
  `amont-agent/crates/amont-agent-guard/src/lib.rs`,
  `amont-agent/crates/amont-agent-guard/src/settings.rs`,
  `amont-agent/crates/amont-agent-guard/src/decision.rs`,
  `amont-agent/src/main.rs`, `amont-agent/tests/names.rs`, and every crate
  source printing the name (`rg -l '"amont-agent[ :/]' crates/*/src src`:
  `candidates_block.rs`, `shape.rs`, `page_open.rs`, `attestations.rs`, …).
- [x] Phase 2 — crate and binary renames and the alias, started only once
  `amont-agent board --repo amont-agent` lists no worktree with unmerged
  work under `crates/` (or its owner is told to rebase):
  `amont-agent/Cargo.toml`, `amont-agent/Cargo.lock`,
  `amont-agent/crates/amont-agent-guard/Cargo.toml` and
  `amont-agent/crates/amont-agent-process/Cargo.toml` (directories renamed
  to `crates/greenpiste-{guard,process}`), `amont-agent/Makefile`,
  `amont-agent/install/install.sh`, `amont-agent/install/install.ps1`,
  `amont-agent/adr-lint.toml`, `amont-agent/greengate.pack`,
  `amont-agent/greengate.conf`,
  `amont-agent/tools/shell-oracle/src/lib.rs` (build `--bin greenpiste`,
  resolve `target/debug/greenpiste`, and read corpus and interpreter-test
  sources under `crates/greenpiste-guard`; retain `AMONT_AGENT_BIN` until
  3b-2). Verify separately with `cargo test --locked --manifest-path
  tools/shell-oracle/Cargo.toml` from a clean checkout, without an old
  build artifact or an `AMONT_AGENT_BIN` override.
- [ ] Phase 3 — packaging and CI: `amont-agent/.github/workflows/release.yaml`
  (version guard names, build, both archives, publish-crates order,
  `standalone`, `publish-tap` writing the new formula and the rename,
  `verify-brew`), `amont-agent/.github/workflows/ci.yaml`,
  `amont-agent/scripts/bump-tap.py`, `amont-agent/scripts/test_bump_tap.py`,
  `amont-agent/scripts/release-manifest.py`,
  `amont-agent/scripts/test_release_manifest.py`,
  `amont-agent/greenharness.toml`,
  `amont-agent/packaging/homebrew/greenpiste.rb` (class `Greenpiste`, `bin.install "greenpiste"`, the alias as a copy, `def refusing_by_default`, a `test do` running both names).
- [x] Phase 4a — docs: `amont-agent/README.md`, `amont-agent/docs/`,
  `amont-agent/CHANGELOG.md`.
- [ ] Phase 4b — release 4.0.0, after this branch merges: the tag only after
  the crates.io token scope is checked; the tap files are written by
  `publish-tap`.
- [ ] Phase 5 — grh and the skills: `grh/catalog.toml`, `grh/src/install.rs`, `grh/src/fetch.rs`, `grh/src/uninstall.rs`, `grh/src/manifest.rs`, `grh/docs/extension-manifest.md`, `grh/tests/cli.rs`,
  `grh/scripts/e2e.sh`, `grh/README.md`, `grh/CHANGELOG.md`, released;
  `greenharness-skills/parts/workflow/skills/plan-review/SKILL.md`,
  `greenharness-skills/parts/workflow/skills/worktree-task/SKILL.md`,
  `greenharness-skills/parts/gate/skills/tag-release/SKILL.md` and the
  other files naming `amont-agent <cmd>`, released.
- [ ] Phase 6 — record: `decisions/docs/programmes/gate-guardrails/map.md`
  (step 3b split into 3b-1 and 3b-2, this plan's outcome).

## Verification

- Phase 1: the names test fails on a planted `"amont-agent: x"` literal and
  passes after; `cargo test --workspace`; a hook decision reads
  `greenpiste/pipe-to-tail: …` (`greenpiste check 'git push 2>&1 | tail'`);
  `greenpiste check 'amont-agent block reinstate x'` and
  `greenpiste check 'greenpiste block reinstate x'` both deny `block-reinstate`.
- Phase 2: `cargo build` produces `target/debug/greenpiste`; with
  `CLAUDE_CONFIG_DIR` set to a scratch dir holding three entries baked to
  `/x/amont-agent`, `greenpiste install --write` leaves exactly the new
  set (15 entries, all `greenpiste`) and `uninstall --write` removes them;
  `install.sh` into a scratch `BIN_DIR`, run twice (the second time over
  an existing alias), leaves `greenpiste` and an `amont-agent` copy, both
  answering `--version`, `cmp` identical.
- Phase 3: `python3 scripts/test_bump_tap.py` and
  `test_release_manifest.py` pass, the latter with cases that fail
  without the behaviour (a manifest already uploaded, a version mismatch,
  sha256s from `greenpiste-*` archives only); `actionlint` on the workflows; the
  version guard refuses a tag that disagrees with `greenharness.toml`; a `workflow_dispatch` dry run of `release.yaml` on the branch builds 6 `greenpiste-*` and 6 `amont-agent-*` archives with the manifest in SHA256SUMS and passes `standalone`; `rg -n 'amont-agent' .github/workflows/release.yaml` leaves only alias lines.
- Phase 4: the release run concludes `success`; crates.io shows
  `greenpiste` 4.0.0 (and the two libraries); the release holds 6
  `greenpiste-*` and 6 `amont-agent-*` archives, `SHA256SUMS` and
  `greenharness.toml`; `standalone` and `verify-brew` pass; the tap holds
  `Formula/greenpiste.rb` and the rename, not `amont-agent.rb`.
- Phase 5: `grh` CI green; `grh install greenpiste` in a scratch HOME where
  the previous grh had installed 3.23.1 fetches `greenpiste-4.0.0-<t>.tar.gz` and its doctor status reads
  `integration.state`.
- After the person's install: `greenpiste --version` is 4.0.0 and
  `/usr/local/bin/amont-agent --version` (the path the 15 entries name) is 4.0.0;
  `greenpiste install --write` leaves 15 entries pointing at the
  greenpiste binary and none at `amont-agent`; a hook denial in the session
  prints `greenpiste/<rule>`.

## Rollback

- Phase 5: revert the grh and skills releases (pin the previous ones).
- Phase 4: first `greenpiste uninstall --write` (3.34.0's `is_ours`
  matches only `amont-agent`, so it would add a second set), then
  `brew uninstall greenpiste` and `install.sh` at 3.34.0; the old formula
  name resolves to greenpiste 4.0.0 through the rename, so a brew
  rollback is a tap commit restoring `Formula/amont-agent.rb` at 3.34.0
  and dropping the rename. Then `amont-agent install --write`.
- Phases 1–3: unreleased code; revert the PR.

## Outcome

`brew install fredericrous/tap/greenpiste` and `grh install greenpiste`
give a working greenpiste 4.0.0, the hooks run as `greenpiste`, and
`amont-agent` answers only as an alias until the census.

## Decision log

- 2026-11-14 (Phase 3 open at push) — the release workflow, scripts, formula
  and manifest are on the branch (8666d057, review fixes 38e6766a); observed
  so far: `test_bump_tap.py` (both prefixes) and `test_release_manifest.py`
  pass, both workflows parse as YAML. Its Verification is a
  `workflow_dispatch` dry run, which runs only on a ref GitHub has, so the
  branch is pushed with Phase 3 unticked and the dry run is dispatched on it
  before the merge; Phase 3 is ticked with that run's result.
- 2026-11-14 (Phase 4a) — done (3fa57f4d): README, `docs/` and the book name
  greenpiste; `docs/install.md` gains "Upgrading from amont-agent" (the
  alias copy, `brew upgrade` through the rename, `greenpiste install
  --write`). Phase 4 split into 4a (docs, on this branch) and 4b (the release,
  after the merge).
- 2026-11-14 — `greengate.conf` is unchanged though Phase 2 names it: its
  `amont:pack` block vendors `greengate.pack` (now `greenpiste suppressions`),
  and editing the vendored copy changes the manifest and turns this
  repository's declared checks off until a `greengate trust`. The alias keeps
  the old line running; the block is re-vendored at the fleet sweep.

- 2026-11-14 (Phase 2) — done (3799489a). Observed: `cargo build` produces `target/debug/greenpiste` (`--version` prints `greenpiste 3.35.0`, the crate version is unchanged until Phase 4); `install --write` with 16 entries baked to an old `amont-agent` path rewrote all 16 to the greenpiste binary, and `uninstall --write` removed them; `install.sh` run twice from a local archive (`GREENPISTE_ARCHIVE_DIR`) left `greenpiste` and an `amont-agent` copy that `cmp` reports identical, and `--version` answers on both; `tools/shell-oracle`'s own tests pass from its manifest. Not run: `install.ps1` (no PowerShell here; Phase 3's standalone check must cover Windows); `make install`. The worktree-hold condition was met: amont-agent 3.35.0 was released from `a5ef8693` before the rename commit was made.

- 2026-11-14 (Phase 1) — done (169892be). Observed: `target/debug/amont-agent --version` prints `greenpiste 3.34.0`; `check 'git push origin main 2>&1 | tail -5'` denies `pipe-to-tail`; `check 'amont-agent block reinstate x'` and `check 'greenpiste block reinstate x'` both deny `block-reinstate`; `tests/names.rs` passes (and failed on a stray Windows-style `.claude\\amont-agent\\` path until the kept list learnt the escaped separator); `cargo test --workspace --locked` passes at the commit gate. The relais worker over-renamed two data names in tests (LSP fixture paths, renamed with their fixtures; one state-store path, restored to `amont-agent`). Phase 2 waits for amont-agent 3.35.0 (perso-0b, feat/clear-at-boundary), at that session's request.

- 2026-11-14 — 3b split as 3a was: 3b-1 (this plan: installed, published
  and printed names) and 3b-2 (names read and written), so the data-name
  coupling with relais and greengate lands in one place.
- 2026-11-14 — major version 4.0.0: the binary and crate names change, as
  greengate's did at 3.0.0.

<!-- panel: repos=amont-agent,grh,greenharness-skills,decisions adds=contract reviewers=backend,lang:python,lang:rust,platform body-sha=f9f1ecc7545a -->
