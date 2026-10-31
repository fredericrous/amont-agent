---
canonical: decisions:docs/plans/2026-10-31-plan-names-its-code-reviewers.md
phases: [2, 3]
status: active
---
# The plan names its code reviewers (pointer)

Canonical plan: [decisions/docs/plans/2026-10-31-plan-names-its-code-reviewers.md](https://github.com/fredericrous/decisions/blob/main/docs/plans/2026-10-31-plan-names-its-code-reviewers.md) (ADR-0052).

This repository carries Phase 2 (`code_roles()`, the `## Code review` parser, `plan-panel --code-review`, `review-brief --profile`) and Phase 3 (the push hook requires each declared profile).

## Decision log

- 2026-11-01 — Phase 3 was built as three commits (3a profile gate, 3b delta carry, 3c provenance and the doctor gap). Each relais run hit its wall time after its worker finished. Each candidate was checked by hand and recorded `salvaged`.
- 2026-11-01 — Deviation from the canonical plan's "reads the canonical plan at the commit that added it": the hook and `review-brief` read the canonical plan from the plan root's checkout (`plan_review::locate`), as `plan-panel` already does. When that file cannot be read, the push is held under deny and journalled `profiles-unreadable` otherwise. Reading at a commit would need the decisions repository's history, which a branch of another repository does not carry.
- 2026-11-01 — The provenance payload's new `code-review <role> <how> <content-id>` line is not yet listed in amont's `docs/provenance.md`, which names each line's owner. That is a follow-up in amont, outside this plan's repositories. Readers match lines by prefix and ignore unknown ones (that document), so nothing breaks meanwhile.

## Verification

- Phase 2, on the canonical plan:
  - `plan-panel --code-review` → one line per repository → `decisions\tnone`, `amont-agent\tbackend,lang:rust,unix,tui`, `greenharness-skills\tnone`, `grh\tbackend,lang:rust,unix,tui`, `adr-lint\tbackend,lang:rust,lang:typescript,unix,tui`, exit 0.
  - `plan-panel` without the flag → same stdout as installed 3.10.0 → `diff` empty.
  - On the 2026-10-31 board plan (no section) → stderr warning, stdout unchanged, exit 0 → as expected.
  - `review-brief --profile unix` → `Profile: unix` and the 11 cli.md judge rules → both present.
  - `--profile po` → exit 2 → exit 2.
  - Without `--profile` → `Declared code review for amont-agent: backend, lang:rust, unix, tui` → present.
- Phase 3, the integration tests in `tests/implementation_review.rs`, which run the real binary on scratch repositories and session transcripts:
  - no section → implementation-review alone passes;
  - `backend` declared with only an implementation-review pass → refused with `missing code review of <repo> tree <12 hex>: code-review-backend (declared in docs/plans/…)`;
  - a `code-review-backend` launch with `Profile: backend` → allowed;
  - a `Profile: lang:ts` pass for a declared `lang:rust` → refused;
  - `Profile: lang:rust` on `code-review-backend` → refused;
  - two Profile lines → refused;
  - a resumed round binds under its launch's profile;
  - each kind of pass never satisfies the other;
  - `canonical:` is followed;
  - the carry: approve at A plus a fix at B → allowed and journalled `carried-from`; approve-with-changes, no implementation-review on B, a changed declaration, a rebase, or a merged plan → each refused;
  - provenance holds `code-review backend reviewed <tree>`;
  - doctor reports each missing code-review agent.

  Full suite: every binary ok.
