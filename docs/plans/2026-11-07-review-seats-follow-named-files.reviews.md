# Reviews: Review seats follow the files a plan names

- **unix** (opus, 29k, 31 s): approve-with-changes. medium: `seats=` position and usage text; medium: keep `none` in the tab field; low: a failed tree read warns. All accepted (the `none` point merged with tui#3).
- **tui** (opus, 30k, 33 s): approve-with-changes. medium: usage text (duplicate of unix#1); medium: stderr advice (accepted); low: a paste-able `none` reason (merged into a third tab field).
- **lang:rust** (opus, 44k, 69 s): approve-with-changes. high: `remit()` and its inverse test (accepted); medium: `large` normalisation, pathspecs from `file_types`, `areas` kept plus `seat_classes`, tests per phase (accepted); low: `ls_tree_z` lists files only (accepted).
- **backend** (opus):
  - round 1 (45k, 53 s): approve-with-changes. high: the area union when a phase names no path (rejected, see the Decision log; per-phase advice instead). Accepted: `large` and `remit` (duplicates), the backtest bar, no new git call, old ledger rows.
  - round 2 (51k, 21 s): approve-with-changes. Accepted the rejection. Carried: #7, a Phase 3 test asserting the three-field line; #8, `areas_of` through a body-free helper; #9, superseded by Codex.
  - after Codex (58k, 27 s): approve-with-changes. Carried: #10, `--stat --no-renames`; #11, a homelab manifest backtest row.
- **codex** (high effort): rework. blocking: `git diff` has no `--pathspec-from-file` (verified, git 2.55.0 exit 129), so bounded argv batches. high: `--no-renames` everywhere, with both Delta shapes. high: Kubernetes, Helm and Flux manifests keep `ops`. All applied.
- Tokens: about 257k across the panel, Codex excluded, over the 150k target. A Rust CLI earns four seats under the new rules too, and backend re-bound three times. The seats this plan builds would not have cut this panel.

