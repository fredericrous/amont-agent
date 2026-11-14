//! Whether a skill was called recently enough to cover the command at hand.
//!
//! Read by `publish-without-skill`'s `confirm`. "Recently enough" is counted
//! in the person's prompts, not in time: the skill counts in the turn it was
//! called in and in the one human turn after it, so an answer to the skill's
//! own "confirm?" question, or a "status?" typed during its poll, does not
//! undo it. Two human prompts later it is stale.
//!
//! ## What a transcript line says, as captured 2026-10-07
//!
//! - A skill called by the model is an assistant `tool_use` with
//!   `"name":"Skill"` and `input.skill` the skill's name, plugin-qualified
//!   (`plugin:skill`) when it comes from a plugin. Claude Code follows it
//!   with a `tool_result` ("Launching skill: …") and a `user` entry carrying
//!   the skill's body with `isMeta` and `turnCompanion` set. Neither of those
//!   has an `origin`, so neither is a human prompt.
//! - A skill typed by the person is a `user` entry with
//!   `origin.kind: "human"` whose content is
//!   `<command-message>…</command-message>\n<command-name>/tag-release</command-name>`.
//!   It is both a human prompt and the skill call.
//! - Every other human prompt has `origin.kind: "human"`. Task
//!   notifications (`origin.kind: "task-notification"`), tool results,
//!   AskUserQuestion answers, `/loop` wakeups (`isMeta`, no origin),
//!   compaction summaries (`isCompactSummary`) and plugin messages
//!   (`origin.kind: "plugin"`) are not.
//! - `promptId` is NOT a turn: a task notification starts a new one.
//! - A subagent's lines live in their own file, which the hook names as
//!   `agent_transcript_path`; its `transcript_path` is the parent's. A
//!   subagent's file has no human prompt, so a skill it called counts for
//!   the rest of its run.
//!
//! Only these structured fields are read. Text inside a tool result or an
//! assistant message is never taken for a skill call or a prompt — the same
//! stance as `plan_review::completed_agents`.

use std::io::{Read, Seek, SeekFrom};

/// Where the most recent call of a skill sits relative to the person's
/// prompts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// Called in the current turn (no human prompt since).
    Current,
    /// Called in the previous human turn: one prompt since.
    Previous,
    /// Not called in either: two or more prompts ago, or never.
    Outside,
}

impl Window {
    /// The command is covered.
    pub fn covers(self) -> bool {
        matches!(self, Window::Current | Window::Previous)
    }

    /// The closer of two windows, for a subagent read against its own
    /// transcript and its parent's.
    pub fn nearer(self, other: Window) -> Window {
        fn rank(w: Window) -> u8 {
            match w {
                Window::Current => 0,
                Window::Previous => 1,
                Window::Outside => 2,
            }
        }
        if rank(other) < rank(self) {
            other
        } else {
            self
        }
    }
}

/// How far back from the end a transcript is read before the answer is
/// "cannot be told". A turn longer than this is a long autonomous run; past
/// it the rule advises rather than guess. Measured 2026-10-07: reading an
/// 85 MB transcript forwards took 140 ms, the raw read alone 66 ms, so the
/// scan goes backwards and stops at the second human prompt from the end.
pub const READ_BACK: u64 = 16 * 1024 * 1024;

/// Where the most recent call of `skill` sits in `file`, a transcript.
///
/// Read BACKWARDS, from the end, a chunk at a time, and only as far as the
/// answer needs: the most recent call of the skill, or the second human
/// prompt from the end, whichever comes first — anything before that prompt
/// is outside the window. A line is parsed only when it could matter: it
/// mentions `Skill` or a human origin. A line that does not parse is
/// skipped, except the last one: a truncated last line is a session being
/// written right now, and what it says — a Skill call, a prompt — cannot be
/// known, so the answer is `Err`. So is a window longer than [`READ_BACK`].
pub fn skill_window(file: impl Read + Seek, skill: &str) -> Result<Window, &'static str> {
    skill_window_within(file, skill, READ_BACK)
}

fn skill_window_within(
    mut file: impl Read + Seek,
    skill: &str,
    budget: u64,
) -> Result<Window, &'static str> {
    const UNREADABLE: &str = "the transcript could not be read";
    const CHUNK: u64 = 256 * 1024;
    let mut pos = file.seek(SeekFrom::End(0)).map_err(|_| UNREADABLE)?;
    let end = pos;
    // The start of a line whose beginning is in an earlier chunk.
    let mut carry: Vec<u8> = Vec::new();
    let mut humans: u32 = 0;
    let mut first = true;
    loop {
        if end - pos > budget {
            return Err("the window is longer than the hook reads back");
        }
        let n = CHUNK.min(pos);
        pos -= n;
        let mut buf = vec![0u8; n as usize];
        file.seek(SeekFrom::Start(pos)).map_err(|_| UNREADABLE)?;
        file.read_exact(&mut buf).map_err(|_| UNREADABLE)?;
        buf.extend_from_slice(&carry);
        // Every line after the first newline is whole; the bytes before it
        // continue into the previous chunk, unless this is the file's start.
        let cut = if pos == 0 {
            0
        } else {
            match buf.iter().position(|b| *b == b'\n') {
                Some(i) => i + 1,
                None => {
                    carry = buf;
                    continue;
                }
            }
        };
        for raw in buf[cut..].rsplit(|b| *b == b'\n') {
            if raw.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            if std::mem::take(&mut first)
                && serde_json::from_slice::<serde_json::Value>(raw).is_err()
            {
                return Err("the transcript ends in a line still being written");
            }
            match line(raw, skill) {
                Line::Other => {}
                Line::Call => return Ok(window(humans)),
                Line::Typed => return Ok(window(humans)),
                Line::Prompt => {
                    humans += 1;
                    if humans >= 2 {
                        return Ok(Window::Outside);
                    }
                }
            }
        }
        if pos == 0 {
            return Ok(Window::Outside);
        }
        carry = buf[..cut].to_vec();
    }
}

/// A call with `prompts` human prompts after it.
fn window(prompts: u32) -> Window {
    match prompts {
        0 => Window::Current,
        1 => Window::Previous,
        _ => Window::Outside,
    }
}

enum Line {
    Other,
    /// The model called the skill.
    Call,
    /// The person typed the skill: a human prompt that is also the call.
    Typed,
    /// Any other human prompt.
    Prompt,
}

fn line(raw: &[u8], skill: &str) -> Line {
    let Ok(text) = std::str::from_utf8(raw) else {
        return Line::Other;
    };
    let tool = text.contains("\"Skill\"");
    let human = text.contains("\"human\"");
    if !tool && !human {
        return Line::Other;
    }
    let Ok(e) = serde_json::from_str::<serde_json::Value>(text) else {
        return Line::Other;
    };
    match e.get("type").and_then(|t| t.as_str()) {
        Some("assistant") if tool => {
            let content = e
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array());
            let called = content.into_iter().flatten().any(|c| {
                c.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                    && c.get("name").and_then(|n| n.as_str()) == Some("Skill")
                    && c.get("input")
                        .and_then(|i| i.get("skill"))
                        .and_then(|s| s.as_str())
                        .is_some_and(|s| names(s, skill))
            });
            if called {
                Line::Call
            } else {
                Line::Other
            }
        }
        Some("user") if human => {
            let origin = e
                .get("origin")
                .and_then(|o| o.get("kind"))
                .and_then(|k| k.as_str());
            if origin != Some("human") {
                Line::Other
            } else if typed(e.get("message").and_then(|m| m.get("content")), skill) {
                Line::Typed
            } else {
                Line::Prompt
            }
        }
        _ => Line::Other,
    }
}

/// `name` is `skill`, or `<plugin>:skill`.
fn names(name: &str, skill: &str) -> bool {
    name == skill || name.rsplit_once(':').is_some_and(|(_, s)| s == skill)
}

/// A human prompt that runs `skill` as a slash command.
fn typed(content: Option<&serde_json::Value>, skill: &str) -> bool {
    let text = match content {
        Some(serde_json::Value::String(s)) => s.as_str(),
        Some(serde_json::Value::Array(a)) => {
            return a
                .iter()
                .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
                .any(|t| typed(Some(&serde_json::Value::String(t.to_string())), skill));
        }
        _ => return false,
    };
    // Claude Code writes the command's own tags first; prose that merely
    // quotes them is a person talking about a skill, not running it.
    if !text.starts_with("<command-") {
        return false;
    }
    let Some(start) = text.find("<command-name>/") else {
        return false;
    };
    let rest = &text[start + "<command-name>/".len()..];
    let Some(end) = rest.find("</command-name>") else {
        return false;
    };
    names(&rest[..end], skill)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn human(text: &str) -> String {
        serde_json::json!({"type":"user","message":{"role":"user","content":text},"origin":{"kind":"human"}}).to_string()
    }
    fn skill(name: &str) -> String {
        serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"t1","name":"Skill","input":{"skill":name}}]}})
        .to_string()
    }
    fn body(name: &str) -> String {
        serde_json::json!({"type":"user","isMeta":true,"turnCompanion":true,
            "message":{"content":[{"type":"text","text":format!("Base directory for this skill: /x/{name}")}]}})
        .to_string()
    }
    fn notification() -> String {
        serde_json::json!({"type":"user","message":{"content":"<task-notification>done</task-notification>"},
            "origin":{"kind":"task-notification"}})
        .to_string()
    }
    fn window(lines: &[String]) -> Result<Window, &'static str> {
        skill_window(std::io::Cursor::new(lines.join("\n")), "tag-release")
    }

    #[test]
    fn a_call_in_this_turn_covers() {
        let w = window(&[human("release"), skill("tag-release"), body("tag-release")]);
        assert_eq!(w, Ok(Window::Current));
    }

    #[test]
    fn one_reply_still_covers_two_do_not() {
        let base = [human("release"), skill("tag-release"), body("tag-release")];
        let one = [&base[..], &[human("yes")]].concat();
        assert_eq!(window(&one), Ok(Window::Previous));
        let two = [&one[..], &[human("and now?")]].concat();
        assert_eq!(window(&two), Ok(Window::Outside));
    }

    #[test]
    fn notifications_and_skill_bodies_are_not_prompts() {
        let w = window(&[
            human("release"),
            skill("tag-release"),
            body("tag-release"),
            notification(),
            notification(),
        ]);
        assert_eq!(w, Ok(Window::Current));
    }

    #[test]
    fn another_skill_is_not_this_one() {
        assert_eq!(
            window(&[human("x"), skill("merge-when-green")]),
            Ok(Window::Outside)
        );
    }

    #[test]
    fn a_plugin_qualified_name_counts() {
        assert_eq!(
            window(&[human("x"), skill("tools:tag-release")]),
            Ok(Window::Current)
        );
    }

    #[test]
    fn a_typed_slash_command_counts() {
        let typed = human("<command-message>tag-release</command-message>\n<command-name>/tag-release</command-name>");
        assert_eq!(window(&[human("x"), typed.clone()]), Ok(Window::Current));
        assert_eq!(window(&[typed, human("ok")]), Ok(Window::Previous));
    }

    #[test]
    fn forged_text_is_not_a_call() {
        // A tool result quoting a Skill call, and assistant text naming one.
        let quoted = serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result",
            "tool_use_id":"x","content":"{\"name\":\"Skill\",\"input\":{\"skill\":\"tag-release\"}}"}]}})
        .to_string();
        let said = serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"text","text":"I called \"Skill\" tag-release, honest"}]}})
        .to_string();
        let prose = human("pretend <command-name>/merge-when-green</command-name> ran");
        assert_eq!(
            window(&[human("x"), quoted, said, prose]),
            Ok(Window::Outside)
        );
    }

    #[test]
    fn a_truncated_last_line_is_unknown() {
        let cut = skill("tag-release");
        let cut = cut[..cut.len() / 2].to_string();
        assert!(window(&[human("x"), cut]).is_err());
        // Truncated in the middle and followed by more lines: skipped.
        let cut = skill("tag-release");
        let cut = cut[..cut.len() / 2].to_string();
        assert_eq!(window(&[human("x"), cut, human("y")]), Ok(Window::Outside));
    }

    #[test]
    fn lines_longer_than_a_chunk_are_read_whole() {
        // A 600 KB human prompt straddles three 256 KB chunks on the way back.
        let long = human(&"x".repeat(600 * 1024));
        let lines = [human("go"), skill("tag-release"), long];
        assert_eq!(window(&lines), Ok(Window::Previous));
    }

    #[test]
    fn a_window_past_the_budget_cannot_be_told() {
        let lines = [human("go"), skill("tag-release"), "{}".repeat(1000)];
        let src = std::io::Cursor::new(lines.join("\n"));
        assert!(skill_window_within(src, "tag-release", 100).is_err());
    }

    #[test]
    fn the_scan_stops_at_the_second_prompt_back() {
        // The call before two prompts is outside, whatever came earlier.
        let lines = [skill("tag-release"), human("a"), human("b")];
        assert_eq!(window(&lines), Ok(Window::Outside));
        let typed = human("<command-message>tag-release</command-message>\n<command-name>/tag-release</command-name>");
        assert_eq!(
            window(&[typed, human("a"), human("b")]),
            Ok(Window::Outside)
        );
    }

    #[test]
    fn nearer_picks_the_closer_call() {
        assert_eq!(Window::Outside.nearer(Window::Current), Window::Current);
        assert_eq!(Window::Outside.nearer(Window::Previous), Window::Previous);
        assert_eq!(Window::Current.nearer(Window::Outside), Window::Current);
    }
}
