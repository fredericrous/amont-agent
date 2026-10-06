---
status: done
branch: feat/rules-json-tap-caveat
repos: [amont-agent, homebrew-tap]
adrs: []
---
# Tap caveat lists the rules that refuse

## Review panel

👉 **Decide:** none — approve if the brew caveat's refusing-rules list should be written by the release from the published binary and checked by `brew test`.
📍 amont-agent + homebrew-tap · plan reviewed, worktree `amont-agent-wt-rules-json` ready · next: Phase 1, the tap formula, first. Panel: backend, lang:rust, tui, unix.
**Changed by review:** the tap test is gated on the formula's version (2.25.0 ignores `--json`, exit 0); `rules` matches its own arguments and gets `--help`; publish step written out exactly.
📄 Full reviews: [reviews](2026-10-06-tap-caveat-lists-refusing-rules.reviews.md)
**Verdicts:** round 1: 4 approve-with-changes; round 2: backend approve-with-changes, then approve on the final body (1 low, applied at implementation).

## Context

`brew install fredericrous/tap/amont-agent` prints a caveat that says "Only
pipe-to-tail refuses a command; every other rule advises or observes". That
has been false since v2.22.0: `plan-review-panel` also ships `deny`
(`src/rules/plan_review_panel.rs:19`, `default_stance: Stance::Deny`). The
caveat is hand-written in the tap (`homebrew-tap/Formula/amont-agent.rb:52`),
and the release's `publish-tap` job runs `scripts/bump-tap.py`, which
rewrites only the version and the four url/sha pairs. So the caveat goes
stale whenever a rule's shipped stance changes, and nothing notices.

The person asked for the real fix: the caveat is generated from the binary
that is being published, and checked against the installed binary.

## Goal

The list of rules that refuse by default, as the brew caveat shows it, is
written by the release from the published binary and verified by
`brew test` on the brewed binary, so it cannot drift from what ships.

## Non-goals

- The rest of the caveat text (install / doctor lines) stays hand-written.
- The other formulas in the tap (`amont`, `aval`, `relais`).
- Changing any rule's stance.

## Behaviour

**1. `amont-agent rules --json`** (amont-agent). One JSON array on stdout
and a final newline, one object per rule and per assertion, in the order
`rules` prints them:

```json
{"id":"pipe-to-tail","kind":"rule","default_stance":"deny",
 "stance":"deny","max_stance":"deny","per_1000":62.3,"measured":"2026-08-20"}
```

- `default_stance` is what ships; `stance` is what is in force on this
  machine (git config), as the text column shows today.
- `kind` is the list an entry is read from: the `RULES` loop writes
  `"rule"`, the `ASSERTIONS` loop `"assertion"`. Stances come from
  `Stance::as_str` (`src/rules/mod.rs:109`).
- Written with the crate's hand-rolled `src/json.rs` (no serde derive).
  `json.rs` gains `float_field(key, value: f32)`: `per_1000` is an `f32`
  (`src/rules/mod.rs:179`), printed with `{}` on the `f32` so `62.3` stays
  `62.3`. A non-finite value is a bug: a test asserts every rule's and
  assertion's evidence is finite, and `float_field` asserts it.
- The keys are an interface (bump-tap.py and the formula read them); a
  test pins the exact key set of every object.
- Arguments, matched directly (not through `flags()`, which accepts every
  known flag and keeps bare words): none → text, unchanged; exactly
  `--json` → JSON; `-h`/`--help` → the usage line `rules [--json]`, exit 0;
  anything else (`--bogus`, `--force`, `foo`, `--json --json`) → exit 2 and
  one stderr line: ``amont-agent: unknown argument `X`; `rules` takes only
  --json``. The top-level usage line becomes `rules [--json]`.

**2. The formula** (homebrew-tap, one-time change). A generated method plus
its use:

```ruby
  # Written by amont-agent's scripts/bump-tap.py from the released binary's
  # `amont-agent rules --json`; never edit by hand.
  def refusing_by_default
    %w[pipe-to-tail plan-review-panel]
  end
```

- The caveat paragraph becomes "These rules refuse a command by default:"
  then one rule per line, indented like the command lines above it, then
  "Every other rule advises or observes; each one's stance is one line
  away:" and `amont-agent status`. One per line keeps it under 80 columns
  as the list grows.
- `test do` (with `require "json"`) is gated on the formula's own version:
  `if version >= Version.new("2.27.0")` (2.26.0 as first planned; see the
  Decision log) it runs `rules --json` on the brewed
  binary, parses it strictly (a parse failure fails the test), and checks
  that the set of `kind == "rule"` entries with `default_stance == "deny"`
  equals `refusing_by_default`; otherwise (up to 2.26.0, which ignore `--json`
  and prints the text table with exit 0) it keeps today's `assert_match
  "pipe-to-tail"`. It reads `default_stance`, not `stance`, so a user's git
  config cannot change the answer. It prints which branch ran.
- The header comment is updated: bump-tap.py also rewrites this method.
- Once the formula is at 2.27.0 the old branch is dead code; a later tap
  change may delete it, but nothing depends on that happening.
- If the next release is not 2.27.0 (a 2.26.1 that already ships
  `--json`), its JSON check stays off until 2.27.0. Acceptable: bump-tap.py
  still writes the list from the binary on every release.

**3. `scripts/bump-tap.py`** (amont-agent). A new argument, the path to the
release binary's `rules --json` output
(`bump-tap.py <version> <SHA256SUMS> <rules.json> <formula>`):

- builds the sorted list of `kind == "rule"` entries whose
  `default_stance == "deny"` (an assertion shipping `deny` is not a rule
  that refuses a command, and is left out);
- replaces the single `%w[...]` line inside `def refusing_by_default`,
  asserting exactly one match (as it already asserts on the version line
  and the four url/sha pairs);
- refuses an empty list (a release where nothing refuses is a surprise
  worth a red run, not a caveat saying "none");
- stays idempotent; its docstring names the new rewrite.

**4. `release.yaml` `publish-tap`** (amont-agent). The download step
fetches the musl archive with the checksums, from the published release:

```sh
set -euo pipefail
musl="amont-agent-$VERSION-x86_64-unknown-linux-musl"
gh release download "v$VERSION" --pattern SHA256SUMS --pattern "$musl.tar.gz"
sha256sum -c --ignore-missing SHA256SUMS
mkdir rel && tar -xzf "$musl.tar.gz" -C rel
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
  "rel/$musl/amont-agent" rules --json > rules.json
test -s rules.json
```

(the step sets `pipefail` itself: the job has no default shell, and
GitHub's default `bash -e` has none), then `bump-tap.py "$VERSION" SHA256SUMS
rules.json tap/Formula/amont-agent.rb`. `verify-brew` already runs `brew
test`, which now checks the list against the brewed binary.

**Order.** The tap change lands first: bump-tap.py asserts the method
exists, so with the old formula the next release's `publish-tap` would
fail. With the tap change alone, the formula is correct now (both names)
and the old bump-tap.py leaves the method alone.

## Phases

- [x] Phase 1 — homebrew-tap: the method, the caveat, the guarded
  `test do`, the header.
- [x] Phase 2 — amont-agent: `rules --json`, its argument handling,
  `float_field`; integration tests in the style of `tests/stance_scope.rs`
  (temporary `GIT_CONFIG_GLOBAL`).
- [x] Phase 3 — amont-agent: bump-tap.py + `publish-tap` step; a test of
  bump-tap.py against a fixture formula and a fixture JSON (one rewrite,
  idempotence, refusal on an empty list and on a missing method, an
  assertion shipping `deny` left out).
- [x] Phase 4 — CHANGELOG `Unreleased`, the `rules` line in usage.

## Decision log

- 2026-10-06 — generated from the binary, not scraped from `rules` text:
  the text column is machine-dependent (`deny (ships as advise)`), and a
  `--json` is what clig.dev asks of a command whose output feeds a program.
- 2026-10-06 — review round 1 (backend, rust, tui, unix): the tap guard
  keys on whether the output parses, not on the exit code (2.25.0 exits 0
  with text); `rules` matches its arguments itself and gets `--help`;
  `float_field` on `f32`; `kind` from the list; the JSON keys are a pinned
  interface; one deny rule per caveat line; the publish step written out.
- 2026-10-06 — review round 2 (backend): the tap test is gated on the
  formula's version (≥ 2.26.0), not on whether the output parses, so a
  broken `--json` later cannot pass silently and no guard removal has to be
  remembered; the step sets `pipefail` itself; exact archive path; test (b)
  picks its rule from `RULES`.
- 2026-10-06 — implementation: v2.26.0 was released from another session
  at 12:18 (`lint-suppression-added`, #71/#72) without `rules --json`, so
  the tap test's gate is 2.27.0, the first release that has it; a 2.26.0
  gate would have failed `brew test` on the live formula. v2.26.0's deny
  list is unchanged (`pipe-to-tail`, `plan-review-panel`).
- 2026-10-06 — implementation: `rules --json --json` says "unexpected
  argument" (the first word is valid, the second is not); every refusal is
  still one line ending "`rules` takes only --json". Help wins wherever
  `-h`/`--help` appears, as in the other subcommands (implementation
  review). An assertion reports `max_stance: "deny"`: assertions have no
  ceiling (`resolve_assertion` caps nothing).
- 2026-10-06 — implementation: the tap carries a pointer file
  (`docs/plans/`, phases [1]), and its formula header no longer describes
  a placeholder with zero checksums (tap implementation review).

## Verification

- Integration tests (`tests/rules_json.rs`, binary run with a temporary
  `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_NOSYSTEM=1`):
  (a) the output parses with `serde_json`, has `RULES.len() +
  ASSERTIONS.len()` entries in `rules` order, each with exactly the keys
  `id kind default_stance stance max_stance per_1000 measured`;
  (b) a rule taken from `RULES` whose `default_stance` is not `deny`
  (not hard-coded, since shipped stances move): with
  `amont.agent.<id>.stance deny` in that config → `stance: "deny"` and its
  shipped `default_stance`; with an empty config → both the shipped one;
  (c) `pipe-to-tail` and `plan-review-panel` have `default_stance: "deny"`;
  (d) `--bogus`, `--force`, `foo`, `--json --json` → exit 2, exactly one
  stderr line; `--help` → exit 0, `rules [--json]`.
- Unit: `float_field("x", 62.3_f32)` → `"x":62.3`; `0.0` → `"x":0`; every
  evidence value finite.
- `rules --json | head -c 10` → no panic text on stderr (SIGPIPE reset).
- bump-tap.py on a copy of the tap formula with the fixture JSON → the
  `%w[...]` line is the sorted deny list; a second run → byte-identical;
  JSON with no deny rule → assertion failure; formula without the method →
  assertion failure; an assertion with `default_stance: "deny"` → not
  listed; `ruby -c` on the result → `Syntax OK`.
- Tap, at 2.26.0 (the formula's version when built): the caveat renders
  through brew's loader with each line ≤ 80 columns, and the gate takes the
  pre-2.27.0 branch. `brew test` itself is not run here: it needs the
  formula installed, which would replace the person's active guard.
- The formula's `test do` comparison, loaded through brew at `version
  "2.27.0"` and run against the new binary: passes; with a third rule
  added to `refusing_by_default` it fails.
- `make check` clean.

### Observed (2026-10-06, before the push)

- `tests/rules_json.rs` 4/4: (a) 37 entries, same ids and order as the text
  table, exactly the seven keys, an assertion present; (b) a rule shipping
  below deny, raised by a temporary global config → `stance: "deny"`, its
  `default_stance` unchanged, empty config → both the shipped value;
  (c) the deny rules are exactly `pipe-to-tail`, `plan-review-panel`;
  (d) `--bogus`, `--force`, `foo`, `--json --json` → exit 2, empty stdout,
  one stderr line; `-h`, `--help`, `--bogus --help`, `--json -h` → exit 0,
  `usage: amont-agent rules [--json]`.
- Unit: `float_field` 62.3 → `62.3`, 0.0 → `0`, 292.6 → `292.6`; NaN
  panics; every rule's and assertion's rate is finite.
- `rules --json 2>err | head -c 10` → `err` empty, pipestatus 0 0.
- `scripts/test_bump_tap.py` 5/5 (sorted deny list, assertion left out,
  idempotent, no-deny refused with nothing written, missing method refused,
  only the `%w` line changes). Against the real tap formula, the new
  binary's JSON and v2.26.0's SHA256SUMS: output byte-identical to the
  tap's formula, second run identical, `ruby -c` OK.
- The release step's shell, run against the real v2.26.0 release:
  checksum `OK`, binary at `rel/amont-agent-2.26.0-x86_64-unknown-linux-musl/amont-agent`
  (static ELF; its `rules --json` runs in CI only, not on this Mac).
- Tap: caveat via `Formulary.from_contents` aligned, widest line 74;
  2.26.0 → pre-2.27.0 branch, whose `assert_match "pipe-to-tail"` holds on
  the text table; a 2.27.0 copy against the new binary → PASS, with
  `no-verify` added → FAIL; `brew style` no offenses; `ruby -c` OK.
- `make check`: fmt, clippy, the bump-tap tests, 568 unit tests and every
  integration suite green.

## Implementation review

amont-agent: approve-with-changes, then approve-with-changes on the delta; fixed: help anywhere, test temp dirs, the tap pointer, this record.
homebrew-tap: approve-with-changes; fixed: the formula header; the 2.27.0 gate and Phase 1 results recorded here.
amont-agent round 1: 54k tokens, 43 s; delta: 28k, 21 s. homebrew-tap round 1: 39k, 30 s.

## Outcome

Shipped on the two branches: `rules --json`, the generated tap caveat, its
`brew test` check from 2.27.0, the tested bump-tap.py.
Follow-up, after the 2.27.0 release: `verify-brew`'s log shows the JSON
branch ("checking refusing_by_default…") and the published caveat lists
the deny rules. Order: tap PR #6 merges before amont-agent's PR, or the
next `publish-tap` fails its assertion.
Surprise: v2.26.0 shipped mid-task from another session, which moved the
gate.

<!-- panel: repos=amont-agent,homebrew-tap reviewers=backend,lang:rust,tui,unix body-sha=8a7fb43fecd3 -->
