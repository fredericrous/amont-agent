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
    /// `full`, `offset:limit` (0-based first line, count), `tail:N`.
    pub window: String,
}

/// What the record says about a read that is about to happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prior {
    /// A certain record of the same agent covers this exact window, on a file
    /// that has not changed since: a true repeat, which may be refused.
    Covered(Seen),
    /// A certain record exists, but the new read is not an exact window, so
    /// nothing can be said to repeat it: advice only.
    Partial(Seen),
}

/// The window of a read that cannot be stated exactly.
pub const INEXACT: &str = "?";

/// The most lines the Read tool delivers when it is given no limit.
pub const READ_CAP: u64 = 2000;

/// A session id that is safe to use as a file name.
pub fn usable_id(session: &str) -> bool {
    !(session.is_empty() || session.contains('/') || session.contains(".."))
}

fn file_for(session: &str, agent: &str) -> Option<PathBuf> {
    if !usable_id(session) || !usable_id(agent) || agent.contains(['\\', '\n']) {
        return None;
    }
    Some(
        crate::journal::dir()?
            .join("sessions")
            .join(format!("{session}.{agent}.log")),
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

/// A window as `(start, count)` or `tail:N`, parsed from its record spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Win {
    Full,
    Head { start: u64, count: u64 },
    Tail(u64),
    Inexact,
}

fn parse_window(w: &str) -> Win {
    if w == "full" {
        return Win::Full;
    }
    if let Some(n) = w.strip_prefix("tail:") {
        return n.parse().map_or(Win::Inexact, Win::Tail);
    }
    match w.split_once(':') {
        Some((a, b)) => match (a.parse(), b.parse()) {
            (Ok(start), Ok(count)) => Win::Head { start, count },
            _ => Win::Inexact,
        },
        None => Win::Inexact,
    }
}

/// Whether a record of `have` puts everything `want` would deliver in context.
fn covers(have: Win, want: Win) -> bool {
    match (have, want) {
        (Win::Inexact, _) | (_, Win::Inexact) => false,
        (Win::Full, _) => true,
        (Win::Tail(a), Win::Tail(b)) => b <= a,
        (
            Win::Head { start: s, count: c },
            Win::Head {
                start: s2,
                count: c2,
            },
        ) => s2 >= s && s2.saturating_add(c2) <= s.saturating_add(c),
        _ => false,
    }
}

/// The record spelling of a Read, from the payload's raw window (`full` or
/// `offset:limit`, `-` for no limit; `offset` as the Read tool counts it, from
/// line 1). Normalised to a 0-based first line and a count. An unwindowed Read
/// is `full` only when the file fits in what the tool delivers.
pub fn read_window(path: &Path, raw: &str) -> String {
    if raw == "full" {
        return if line_count(path).is_some_and(|n| n <= READ_CAP) {
            "full".to_string()
        } else {
            format!("0:{READ_CAP}")
        };
    }
    let Some((o, l)) = raw.split_once(':') else {
        return INEXACT.to_string();
    };
    let Ok(o) = o.parse::<u64>() else {
        return INEXACT.to_string();
    };
    let start = o.saturating_sub(1);
    let count = if l == "-" {
        READ_CAP
    } else {
        match l.parse::<u64>() {
            Ok(n) => n,
            Err(_) => return INEXACT.to_string(),
        }
    };
    format!("{start}:{count}")
}

/// Lines in a file, `None` when it cannot be counted cheaply.
fn line_count(path: &Path) -> Option<u64> {
    use std::io::Read;
    let meta = std::fs::metadata(path).ok()?;
    // Over this a file is certainly past the cap in any text a person reads.
    if meta.len() > 64 * 1024 * 1024 {
        return None;
    }
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 64 * 1024];
    let mut lines = 0u64;
    let mut last = b'\n';
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        lines += buf[..n].iter().filter(|b| **b == b'\n').count() as u64;
        last = buf[n - 1];
    }
    if last != b'\n' {
        lines += 1;
    }
    Some(lines)
}

/// What the record of this agent says about reading `window` of `path`: a
/// certain record, of this agent, on a file unchanged since, with no write in
/// between. An uncertain record is skipped, so it never speaks.
pub fn prior(session: &str, agent: &str, path: &Path, window: &str) -> Option<Prior> {
    let text = std::fs::read_to_string(file_for(session, agent)?).ok()?;
    let want = path.to_string_lossy();
    let new = parse_window(window);
    let now = fingerprint(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let mut other: Option<Seen> = None;
    for (i, line) in lines.iter().enumerate().rev() {
        let mut parts = line.splitn(7, ' ');
        let (Some(_seq), Some(kind), Some(bytes), Some(mtime), Some(cert), Some(win), Some(p)) = (
            parts.next(),
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
        if kind != "read" {
            // A write since: what is in context is stale. Older reads are
            // older than that write.
            break;
        }
        let (Ok(bytes), Ok(mtime)) = (bytes.parse::<u64>(), mtime.parse::<u128>()) else {
            continue;
        };
        // Changed, or gone: reading again is the right thing to do.
        if now != (bytes, mtime) || cert != "certain" {
            continue;
        }
        let seen = Seen {
            calls_ago: total - i,
            bytes,
            window: win.to_string(),
        };
        if new != Win::Inexact && covers(parse_window(win), new) {
            return Some(Prior::Covered(seen));
        }
        other.get_or_insert(seen);
    }
    if new == Win::Inexact {
        return other.map(Prior::Partial);
    }
    None
}

/// Remember a file operation. A `read` is stamped with the file as it is
/// NOW, so call this once the read has actually happened, not before.
/// `certain` is whether the whole window is known to be in context.
pub fn record(session: &str, agent: &str, kind: &str, path: &Path, window: &str, certain: bool) {
    let Some(file) = file_for(session, agent) else {
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
    let cert = if certain { "certain" } else { "uncertain" };
    let line = format!(
        "{seq} {kind} {bytes} {mtime} {cert} {window} {}\n",
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

/// The context was compacted: the main conversation no longer holds what it
/// read. Its record is replaced by an empty one, written beside it and renamed
/// over it. Subagents keep theirs.
pub fn compact(session: &str) {
    let Some(file) = file_for(session, "main") else {
        return;
    };
    let Some(dir) = file.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    crate::journal::private(dir, 0o700);
    let tmp = dir.join(format!("{session}.main.log.tmp{}", std::process::id()));
    if std::fs::write(&tmp, b"").is_err() || std::fs::rename(&tmp, &file).is_err() {
        let _ = std::fs::remove_file(&tmp);
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

    fn seen_calls(s: &str, a: &Path, w: &str) -> Option<Prior> {
        prior(s, "main", a, w)
    }

    #[test]
    fn a_read_is_remembered_until_something_writes_the_file() {
        with_home("remembered", |dir| {
            let a = dir.join("a.rs");
            let b = dir.join("b.rs");
            std::fs::write(&a, "x".repeat(1200)).unwrap();
            std::fs::write(&b, "x".repeat(10)).unwrap();
            assert_eq!(seen_calls("s1", &a, "full"), None);
            record("s1", "main", "read", &a, "full", true);
            record("s1", "main", "read", &b, "full", true);
            let Some(Prior::Covered(seen)) = seen_calls("s1", &a, "full") else {
                panic!("a repeat");
            };
            assert_eq!(seen.calls_ago, 2);
            assert_eq!(seen.bytes, 1200);
            record("s1", "main", "write", &a, "full", true);
            assert_eq!(seen_calls("s1", &a, "full"), None);
            record("s1", "main", "read", &a, "10:40", true);
            assert!(matches!(
                seen_calls("s1", &a, "10:40"),
                Some(Prior::Covered(_))
            ));
            // Another session, or another agent, knows nothing.
            assert_eq!(seen_calls("s2", &a, "10:40"), None);
            assert_eq!(prior("s1", "sub-1", &a, "10:40"), None);
        });
    }

    #[test]
    fn windows_cover_by_direction() {
        with_home("windows", |dir| {
            let a = dir.join("a.rs");
            std::fs::write(&a, "x".repeat(1200)).unwrap();
            record("s1", "main", "read", &a, "0:20", true);
            record("s1", "main", "read", &a, "tail:20", true);
            record("s1", "main", "read", &a, "100:20", true);
            assert!(matches!(
                seen_calls("s1", &a, "0:20"),
                Some(Prior::Covered(_))
            ));
            assert!(matches!(
                seen_calls("s1", &a, "0:10"),
                Some(Prior::Covered(_))
            ));
            assert!(matches!(
                seen_calls("s1", &a, "tail:5"),
                Some(Prior::Covered(_))
            ));
            assert_eq!(seen_calls("s1", &a, "0:21"), None);
            assert_eq!(seen_calls("s1", &a, "full"), None);
            assert!(matches!(
                seen_calls("s1", &a, INEXACT),
                Some(Prior::Partial(_))
            ));
        });
    }

    #[test]
    fn an_uncertain_record_never_speaks() {
        with_home("uncertain", |dir| {
            let a = dir.join("a.rs");
            std::fs::write(&a, "x".repeat(1200)).unwrap();
            record("s1", "main", "read", &a, "full", false);
            assert_eq!(seen_calls("s1", &a, "full"), None);
            assert_eq!(seen_calls("s1", &a, INEXACT), None);
        });
    }

    #[test]
    fn compaction_empties_the_main_record_only() {
        with_home("compact", |dir| {
            let a = dir.join("a.rs");
            std::fs::write(&a, "x".repeat(1200)).unwrap();
            record("s1", "main", "read", &a, "full", true);
            record("s1", "sub", "read", &a, "full", true);
            compact("s1");
            assert_eq!(prior("s1", "main", &a, "full"), None);
            assert!(prior("s1", "sub", &a, "full").is_some());
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
            record("s1", "main", "read", &a, "full", true);
            assert!(seen_calls("s1", &a, "full").is_some());
            std::fs::write(&a, "y".repeat(1201)).unwrap();
            assert_eq!(seen_calls("s1", &a, "full"), None);
            record("s1", "main", "read", &a, "full", true);
            assert!(seen_calls("s1", &a, "full").is_some(), "read again");
            std::fs::remove_file(&a).unwrap();
            assert_eq!(seen_calls("s1", &a, "full"), None);
        });
    }

    #[test]
    fn a_hostile_id_is_not_a_path() {
        assert!(file_for("../etc", "main").is_none());
        assert!(file_for("a/b", "main").is_none());
        assert!(file_for("", "main").is_none());
        assert!(file_for("s", "../x").is_none());
        assert!(file_for("s", "").is_none());
    }

    #[test]
    fn read_windows_are_normalised() {
        with_home("rw", |dir| {
            let small = dir.join("s.txt");
            std::fs::write(&small, "a\nb\n").unwrap();
            let big = dir.join("b.txt");
            std::fs::write(&big, "a\n".repeat(3000)).unwrap();
            assert_eq!(read_window(&small, "full"), "full");
            assert_eq!(read_window(&big, "full"), "0:2000");
            assert_eq!(read_window(&big, "2000:-"), "1999:2000");
            assert_eq!(read_window(&big, "0:50"), "0:50");
            assert_eq!(read_window(&big, "101:20"), "100:20");
        });
    }
}
