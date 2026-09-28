---
status: done
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
   `@duro-app/ui` in `dependencies` or as a required peer. Repo-level `gated()` and the `amont.agent.push-preview.ui`
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
- 2026-09-28: the brief said a UI dependency in deps, devDeps or peerDeps
  marks a package as UI. duro-design-system's `packages/cli` — the very
  package that must NOT count — lists `@duro-app/ui` in `devDependencies`
  and as an optional peer, so that rule could not pass its own acceptance.
  Only `dependencies` and required (non-optional) peers count. Every real
  UI package surveyed (duro-design-system `ui`, `diagrams`, `tokens`,
  `ui-email`; application-landscape; duro-app) still counts, by a `dev`
  script or a required peer / dependency.
- 2026-09-28: a `*`-prefixed line is a comment unless it reads like code
  (ends in `{`, `}` or `;`, or holds a `{` outside a JSDoc `@tag` line), so
  CSS's `* { … }` counts. Real comment text with `;` mid-line (#301) stays
  a comment.
- 2026-09-28: with no single base (new branch, no `origin/HEAD`), the
  comment check reads `git log -p --first-parent` over the unpublished
  commits; comments-only in each is comments-only in all.

## Verification

| input | expected | actual |
|---|---|---|
| duro-design-system clone, change under `packages/cli/src` only | not advised | not advised (`unconfirmed no_interface_change … duro-design-system`) |
| duro-design-system clone, change to `packages/ui/src/components/ActionBar/ActionBar.tsx` | advised | advised |
| application-landscape clone, comment-only change to `app/components/graph/GraphCanvas.tsx` | not advised | not advised |
| application-landscape clone, `<p>` → `<p className="pilot">` in `DangerZone.tsx` | advised | advised |
| application-landscape clone, the real #301 branch against its own base | not advised | not advised |
| `cd <clone> && amont-agent preview register …` from a session cwd outside the clone | journal `registered` with the clone's name | `registered pilot application-landscape … application-landscape@310e67e` |
| marked question answered after `sleep 3`, payload `duration_ms` 0 | latency ≥3s in the journal | `approved … by option,3s,dur=0ms`; the push is then silent |
| `make check` | green | green (457 unit tests + integration suites; `tests/preview.rs` 22) |

## Outcome

All five fixed, each with an end-to-end test through the real binary in
`tests/preview.rs` and falsified once (the fix broken, the test red, the fix
restored). Released as 2.20.0; not pushed from this worktree.
