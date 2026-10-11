---
canonical: relais:docs/plans/2026-10-11-quiet-run-live-in-pane.md
phases: [2]
status: done
---
Part of [A quiet run is live in relais's pane](https://github.com/fredericrous/relais/blob/main/docs/plans/2026-10-11-quiet-run-live-in-pane.md).

This repository carries phase 2: while a quiet run is live it writes one active-run entry, `${XDG_STATE_HOME:-~/.local/state}/quiet/active/<pid>-<n>.json` (or `$QUIET_ACTIVE_DIR`), rewrites it with `exit`/`ended` when the run ends and leaves it for relais's watcher or a later sweep; relais's pane tails the run's log from it.

## Observed (phase 2)

| Input | Expected | Observed |
|---|---|---|
| `quiet -- sleep 3` with `CLAUDE_CODE_SESSION_ID=sess-1`, `QUIET_ACTIVE_DIR` a temp dir | entry with the 8 documented fields, 0600 in a 0700 dir; after exit `exit` 0, `ended`, and `log` naming the renamed `.log` | as expected (`an_active_entry_lives_through_the_run_and_stays_after_it`) |
| SIGINT during the run | entry ends with `ended` and `exit` 130, or null if the signal lands before the child exists | as expected (`a_signal_still_ends_the_entry` accepts either) |
| dead-pid entry; `ended` 120 s old; `ended` 1 s old | first two swept by the next run, the third kept | as expected |
| unwritable `QUIET_ACTIVE_DIR` | normal result and exit code, no entry | as expected (`an_unwritable_active_dir_changes_nothing`) |
| `.amont-agent-tmp-*.json` 2 h old / fresh | old one swept, fresh one kept | as expected (`a_killed_writers_temp_file_is_swept_once_old`) |
| no `CLAUDE_CODE_SESSION_ID` | no entry | as expected (`no_session_writes_no_entry`) |
| `quiet -- no-such-command-xyz` | exit 127; any entry carries `exit` 127, not null | as expected (`a_command_that_cannot_start_records_its_exit`) |
| end to end with relais's branch `quiet watch` and a failing `quiet -- job.sh` (4 lines, 1 s apart) | `started` with the session, live `output` per line, `ended` exit 3, entry deleted | as expected (2026-10-11) |

relais run `run-65d9602ab447e-18448` wrote the entry; review findings fixed in the session: the ended entry names the final `.log` (matching amont, so the watcher's last drain reads the real file), `write` returns `io::Result`, a run with no log file writes no entry (documented), and killed writers' temp files are swept after an hour.

Implementation review: round 1 approve-with-changes (6 findings); all fixed in `5647afd` (no entry without a session, real exit on early returns, own-tool sweep, `std::process::id`) and in this record (SIGINT row, contract sentence in relais's plan). This slice is done.
Delta and bind: the one new finding (a vacuous 127 test) fixed in `c5909e9`; final tree **approve**.
