//! The batch a Bash call belongs to, read from the session transcript.
//!
//! Read by `cd-carries-over`'s `confirm`. Claude Code writes one assistant
//! line per content block of a model message, all sharing `message.id`. A
//! parallel batch's `tool_use` blocks are therefore siblings in the file, and
//! each sibling's `tool_result` is a later `user` line. Every line's `cwd` is
//! the shell's directory when the line was flushed, so the batch's start is
//! read from the line before the message, never from the message's own.
//! Measured 2026-10-27: when a sibling runs, its own `tool_use` line and the
//! earlier siblings' results are already in the file.
//!
//! Only the tail of the file is read ([`TAIL_BYTES`]): the call being asked
//! about is the newest tool call, so its batch is at the end. A batch that
//! starts before the window, a truncated line or an unreadable file leaves
//! the answer unknown, which the caller treats as "nothing found".
//!
//! Only structured fields are read; text inside a tool result is never taken
//! for a tool call.

use std::io::{Read, Seek, SeekFrom};

use serde_json::Value;

/// How far back from the end the transcript is read.
pub const TAIL_BYTES: u64 = 1024 * 1024;

/// An earlier Bash call of the same assistant message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sibling {
    pub command: String,
    /// Its `tool_result` was found and is not an error.
    pub succeeded: bool,
}

/// What the transcript says about the batch a call belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    /// The `cwd` recorded on the assistant message's line: where the batch
    /// started.
    pub cwd: String,
    /// The Bash calls before this one in the same message, in order.
    pub siblings: Vec<Sibling>,
}

/// The batch of the call `tool_use_id`, or `None` when the transcript does
/// not show it within the tail window.
pub fn batch_of(mut file: impl Read + Seek, tool_use_id: &str) -> Option<Batch> {
    if tool_use_id.is_empty() {
        return None;
    }
    let end = file.seek(SeekFrom::End(0)).ok()?;
    let start = end.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::with_capacity((end - start) as usize);
    file.take(end - start).read_to_end(&mut bytes).ok()?;
    let mut lines = bytes.split(|b| *b == b'\n');
    if start > 0 {
        // The first line begins before the window.
        lines.next();
    }
    let lines: Vec<&str> = lines
        .filter_map(|raw| std::str::from_utf8(raw).ok())
        .filter(|text| !text.trim().is_empty())
        .collect();

    let message_id = lines.iter().rev().find_map(|text| {
        if !text.contains(tool_use_id) {
            return None;
        }
        let entry = parse_assistant(text)?;
        let has_call = blocks(&entry).any(|b| block_id(b) == Some(tool_use_id));
        if !has_call {
            return None;
        }
        Some(entry.get("message")?.get("id")?.as_str()?.to_string())
    })?;

    // A line's `cwd` is stamped when it is flushed, not when it is created:
    // on 2026-10-27 every line of a batch whose first call ran `cd …/relais`
    // carried `…/relais`, the first one included. Where the batch started is
    // the `cwd` of the last line written before the message.
    let mut cwd = None;
    for text in &lines {
        let Ok(entry) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        let id = entry
            .get("message")
            .and_then(|m| m.get("id"))
            .and_then(Value::as_str);
        if id == Some(message_id.as_str()) {
            break;
        }
        if let Some(c) = entry.get("cwd").and_then(Value::as_str) {
            cwd = Some(c.to_string());
        }
    }
    let cwd = cwd?;

    let mut earlier: Vec<(String, String)> = Vec::new();
    'scan: for text in &lines {
        if !text.contains(&message_id) {
            continue;
        }
        let Some(entry) = parse_assistant(text) else {
            continue;
        };
        let id = entry
            .get("message")
            .and_then(|m| m.get("id"))
            .and_then(Value::as_str);
        if id != Some(message_id.as_str()) {
            continue;
        }
        for block in blocks(&entry) {
            let Some(id) = block_id(block) else { continue };
            if id == tool_use_id {
                break 'scan;
            }
            if block.get("name").and_then(Value::as_str) != Some("Bash") {
                continue;
            }
            let command = block
                .get("input")
                .and_then(|i| i.get("command"))
                .and_then(Value::as_str);
            if let Some(command) = command {
                earlier.push((id.to_string(), command.to_string()));
            }
        }
    }

    let mut siblings: Vec<Sibling> = earlier
        .iter()
        .map(|(_, command)| Sibling {
            command: command.clone(),
            succeeded: false,
        })
        .collect();
    for text in &lines {
        if !text.contains("tool_result") {
            continue;
        }
        if !earlier.iter().any(|(id, _)| text.contains(id.as_str())) {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) != Some("user") {
            continue;
        }
        for block in blocks(&entry) {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(slot) = earlier.iter().position(|(id, _)| {
                Some(id.as_str()) == block.get("tool_use_id").and_then(Value::as_str)
            }) else {
                continue;
            };
            let is_error = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            siblings[slot].succeeded = !is_error;
        }
    }
    Some(Batch { cwd, siblings })
}

fn parse_assistant(text: &str) -> Option<Value> {
    let entry: Value = serde_json::from_str(text).ok()?;
    (entry.get("type").and_then(Value::as_str) == Some("assistant")).then_some(entry)
}

fn blocks(entry: &Value) -> impl Iterator<Item = &Value> {
    entry
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// The id of a `tool_use` block.
fn block_id(block: &Value) -> Option<&str> {
    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
        return None;
    }
    block.get("id").and_then(Value::as_str)
}
