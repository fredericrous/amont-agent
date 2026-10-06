# lint-suppression-added: plan reviews

### Round 1
- **backend:** approve-with-changes (32k, 34 s). Findings:
  - whole-line multiset fires on edited suppressed lines (high);
  - silent fragment fallback;
  - no latency budget;
  - script/Rust drift;
  - session_state record order;
  - baseline window not pinned.
- **lang:rust:** approve-with-changes (43k, 51 s). Findings:
  - an unbounded or FIFO read can hang the hook (high);
  - whole-line multiset;
  - no TOML/YAML parser crate, so state the scanner and its false negatives;
  - indented attributes and cfg_attr;
  - session_state test;
  - script drift.
- **unix:** approve-with-changes (37k, 44 s). Findings:
  - bounded read via metadata, is_file and take (high);
  - CRLF;
  - CLAUDE_CONFIG_DIR in the piloted command;
  - script BrokenPipe and exit codes;
  - latency measurement.
- **tui:** approve-with-changes (42k, 57 s). Findings:
  - journal excerpt lacks the marker;
  - exact advise text (prefix, absolute path, fragment form);
  - the script cannot match the whole-file rule, so it is fragment-only with a shared fixture;
  - script CLI parity.

### Round 2
- **backend:** approve-with-changes (36k, 40 s). All 6 round-1 findings resolved. New findings:
  - `git -c` cannot reach the hook, so use GIT_CONFIG_GLOBAL;
  - swap false negative;
  - how line numbers come from counts;
  - the fire-path latency;
  - the MultiEdit fallback fragments.

### Final
- **backend:** approve-with-changes (35k, 35 s). All 5 round-2 findings resolved. New low-cost notes, carried to implementation:
  - ReadFixture only builds big.rs, so add `with_content`;
  - the reported line is approximate when the marker already exists earlier in the file.
