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

/// Raw stdout of a git command run inside `dir`, optionally fed `input` on
/// stdin: bytes as git wrote them, never trimmed or decoded.
///
/// Three answers, because `plan-review-panel` must tell them apart: `Err`
/// is "git could not be asked" (the person is asked instead), `Ok(None)` is
/// git answering no, `Ok(Some)` is the output. [`stdout_in`] folds the
/// first two together and trims, which breaks a `-z` listing whose last
/// name ends in a space.
pub fn bytes_in(
    dir: &std::path::Path,
    args: &[&str],
    input: Option<&[u8]>,
) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Write;
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = retrying(|| cmd.spawn())?;
    // Fed from a thread: `cat-file --batch` writes while it reads, and a
    // pipe filled on both sides at once deadlocks.
    let feeder = match (input, child.stdin.take()) {
        (Some(bytes), Some(mut stdin)) => {
            let bytes = bytes.to_vec();
            Some(std::thread::spawn(move || {
                let _ = stdin.write_all(&bytes);
            }))
        }
        _ => None,
    };
    let out = child.wait_with_output()?;
    if let Some(f) = feeder {
        let _ = f.join();
    }
    Ok(out.status.success().then_some(out.stdout))
}

/// Every path in `tree`, recursively, as raw bytes split on NUL.
/// Submodules are gitlinks, listed and not entered.
pub fn ls_tree_z(dir: &std::path::Path, tree: &str) -> std::io::Result<Option<Vec<Vec<u8>>>> {
    let out = bytes_in(
        dir,
        &[
            "ls-tree",
            "-r",
            "-z",
            "--full-tree",
            "--name-only",
            "--end-of-options",
            tree,
        ],
        None,
    )?;
    Ok(out.map(|b| {
        b.split(|c| *c == 0)
            .filter(|p| !p.is_empty())
            .map(<[u8]>::to_vec)
            .collect()
    }))
}

/// Every entry of `tree`, recursively, as `ls-tree -r -z` prints it: raw
/// `<mode> <type> <oid>\t<path>` records split on NUL, in git's own order.
/// The content of a tree stated without reading any blob, which is what
/// `implementation-review` hashes into a canonical tree id.
pub fn ls_tree_entries(dir: &std::path::Path, tree: &str) -> std::io::Result<Option<Vec<Vec<u8>>>> {
    let out = bytes_in(
        dir,
        &[
            "ls-tree",
            "-r",
            "-z",
            "--full-tree",
            "--end-of-options",
            tree,
        ],
        None,
    )?;
    Ok(out.map(|b| {
        b.split(|c| *c == 0)
            .filter(|p| !p.is_empty())
            .map(<[u8]>::to_vec)
            .collect()
    }))
}

/// Named blobs, as `cat-file --batch` returned them.
pub type Blobs = Vec<(String, Vec<u8>)>;

/// The contents of `names` (each `<tree>:<path>`), in one `cat-file
/// --batch`. A name git reports missing is left out of the answer.
pub fn cat_file_batch(dir: &std::path::Path, names: &[String]) -> std::io::Result<Option<Blobs>> {
    if names.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let mut input = Vec::new();
    for n in names {
        input.extend_from_slice(n.as_bytes());
        input.push(b'\n');
    }
    let Some(out) = bytes_in(dir, &["cat-file", "--batch"], Some(&input))? else {
        return Ok(None);
    };
    let mut found = Vec::new();
    let mut at = 0;
    for name in names {
        let Some(nl) = out[at..].iter().position(|c| *c == b'\n') else {
            break;
        };
        let header = String::from_utf8_lossy(&out[at..at + nl]).into_owned();
        at += nl + 1;
        // `<oid> <type> <size>`, or `<name> missing`.
        let mut parts = header.rsplitn(3, ' ');
        let (Some(size), Some(_kind)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(size) = size.parse::<usize>() else {
            continue;
        };
        if at + size > out.len() {
            break;
        }
        found.push((name.clone(), out[at..at + size].to_vec()));
        at += size + 1;
    }
    Ok(Some(found))
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
