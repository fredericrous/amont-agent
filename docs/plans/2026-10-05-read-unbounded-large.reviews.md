# read-unbounded-large: plan reviews

**Round 1, backend: approve-with-changes (37k, 54 s).** [high] remedy's "say so" contradicts max Deny; [med] KB on disk ≠ what Read loads (2,000-line cap); [med] `reread.is_none()` wrong under observe; [med] verification not observable, wk.py in scratchpad; [low] Flat(6) hides rising tail; [low] pseudocode missing `decision::phrase`/`persisted`. All resolved in round 2.

**Round 1, lang:rust: approve-with-changes (40k, 69 s).** [med] observe double-journal; [med] message order + undefined `persisted`; [low] no `tempfile`, use `std::env::temp_dir`; [low] substring/case-sensitive exemptions → `components()` + `eq_ignore_ascii_case`; [low] `div_ceil` boundary; [low] "one message" isn't a tier invariant. All applied.

**Round 1, tui: approve-with-changes (37k, 63 s).** [high] sketch contradictory + observe gap; [high] deny needs a named escape (`offset`+`limit`); [med] quote the path in the shell remedy; [med] live probes need fresh session ids, assert absence of rule id; [low] case-insensitive media; [low] KB rounding. All applied.

**Round 1, unix: approve-with-changes (34k, 42 s).** [med] KB overstates past 2,000 lines; [low] observe precedence; [low] case-insensitive extensions; [low] probes check rc + empty stderr; [low] document stat-only / FIFO test. All applied.

**Round 2, backend: approve-with-changes (39k, 67 s).** Round-1 items 1–6 resolved. New: [med] use `send_with_global` for the observe test; [low] escape wording → "lines you need"; [low] quote real `status` row. All applied.

**Round 2 rebind, backend: approve (32k, 26 s).** All resolved. [low] Verification-2 probes write to the real journal → run with `CLAUDE_CONFIG_DIR=<scratch>` (carried over to implementation, body not edited). Cosmetic: give the cfg string a trailing `\n` like `ADVISE_FANOUT`.

