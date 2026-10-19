---
status: done
branch: refactor/guard-process-edges
repos: [amont-agent]
adrs: [ADR-0039, ADR-0011, ADR-0030, ADR-0010]
---
# Plan: amont-agent's guard and work process, two components

## Review panel

👉 **Decide:** none. Approve if a compiler-held boundary (guard ← process ← binary crates, with no behaviour change) is worth two PRs, two new crates.io names and one minor release.
📍 amont-agent · reviewed, no code yet · next: Phase 1, breaking the guard→process edges inside the one crate. Panel: backend (opus-5-5), platform (opus-5-5), codex (gpt).
**Changed by review:**
- `push_preview` is a process rule, and the table is assembled in `main.rs`.
- The release pipeline gains a version check, a per-crate publish loop, a dry-run gate, crate-local corpus files, and a tap that waits for crates.io.
- `record_before`'s policy stays in preview.
**Verdicts:** backend rework, then approve-with-changes twice, then approve. Platform approve-with-changes. Codex rework, fixed. 4 findings carried into implementation.

📄 Full reviews: [2026-10-19-guard-and-work-process-components.reviews.md](2026-10-19-guard-and-work-process-components.reviews.md)

## Context

The SOLID audit (2026-10-15) found that amont-agent has two reasons to change.

- **The guard** reads an agent's tool calls: shell analysis, rules, stances, the journal. It changes when shell syntax or agent habits change.
- **The work process** is the plan-review panel, the implementation review, preview approvals, provenance, `shipped` and the attestations sweep. It changes when the fleet's process changes (ADR-0022, 0028, 0034, 0036).

ADR-0039 (`harness.tool-boundaries`) settled this at the tool level. The work process stays in amont-agent, because each of its verdicts holds or releases a tool call. Nothing settled the inside of the binary. Today it is one crate with no `lib`: 48,380 lines in about 49 private modules, and the two concerns depend on each other freely.

- **Guard → process edges.** These are the ones that make the guard change when the process does.
  - `assertions/push_published.rs:49` and `assertions/land_retry.rs:51` read preview's push and land records (`preview::record_before`, `take_before`, `land_judged`; the stores are `previews/pushes` and `previews/lands`).
  - `doctor.rs` imports `attestations::{summary, last_scan_failure, mb_text, DAY, Env}`.
  - `hook.rs` dispatches the process's events (`hook.rs:72-119`, `:152`, `:200-219`, `:364`, `:397`, `:543`). That is the composition root's job, not a defect.
- **Cycles inside the process.** These do not block the split (see Choices); breaking them is cleanup.
  - `preview` ↔ `plan_review`: `preview` uses `canonical`, `body_sha`, `shas` and `Fence`; `plan_review` uses `preview::package_is_ui`.
  - `preview` ↔ `implementation_review`: `preview` uses `content_id`; `implementation_review::provenance` (`:609-619`) calls `preview::outcome`.
- The README and the Cargo description still say amont-agent only reads shell commands.

Per the fleet's canon, a boundary only holds when something enforces it. `components.acyclic-dependencies` is a constraint, and amont's no-deps script is the precedent for "a guard that cannot fail is decoration". In Rust that something is the crate graph. amont (four crates) and aval (`aval-core` and `aval`) are already workspaces.

The intended outcome is three crates in an amont-agent workspace, with dependencies pointing one way: guard ← process ← binary. The compiler then refuses a guard module that reaches into the process. One binary and one release stay, with no change in behaviour, state paths or CLI.

## Design

| Crate | Holds | Depends on |
|---|---|---|
| `amont-agent-guard` (lib) | shell, analysis, stance, payload, decision, rules (with two process rules' declarations; see below), assertions, journal (the state root), session_state, push_target, land, stale, lsp, quiet, excerpt, settings, doctor, the rule-evaluation tools (backtest, mine, corpus, graduate, compliance, transcript), and the shared utilities (`atomic`, `git`, `gitconfig`, `json`, `civil`, `term`, `ui`) | `amont-quiet`, serde_json, serde |
| `amont-agent-process` (lib) | plan_review, plan_words, plans, plan_phases, the `push_preview` rule, plan_apply, review_outcomes, implementation_review, preview, guide, provenance, shipped, attestations | guard, pulldown-cmark |
| `amont-agent` (bin) | `main.rs` (the CLI) and `hook.rs` (event dispatch): the composition root that wires both | both |

Choices:

- **The rule table is assembled in `main.rs`** (`systems.construction-in-main`). `push_preview`'s `confirm` calls `preview::needs_preview` (`rules/push_preview.rs:102`; `preview.rs:471`), so it is a process rule in full and moves to the process crate. The guard keeps its own rules, and the declarations of `plan_review_panel` and `plan_phases_open`, whose `examine` never matches and whose judgement already lives in the process modules. The guard's readers of `RULES` (22 references in 7 files: `stance`, `ui`, `doctor`, `corpus`, `mine`, `rules/mod`, `main`) take a `&[Rule]` parameter (today's type, `rules/mod.rs:473`), and `main.rs` passes one table in today's order (`rules/mod.rs:473-518`), with `push_preview` at its current position. There is no global registry.
- **The push and land records split between data and policy.** A record is a fact about what a push or a land did, and both the guard's assertions and preview read it. A guard module, `push_record`, takes the record types, their persistence and readers (`take_before`, `land_judged`) and the upkeep of the land records. `record_before` stays in preview, because it computes preview policy (`gated`, `carries_ui` and `approved` at `preview.rs:1786`, `:1802` and `:1811`), and it writes through `push_record`. The paths `previews/pushes` and `previews/lands` are kept, so no state migrates. The assertions' outcomes are compared before and after for a gated and an ungated push, approved and not.
- **`doctor` receives its process sections.** It takes extra report sections from its caller (dependency inversion), and `main.rs` passes the attestations summary. The doctor output stays the same.
- **The cycles are broken by moving helpers to where they belong (Fowler, Move Function).**
  - `canonical`, `body_sha`, `shas`, `Fence` and `package_is_ui`, which are facts about a plan's text, go into a new `plan_text` module that both `plan_review` and `preview` import.
  - `content_id` and `content_id_of` move to their own `content_id` module.
  - The provenance filing at `implementation_review::provenance` (`:609-619`) moves into `provenance.rs`, which asks `preview` for the outcome. `implementation_review` then no longer calls `preview`.
  - These cycles all sit inside the process crate, and Rust allows cycles within a crate, so the split does not need them broken. Breaking them is cleanup, done because each helper belongs to the module it describes. There is no `preview` ↔ `plan_phases` cycle: the only edge is `preview.rs:1704` → `plan_phases::is_plan_name`.
- **Visibility.** Items crossing a crate become `pub`. Everything else stays `pub(crate)`.
- **Release.** The workspace version is shared, as amont's is. The two libraries are published to crates.io before the binary (`cargo install amont-agent` resolves dependencies by version), in the order guard, process, binary, with the same "already uploaded" tolerance amont's release loop has. They are named `amont-agent-guard` and `amont-agent-process`; both names are checked as unclaimed on crates.io before the first release.

## Changes

### Phase 1: no cycles, no guard→process edge (one crate, behaviour unchanged)

1. Add the `plan_text` and `content_id` modules. Callers are updated, and LSP findReferences is run before each move.
2. Move the provenance filing into `provenance.rs`.
3. Add the `push_record` guard module, with the record types, persistence, readers and land-record upkeep. Preview's `record_before` builds each record from its policy and writes through it; the two assertions read through it.
4. Move `rules/push_preview.rs` beside the process modules, and give the guard's `RULES` readers a `&[Rule]` parameter, with the table assembled in `main.rs`.
5. `doctor` takes its process sections from `main.rs`.
6. Add `tests/boundaries.rs`. It matches every `crate::<module>` path, in `use` lines and inline, and fails when a guard module names a process module, or a process module names the binary's `hook`. The test lists the guard and process sets. It is temporary scaffolding until Phase 2, where the crate graph takes over and the test is removed with it.

### Phase 2: the workspace

1. Create `crates/amont-agent-guard`, `crates/amont-agent-process` and `crates/amont-agent` with `git mv`. In the same commit, make the smallest manifest, path and visibility repairs that let the workspace build, because every commit must pass the repository's pre-commit fmt and clippy gates. The moved files stay above git's rename threshold, so `git log --follow` keeps each one's history.
2. Move unit tests with their modules, except the tests that loop over the whole rule table (`corpus.rs:464-479`: every rule has an embedded corpus that matches its file). Those move to the `amont-agent` crate and run against the table `main.rs` builds, and a check asserts that table holds 45 rules, `push-preview` among them. The integration tests in `tests/` spawn the binary and stay with the `amont-agent` crate. The stdout architecture check (`tests/hook.rs:273`), which today scans only its own package's `src`, scans all three crates' sources, fails when one is missing, and is proven by a planted stdout write in each library.
3. Update the workspace `Cargo.toml`: the shared version, MSRV 1.85.0, the `test-hooks` feature forwarded to every crate that has test hooks, and internal dependencies as `path` plus `version = "=X.Y.Z"`.
4. Update the Makefile and `ci.yaml`: `--workspace` on clippy, test and the 1.85.0 `--locked` check, and `-p amont-agent --features test-hooks --test quiet` where a test target is named.
5. Update `release.yaml`:
   - the `guard` job reads the version with `cargo metadata --no-deps` and asserts all three crates agree (it greps `^version = ` today, `release.yaml:44`);
   - the build uses `-p amont-agent`, `Cargo.lock` stays at the root, and README and LICENSE are copied from the root (`:132`, `:142-148`);
   - `publish-crates` first runs a `cargo publish --workspace --dry-run` gate, then loops `cargo publish -p <crate> --locked` over guard, process and binary, with amont's "already uploaded" tolerance, so a rerun resumes a partial publish (today it is a single `cargo publish`, `:334`);
   - `publish-tap` also needs `publish-crates`, so brew never ships a version that `cargo install` cannot build (`:358`).
6. Move `tests/corpus` into the guard crate. The `include_str!` calls at `corpus.rs:104-225` and `CARGO_MANIFEST_DIR` at `corpus.rs:87` and `lsp.rs:500` must resolve inside it. Check `cargo package --list` for each crate.
7. Before tagging, confirm with the person that amont-agent's own `CARGO_REGISTRY_TOKEN` can create new crates.
8. Update the README and the Cargo description: amont-agent guards an agent's tool calls, including the work-process verdicts that hold one (ADR-0039). The docs say which crate owns which page.
9. Release a minor version. Rollback: before the release, revert the PR. After publishing, fix forward in the next version, because crates.io can only yank and the package names are permanent.

Each phase is one PR, through `worktree-task`, then `merge-when-green`. The release goes through `tag-release`.

## Verification

- Phase 1:
  - `tests/boundaries.rs` passes. A planted `use crate::preview` in `doctor.rs` makes it fail, and so does a planted inline `crate::preview::outcome()` call.
  - `make test` and `--features test-hooks` report the same passing test count as main before the change, with 0 failed.
  - `amont-agent doctor` output is the same before and after, on this machine.
- Phase 2:
  - `cargo tree -p amont-agent-guard` lists neither `amont-agent-process` nor `amont-agent`.
  - A planted `use amont_agent_process::…` in the guard fails to compile.
  - The same passing test count as main, with 0 failed, and `cargo +1.85.0 build --locked` passes.
  - `cargo publish --workspace --dry-run` (cargo ≥ 1.90; the MSRV applies to the build only) passes for all three crates.
- Both phases:
  - `amont-agent rules --json` is byte-identical before and after.
  - The state directory tree under `~/.claude/amont-agent/` is unchanged after one hook run of each kind.
  - A full plan cycle on this machine still holds and releases. A plan exit, an implementation-review push and a preview question each leave the same `journal.log` deny and pass entries as on the previous release.
- After the release, `amont-agent --version` on this machine shows it, and the hook's journal records events from the new build.

## Outcome

Phase 1, as observed before the push:

| Check | Expected | Actual |
|---|---|---|
| the test suite (`cargo test --features test-hooks`) | main's passing count, 0 failed | main: 1158 passed in 23 binaries. After: 1160 passed in 24 binaries (the 2 tests in `tests/boundaries.rs`), 0 failed. `make test`'s plain `cargo test --locked` also passes |
| `amont-agent rules --json` | byte-identical | identical (`cmp`). 48 entries on main and after: 45 rules in the table and 3 assertions |
| `amont-agent doctor` | the same output | identical once relative ages are normalised; the attestations line stays last, as on main |
| `tests/boundaries.rs` | passes; a planted edge fails it | passes. The planted `use` line, inline path, root edge and grouped import (`use crate::{json, plan_review::Fence}`) are each found; a comment and a test module are not |
| clippy `--all-targets --features test-hooks -D warnings`, fmt | clean | clean |
| the assertions' outcomes for a gated and an ungated push, approved and not | unchanged | covered by the existing integration tests, which pass unchanged: `tests/preview.rs` (push-published journal lines, `:1075`) and `tests/implementation_review.rs` (land-retry, `:1245`, `:1348`). The record format is byte-for-byte the same: a missing `before` still writes `-` |
| the state-directory tree after one hook run of each kind; a full plan cycle's journal entries | unchanged | not run: these need a live session after the release. The integration tests spawn the binary against a scratch state directory, and they pass |

Phase 2, as observed before the push:

| Check | Expected | Actual |
|---|---|---|
| `cargo tree -p amont-agent-guard` | names neither the process nor the binary | its direct dependencies are `amont-quiet`, `serde` and `serde_json`, and nothing else |
| a planted `use amont_agent_process as _planted;` in the guard's `doctor.rs` | fails to compile | `error[E0432]: unresolved import amont_agent_process`; the file was then restored |
| the test suite | main's passing count, 0 failed | 1160 passed, 0 failed, the same count as main: Phase 1's two `tests/boundaries.rs` tests went with the file, and two unit tests run in a new lib target |
| `cargo +1.85.0 check --workspace --locked --all-targets` | passes | passes |
| `cargo publish --workspace --dry-run` (cargo 1.94.1) | packages all three crates | it packages all three, in order guard, process, binary; the guard's package holds its 45 corpus files |
| the stdout check (`tests/hook.rs`), with a planted `println!` in each library's `lib.rs` | fails, once per library | it panicked at `tests/hook.rs:309` for the guard and again for the process; both files were restored |
| `amont-agent rules --json` and `doctor`, main and this branch run back to back | byte-identical | both identical (`cmp`, `diff`); `rules --json` is 6,885 bytes |
| the pre-commit gate (fmt, clippy `-D warnings`, the tests) | passes | 25 checks passed |
| the state-directory tree, and a full plan cycle's journal entries | unchanged | not run: they need the released binary in a live session. They are checked when the release is installed, along with `amont-agent --version` |

## Decision log

- 2026-10-19: `rules/implementation_review.rs` was not declaration-only, as the plan assumed. Its `confirm` calls `implementation_review::confirm_push`. `tests/boundaries.rs` found it, and the review panel had not. It moved with `push_preview`, to `src/implementation_review_rule.rs`, at the same position in the table. The guard keeps two process-rule declarations: `plan_review_panel` and `plan_phases_open`.
- 2026-10-19: moving the provenance filing into `provenance.rs` would have made a new `preview` ↔ `provenance` cycle, because `preview::outcome` returned `provenance::Preview`. That outcome type moved to the module that owns it, as `preview::Outcome`. `provenance` now depends on `preview`, and `preview` no longer depends on `provenance`.
- 2026-10-19: `sha256` and `hex` moved to `src/sha256.rs`, a shared utility. Otherwise `plan_text`, which needs them, would have depended on `plan_review` again.
- 2026-10-19: the table is 45 rules, not the 48 `rules --json` lists: those are rules plus assertions.
- 2026-10-19: #126's `tests (windows-latest)` job failed in `tests/boundaries.rs` only. Report paths used `\`, and on a CRLF checkout the test block was never found, so test modules that use `rule_table` were scanned. The test now normalises line endings and joins path components with `/`. On a scratch copy whose sources were converted to CRLF, the old test failed with Windows' exact list and the fixed one passed.
- 2026-10-19: the binary stays the workspace's root package (`members = [".", …]`), not `crates/amont-agent` as Phase 2 step 1 said. amont fetches this repository's fixtures by raw URL, and `tests/`, `tools/` and the Makefile's `-p amont-agent --test quiet` all name root paths. Keeping the root keeps them working with no second change. The compiler holds the boundary either way.
- 2026-10-19: `tests/boundaries.rs` was removed, as planned: the crate graph replaces it, as shown by the planted import above.
- 2026-10-19: `Count::mul`/`add` and `Tri::not` became public with the move, and clippy's `should_implement_trait` fired on them. They now implement `std::ops::{Mul, Add, Not}`. The bodies are unchanged and the lint stays on.
- 2026-10-19: lib crates run doctests. Two indented blocks in `guidance.rs` and `gitconfig.rs` were shell and config text, not Rust, so they are now fenced as `text`.
- 2026-10-19: `doctor` on this machine read "attestations: 138 → 139 directories" between two runs an hour apart. That is a new attestation directory, not a difference in output: the back-to-back run above is identical.
- 2026-10-19: `amont-agent-guard` and `amont-agent-process` both return 404 on the crates.io API, so they are unclaimed.

## Implementation review

approve-with-changes, then approve-with-changes on the Delta.
- Fixed: the scanner reads grouped imports; `holds-until:` added; a stale `RULES` link; the moved `remove_file` now gives its reason.
- Recorded here: the deviation, the rule count, and the checks that did not run.
- Deliberate: the scanner does not read a nested group (`crate::{a::{b}, c}`). No file in `src` has one, and the test is scaffolding that Phase 2's crate graph replaces.
- Windows fix: approve-with-changes, with two low findings kept deliberately. There is no CRLF regression test, because the Windows CI job runs this test on CRLF sources on every PR. The match on the test-block marker stays literal; no source deviates from it, and the test retires in Phase 2.
- Round 1 took 44k tokens and 76 s; the Delta, 49k and 21 s; the Windows fix review, 18k and 11 s.

Phase 2: approve-with-changes, 33k tokens, 24 s.
- Fixed in this record: the `cargo tree` check, the planted import and the planted stdout write in each library were run and recorded. So were the dry run and the identity checks, both re-run on the final commit. The layout, the trait impls, the `text` fences and the removed test each have a Decision-log line.
- Deliberate: the state-directory and plan-cycle checks run on the released binary, as the Outcome says. Confirming the token scope (step 7) is a question for the person, asked before the tag.

<!-- panel: repos=amont-agent adds=ops reviewers=backend,platform,codex body-sha=25225b5a758c -->
