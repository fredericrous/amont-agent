//! Where the parallel batch a Bash call belongs to started.
//!
//! Read by `cd-carries-over`'s `confirm`. The transcript cannot answer it:
//! Claude Code writes a model response's `tool_use` lines and their results
//! only once the whole batch has run (probe, 2026-10-28: at the `PreToolUse`
//! of both calls of a batch the file ended on the response's `api-request`
//! line, with nothing of the batch after it). So the hook keeps the answer
//! itself, in one small file per session and transcript under
//! `<journal dir>/sessions/`:
//!
//! - the batch key: the byte offset of the last `api-request` line, which is
//!   written before the response's tool calls and is the same for every call
//!   of one batch;
//! - the shell's directory at the first call of the batch;
//! - the command of every call of the batch so far, one per line.
//!
//! The first call of a new batch overwrites the file. Everything fails toward
//! silence: an unreadable transcript, no `api-request` line, a file that
//! cannot be read or written all read as "first call of its batch".

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// How far back from the end the transcript is read for the key. The
/// `api-request` line of the running response is among the last lines.
pub const TAIL_BYTES: u64 = 64 * 1024;

const MARKER: &[u8] = b"\"type\":\"api-request\"";

/// The batch a call belongs to, as seen by a later call of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    /// The shell's directory at the first call of the batch.
    pub cwd: String,
    /// The commands of the batch's earlier calls, in order.
    pub earlier: Vec<String>,
}

/// The byte offset of the last `api-request` line in `file`.
pub fn batch_key(mut file: impl Read + Seek) -> Option<u64> {
    let end = file.seek(SeekFrom::End(0)).ok()?;
    let start = end.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::with_capacity((end - start) as usize);
    file.take(end - start).read_to_end(&mut bytes).ok()?;
    let mut line_start = 0usize;
    let mut last = None;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            let line = &bytes[line_start..i];
            if line.windows(MARKER.len()).any(|w| w == MARKER) {
                last = Some(start + line_start as u64);
            }
            line_start = i + 1;
        }
    }
    last
}

/// The state file for a session and the transcript the call is written to.
pub fn state_file(session: &str, transcript: &Path) -> Option<PathBuf> {
    if !crate::session_state::usable_id(session) {
        return None;
    }
    let stem = transcript.file_stem()?.to_str()?;
    if !crate::session_state::usable_id(stem) {
        return None;
    }
    Some(
        crate::journal::dir()?
            .join("sessions")
            .join(format!("{session}.{stem}.batch")),
    )
}

/// Record this call in its batch and return where the batch started, or
/// `None` when this is the batch's first call (or the record is unusable).
pub fn record(state: &Path, key: u64, cwd: &str, command: &str) -> Option<Start> {
    let flat = command.replace('\n', " ");
    let prior = std::fs::read_to_string(state).ok().and_then(|text| {
        let mut lines = text.lines();
        let (k, c) = lines.next()?.split_once('\t')?;
        (k.parse::<u64>().ok()? == key).then(|| Start {
            cwd: c.to_string(),
            earlier: lines.map(str::to_string).collect(),
        })
    });
    let text = match &prior {
        Some(p) => {
            let mut lines = vec![format!("{key}\t{}", p.cwd)];
            lines.extend(p.earlier.iter().cloned());
            lines.push(flat);
            lines.join("\n") + "\n"
        }
        None => format!("{key}\t{cwd}\n{flat}\n"),
    };
    if let Some(dir) = state.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(state, text).ok();
    prior
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn the_key_is_the_last_api_request_line() {
        let text = "{\"type\":\"user\"}\n{\"type\":\"api-request\"}\n{\"type\":\"pr-link\"}\n";
        assert_eq!(batch_key(Cursor::new(text)), Some(16));
        assert_eq!(batch_key(Cursor::new("{\"type\":\"user\"}\n")), None);
    }

    #[test]
    fn later_calls_of_a_batch_see_its_start_and_a_new_key_starts_over() {
        let dir = std::env::temp_dir().join(format!("amont-agent-batch-{}", std::process::id()));
        let state = dir.join("s.t.batch");
        assert_eq!(record(&state, 7, "/w", "cd /w/relais && pwd"), None);
        let start = record(&state, 7, "/w/relais", "pwd").expect("same batch");
        assert_eq!(start.cwd, "/w");
        assert_eq!(start.earlier, vec!["cd /w/relais && pwd".to_string()]);
        let third = record(&state, 7, "/w/relais", "ls").expect("same batch");
        assert_eq!(third.earlier.len(), 2);
        assert_eq!(record(&state, 9, "/w/relais", "pwd"), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
