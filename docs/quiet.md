# `amont-agent quiet`

```
amont-agent quiet [--max-bytes N[k]] -- <cmd…>
```

Runs `<cmd>` directly (a pipeline is the caller's `sh -c`) with its output in
a log file, and prints:

- on a pass: `ok 12.3s log=<path>`
- on a fail: `exit N 12.3s`, an excerpt of the failure, then `full log: <path>`

The exit code is the command's own. This is the point: `cargo test | tail`
exits with `tail`'s status, so a failing run reads as a pass.

## Exit codes

| code | meaning |
|---|---|
| 2 | usage: missing `--`, no command, `--max-bytes` outside 1 to 48k |
| 126 | the command cannot be executed |
| 127 | the command was not found |
| 128+n | the command died of signal n (`kill -TERM $$` gives 143) |
| other | the command's own code |

## Capture

Stdout and stderr share one pipe, so their order is kept. A reader drains it
continuously into `<journal dir>/logs/<UTC-ts>-<program>.log.tmp` (0600, in a
0700 directory), which can be tailed while the run goes on, and renames it to
`.log` when the command exits. The newest 50 `.log` files are kept; a `*.tmp`
is never pruned.

- stdin is null; the command runs in its own process group.
- SIGINT and SIGTERM are forwarded to that group. An interrupted run prints
  `interrupted, full log: <path>` after the rename, so no `.tmp` is left.
- EPIPE on our stdout is ignored: the wrapper keeps draining and exits with the
  command's code.
- A run still going after 5 s prints `running <program>, log <path>.tmp` on
  stderr; a quicker one prints no such line.
- A storage error (create, write, rename) never stops the draining. The
  excerpt and the real exit code still print, then `log not written: <reason>`,
  or `log left at <path>.tmp` if only the rename failed.
- Under `CI` (set, and not `false` or `0`) a failure prints the whole log
  rather than the excerpt, since the path would vanish with the runner.

## The excerpt

Failure blocks, in order, then the runner's last 12 lines. A block starts at a
line holding a failure word (`error`, `failed`, `failure`, `panicked`,
`assert`, `traceback`, `exception`, `fail`, `✗`, `×`, or a `…Error` type) and
runs to the next blank line or runner header (`---- `, `failures:`,
`test result:`, `=====`, `FAIL `); cargo's whole `failures:` section is
taken. `0 failed`, `no errors found` and passing tests whose name holds
`error` do not start one, and build noise (`Compiling`, `Downloaded`) never
does. Nothing matched: the last 40 lines. The heuristic never decides pass or
fail; the exit code does.

`--max-bytes` sets the excerpt total (default 16k); past it one line
`… N more lines, full log: <path>` replaces the rest. Memory is bounded at
64 KB whatever the output size.

Golden fixtures for the heuristic live in `tests/fixtures/excerpt/`, pinned by
`MANIFEST.sha256`; amont keeps a copy.

## Tests

`cargo test --features test-hooks --test quiet`. The `test-hooks` feature adds
failure injection (`AMONT_AGENT_TEST_LOG_FAIL_AFTER=<bytes>` and a few
reporting variables) that a normal build does not contain.
