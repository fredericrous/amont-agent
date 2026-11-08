---
canonical: fredericrous/adr-lint docs/plans/2026-11-08-ci-fetches-release-assets-api.md
phases: [2]
status: done
branch: fix/gh-api-downloads
repos: [amont-agent]
---

# CI fetches GitHub release assets through the API, fleet-wide (pointer)

This repository's slice of the fleet plan: Phase 2. The plan, its review
and its recipe are canonical in adr-lint (pilot: fredericrous/adr-lint#53).

## Code review

- amont-agent: backend, platform

## Outcome

- input: "Ensure aval" script, `AVAL_VERSION=1.10.0` → expected: fetched through gh and extracted → actual: as expected.
- input: "adr-lint sync --check" script, `ADR_LINT=0.8.0` → expected: fetched through gh, `sha256sum -c` OK, extracted → actual: as expected (no `set -e` in that step, as on main).
- input: "install mdbook", 0.4.40 → expected: fetched and extracted → actual: as expected.
- input: repository-wide grep → expected: only non-goals → actual: `install/install.sh`, `install/install.ps1`, README/docs/CHANGELOG (product installers), and a fixture in `scripts/test_bump_tap.py:37`.
- To observe after the merge: dispatch `docs.yaml` on main.

## Implementation review

- amont-agent implementation-review: approve (low: record outcome, done here); 59k, 35 s (opus).
- amont-agent backend: approve-with-changes, all low; deliberate: the blanket 3-attempt retry is the fleet policy (Decision log), and the missing `set -e` in ci.yaml is main's; 51k, 33 s (opus).
- amont-agent platform: approve, no findings; 69k, 36 s (opus).
