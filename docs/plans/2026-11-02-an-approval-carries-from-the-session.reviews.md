# Full reviews: an approval carries from the session

Reference for [the plan](2026-11-02-an-approval-carries-from-the-session.md).

- **backend** round 1 (37k, 71 s), approve-with-changes, all applied:
  - (high) read the transcript first;
  - (medium) an unreadable declaration means Missing;
  - (medium) a real live check and a 14-day count;
  - (low) the TREE-first grep and a rollback.
- **backend** round 2 (27k, 30 s), approve-with-changes, carried into implementation:
  - (medium) the 14-day count needs a bar: 0 re-runs, due 2026-11-15;
  - (low) seven existing carry tests, not four;
  - (low) a Missing-declaration test;
  - (low) the rollback line's placement.
- **codex** (gpt-6-astra, high effort), approve-with-changes:
  - (high, applied) a pointer's historical declaration needs a fixed revision tied to the reviewed commit, else Missing;
  - (medium, carried) derive each commit's changed-plan list against the merge-base, with a restored-plan test.
- **backend** delta after Codex (24k, 46 s), approve-with-changes, carried into implementation:
  - (medium) no revision evidence exists today, so a pointer is always Missing; recording the revision is a follow-up;
  - (low) Missing falls through to filed passes;
  - (low) also test an unchanged pointer declaration.
