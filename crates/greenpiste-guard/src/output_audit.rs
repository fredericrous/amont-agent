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
//! - A result counts once per `tool_use_id`, an agent launch once per its
//!   `tool_use` id, and a message's usage once per `message.id`: a resumed
//!   session copies its history into a new file, and a streamed message
//!   repeats its usage on every snapshot (the last snapshot in the window
//!   wins).
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
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use crate::transcript::{files, Roots};

/// A line longer than this is not parsed; it is counted as oversized and
/// skipped to its end without being held in memory whole.
const MAX_LINE: u64 = 16 * 1024 * 1024;

/// What a one-line quiet summary is allowed to carry.
pub const SUMMARY_BYTES: u64 = 200;

/// A UTC instant as transcripts write it, normalised so two instants compare
/// as strings: `YYYY-MM-DDTHH:MM:SS.mmmZ`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant(String);

impl Instant {
    /// `YYYY-MM-DDTHH:MM:SS[.f…]Z` with each field in range; any other shape
    /// (an offset, a missing `Z`, month 13) is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let bytes = text.as_bytes();
        let number = |range: std::ops::Range<usize>| -> Option<u32> {
            let part = bytes.get(range)?;
            if !part.iter().all(u8::is_ascii_digit) {
                return None;
            }
            std::str::from_utf8(part).ok()?.parse().ok()
        };
        let separators = text.len() >= 20
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':'
            && bytes[16] == b':'
            && text.ends_with('Z');
        if !separators {
            return None;
        }
        number(0..4)?;
        let in_range = (1..=12).contains(&number(5..7)?)
            && (1..=31).contains(&number(8..10)?)
            && number(11..13)? < 24
            && number(14..16)? < 60
            && number(17..19)? < 61;
        if !in_range {
            return None;
        }
        let fraction = &text[19..text.len() - 1];
        let millis = if fraction.is_empty() {
            "000".to_string()
        } else {
            let digits = fraction.strip_prefix('.')?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            format!("{:0<3}", &digits[..digits.len().min(3)])
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

/// The kind of call a tool use is, for the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// The log of a failed CI job (`--log-failed`): diagnostics.
    CiLogFailed,
    /// Any other CI log or artifact download.
    CiLog,
    CiStatus,
    Check,
    CheckQuiet,
    CheckGitHook,
    Git,
    Shell,
    Read,
    Search,
    Edit,
    Agent,
    Lsp,
    Web,
    Mcp,
    OtherTool,
    /// A result whose call is not in its file.
    Unmatched,
}

impl Kind {
    pub const ALL: [Kind; 17] = [
        Kind::CiLogFailed,
        Kind::CiLog,
        Kind::CiStatus,
        Kind::Check,
        Kind::CheckQuiet,
        Kind::CheckGitHook,
        Kind::Git,
        Kind::Shell,
        Kind::Read,
        Kind::Search,
        Kind::Edit,
        Kind::Agent,
        Kind::Lsp,
        Kind::Web,
        Kind::Mcp,
        Kind::OtherTool,
        Kind::Unmatched,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::CiLogFailed => "ci-log-failed",
            Kind::CiLog => "ci-log",
            Kind::CiStatus => "ci-status",
            Kind::Check => "check",
            Kind::CheckQuiet => "check-quiet",
            Kind::CheckGitHook => "check-git-hook",
            Kind::Git => "git",
            Kind::Shell => "shell",
            Kind::Read => "read",
            Kind::Search => "search",
            Kind::Edit => "edit",
            Kind::Agent => "agent",
            Kind::Lsp => "lsp",
            Kind::Web => "web",
            Kind::Mcp => "mcp",
            Kind::OtherTool => "other-tool",
            Kind::Unmatched => "unmatched",
        }
    }

    /// Ordered rules over the tool name and, for Bash, the command text.
    pub fn of(tool: &str, command: &str) -> Kind {
        match tool {
            "Bash" => Kind::of_command(command),
            "Read" => Kind::Read,
            "Grep" | "Glob" => Kind::Search,
            "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Kind::Edit,
            "Agent" | "Task" => Kind::Agent,
            "LSP" => Kind::Lsp,
            "WebFetch" | "WebSearch" => Kind::Web,
            name if name.starts_with("mcp__") => Kind::Mcp,
            _ => Kind::OtherTool,
        }
    }

    fn of_command(command: &str) -> Kind {
        let has = |needle: &str| command.contains(needle);
        let gh_run = has("gh run view") || has("gh run download");
        if gh_run && has("--log-failed") {
            return Kind::CiLogFailed;
        }
        if (gh_run && (has("--log") || has("download")))
            || (has("gh api") && has("/logs"))
            || (has("actions/runs") && has("/logs"))
        {
            return Kind::CiLog;
        }
        if gh_run || has("gh run list") || has("gh run watch") || has("gh pr checks") {
            return Kind::CiStatus;
        }
        if has(concat!(crate::bin!(), " quiet")) {
            return Kind::CheckQuiet;
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
            return Kind::Check;
        }
        if has("git commit") || has("git push") {
            return Kind::CheckGitHook;
        }
        if command.trim_start().starts_with("git ") || has("git -C") {
            return Kind::Git;
        }
        Kind::Shell
    }
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

/// What the walk could and could not read: a partial walk says so.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub files_scanned: u64,
    /// Last written before the window: they cannot hold a line in it.
    pub files_skipped: u64,
    pub files_unreadable: u64,
    /// A read failed partway: the rest of the file is not counted.
    pub files_truncated: u64,
    pub lines: u64,
    pub bad_lines: u64,
    pub oversized_lines: u64,
    pub untimed_lines: u64,
    pub duplicate_results: u64,
}

impl Coverage {
    /// Whether every file under the roots was read to its end.
    pub fn complete(&self) -> bool {
        self.files_unreadable == 0 && self.files_truncated == 0
    }
}

/// The aggregate over a window.
#[derive(Debug, Default)]
pub struct Audit {
    pub coverage: Coverage,
    pub sessions: u64,
    pub agent_launches: u64,
    /// The main conversation's results, by kind.
    pub main: BTreeMap<Kind, Bucket>,
    /// Results inside subagents (`isSidechain`): their context, not the
    /// parent's.
    pub sidechain: BTreeMap<Kind, Bucket>,
    pub usage: BTreeMap<String, Usage>,
}

/// Whose context a line belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    Main,
    /// A subagent's (`isSidechain`).
    Subagent,
}

/// One walk over the transcripts: what it has counted so far.
struct Scan<'a> {
    window: &'a Window,
    audit: Audit,
    sessions: HashSet<String>,
    results: HashSet<String>,
    launches: HashSet<String>,
    messages: HashMap<String, (String, Usage)>,
}

/// Walks every transcript under `roots` once and aggregates the lines in
/// `window`.
pub fn audit(roots: &Roots, window: &Window) -> Audit {
    let mut scan = Scan {
        window,
        audit: Audit::default(),
        sessions: HashSet::new(),
        results: HashSet::new(),
        launches: HashSet::new(),
        messages: HashMap::new(),
    };
    let since = window.since.unix_seconds();
    for path in files(roots) {
        if modified_before(&path, since) {
            scan.audit.coverage.files_skipped += 1;
            continue;
        }
        scan.file(&path);
    }
    scan.finish()
}

fn modified_before(path: &Path, since: i64) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .is_some_and(|age| (age.as_secs() as i64) < since)
}

/// The text bytes of a tool result's `content`, and whether it held a
/// non-text block.
fn result_size(content: &serde_json::Value) -> (u64, bool) {
    match content {
        serde_json::Value::String(text) => (text.len() as u64, false),
        serde_json::Value::Array(blocks) => {
            blocks.iter().fold((0, false), |(bytes, other), block| {
                let text = (block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
                    .then(|| block.get("text").and_then(serde_json::Value::as_str))
                    .flatten();
                match text {
                    Some(text) => (bytes + text.len() as u64, other),
                    None => (bytes, true),
                }
            })
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::Object(_) => (0, false),
    }
}

/// One line read from a transcript.
enum Line {
    End,
    Text(Vec<u8>),
    Oversized,
    Failed,
}

/// Reads one line, holding at most [`MAX_LINE`] bytes of it: a longer line
/// is skipped to its end.
fn next_line(reader: &mut BufReader<fs::File>) -> Line {
    let mut raw = Vec::new();
    match reader
        .by_ref()
        .take(MAX_LINE + 1)
        .read_until(b'\n', &mut raw)
    {
        Err(_) => Line::Failed,
        Ok(0) => Line::End,
        Ok(_) if raw.len() as u64 <= MAX_LINE || raw.ends_with(b"\n") => Line::Text(raw),
        Ok(_) => loop {
            // Discard the rest of the oversized line, a buffer at a time.
            let (done, used) = match reader.fill_buf() {
                Err(_) => return Line::Failed,
                Ok([]) => (true, 0),
                Ok(buf) => match buf.iter().position(|b| *b == b'\n') {
                    Some(at) => (true, at + 1),
                    None => (false, buf.len()),
                },
            };
            reader.consume(used);
            if done {
                return Line::Oversized;
            }
        },
    }
}

impl Scan<'_> {
    fn file(&mut self, path: &Path) {
        let Ok(file) = fs::File::open(path) else {
            self.audit.coverage.files_unreadable += 1;
            return;
        };
        self.audit.coverage.files_scanned += 1;
        // The calls of this file: tool_use id → kind.
        let mut calls: HashMap<String, Kind> = HashMap::new();
        let mut reader = BufReader::new(file);
        loop {
            let raw = match next_line(&mut reader) {
                Line::End => break,
                Line::Failed => {
                    self.audit.coverage.files_truncated += 1;
                    break;
                }
                Line::Oversized => {
                    self.audit.coverage.lines += 1;
                    self.audit.coverage.oversized_lines += 1;
                    continue;
                }
                Line::Text(raw) => raw,
            };
            self.audit.coverage.lines += 1;
            let Ok(entry) = serde_json::from_slice::<serde_json::Value>(&raw) else {
                self.audit.coverage.bad_lines += 1;
                continue;
            };
            for (id, kind) in calls_in(&entry) {
                calls.insert(id, kind);
            }
            self.entry(&entry, &calls);
        }
    }

    fn entry(&mut self, entry: &serde_json::Value, calls: &HashMap<String, Kind>) {
        let Some(at) = entry
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .and_then(Instant::parse)
        else {
            self.audit.coverage.untimed_lines += 1;
            return;
        };
        if at < self.window.since || at >= self.window.until {
            return;
        }
        if let Some(session) = entry.get("sessionId").and_then(serde_json::Value::as_str) {
            self.sessions.insert(session.to_string());
        }
        let context = if entry
            .get("isSidechain")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            Context::Subagent
        } else {
            Context::Main
        };
        let message = entry.get("message");
        if let Some(message) = message
            .filter(|m| m.get("role").and_then(serde_json::Value::as_str) == Some("assistant"))
        {
            self.usage(message);
        }
        for block in content(entry) {
            match block.get("type").and_then(serde_json::Value::as_str) {
                Some("tool_use") => self.launch(block, context),
                Some("tool_result") => self.result(block, context, calls),
                Some(_) | None => {}
            }
        }
    }

    fn launch(&mut self, block: &serde_json::Value, context: Context) {
        let name = block.get("name").and_then(serde_json::Value::as_str);
        let id = block.get("id").and_then(serde_json::Value::as_str);
        if let (Some("Agent" | "Task"), Some(id), Context::Main) = (name, id, context) {
            if self.launches.insert(id.to_string()) {
                self.audit.agent_launches += 1;
            }
        }
    }

    fn result(
        &mut self,
        block: &serde_json::Value,
        context: Context,
        calls: &HashMap<String, Kind>,
    ) {
        let Some(id) = block.get("tool_use_id").and_then(serde_json::Value::as_str) else {
            return;
        };
        if !self.results.insert(id.to_string()) {
            self.audit.coverage.duplicate_results += 1;
            return;
        }
        let kind = calls.get(id).copied().unwrap_or(Kind::Unmatched);
        let (bytes, non_text) =
            result_size(block.get("content").unwrap_or(&serde_json::Value::Null));
        let failed = block
            .get("is_error")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let buckets = match context {
            Context::Subagent => &mut self.audit.sidechain,
            Context::Main => &mut self.audit.main,
        };
        let bucket = buckets.entry(kind).or_default();
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

    /// Keeps the last usage a message reported in the window: a streamed
    /// message repeats it on every snapshot, and the last one is final.
    fn usage(&mut self, message: &serde_json::Value) {
        let (Some(id), Some(usage)) = (
            message.get("id").and_then(serde_json::Value::as_str),
            message.get("usage"),
        ) else {
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
        self.messages.insert(
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

    fn finish(self) -> Audit {
        let mut audit = self.audit;
        audit.sessions = self.sessions.len() as u64;
        for (model, usage) in self.messages.into_values() {
            let total = audit.usage.entry(model).or_default();
            total.messages += 1;
            total.input += usage.input;
            total.cache_creation += usage.cache_creation;
            total.cache_read += usage.cache_read;
            total.output += usage.output;
        }
        audit
    }
}

/// The content blocks of a transcript line's message.
fn content(entry: &serde_json::Value) -> &[serde_json::Value] {
    entry
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(serde_json::Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The calls a line makes, whatever its time: a result in the window may
/// answer a call made just before it.
fn calls_in(entry: &serde_json::Value) -> Vec<(String, Kind)> {
    content(entry)
        .iter()
        .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use"))
        .filter_map(|block| {
            let id = block.get("id").and_then(serde_json::Value::as_str)?;
            let tool = block
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let command = block
                .get("input")
                .and_then(|input| input.get("command"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            Some((id.to_string(), Kind::of(tool, command)))
        })
        .collect()
}

impl Audit {
    /// The aggregate as JSON: counts only, no command or result text.
    pub fn to_json(&self, window: &Window) -> serde_json::Value {
        let buckets = |map: &BTreeMap<Kind, Bucket>| {
            serde_json::Value::Object(
                map.iter()
                    .map(|(kind, b)| {
                        (
                            kind.as_str().to_string(),
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
        let coverage = &self.coverage;
        serde_json::json!({
            "window": {"since": window.since.as_str(), "until": window.until.as_str()},
            "definitions": {
                "result_bytes": "UTF-8 bytes of tool-result text, once per tool_use_id",
                "over_summary_bytes": format!("bytes of passing results beyond {SUMMARY_BYTES} B each: a proxy for avoidable output"),
                "agent_launches": "Agent/Task calls in the main conversation, once per tool_use id",
                "usage": "assistant-reported tokens, once per message.id; not billing, not quota",
            },
            "coverage": {
                "complete": coverage.complete(),
                "files_scanned": coverage.files_scanned,
                "files_skipped_before_window": coverage.files_skipped,
                "files_unreadable": coverage.files_unreadable,
                "files_truncated": coverage.files_truncated,
                "lines": coverage.lines, "bad_lines": coverage.bad_lines,
                "oversized_lines": coverage.oversized_lines,
                "untimed_lines": coverage.untimed_lines,
                "duplicate_results": coverage.duplicate_results,
            },
            "sessions": self.sessions,
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

    /// A transcript root holding one file per entry of `files`.
    fn roots_with(name: &str, files: &[&[String]]) -> (Roots, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "amont-agent-output-audit-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir); // absent on a first run, which is fine
        fs::create_dir_all(dir.join("p")).unwrap();
        for (at, lines) in files.iter().enumerate() {
            fs::write(
                dir.join("p").join(format!("s{at}.jsonl")),
                lines.join("\n") + "\n",
            )
            .unwrap();
        }
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
    fn instants_normalise_compare_half_open_and_check_ranges() {
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
        for bad in [
            "2026-11-01T00:00:00+01:00",
            "yesterday",
            "2026-13-01T00:00:00Z",
            "2026-11-32T00:00:00Z",
            "2026-11-01T24:00:00Z",
            "2026-11-01T00:00:00.Z",
        ] {
            assert!(Instant::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn bash_commands_fall_in_hand_checked_kinds() {
        let kind = |tool: &str, command: &str| Kind::of(tool, command).as_str();
        assert_eq!(kind("Bash", "gh run view 1 --log-failed"), "ci-log-failed");
        assert_eq!(kind("Bash", "gh run view 1 --log"), "ci-log");
        assert_eq!(
            kind("Bash", "gh api repos/o/r/actions/jobs/1/logs"),
            "ci-log"
        );
        assert_eq!(kind("Bash", "gh run view 1 --json conclusion"), "ci-status");
        assert_eq!(kind("Bash", "gh pr checks 3"), "ci-status");
        assert_eq!(
            kind("Bash", "greenpiste quiet -- cargo test"),
            "check-quiet"
        );
        assert_eq!(kind("Bash", "cargo test -p x"), "check");
        assert_eq!(kind("Bash", "git commit -m x"), "check-git-hook");
        assert_eq!(kind("Bash", "git status"), "git");
        assert_eq!(kind("Bash", "ls"), "shell");
        assert_eq!(kind("mcp__aval__aval_rule", ""), "mcp");
        assert_eq!(kind("Read", ""), "read");
        let names: HashSet<&str> = Kind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(names.len(), Kind::ALL.len(), "every kind has its own name");
    }

    #[test]
    fn results_and_launches_count_once_across_copies_and_inside_the_window() {
        let long = "x".repeat(1_000);
        let session = vec![
            call("2026-11-01T01:00:00Z", "a", "Bash", "cargo test"),
            result("2026-11-01T01:00:01Z", "a", &long, Ended::Passed),
            call("2026-11-01T02:00:00Z", "b", "Bash", "cargo test"),
            result("2026-11-01T02:00:01Z", "b", "boom", Ended::Failed),
            call("2026-11-01T02:30:00Z", "g", "Agent", ""),
            // At `until`: excluded.
            call("2026-11-02T00:00:00Z", "c", "Read", ""),
            result("2026-11-02T00:00:00Z", "c", &long, Ended::Passed),
            // A result whose call is in no file.
            result("2026-11-01T03:00:00Z", "z", "abc", Ended::Passed),
            r#"{"message":{"role":"user","content":[]}}"#.to_string(),
            "not json".to_string(),
        ];
        // A resumed session's copy repeats the whole history in another file.
        let (roots, dir) = roots_with("copies", &[&session, &session]);
        let audit = audit(&roots, &window());
        let check = audit.main.get(&Kind::Check).unwrap();
        assert_eq!((check.calls, check.ok, check.failed), (2, 1, 1));
        assert_eq!((check.result_bytes, check.over_summary), (1_004, 800));
        assert_eq!(audit.agent_launches, 1);
        assert_eq!(audit.main.get(&Kind::Unmatched).map(|b| b.calls), Some(1));
        assert!(!audit.main.contains_key(&Kind::Read), "until is excluded");
        assert_eq!(audit.coverage.duplicate_results, 3);
        assert_eq!(audit.coverage.untimed_lines, 2);
        assert_eq!(audit.coverage.bad_lines, 2);
        assert_eq!((audit.coverage.files_scanned, audit.sessions), (2, 1));
        assert!(audit.coverage.complete());
        let _ = fs::remove_dir_all(dir); // best effort: a temp dir
    }

    #[test]
    fn a_file_last_written_before_the_window_is_skipped_unread() {
        let lines = vec![
            call("2026-11-01T01:00:00Z", "a", "Bash", "ls"),
            result("2026-11-01T01:00:01Z", "a", "x", Ended::Passed),
        ];
        let (roots, dir) = roots_with("old", &[&lines]);
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(86_400);
        fs::File::options()
            .write(true)
            .open(dir.join("p").join("s0.jsonl"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let audit = audit(&roots, &window());
        assert_eq!(
            (audit.coverage.files_skipped, audit.coverage.files_scanned),
            (1, 0)
        );
        assert!(audit.main.is_empty());
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
        let (roots, dir) = roots_with("usage", &[&lines]);
        let audit = audit(&roots, &window());
        let opus = audit.usage.get("opus").unwrap();
        assert_eq!(
            (opus.messages, opus.input, opus.cache_read, opus.output),
            (1, 10, 100, 7)
        );
        let search = audit.sidechain.get(&Kind::Search).unwrap();
        assert_eq!((search.result_bytes, search.non_text), (3, 1));
        assert!(audit.main.is_empty());
        let _ = fs::remove_dir_all(dir); // best effort: a temp dir
    }
}
