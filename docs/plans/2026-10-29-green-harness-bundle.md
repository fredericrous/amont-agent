---
canonical: github:fredericrous/decisions/docs/plans/2026-10-29-green-harness-bundle.md
phases: [2c]
status: done
branch: feat/grh-queries
---
# Green Harness bundle

Pointer. The plan and its review panel live in decisions, with ADR-0050
(harness.installer). This repository carries phase 2c: `doctor --json` integration state, and the plan-panel and implementation-review guards reporting a gap when their agents are absent (ADR-0050, harness.standalone-tools).

## Verification

| Check | Input | Expected | Actual |
|---|---|---|---|
| doctor --json, absent | scratch config, no hook entry | integration.state absent, missing lists the targets, stdout one JSON document | as expected (`json_reports_an_absent_integration_apart_from_health`) |
| doctor --json, present | fixture from `settings::TARGETS` | present, missing empty | as expected |
| doctor --json, partial / unknown | some targets / unparsable settings | partial / unknown | as expected (tests/doctor.rs, 23 passed) |
| doctor --help | `doctor --help`, `-h` | usage on stdout, exit 0 | as expected |
| plan gap | no panel agent anywhere, plan without review | allowed, one `grh install workflow` line | as expected (tests/plan_review.rs, 21 passed) |
| push gap | no implementation-review agent, plan-carrying branch | allowed, one gap line | as expected (tests/implementation_review.rs) |
| review honoured without an agent | transcript holds a passing review | silent pass, provenance written | as expected (`a_review_in_the_transcript_is_honoured_with_no_agent_installed`) |
| unreadable sources | agents/ or plugin cache at 0o000 | deny holds | as expected (agents unit tests, 9 passed) |
| lint | `cargo clippy --all-targets -D warnings` | clean | clean |

## Implementation review

approve-with-changes after one delta (61k + 69k), following a relais rework round (11 findings). Fixed: the gap is judged only after the evidence lookup, an Integration enum, one reader of installed_plugins.json, unreadable sources keep the deny, stdout-only JSON tests, and the plan side checks the repositories the plan names.
