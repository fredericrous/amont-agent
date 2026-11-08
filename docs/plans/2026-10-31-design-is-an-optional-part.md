---
canonical: fredericrous/decisions docs/plans/2026-10-31-design-is-an-optional-part.md
phases: [7]
status: active
branch: refactor/ui-packages-no-duro
---
# Design is an optional part: amont-agent drops the Duro marker (pointer)

This repository's slice of the ADR-0048 plan is section 7:

- `@duro-app/ui` leaves `UI_PACKAGES`, so the harness names no design
  system;
- the `package_is_ui` doc comment, the preview tests and the two docs
  pages are generalised.

No release: nothing later in the plan needs it, so it rides the next
release.

## Implementation review

- Round 1: approve (44k, 15 s). Deliberate: `tests/preview.rs` keeps `@duro-app/ui` in a CLI fixture as a non-marker in dev and optional-peer position. It is test data, not a dependency of the harness. The survey covered 13 package.json files across the 11 consumers.
- Verification: all 13 Duro consumer package.json files declare `react` beside `@duro-app/ui`, so detection is unchanged. `cargo test -p amont-agent-process a_package_json_looks_like`: 1 passed. No release.
