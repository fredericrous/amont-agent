---
status: active
---
# Preview gate: soak fixes

## Goal

Fix the five defects the first day of real use (2026-09-28) found in the
preview-approval gate (ADR-0023, shipped in 2.18.0), so the soak measures
the gate rather than its bugs.

## Behaviour

1. `cd <literal path> && … && amont-agent preview register …` binds. Only
   leading `cd` clauses joined by `&&` may precede it; the repository is
   resolved through them and must still equal the printed JSON's repo and
   HEAD. Anything else stays `unbound`.
2. A changed file counts as a UI change only when it matches the UI path /
   extension filter AND its nearest `package.json` (walking up to the repo
   root, read from the pushed commit) has a `dev` script or depends on
   `react`, `react-dom`, `react-native`, `react-strict-dom` or
   `@duro-app/ui`. Repo-level `gated()` and the `amont.agent.push-preview.ui`
   override are unchanged.
3. A `.ts/.tsx/.js/.jsx/.css` file whose every changed non-blank line is a
   comment line does not count as a UI change. Any doubt counts as UI.
4. Preview / publication journal lines name the resolved repository (the
   push target's, the registration's), not the session cwd's.
5. Answer latency is measured by the hook: PreToolUse time stored in
   `asks/<tool_use_id>`, PostToolUse time minus it. Journalled as
   `option,<secs>s,dur=<duration_ms>ms`.

## Phases

1. Plan (this file).
2. One commit per fix, each with an end-to-end test through the real binary
   (`tests/preview.rs`), falsified once.
3. Docs (`docs/preview.md`, CHANGELOG), `make check`, piloted run on scratch
   clones of duro-design-system and application-landscape.
4. Plan record, then `chore: release 2.20.0`.

## Decision log

- 2026-09-28: origin/main already carries 2.19.0 (request-fanout), so this
  release is 2.20.0, not the 2.19.0 the brief assumed.

## Verification

| input | expected | actual |
|---|---|---|
| duro-design-system clone, change under `packages/cli/src` only | not advised | _pending_ |
| duro-design-system clone, change under `packages/ui/src` | advised | _pending_ |
| application-landscape clone, comment-only `.tsx` change under `app/` | not advised | _pending_ |
| application-landscape clone, real JSX change | advised | _pending_ |
| `cd <clone> && amont-agent preview register …` | journal `registered` with the clone's name | _pending_ |
| marked question answered after ~3s | latency ≥3s in the journal | _pending_ |

## Outcome

_pending_
