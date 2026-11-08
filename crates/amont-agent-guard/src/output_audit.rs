//! Where tool output goes: a reproducible aggregate of the bytes tool
//! results put into a session's context over a fixed UTC window, by kind of
//! call, beside the token usage the transcripts report (deliberate-output
//! plan, Phase 2).
//!
//! What is counted, and what is not:
//! - **Result bytes** are the UTF-8 bytes of a tool result's text, as the
//!   model received it. Non-text blocks (images) are counted, not sized.
//! - **Usage** is what the assistant messages report (`input_tokens`,
//!   `cache_creation_input_tokens`, `cache_read_input_tokens`,
//!   `output_tokens`). It is not billing, and neither it nor the bytes is a
//!   quota figure.
//! - A result is counted once per `tool_use_id` and a message's usage once
//!   per `message.id`: a resumed session copies its history into a new file,
//!   and a streamed message repeats its usage on every snapshot.
//! - A line is in the window when its `timestamp` (UTC, `Z`) is in
//!   `[since, until)`. A line with no readable timestamp is counted and left
//!   out.
//! - **Over 200 B** is the share of a call's passing result beyond 200 bytes:
//!   what a one-line quiet summary would not have carried. It is a proxy for
//!   avoidable output, not a measure of it.
//!
//! Output carries no command text and no result text: only counts.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::transcript::{files, Roots};

/// A line longer than this is counted, not parsed.
const MAX_LINE: usize = 16 * 1024 * 1024;

/// What a one-line quiet summary is allowed to carry.
pub const SUMMARY_BYTES: u64 = 200;

/// A UTC instant as transcripts write it, normalised so two instants compare
/// as strings: `YYYY-MM-DDTHH:MM:SS.mmmZ`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant(String);

impl Instant {
    /// `YYYY-MM-DDTHH:MM:SS[.f…]Z`; any other shape (an offset, a missing
    /// `Z`) is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let bytes = text.as_bytes();
        let digits = |range: std::ops::Range<usize>| {
            bytes
                .get(range.clone())
                .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
        };
        let shaped = text.len() >= 20
            && digits(0..4)
            && bytes[4] == b'-'
            && digits(5..7)
            && bytes[7] == b'-'
            && digits(8..10)
            && bytes[10] == b'T'
            && digits(11..13)
            && bytes[13] == b':'
            && digits(14..16)
            && bytes[16] == b':'
            && digits(17..19)
            && text.ends_with('Z');
        if !shaped {
            return None;
        }
        let fraction = &text[19..text.len() - 1];
        let millis = match fraction.strip_prefix('.') {
            None if fraction.is_empty() => "000".to_string(),
            Some(f) if !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()) => {
                format!("{:0<3}", &f[..f.len().min(3)])
            }
            _ => return None,
        };
        Some(Self(format!("{}.{millis}Z", &text[..19])))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Seconds since the Unix epoch, for comparing with a file's mtime.
    fn unix_seconds(&self) -> i64 {
        let text = &self.0;
        let field = |range: std::ops::Range<usize>| text[range].parse::<i64>().unwrap_or(0);
        let days = crate::civil::days_from_civil(field(0..4), field(5..7), field(8..10));
        days * 86_400 + field(11..13) * 3_600 + field(14..16) * 60 + field(17..19)
    }
}

/// The half-open window `[since, until)`.
#[derive(Debug, Clone)]
pub struct Window {
    pub since: Instant,
    pub until: Instant,
}

/// One kind of call's totals.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Bucket {
    pub calls: u64,
    pub ok: u64,
    pub failed: u64,
    pub result_bytes: u64,
    /// Bytes of passing results beyond [`SUMMARY_BYTES`] each.
    pub over_summary: u64,
    /// Results with a non-text block (an image).
    pub non_text: u64,
}

/// Token usage as the assistant messages report it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Usage {
    pub messages: u64,
    pub input: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub output: u64,
}

/// The aggregate over a window.
#[derive(Debug, Default)]
pub struct Audit {
    pub files_scanned: u64,
    /// Files whose last write is before the window: they cannot hold a line
    /// in it.
    pub files_skipped: u64,
    pub lines: u64,
    pub bad_lines: u64,
    pub oversized_lines: u64,
    pub untimed_lines: u64,
    pub sessions: HashSet<String>,
    /// Results whose `tool_use_id` was already counted.
    pub duplicate_results: u64,
    /// Results with no matching call in their file.
    pub unmatched_results: u64,
    pub agent_launches: u64,
    /// By category, for the main conversation.
    pub main: BTreeMap<&'static str, Bucket>,
    /// By category, inside subagents (`isSidechain`): their context, not the
    /// parent's.
    pub sidechain: BTreeMap<&'static str, Bucket>,
    pub usage: BTreeMap<String, Usage>,
}

/// The kind of call a tool use is, for the report. Ordered rules over the
/// tool name and, for Bash, the command text.
pub fn category(tool: &str, command: &str) -> &'static str {
    match tool {
        "Bash" => bash_category(command),
        "Read" => "read",
        "Grep" | "Glob" => "search",
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => "edit",
        "Agent" | "Task" => "agent",
        "LSP" => "lsp",
        "WebFetch" | "WebSearch" => "web",
        name if name.starts_with("mcp__") => "mcp",
        _ => "other-tool",
    }
}

fn bash_category(command: &str) -> &'static str {
    let has = |needle: &str| command.contains(needle);
    let gh_run = has("gh run view") || has("gh run download");
    if (gh_run && (has("--log") || has("download")))
        || (has("gh api") && has("/logs"))
        || (has("actions/runs") && has("/logs"))
    {
        return "ci-log";
    }
    if gh_run || has("gh run list") || has("gh run watch") || has("gh pr checks") {
        return "ci-status";
    }
    if has("amont-agent quiet") {
        return "check-quiet";
    }
    const CHECKS: [&str; 18] = [
        "cargo test",
        "cargo build",
        "cargo clippy",
        "cargo check",
        "make check",
        "make test",
        "make lint",
        "npm test",
        "npm run test",
        "pnpm test",
        "pnpm run test",
        "pnpm exec vitest",
        "vitest",
        "pytest",
        "go test",
        "amont run",
        "amont land",
        "amont rehearse",
    ];
    if CHECKS.iter().any(|check| has(check)) {
        return "check";
    }
    if has("git commit") || has("git push") {
        return "check-git-hook";
    }
    if command.trim_start().starts_with("git ") || has("git -C") {
        return "git";
    }
    "shell"
}

/// The text bytes of a tool result's `content`, and whether it held a
/// non-text block.
fn result_size(content: &serde_json::Value) -> (u64, bool) {
    match content {
        serde_json::Value::String(text) => (text.len() as u64, false),
        serde_json::Value::Array(blocks) => {
            blocks.iter().fold((0, false), |(bytes, other), block| {
                match block.get("type").and_then(serde_json::Value::as_str) {
                    Some("text") => (
                        bytes
                            + block
                                .get("text")
                                .and_then(serde_json::Value::as_str)
                                .map_or(0, |t| t.len() as u64),
                        other,
                    ),
                    _ => (bytes, true),
                }
            })
        }
        _ => (0, false),
    }
}

/// Walks every transcript under `roots` once and aggregates the lines in
/// `window`.
pub fn audit(roots: &Roots, window: &Window) -> Audit {
    let mut audit = Audit::default();
    let mut counted: HashSet<String> = HashSet::new();
    let mut messages: HashMap<String, (String, Usage)> = HashMap::new();
    let since = window.since.unix_seconds();
    for path in files(roots) {
        if modified_before(&path, since) {
            audit.files_skipped += 1;
            continue;
        }
        audit.files_scanned += 1;
        audit_file(&path, window, &mut audit, &mut counted, &mut messages);
    }
    for (model, usage) in messages.into_values() {
        let total = audit.usage.entry(model).or_default();
        total.messages += 1;
        total.input += usage.input;
        total.cache_creation += usage.cache_creation;
        total.cache_read += usage.cache_read;
        total.output += usage.output;
    }
    audit
}

fn modified_before(path: &Path, since: i64) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .is_some_and(|age| (age.as_secs() as i64) < since)
}

fn audit_file(
    path: &PathBuf,
    window: &Window,
    audit: &mut Audit,
    counted: &mut HashSet<String>,
    messages: &mut HashMap<String, (String, Usage)>,
) {
    let Ok(file) = fs::File::open(path) else {
        return;
    };
    // The calls of this file: tool_use id → category.
    let mut calls: HashMap<String, &'static str> = HashMap::new();
    let mut reader = BufReader::new(file);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        audit.lines += 1;
        if raw.len() > MAX_LINE {
            audit.oversized_lines += 1;
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<serde_json::Value>(&raw) else {
            audit.bad_lines += 1;
            continue;
        };
        let message = entry.get("message");
        let blocks = message
            .and_then(|m| m.get("content"))
            .and_then(serde_json::Value::as_array);
        // Calls are recorded whatever their time: a result in the window may
        // answer a call made just before it.
        for block in blocks.into_iter().flatten() {
            if block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use") {
                if let Some(id) = block.get("id").and_then(serde_json::Value::as_str) {
                    let tool = block
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    let command = block
                        .get("input")
                        .and_then(|input| input.get("command"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    calls.insert(id.to_string(), category(tool, command));
                }
            }
        }
        let Some(at) = entry
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .and_then(Instant::parse)
        else {
            audit.untimed_lines += 1;
            continue;
        };
        if at < window.since || at >= window.until {
            continue;
        }
        if let Some(session) = entry.get("sessionId").and_then(serde_json::Value::as_str) {
            audit.sessions.insert(session.to_string());
        }
        let sidechain = entry
            .get("isSidechain")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let role = message
            .and_then(|m| m.get("role"))
            .and_then(serde_json::Value::as_str);
        if role == Some("assistant") {
            record_usage(message, messages);
        }
        for block in blocks.into_iter().flatten() {
            match block.get("type").and_then(serde_json::Value::as_str) {
                Some("tool_use") => {
                    let name = block.get("name").and_then(serde_json::Value::as_str);
                    if matches!(name, Some("Agent" | "Task")) && !sidechain {
                        audit.agent_launches += 1;
                    }
                }
                Some("tool_result") => {
                    let Some(id) = block.get("tool_use_id").and_then(serde_json::Value::as_str)
                    else {
                        continue;
                    };
                    if !counted.insert(id.to_string()) {
                        audit.duplicate_results += 1;
                        continue;
                    }
                    let kind = calls.get(id).copied().unwrap_or_else(|| {
                        audit.unmatched_results += 1;
                        "unmatched"
                    });
                    let (bytes, non_text) =
                        result_size(block.get("content").unwrap_or(&serde_json::Value::Null));
                    let failed = block
                        .get("is_error")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    let bucket = if sidechain {
                        audit.sidechain.entry(kind).or_default()
                    } else {
                        audit.main.entry(kind).or_default()
                    };
                    bucket.calls += 1;
                    bucket.result_bytes += bytes;
                    bucket.non_text += u64::from(non_text);
                    if failed {
                        bucket.failed += 1;
                    } else {
                        bucket.ok += 1;
                        bucket.over_summary += bytes.saturating_sub(SUMMARY_BYTES);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Keeps the last usage a message reported: a streamed message repeats it on
/// every snapshot, and the last one is final.
fn record_usage(
    message: Option<&serde_json::Value>,
    messages: &mut HashMap<String, (String, Usage)>,
) {
    let Some(message) = message else {
        return;
    };
    let Some(id) = message.get("id").and_then(serde_json::Value::as_str) else {
        return;
    };
    let Some(usage) = message.get("usage") else {
        return;
    };
    let field = |name: &str| {
        usage
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let model = message
        .get("model")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    messages.insert(
        id.to_string(),
        (
            model,
            Usage {
                messages: 1,
                input: field("input_tokens"),
                cache_creation: field("cache_creation_input_tokens"),
                cache_read: field("cache_read_input_tokens"),
                output: field("output_tokens"),
            },
        ),
    );
}

impl Audit {
    /// The aggregate as JSON: counts only, no command or result text.
    pub fn to_json(&self, window: &Window) -> serde_json::Value {
        let buckets = |map: &BTreeMap<&'static str, Bucket>| {
            serde_json::Value::Object(
                map.iter()
                    .map(|(kind, b)| {
                        (
                            (*kind).to_string(),
                            serde_json::json!({
                                "calls": b.calls, "ok": b.ok, "failed": b.failed,
                                "result_bytes": b.result_bytes,
                                "over_summary_bytes": b.over_summary,
                                "non_text_results": b.non_text,
                            }),
                        )
                    })
                    .collect(),
            )
        };
        let usage = serde_json::Value::Object(
            self.usage
                .iter()
                .map(|(model, u)| {
                    (
                        model.clone(),
                        serde_json::json!({
                            "messages": u.messages, "input_tokens": u.input,
                            "cache_creation_input_tokens": u.cache_creation,
                            "cache_read_input_tokens": u.cache_read,
                            "output_tokens": u.output,
                        }),
                    )
                })
                .collect(),
        );
        serde_json::json!({
            "window": {"since": window.since.as_str(), "until": window.until.as_str()},
            "definitions": {
                "result_bytes": "UTF-8 bytes of tool-result text, once per tool_use_id",
                "over_summary_bytes": format!("bytes of passing results beyond {SUMMARY_BYTES} B each: a proxy for avoidable output"),
                "usage": "assistant-reported tokens, once per message.id; not billing, not quota",
            },
            "coverage": {
                "files_scanned": self.files_scanned, "files_skipped_before_window": self.files_skipped,
                "lines": self.lines, "bad_lines": self.bad_lines,
                "oversized_lines": self.oversized_lines, "untimed_lines": self.untimed_lines,
                "duplicate_results": self.duplicate_results,
                "unmatched_results": self.unmatched_results,
            },
            "sessions": self.sessions.len(),
            "agent_launches": self.agent_launches,
            "main": buckets(&self.main),
            "sidechain": buckets(&self.sidechain),
            "usage_by_model": usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Window {
        Window {
            since: Instant::parse("2026-11-01T00:00:00Z").unwrap(),
            until: Instant::parse("2026-11-02T00:00:00Z").unwrap(),
        }
    }

    fn roots_with(name: &str, lines: &[String]) -> (Roots, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "amont-agent-output-audit-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir); // absent on a first run, which is fine
        fs::create_dir_all(dir.join("p")).unwrap();
        fs::write(dir.join("p").join("s.jsonl"), lines.join("\n") + "\n").unwrap();
        (
            Roots {
                dirs: vec![dir.clone()],
                how: crate::transcript::Discovery::Explicit,
            },
            dir,
        )
    }

    fn call(at: &str, id: &str, tool: &str, command: &str) -> String {
        serde_json::json!({"timestamp": at, "sessionId": "s1", "message": {"role": "assistant",
            "content": [{"type": "tool_use", "id": id, "name": tool, "input": {"command": command}}]}})
        .to_string()
    }

    #[derive(Clone, Copy)]
    enum Ended {
        Passed,
        Failed,
    }

    fn result(at: &str, id: &str, text: &str, ended: Ended) -> String {
        let error = matches!(ended, Ended::Failed);
        serde_json::json!({"timestamp": at, "sessionId": "s1", "message": {"role": "user",
            "content": [{"type": "tool_result", "tool_use_id": id, "content": text, "is_error": error}]}})
        .to_string()
    }

    #[test]
    fn instants_normalise_and_compare_half_open() {
        let window = window();
        let at = |t: &str| Instant::parse(t).unwrap();
        assert_eq!(at("2026-11-01T00:00:00Z"), at("2026-11-01T00:00:00.000Z"));
        assert_eq!(
            at("2026-11-01T00:00:00.5Z").as_str(),
            "2026-11-01T00:00:00.500Z"
        );
        assert!(at("2026-11-01T00:00:00Z") >= window.since);
        assert!(
            at("2026-11-02T00:00:00Z") >= window.until,
            "until is excluded"
        );
        assert!(Instant::parse("2026-11-01T00:00:00+01:00").is_none());
        assert!(Instant::parse("yesterday").is_none());
    }

    #[test]
    fn bash_commands_fall_in_hand_checked_categories() {
        assert_eq!(category("Bash", "gh run view 1 --log-failed"), "ci-log");
        assert_eq!(
            category("Bash", "gh api repos/o/r/actions/jobs/1/logs"),
            "ci-log"
        );
        assert_eq!(
            category("Bash", "gh run view 1 --json conclusion"),
            "ci-status"
        );
        assert_eq!(category("Bash", "gh pr checks 3"), "ci-status");
        assert_eq!(
            category("Bash", "amont-agent quiet -- cargo test"),
            "check-quiet"
        );
        assert_eq!(category("Bash", "cargo test -p x"), "check");
        assert_eq!(category("Bash", "git commit -m x"), "check-git-hook");
        assert_eq!(category("Bash", "git status"), "git");
        assert_eq!(category("Bash", "ls"), "shell");
        assert_eq!(category("mcp__aval__aval_rule", ""), "mcp");
        assert_eq!(category("Read", ""), "read");
    }

    #[test]
    fn results_are_sized_once_and_bucketed_by_their_call() {
        let long = "x".repeat(1_000);
        let lines = vec![
            call("2026-11-01T01:00:00Z", "a", "Bash", "cargo test"),
            result("2026-11-01T01:00:01Z", "a", &long, Ended::Passed),
            // A resumed session repeats the same result: counted once.
            result("2026-11-01T01:00:01Z", "a", &long, Ended::Passed),
            call("2026-11-01T02:00:00Z", "b", "Bash", "cargo test"),
            result("2026-11-01T02:00:01Z", "b", "boom", Ended::Failed),
            // Outside the window: not counted.
            call("2026-11-02T01:00:00Z", "c", "Read", ""),
            result("2026-11-02T01:00:01Z", "c", &long, Ended::Passed),
            "not json".to_string(),
        ];
        let (roots, dir) = roots_with("bucket", &lines);
        let audit = audit(&roots, &window());
        let check = audit.main.get("check").unwrap();
        assert_eq!((check.calls, check.ok, check.failed), (2, 1, 1));
        assert_eq!(check.result_bytes, 1_004);
        assert_eq!(check.over_summary, 800);
        assert_eq!(audit.duplicate_results, 1);
        assert_eq!(audit.bad_lines, 1);
        assert!(!audit.main.contains_key("read"));
        assert_eq!(audit.sessions.len(), 1);
        let _ = fs::remove_dir_all(dir); // best effort: a temp dir
    }

    #[test]
    fn streamed_usage_counts_once_and_sidechains_apart() {
        let usage = |out: u64| {
            serde_json::json!({"timestamp": "2026-11-01T03:00:00Z", "sessionId": "s1",
                "message": {"role": "assistant", "id": "m1", "model": "opus",
                "usage": {"input_tokens": 10, "cache_read_input_tokens": 100,
                          "cache_creation_input_tokens": 5, "output_tokens": out},
                "content": [{"type": "text", "text": "hi"}]}})
            .to_string()
        };
        let side = serde_json::json!({"timestamp": "2026-11-01T04:00:00Z", "sessionId": "s1",
            "isSidechain": true, "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "z", "content": [
                {"type": "text", "text": "abc"}, {"type": "image", "source": {}}]}]}})
        .to_string();
        let lines = vec![
            usage(1),
            usage(7),
            call("2026-11-01T04:00:00Z", "z", "Grep", ""),
            side,
        ];
        let (roots, dir) = roots_with("usage", &lines);
        let audit = audit(&roots, &window());
        let opus = audit.usage.get("opus").unwrap();
        assert_eq!(
            (opus.messages, opus.input, opus.cache_read, opus.output),
            (1, 10, 100, 7)
        );
        let search = audit.sidechain.get("search").unwrap();
        assert_eq!((search.result_bytes, search.non_text), (3, 1));
        assert!(audit.main.is_empty());
        let _ = fs::remove_dir_all(dir); // best effort: a temp dir
    }
}
