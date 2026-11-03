---
canonical: decisions:docs/plans/2026-11-02-show-the-page-before-asking.md
phases: [2]
status: done
---
# Show the page before asking about it (pointer)

Canonical plan: [decisions/docs/plans/2026-11-02-show-the-page-before-asking.md](https://github.com/fredericrous/decisions/blob/main/docs/plans/2026-11-02-show-the-page-before-asking.md) (ADR-0058, rule `work.page-shown-before-asked`).

This repository carries Phase 2:

- `amont-agent open <url|path>`, with the per-machine opener `amont.agent.opener`;
- `preview register --open` routed through the same opener, with `opened` in its JSON;
- the doctor's `opener` finding;
- the `ask-unopened-page` rule;
- the backtest.

## Code review

- amont-agent: backend, lang:rust, unix (tui left out: one new subcommand, `open`, whose only output is an error line; it follows the CLI's existing error style)

## Decision log

- 2026-11-02 — The hook cites `work.page-shown-before-asked`, the rule ADR-0058 adopted (see the canonical Decision log). The pack refresh is the branch's first commit.
- 2026-11-02 — Another session's `fix/preview-squashed-stale-base` touches `preview.rs`, `tests/preview.rs` and the CHANGELOG. This branch touches `preview.rs` only to add the registration's `opened` field (no approval logic), and rebased on 3.15.1 and #195. The CHANGELOG is the only shared file.

- 2026-11-03 — Phase 2 was built in two relais runs. The `open` command's worker hit its wall time with a complete candidate, which was committed through the gate and recorded `salvaged` (89dfd8d). The rule's run was approved after review. Its low findings were fixed by hand: no output parameter, an accurate module comment, a `holds-until:` on "asked again" standing in for "the page changed". Deliberate: a `cd` carried over from an earlier Bash call is covered, because the transcript's per-entry `cwd` follows it. `tests/hook.rs` exempts `open.rs` from the stdout check, because it prints only its own `--help` under the `open` verb.

- 2026-11-03 — Code review round 1 (implementation-review, backend, lang:rust, unix): fixed in one relais run (budget-stopped with its candidate, verified, approved) plus by hand. The fixes: option parsing; a create_new 0600 stderr file; the Windows default is now rundll32 instead of cmd; a broken git config is an error, not a silent fallback; typed `OpenError` and `Blind`; `opened` on the registration; register counted only in standalone shapes; no output parameter; names; same-line `let _` reasons; a test for the unset doctor line. A hook-denied call is recorded `is_error: true` (checked in session 81c1a1d5), so a denied ask is not a window boundary. The person chose "Ship as is" on the scan cost (2026-11-03).

## Implementation review

- implementation-review: round 1 approve-with-changes, 3 blocking code-rule items (same-line `let _` reasons, the `settle` output parameter, names), fails-first, unset-opener test and register shape all fixed. Delta approve-with-changes, 2 low: plan text updated here (the Windows default is now `rundll32`, and `preview.rs` gains `opened`; the canonical plan is corrected when Phase 5 closes); deliberate: a page whose written form starts with `-` gets a deny command that exits 2, and the hook does not count `amont-agent open -- <page>` either, so such a question stays denied until the stance is set to `advise` (`git config amont.agent.ask-unopened-page.stance advise`); none of the 269 backtest questions named such a page. holds-until: one does, then `open_target` accepts `[open, --, target]` and the deny command adds `-- `. 104k+134k.
- backend: round 1 approve-with-changes, 4 fixed (a git config failure is an error, the unlinked log carries a `holds-until:`, register shape, Windows default); Delta approve. 70k+77k.
- lang:rust: round 1 approve-with-changes, 7 fixed (`OpenError`, create_new 0600, same-line reasons, OsStr, register shape, `Blind`, `opened` on the registration); Delta approve. 86k+99k.
- unix: round 1 approve-with-changes, 3 fixed (option parsing, create_new, rundll32); Delta approve-with-changes, 1 low deliberate: after `--` a word holding `://` reaches the opener as written; whoever types `--` has said the next word is an operand. 63k+70k.

## Verification

Rows are in the canonical plan; results are recorded here (2026-11-03, rebased on 3.15.1).

| Check | Actual |
| --- | --- |
| opener | `tests/open.rs`: the template's temp file holds exactly the `file://` URL; a path with a space and `#` is one percent-encoded argv entry; missing file exit 1, no argument exit 2, an opener failing within 2 s exit 1; `register --open` reports `opened` false/true with exit 0 (`tests/preview.rs`); doctor `opener` finding (`tests/doctor.rs`) |
| fails first, passes, preview-url, window, narrow, no transcript | `tests/ask_unopened_page.rs`: 15 end-to-end hook tests pass, including the A/B/C mockup question denied and literal matching of `#/A` vs `#/B`, `/options` vs `/options/` |
| backtest | every AskUserQuestion of the last 14 days (41 transcripts, 269 asks) replayed through the release binary with the transcript cut at that ask: first run 6 fires, 3 denies, 1 false (a bare `.html` in an option's text); fixed (`an_extension_alone_is_not_a_page`); re-run: 5 fires, 2 denies (the A/B/C placement question; "Quels ajouts veux-tu dans roadmap.html ?"), 0 false; 3 advises (windows past the 16 MiB read-back) |
| fails first | the behavioural tests of `tests/ask_unopened_page.rs` (without the one that imports `page_open`) against 3.15.1: 9 fail (every deny and advise case, the A/B/C mockup question included), 5 pass (the allow cases, trivially without a rule) |
| suite | `cargo test --workspace` on 10b73a5 (rebased on #195): 1537 passed, 0 failed |
| cost | hook call without a page: p50 89 ms, p95 110 ms (process start and every rule); the scan adds about 12 ms on a short session, 80 to 170 ms on long ones, about 790 ms at worst, only for a question that names a page (5 of 269). Over the 2 ms budget: the person chose "Ship as is" (AskUserQuestion, 2026-11-03) |
