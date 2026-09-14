//! What this session has already read — the one fact `file-reread` needs.
//!
//! One append-only file per session under `<journal dir>/sessions/`, one
//! line per file operation: a sequence number, `read` or `write`, the byte
//! count, the modification time, the window (`full` or `offset:limit`), and
//! the absolute path. Nothing else in the crate keeps state between calls,
//! and this is the narrowest shape that answers the question "was this file
//! already read, and has anything changed it since?". Everything here fails
//! toward silence: a directory that cannot be created, a line that cannot be
//! parsed, a file that cannot be written — all read as "not seen".
//!
//! ## The record is not trusted on its own
//!
//! The Edit and Write tools are recorded, but they are not the only things
//! that write a file: `sed -i`, a formatter, `git checkout`, another session
//! in the same checkout. So a read is recorded together with the file's size
//! and modification time, and [`last_read`] answers "seen" only while the
//! file on disk still matches. A file changed behind the session's back is
//! read again without comment, the same as one the session edited itself.
//!
//! A line from before the fingerprint was recorded has one field fewer and
//! its path lands in the wrong column, so it matches nothing and reads as
//! "not seen" — the safe direction, for the one session that straddles the
//! upgrade.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// How many file operations ago this path was last read.
    pub calls_ago: usize,
    pub bytes: u64,
    /// `full`, or `offset:limit` as the Read tool was given.
    pub window: String,
}

fn file_for(session: &str) -> Option<PathBuf> {
    if session.is_empty() || session.contains('/') || session.contains("..") {
        return None;
    }
    Some(
        crate::journal::dir()?
            .join("sessions")
            .join(format!("{session}.log")),
    )
}

/// Size and modification time, the two facts a write cannot help changing.
/// `None` when the path is not a readable file — which is also the answer
/// when a file the session read has since been removed.
fn fingerprint(path: &Path) -> Option<(u64, u128)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((meta.len(), mtime))
}

/// The last read of `path` in this session with no write to it since — by
/// this session's Edit or Write, or by anything else: the file on disk must
/// still be the size and age it was when it was read.
pub fn last_read(session: &str, path: &Path) -> Option<Seen> {
    let text = std::fs::read_to_string(file_for(session)?).ok()?;
    let want = path.to_string_lossy();
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    for (i, line) in lines.iter().enumerate().rev() {
        let mut parts = line.splitn(6, ' ');
        let (Some(_seq), Some(kind), Some(bytes), Some(mtime), Some(window), Some(p)) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            continue;
        };
        if p != want {
            continue;
        }
        return match kind {
            "read" => {
                let bytes: u64 = bytes.parse().ok()?;
                let mtime: u128 = mtime.parse().ok()?;
                // Changed, or gone: what is in context is stale, and reading
                // again is the right thing to do.
                if fingerprint(path)? != (bytes, mtime) {
                    return None;
                }
                Some(Seen {
                    calls_ago: total - i,
                    bytes,
                    window: window.to_string(),
                })
            }
            // A write since the last read: what is in context is stale, and
            // reading again is the right thing to do.
            _ => None,
        };
    }
    None
}

/// Remember a file operation. A `read` is stamped with the file as it is
/// NOW, so call this once the read has actually happened, not before.
pub fn record(session: &str, kind: &str, path: &Path, window: &str) {
    let Some(file) = file_for(session) else {
        return;
    };
    let Some(dir) = file.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    crate::journal::private(dir, 0o700);
    let seq = std::fs::read_to_string(&file)
        .map(|t| t.lines().count())
        .unwrap_or(0);
    // A session that never ends would grow this without bound; past a few
    // thousand operations the oldest are the least likely to matter.
    if seq > 4000 {
        if let Ok(t) = std::fs::read_to_string(&file) {
            let keep: Vec<&str> = t.lines().rev().take(2000).collect();
            let mut back: Vec<&str> = keep.into_iter().rev().collect();
            back.push("");
            let _ = std::fs::write(&file, back.join("\n"));
        }
    }
    let (bytes, mtime) = fingerprint(path).unwrap_or((0, 0));
    let line = format!(
        "{seq} {kind} {bytes} {mtime} {window} {}\n",
        path.to_string_lossy().replace('\n', " ")
    );
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Session files older than a week are nobody's session any more.
pub fn sweep() {
    let Some(dir) = crate::journal::dir().map(|d| d.join("sessions")) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let week = std::time::Duration::from_secs(7 * 24 * 3600);
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > week);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `CLAUDE_CONFIG_DIR` is process-wide and the test threads are not, so
    /// the tests that set it take turns.
    fn with_home<T>(name: &str, f: impl FnOnce(&Path) -> T) -> T {
        static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let dir =
            std::env::temp_dir().join(format!("amont-agent-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
        let out = f(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn a_read_is_remembered_until_something_writes_the_file() {
        with_home("remembered", |dir| {
            let a = dir.join("a.rs");
            let b = dir.join("b.rs");
            std::fs::write(&a, "x".repeat(1200)).unwrap();
            std::fs::write(&b, "x".repeat(10)).unwrap();
            assert_eq!(last_read("s1", &a), None);
            record("s1", "read", &a, "full");
            record("s1", "read", &b, "full");
            let seen = last_read("s1", &a).unwrap();
            assert_eq!(seen.calls_ago, 2);
            assert_eq!(seen.bytes, 1200);
            record("s1", "write", &a, "full");
            assert_eq!(last_read("s1", &a), None);
            record("s1", "read", &a, "10:40");
            assert_eq!(last_read("s1", &a).unwrap().window, "10:40");
            // Another session knows nothing.
            assert_eq!(last_read("s2", &a), None);
        });
    }

    /// The record is checked against the file, not trusted: a write nobody
    /// told us about — a different size here, so the clock's resolution is
    /// not what the test rests on — makes the read stale, and so does the
    /// file going away.
    #[test]
    fn a_file_changed_on_disk_is_no_longer_seen() {
        with_home("changed", |dir| {
            let a = dir.join("a.rs");
            std::fs::write(&a, "x".repeat(1200)).unwrap();
            record("s1", "read", &a, "full");
            assert!(last_read("s1", &a).is_some());
            std::fs::write(&a, "y".repeat(1201)).unwrap();
            assert_eq!(last_read("s1", &a), None);
            record("s1", "read", &a, "full");
            assert!(last_read("s1", &a).is_some(), "read again, unchanged since");
            std::fs::remove_file(&a).unwrap();
            assert_eq!(last_read("s1", &a), None);
        });
    }

    /// A line written before the fingerprint had a column fewer. It must read
    /// as "not seen", never as a read of some other path.
    #[test]
    fn a_line_from_before_the_fingerprint_is_not_seen() {
        with_home("old-line", |dir| {
            let a = dir.join("a.rs");
            std::fs::write(&a, "x".repeat(1200)).unwrap();
            let file = file_for("s1").unwrap();
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, format!("0 read 1200 full {}\n", a.display())).unwrap();
            assert_eq!(last_read("s1", &a), None);
        });
    }

    #[test]
    fn a_hostile_session_id_is_not_a_path() {
        assert!(file_for("../etc").is_none());
        assert!(file_for("a/b").is_none());
        assert!(file_for("").is_none());
    }
}
