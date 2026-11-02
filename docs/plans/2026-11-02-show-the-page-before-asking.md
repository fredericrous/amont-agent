---
canonical: decisions:docs/plans/2026-11-02-show-the-page-before-asking.md
phases: [2]
status: active
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
- 2026-11-02 — Another session's `fix/preview-squashed-stale-base` touches `preview.rs`, `tests/preview.rs` and the CHANGELOG. This branch avoids `preview.rs` and rebases on whichever lands first. The CHANGELOG is the only shared file.

## Verification

Rows are in the canonical plan; results are recorded here.
