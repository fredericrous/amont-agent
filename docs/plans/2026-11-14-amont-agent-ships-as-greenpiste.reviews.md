# amont-agent ships as greenpiste — full reviews

Shared brief: /tmp/gg-sweep/panel-3b1-brief.md.

**Round 1** (body e53dd49a6352):
- backend, approve-with-changes (44k / 77 s):
  - Rules that match a command must accept the alias (blocking, applied).
  - Brew rollback (merged).
  - Check `/usr/local/bin/amont-agent` (applied).
  - Phase 2 waits on open worktrees (narrowed, applied).
  - The prefix change splits transcript counts (a note, rejected).
- lang:python, approve-with-changes (34k / 33 s):
  - Standalone check through `install.sh` (high, applied).
  - A formula template (rejected: duplicates platform#1).
  - Both archive families smoke-tested and compared with `cmp` (applied).
  - `install.sh` run twice over an existing alias (applied).
  - Release-manifest test cases (applied).
  - Names test scope (rejected: duplicates lang:rust#3).
- lang:rust, approve-with-changes (46k / 75 s):
  - grh treats a recorded binary of another name as stale (high, applied).
  - `[[pinned]]` instead of `[[part]]` (applied).
  - Names test on `\bamont-agent\b` anywhere in a string literal (applied).
  - Rollback runs uninstall first (merged).
- platform, approve-with-changes (48k / 76 s):
  - `packaging/homebrew/greenpiste.rb` (high, applied).
  - Check-or-upload instead of `--clobber` (high, applied).
  - Brew rollback (high, merged).
  - `release-manifest.py` repo and prefix (applied).
  - verify-brew runs the upgrade through the rename (applied).
  - A `workflow_dispatch` dry run (applied).

**Round 2** (body 4632ca4ade11):
- backend, approve-with-changes (56k / 28 s):
  - The names test's allow-list for 3b-2 names (high, applied).
  - The grh code-review reasons (medium, carried).
  - A post-publish failure is fixed forward as 4.0.1 (low, carried).
- lang:python approve (41k / 11 s).
- lang:rust approve (50k / 11 s).
- platform, approve-with-changes (53k / 18 s): `test_bump_tap.py` case with both prefixes (low, carried).

**Codex** (body 130499ae1749), approve-with-changes:
- shell-oracle builds the old binary (high, applied).
- grh places the alias copy (high, applied).
- grh refreshes a grh-owned hook integration on rename (high, applied).

**Final binds:**
- After Codex: backend, lang:python and lang:rust approve. lang:rust found that `bin_aliases` must be a declared manifest field (high, applied). Platform approved and carried a grh rollback step (medium).
- On body f9f1ecc7545a: all four approve. Carried: the grh pin ships in the same grh release as `bin_aliases` support, and a rollback restores the old catalog entry (platform, medium); `#[serde(default)]` on the field, with a parse test for a manifest without it (backend); a test that `bin_aliases` survives `release-manifest.py` (lang:python).

**Carried into implementation (7):**
1. The grh code-review line's tui and architect reasons.
2. A post-publish failure means 4.0.1; the dry run skips the publish jobs.
3. `test_bump_tap.py` gets a SHA256SUMS holding both prefixes.
4. grh rollback: uninstall first.
5. The grh pin ships in the same release as `bin_aliases` support, with a catalog restore on rollback.
6. `#[serde(default)]` on `bin_aliases`, with a test for a manifest without it.
7. A test that `bin_aliases` survives `release-manifest.py`.

