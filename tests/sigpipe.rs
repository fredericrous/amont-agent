//! `amont-agent explain … | head` must die quietly, like every other Unix
//! filter.
//!
//! Rust ignores SIGPIPE at startup, so the first `head` that closed the pipe
//! early turned a listing into a panic with a full backtrace — "failed
//! printing to stdout: Broken pipe" — which reads as a crash in a tool whose
//! entire claim is that it is composed and predictable in front of your
//! shell. `main` restores SIGPIPE's default disposition; this test is what
//! stops that line being "simplified" away.
//!
//! It is also the bug that the split introduced by omission: amont's `main`
//! has carried `die_on_sigpipe` since `amont list | head` panicked, and this
//! crate left the workspace with the modules it imported rather than the ones
//! it needed. The release dry run for v2.0.0 found it, from a smoke step that
//! did `amont-agent --help | grep -q`.
//!
//! Unix-only by nature: Windows has no SIGPIPE, and there the closed-pipe
//! write comes back as an `Err` instead of a signal.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

/// A transcript with enough matching tool calls that the writer cannot
/// possibly finish before `head` exits.
///
/// The size is the point, and amont's equivalent test says why: the panic is
/// racy to reproduce with small output, because the whole thing may fit the
/// kernel's pipe buffer and be written before the reader hangs up. A
/// regression test that only sometimes meets the condition it guards is a
/// coin, not a test.
fn big_transcript() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("amont-agent-sigpipe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let mut out = String::new();
    for i in 0..4000 {
        // `timestamp` is NOT decoration: entries without one are skipped
        // before the tool-call blocks are even looked at, so a fixture
        // missing it parses to zero calls and prints almost nothing — which
        // is how the first version of this test passed against the very bug
        // it was written to catch.
        let day = i % 28 + 1;
        out.push_str(&format!(
            r#"{{"sessionId":"s","cwd":"/tmp","timestamp":"2026-08-{day:02}T10:00:00.000Z","type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t{i}","name":"Bash","input":{{"command":"git push origin branch-{i} 2>&1 | tail -5"}}}}]}}}}"#
        ));
        out.push('\n');
    }
    std::fs::write(dir.join("session.jsonl"), out).expect("write transcript");
    dir
}

#[test]
fn a_closed_pipe_kills_a_listing_quietly() {
    let dir = big_transcript();
    let bin = env!("CARGO_BIN_EXE_amont-agent");
    let err = dir.join("stderr.txt");
    let status = dir.join("status.txt");

    // Two things this shell line gets right that the obvious version does
    // not, both learned the hard way:
    //
    //   * stderr is redirected to a FILE, not folded into the pipe. Writing
    //     `2>&1 | head -1` sends the panic message into the same pipe `head`
    //     is about to close, so `head` eats the very evidence the test is
    //     looking for — and the test passes against the bug.
    //   * the WRITER's exit status is captured, not the pipeline's. A
    //     pipeline reports its LAST command's status, so `head` succeeding
    //     reports 0 however the binary died. (This is, with some irony, the
    //     exact mistake `pipe-to-tail` exists to refuse.) `$?` inside the
    //     group, written to a file, is the POSIX way to get at it — no
    //     PIPESTATUS, which is a bashism.
    let script = format!(
        "{{ '{}' explain pipe-to-tail --transcripts '{}' --sample 4000 2>'{}'; echo $? > '{}'; }} | head -1",
        bin,
        dir.display(),
        err.display(),
        status.display()
    );

    // The guard on the guard: if this listing ever stops being bigger than a
    // pipe buffer, the race never happens and the test below stops testing
    // anything. It must say so rather than going quietly green.
    let full = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "'{}' explain pipe-to-tail --transcripts '{}' --sample 4000",
            bin,
            dir.display()
        ))
        .output()
        .expect("run the listing");
    assert!(
        full.stdout.len() > 128 * 1024,
        "the fixture must out-write a pipe buffer or the race never happens; \
         got {} bytes",
        full.stdout.len()
    );

    Command::new("sh")
        .arg("-c")
        .arg(&script)
        .output()
        .expect("run the pipeline");

    let code: i32 = std::fs::read_to_string(&status)
        .expect("the writer recorded its status")
        .trim()
        .parse()
        .expect("a numeric status");
    let stderr = std::fs::read_to_string(&err).unwrap_or_default();

    assert_ne!(
        code, 101,
        "101 is a Rust panic: the binary blew up writing to a closed pipe \
         instead of dying from the signal.\n{stderr}"
    );
    assert!(
        !stderr.contains("Broken pipe") && !stderr.contains("panicked"),
        "a closed pipe must not be reported as a failure to print:\n{stderr}"
    );
    assert_eq!(
        code, 141,
        "128 + SIGPIPE(13) is what a Unix filter exits with when its reader \
         hangs up.\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The same for the small, always-available listing.
///
/// OPPORTUNISTIC, not a guard, and the difference is worth stating: `rules`
/// prints well under a pipe buffer, so on most machines the writer finishes
/// before the reader hangs up and this passes whether or not the fix is
/// there. It earns its place because it is the shape people actually type,
/// and because it is free — but the test that would fail without
/// `die_on_sigpipe` is the one above.
#[test]
fn rules_survives_a_closed_pipe() {
    let bin = env!("CARGO_BIN_EXE_amont-agent");
    for reader in ["head -1", "grep -q pipe-to-tail"] {
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("'{bin}' rules 2>&1 | {reader}"))
            .output()
            .expect("run the pipeline");
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !combined.contains("Broken pipe") && !combined.contains("panicked"),
            "`rules | {reader}`:\n{combined}"
        );
    }
}

/// And `--help`, the very first thing anybody runs, and the exact command
/// whose `| grep -q backtest` failed the v2.0.0 release dry run on
/// aarch64-apple-darwin.
///
/// Opportunistic for the same reason as above — 1.5KB against a 64KB buffer
/// — which is precisely why it failed on ONE of six build targets and not
/// the rest. A race that shows up on one runner is still a bug on all of
/// them.
#[test]
fn help_survives_a_closed_pipe() {
    let bin = env!("CARGO_BIN_EXE_amont-agent");
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!("'{bin}' --help 2>&1 | grep -q backtest"))
        .output()
        .expect("run the pipeline");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.is_empty(), "expected silence, got:\n{combined}");
    assert!(
        out.status.success(),
        "`--help | grep -q backtest` must pass"
    );
}
