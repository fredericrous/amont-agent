---
canonical: fredericrous/decisions docs/plans/2026-11-08-amont-moves-to-greenharness.md
phases: [6]
status: active
branch: fix/repo-moved-greenpiste
repos: [amont-agent]
---
# amont and amont-agent move to greenharness (pointer)

This repository's slice of the fleet plan: Phase 6 (its strings name the new
home, then release 3.23.1 from it).

## Code review
- amont-agent: backend, unix (tui left out: no screen or output change; lang:rust left out: one manifest field, no Rust code)

## Implementation review
- implementation-review: approve (delta); fixed: book `site-url` → `/greenpiste/`; the `amont.conf` pack-line deferral is recorded in the canonical plan's Decision log; candidates `r` ruled fine; 62k/54 s + 36k/37 s.
- backend: approve (delta); fixed: `site-url`, the homepage refusal names both owners; 58k/75 s + 23k/15 s.
- unix: approve (delta); fixed: `site-url`, the homepage message; 53k/47 s + 22k/14 s.

## Verification
- input: `scripts/test_bump_tap.py` on the new and the old `bump-tap.py` → expected: pass, then the three owner tests fail → actual: 8/8 OK; on the old script exactly 3 fail.
- input: grep for `fredericrous/amont`, `fredericrous/amont-agent`, `fredericrous.github.io` outside history and the guard corpora → expected: `amont.conf`'s pack lines (deferred), the bump-tap patterns and tests → actual: exactly those.
