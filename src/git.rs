//! Thin wrappers over the `git` calls this guard makes.
//!
//! Vendored from `amont-runtime`, trimmed to the three shapes this crate
//! needs: a config read that keeps its exit codes apart ([`output`]), a
//! question asked inside a named directory ([`stdout_in`]), and an action
//! whose only interesting result is whether it worked ([`succeeds_in`]).
//!
//! Everything here runs in a `SessionStart` or `PreToolUse` hook, i.e. in
//! front of somebody who is waiting. Nothing blocks without a bound, and
//! every failure to ASK is treated as "no opinion" rather than as an answer.

use std::process::{Command, Stdio};

/// Run `cmd`, retrying the transient SPAWN failures a loaded machine
/// produces: EINTR, EAGAIN (fork pressure), ETXTBSY (another thread's
/// fork-to-exec window still holding a write descriptor on the executable).
/// A NON-ZERO EXIT IS NEVER RETRIED — that is git answering; this covers
/// only "git could not be asked".
///
/// A machine running several coding agents at once is exactly the loaded
/// machine this protects against, and a single failed fork here turns a
/// stance lookup into a silent default.
fn retrying<T>(mut attempt: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut delay = std::time::Duration::from_millis(10);
    for tries_left in [2u8, 1, 0] {
        match attempt() {
            Err(e) if tries_left > 0 && transient(&e) => {
                std::thread::sleep(delay);
                delay *= 3;
            }
            other => return other,
        }
    }
    unreachable!("the zero-tries arm returns")
}

/// The retryable kinds, matched on raw OS codes because the precise
/// `io::ErrorKind` variants (`ExecutableFileBusy`, `ResourceBusy`) are not
/// stable at this crate's MSRV: EINTR(4), EAGAIN(11 linux / 35 mac),
/// ETXTBSY(26).
fn transient(e: &std::io::Error) -> bool {
    if matches!(
        e.kind(),
        std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
    ) {
        return true;
    }
    matches!(e.raw_os_error(), Some(4 | 11 | 26 | 35))
}

/// stdout of a git command run inside `dir`.
///
/// `-C dir`, not `current_dir`: this process may be answering about a
/// repository it is not standing in — the payload names a `cwd` and the
/// answer must be the one git would give THERE, since config is
/// per-repository.
pub fn stdout_in(dir: &std::path::Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args).stderr(Stdio::null());
    let out = retrying(|| cmd.output()).ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Did a git command run inside `dir` succeed? Output discarded; `false`
/// covers "git could not be asked" too.
pub fn succeeds_in(dir: &std::path::Path, args: &[&str]) -> bool {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    retrying(|| cmd.status())
        .map(|s| s.success())
        .unwrap_or(false)
}

pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// A git command's full result, for the caller that must tell one kind of
/// failure from another.
///
/// [`stdout_in`] collapses every non-zero exit to `None` and discards stderr,
/// which is the right shape for "cannot tell, do not block". It is the wrong
/// shape for reading configuration: `git config --get` exits **1** for a key
/// nobody set and **128** for a key set to something git itself refuses to
/// parse, and those two must not become the same answer — one is a default,
/// the other is a mistake somebody needs to be told about. See `gitconfig`.
pub fn output(args: &[&str]) -> Option<Output> {
    let mut cmd = Command::new("git");
    cmd.args(args).stdin(Stdio::null());
    let out = retrying(|| cmd.output()).ok()?;
    Some(Output {
        // A process killed by a signal has no code. Treat that as "git did
        // not answer" rather than inventing one; the caller falls back.
        code: out.status.code()?,
        stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_covers_the_fork_pressure_kinds_and_nothing_else() {
        for code in [4, 11, 26, 35] {
            assert!(
                transient(&std::io::Error::from_raw_os_error(code)),
                "raw {code} is a loaded-machine hiccup"
            );
        }
        assert!(transient(&std::io::Error::from(
            std::io::ErrorKind::Interrupted
        )));
        assert!(!transient(&std::io::Error::from(
            std::io::ErrorKind::NotFound
        )));
        assert!(!transient(&std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        )));
    }
}
