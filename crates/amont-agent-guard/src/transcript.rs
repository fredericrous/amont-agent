//! Reading Claude Code's session transcripts.
//!
//! Claude Code appends one JSON object per line to
//! `~/.claude/projects/<encoded-project-path>/<session-uuid>.jsonl`. Every tool
//! call it ever made is in there, which is what makes a rule's fire rate a
//! measurable quantity instead of an opinion.
//!
//! Four decisions in here exist because getting them wrong produces numbers
//! that look right:
//!
//! 1. **Bucket on the entry's own `timestamp`, never the file's mtime.** A
//!    session open for six hours has one mtime and hundreds of entries spread
//!    across it; a resumed session has an mtime weeks after its contents. Using
//!    mtime collapses a whole session into whatever day it was last touched,
//!    and the resulting weekly table is confidently wrong. Measured both ways
//!    while planning this: they disagree materially.
//!
//! 2. **Filter on the entry's own `cwd`, never the directory name.** The
//!    project directory is a lossy encoding of a path —
//!    `-Users-me-Developer-my-project` cannot be decoded, because a literal
//!    dash and a path separator both became `-`.
//!
//! 3. **Deduplicate on `tool_use.id`.** Resuming or forking a session copies
//!    earlier entries into the new transcript. Those are the same tool call
//!    observed twice, and counting them twice inflates exactly the busy
//!    sessions that dominate a bucket.
//!
//! 4. **A truncated final line is normal, not corruption.** A session being
//!    written right now ends mid-object. Skip it, count it, keep the file.
//!
//! ## What is not here
//!
//! Sessions run on the web are not on this machine, so no scan can see them.
//! Every report says so rather than quietly under-reporting.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::civil::{day_of, Day};

/// A single tool invocation, borrowed from the parsed line it came from.
///
/// The backtester reads only `command`, `day` and `cwd` today. The rest is what
/// `explain` and the labelled-corpus export need in order to say WHICH session
/// and which call a sample came from, which is the difference between a sample
/// a human can go and check and one they have to take on trust.
#[allow(dead_code)]
pub struct ToolCall<'a> {
    /// `tool_use.id` — the deduplication key.
    pub id: &'a str,
    pub tool: &'a str,
    /// `input.command` for Bash. Empty for a tool that has no such field.
    pub command: &'a str,
    pub day: Day,
    pub session: &'a str,
    /// The entry's own working directory, which is the only trustworthy one.
    pub cwd: &'a str,
    /// A subagent's call. These count: the hook fires for them too.
    pub sidechain: bool,
}

/// How the transcript roots were found, so a report can say where it looked.
#[derive(Debug, PartialEq, Eq)]
pub enum Discovery {
    Explicit,
    ConfigDirEnv,
    HomeDefault,
}

pub struct Roots {
    pub dirs: Vec<PathBuf>,
    /// Reported by `doctor`, which has to say where it looked before it can
    /// claim the guard is or is not alive.
    #[allow(dead_code)]
    pub how: Discovery,
}

#[derive(Debug)]
pub enum ScanError {
    NoHome,
    Missing(PathBuf),
}

impl ScanError {
    pub fn explain(&self) -> String {
        match self {
            ScanError::NoHome => {
                "cannot find your home directory: set CLAUDE_CONFIG_DIR or pass --transcripts"
                    .to_string()
            }
            ScanError::Missing(p) => format!(
                "no transcripts at {} — pass --transcripts <dir> if they live elsewhere",
                p.display()
            ),
        }
    }
}

/// Where the transcripts are. `$CLAUDE_CONFIG_DIR` wins over `$HOME/.claude`
/// because that is the override Claude Code itself honours.
pub fn roots(explicit: &[PathBuf]) -> Result<Roots, ScanError> {
    if !explicit.is_empty() {
        for d in explicit {
            if !d.is_dir() {
                return Err(ScanError::Missing(d.clone()));
            }
        }
        return Ok(Roots {
            dirs: explicit.to_vec(),
            how: Discovery::Explicit,
        });
    }
    let how = if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
        Discovery::ConfigDirEnv
    } else {
        Discovery::HomeDefault
    };
    let base = crate::settings::config_dir().ok_or(ScanError::NoHome)?;
    let projects = base.join("projects");
    if !projects.is_dir() {
        return Err(ScanError::Missing(projects));
    }
    Ok(Roots {
        dirs: vec![projects],
        how,
    })
}

pub struct Scan<'a> {
    pub roots: &'a Roots,
    /// Only calls to this tool. Also the second-stage prefilter.
    pub tool: &'static str,
    pub since: Option<Day>,
}

/// What the walk saw. Reported so a surprising rate can be checked against a
/// surprising denominator rather than believed.
#[derive(Default, Debug)]
pub struct Stats {
    pub files: usize,
    pub bytes: u64,
    pub lines: u64,
    pub parsed: u64,
    pub calls: u64,
    pub duplicates: u64,
    pub bad_lines: u64,
    pub oversized: u64,
    pub truncated_tail: usize,
    /// Tool calls of EVERY tool, not just the one being scanned. Only
    /// [`for_each_transcript`] fills this: it is the clock a "within N tool
    /// calls" window is counted against, and a window counted in Bash calls
    /// alone would call two commands adjacent across ten minutes of reading
    /// files.
    pub events: u64,
}

/// What the transcript says became of a call.
///
/// Derived from the transcript and nothing else — no re-running, no guessing
/// from the text of the command. That is the whole basis on which a mined
/// number is allowed to be believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A result came back with no error flag.
    Ok,
    /// `is_error` on the tool result: a non-zero exit, a refused permission,
    /// a run the harness killed.
    Failed,
    /// No result in this transcript. The session ended, was forked, or was
    /// interrupted before one was written — which is not the same as success
    /// and is never counted as either.
    Unknown,
}

/// One tool call, owned, with its outcome attached.
///
/// [`ToolCall`] borrows from the line it came from, which is what keeps the
/// backtester's walk allocation-free. Mining cannot work that way: an outcome
/// arrives on a LATER line than the call it belongs to, and a window over
/// neighbouring calls needs the neighbours to still exist. So this one owns
/// its strings and a whole transcript's worth is handed over at once.
///
/// Several fields are carried for the same reason [`ToolCall`]'s are: an
/// id, a session and a cwd are what let a report say WHICH call a number came
/// from, and a mined number nobody can go and check is a number nobody should
/// believe. Re-deriving them later is how a reader quietly loses that.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Record {
    pub id: String,
    pub tool: String,
    /// `input.command` for Bash; empty for every other tool.
    pub command: String,
    pub day: Day,
    pub session: String,
    pub cwd: String,
    /// `message.model` on the assistant turn that issued the call — which
    /// model made this mistake. Empty when the transcript does not say.
    pub model: String,
    pub sidechain: bool,
    pub outcome: Outcome,
}

/// A line longer than this is not a tool call we can use. The largest command
/// in the corpus measured 13 KB; a megabyte is a base64 payload or a corrupt
/// line, and either way parsing it costs more than it can be worth.
const MAX_LINE: usize = 1024 * 1024;

/// Every `.jsonl` under the roots, sorted so a run is reproducible.
pub fn files(roots: &Roots) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in &roots.dirs {
        collect(dir, &mut out);
    }
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => collect(&path, out),
            Ok(t) if t.is_file() && path.extension().is_some_and(|x| x == "jsonl") => {
                out.push(path);
            }
            _ => {}
        }
    }
}

/// Walk every transcript, calling `f` once per DEDUPLICATED tool call.
///
/// Single-threaded on purpose, for now. The corpus is ~800 MB and the two-stage
/// prefilter drops all but a few percent of lines before any parsing happens;
/// measure before adding threads, because a parallel walk has to reconcile the
/// dedup set anyway and a result that depends on scheduling is worse than a
/// result that takes another second.
pub fn for_each_call<F>(scan: &Scan, mut f: F) -> Result<Stats, ScanError>
where
    F: FnMut(&ToolCall),
{
    let mut stats = Stats::default();
    let mut seen: HashSet<String> = HashSet::new();
    let needle_tool = format!("\"{}\"", scan.tool);

    for path in files(scan.roots) {
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        stats.files += 1;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        loop {
            line.clear();
            let read = match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(n) => n,
                // Invalid UTF-8 mid-file: the rest of this transcript is not
                // readable, but the ones already counted still are.
                Err(_) => break,
            };
            stats.bytes += read as u64;
            stats.lines += 1;

            let complete = line.ends_with('\n');
            let raw = line.trim_end();
            if raw.is_empty() {
                continue;
            }
            if raw.len() > MAX_LINE {
                stats.oversized += 1;
                continue;
            }

            // Stage one: does this line mention a tool call at all? Stage two:
            // does it mention OUR tool? Both are substring tests over a
            // superset of what the parser would accept, so neither can change
            // the result — only how much text reaches the parser. Measured on
            // the real corpus: 110k lines to 43k.
            if !raw.contains("\"tool_use\"") || !raw.contains(&needle_tool) {
                continue;
            }

            let Ok(entry) = serde_json::from_str::<serde_json::Value>(raw) else {
                // An unparseable LAST line with no newline is a session being
                // written as we read it. Anywhere else it is a damaged line.
                if !complete {
                    stats.truncated_tail += 1;
                } else {
                    stats.bad_lines += 1;
                }
                continue;
            };
            stats.parsed += 1;

            let Some(day) = entry
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(day_of)
            else {
                continue;
            };
            if scan.since.is_some_and(|s| day < s) {
                continue;
            }

            let session = entry
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let cwd = entry.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
            let sidechain = entry
                .get("isSidechain")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let Some(blocks) = entry
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            else {
                continue;
            };

            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                    continue;
                }
                let Some(tool) = block.get("name").and_then(|n| n.as_str()) else {
                    continue;
                };
                if tool != scan.tool {
                    continue;
                }
                let Some(id) = block.get("id").and_then(|i| i.as_str()) else {
                    continue;
                };
                if !seen.insert(id.to_string()) {
                    stats.duplicates += 1;
                    continue;
                }
                let command = block
                    .get("input")
                    .and_then(|i| i.get("command"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                stats.calls += 1;
                f(&ToolCall {
                    id,
                    tool,
                    command,
                    day,
                    session,
                    cwd,
                    sidechain,
                });
            }
        }
    }
    Ok(stats)
}

/// Walk the transcripts a FILE at a time, handing each file's tool calls over
/// in the order they were made, with each call's outcome already attached.
///
/// The backtester's walk cannot answer the two questions mining asks — what
/// happened to this call, and what did the model do next — because both live
/// on later lines than the call itself. This one buffers a transcript, joins
/// each result back to its call, and then hands the whole sequence over.
///
/// Every tool is yielded, not just `scan.tool`. A Read between two Bash calls
/// is not a Bash call, but it is time passing, and "the next equivalent
/// command within three tool calls" means three of anything.
///
/// ## What a window loses at a file boundary
///
/// Deduplication is by `tool_use.id` across the whole run, as it is for the
/// backtester and for the same reason: resuming a session copies earlier
/// entries into the new transcript. The copies are dropped where they are met
/// again, which means the calls at the END of a resumed session have no
/// successor in their own file and the ones at the start of the next file
/// have no predecessor. Corrections that straddle that seam are missed. The
/// direction of the error is the safe one — a shape looks better behaved than
/// it was, never worse.
pub fn for_each_transcript<F>(scan: &Scan, mut f: F) -> Result<Stats, ScanError>
where
    F: FnMut(&[Record]),
{
    let mut stats = Stats::default();
    let mut seen: HashSet<String> = HashSet::new();

    for path in files(scan.roots) {
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        stats.files += 1;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let mut records: Vec<Record> = Vec::new();
        // `tool_use.id` to its position in `records`, so a result line can
        // find the call it belongs to without a second pass over the file.
        let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        loop {
            line.clear();
            let read = match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            stats.bytes += read as u64;
            stats.lines += 1;
            let complete = line.ends_with('\n');
            let raw = line.trim_end();
            if raw.is_empty() {
                continue;
            }
            if raw.len() > MAX_LINE {
                stats.oversized += 1;
                continue;
            }

            // A RESULT line. Read with substring scans rather than a JSON
            // parse: a result carries the whole output of the command, which
            // is where all the bytes in a transcript are, and the two facts
            // wanted here are a fixed id and a boolean. Parsing megabytes to
            // read a flag would make mining cost more than the backtest it
            // sits beside.
            if let Some(id) = tool_use_id(raw) {
                if let Some(&i) = at.get(id) {
                    records[i].outcome = if errored(raw) {
                        Outcome::Failed
                    } else {
                        Outcome::Ok
                    };
                }
                continue;
            }
            if !raw.contains("\"tool_use\"") {
                continue;
            }
            let Ok(entry) = serde_json::from_str::<serde_json::Value>(raw) else {
                if !complete {
                    stats.truncated_tail += 1;
                } else {
                    stats.bad_lines += 1;
                }
                continue;
            };
            stats.parsed += 1;

            let Some(day) = entry
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(day_of)
            else {
                continue;
            };
            if scan.since.is_some_and(|s| day < s) {
                continue;
            }
            let session = entry
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let cwd = entry.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
            let sidechain = entry
                .get("isSidechain")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let message = entry.get("message");
            let model = message
                .and_then(|m| m.get("model"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let Some(blocks) = message
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            else {
                continue;
            };
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                    continue;
                }
                let Some(tool) = block.get("name").and_then(|n| n.as_str()) else {
                    continue;
                };
                let Some(id) = block.get("id").and_then(|i| i.as_str()) else {
                    continue;
                };
                if !seen.insert(id.to_string()) {
                    stats.duplicates += 1;
                    continue;
                }
                let command = block
                    .get("input")
                    .and_then(|i| i.get("command"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                stats.events += 1;
                if tool == scan.tool {
                    stats.calls += 1;
                }
                at.insert(id.to_string(), records.len());
                records.push(Record {
                    id: id.to_string(),
                    tool: tool.to_string(),
                    command: command.to_string(),
                    day,
                    session: session.to_string(),
                    cwd: cwd.to_string(),
                    model: model.to_string(),
                    sidechain,
                    outcome: Outcome::Unknown,
                });
            }
        }
        if !records.is_empty() {
            f(&records);
        }
    }
    Ok(stats)
}

/// The `tool_use_id` of a result line, or `None` when this is not one.
fn tool_use_id(raw: &str) -> Option<&str> {
    const KEY: &str = "\"tool_use_id\":\"";
    let start = raw.find(KEY)? + KEY.len();
    let rest = &raw[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// Did this result line carry the error flag?
///
/// The LAST `"is_error":` on the line, because the flag sits at the end of
/// the tool-result block while the output that precedes it may quote
/// anything at all — including, on a bad day, a transcript of its own.
fn errored(raw: &str) -> bool {
    const KEY: &str = "\"is_error\":";
    match raw.rfind(KEY) {
        Some(i) => raw[i + KEY.len()..].trim_start().starts_with("true"),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both of these are substring scans over somebody else's JSON, chosen
    /// over a parse because a result line carries the whole output of the
    /// command. That trade is only safe while the scans are exactly right.
    #[test]
    fn a_result_line_is_recognised_by_its_tool_use_id() {
        let line = r#"{"message":{"content":[{"tool_use_id":"toolu_01ab","type":"tool_result","content":"ok","is_error":false}]}}"#;
        assert_eq!(tool_use_id(line), Some("toolu_01ab"));
        assert!(!errored(line));
    }

    #[test]
    fn the_error_flag_is_read_from_the_end_of_the_line() {
        let line = r#"{"message":{"content":[{"tool_use_id":"t1","type":"tool_result","content":"Exit code 2","is_error":true}]}}"#;
        assert!(errored(line));
    }

    /// A command whose OUTPUT quotes a transcript would otherwise decide its
    /// own outcome. The real flag is the last one on the line, after the
    /// content it is a verdict on.
    #[test]
    fn output_that_quotes_the_flag_does_not_decide_the_outcome() {
        let line = r#"{"message":{"content":[{"tool_use_id":"t1","type":"tool_result","content":"grepped: \"is_error\":true","is_error":false}]}}"#;
        assert!(!errored(line), "the last flag on the line is the real one");
    }

    #[test]
    fn a_call_line_is_not_a_result_line() {
        let line = r#"{"message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#;
        assert_eq!(tool_use_id(line), None);
    }
}
