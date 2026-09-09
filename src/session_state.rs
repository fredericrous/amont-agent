//! What this session has already read — the one fact `file-reread` needs.
//!
//! One append-only file per session under `<journal dir>/sessions/`, one
//! line per file operation: a sequence number, `read` or `write`, the byte
//! count, the window (`full` or `offset:limit`), and the absolute path.
//! Nothing else in the crate keeps state between calls, and this is the
//! narrowest shape that answers the question "was this file already read,
//! and has anything written it since?". Everything here fails toward
//! silence: a directory that cannot be created, a line that cannot be
//! parsed, a file that cannot be written — all read as "not seen".

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

/// The last read of `path` in this session with no write to it since.
pub fn last_read(session: &str, path: &Path) -> Option<Seen> {
    let text = std::fs::read_to_string(file_for(session)?).ok()?;
    let want = path.to_string_lossy();
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    for (i, line) in lines.iter().enumerate().rev() {
        let mut parts = line.splitn(5, ' ');
        let (Some(_seq), Some(kind), Some(bytes), Some(window), Some(p)) = (
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
            "read" => Some(Seen {
                calls_ago: total - i,
                bytes: bytes.parse().unwrap_or(0),
                window: window.to_string(),
            }),
            // A write since the last read: what is in context is stale, and
            // reading again is the right thing to do.
            _ => None,
        };
    }
    None
}

pub fn record(session: &str, kind: &str, path: &Path, bytes: u64, window: &str) {
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
    let line = format!(
        "{seq} {kind} {bytes} {window} {}\n",
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

    fn with_home<T>(f: impl FnOnce() -> T) -> T {
        let dir = std::env::temp_dir().join(format!("amont-agent-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", &dir);
        let out = f();
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn a_read_is_remembered_until_something_writes_the_file() {
        with_home(|| {
            let p = Path::new("/tmp/x/a.rs");
            assert_eq!(last_read("s1", p), None);
            record("s1", "read", p, 1200, "full");
            record("s1", "read", Path::new("/tmp/x/b.rs"), 10, "full");
            let seen = last_read("s1", p).unwrap();
            assert_eq!(seen.calls_ago, 2);
            assert_eq!(seen.bytes, 1200);
            record("s1", "write", p, 0, "full");
            assert_eq!(last_read("s1", p), None);
            record("s1", "read", p, 1300, "10:40");
            assert_eq!(last_read("s1", p).unwrap().window, "10:40");
            // Another session knows nothing.
            assert_eq!(last_read("s2", p), None);
        });
    }

    #[test]
    fn a_hostile_session_id_is_not_a_path() {
        assert!(file_for("../etc").is_none());
        assert!(file_for("a/b").is_none());
        assert!(file_for("").is_none());
    }
}
