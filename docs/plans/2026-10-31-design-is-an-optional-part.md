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
