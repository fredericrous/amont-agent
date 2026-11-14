//! The `clear-at-boundary` judgement of a `Stop`: what a transcript tail says
//! about the context size and the current turn, whether the turn reached a
//! boundary, whether a relais run still waits on this session, and whether
//! the repositories the turn touched are clean. The reading of the tail and
//! of relais messages is pure; [`on_stop`] is the shell around it, which
//! spawns git, keeps the session's state file and journals each outcome.
//! `src/hook.rs` orders it against `plan-phases-open`;
//! `docs/plans/2026-11-13-advise-clear-at-a-boundary.md` is the
//! specification.
//!
//! greenpiste never spawns `relais` (ADR-0064): the run state is read from
//! what the transcript's own relais tool results already showed, through
//! the phrases in [`relais_messages`].

use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use amont_agent_guard::atomic::write_atomic;
use amont_agent_guard::decision::{self, Decision};
use amont_agent_guard::git::{bytes_bounded_detail, BoundedFailure};
use amont_agent_guard::journal;
use amont_agent_guard::payload::Stop;
use amont_agent_guard::rules::clear_at_boundary::RULE;
use amont_agent_guard::rules::Stance;
use serde_json::Value;

use crate::plan_phases::PhaseFact;

pub use amont_agent_guard::rules::clear_at_boundary::MARKER;

/// The context size, in tokens, past which a clear pays for itself.
///
/// Break-even: a fresh session re-reads its prefix R once at the 5-minute
/// cache write price, 25 times a cache read on Opus 5.5 (`25·R`), while the
/// session kept going pays a read `R` less than a read of the bigger context
/// `S` each later turn (`S − R`). Clearing wins after `k = 25·R/(S − R)`
/// further turns. With the prefix R of about 100k tokens, S = 200k gives
/// `k = 25`: a session expected to run 25 more turns repays the clear, and
/// every larger S repays it sooner.
pub const SIZE: u64 = 200_000;

/// The most advices one session gets; from the next boundary on the rule is
/// silent (`watched cap`). Backtest: 789 advices, 44 sessions over 3, at
/// most 54 in one; a cap of 3 leaves 174 advices and
/// keeps 30 of 31 compacted sessions advised before their first compaction.
pub const MAX_ADVICES_PER_SESSION: u32 = 3;

/// The first chunk of a transcript tail.
pub const TAIL_FIRST_CHUNK: u64 = 64 * 1024;
/// The largest tail read.
// holds-until: a session's relais history fits in 8 MB of transcript; past
// that, a run index kept in the state file instead of the tail.
pub const TAIL_CAP: u64 = 8 * 1024 * 1024;

/// The entries of a transcript's tail, oldest first.
#[derive(Debug)]
pub struct Tail {
    pub entries: Vec<Value>,
    /// The cap was reached before the start of the file: older entries exist
    /// that were not read (the hook journals `tail-capped`).
    pub capped: bool,
}

/// [`read_tail_until`] with the defaults, stopping once both the context size
/// and the turn window are in the tail.
pub fn read_tail(path: &Path) -> std::io::Result<Tail> {
    read_tail_until(path, TAIL_FIRST_CHUNK, TAIL_CAP, |entries| {
        context_size(entries).is_some() && turn_window(entries).is_some()
    })
}

/// Read a JSONL file backwards in chunks of `first_chunk` bytes, doubling up
/// to `cap`, until `enough` accepts the entries read, the file start is
/// reached or the cap is.
///
/// A chunk that starts inside the file begins with a cut line, which is
/// dropped; a line that is not JSON is skipped.
pub fn read_tail_until(
    path: &Path,
    first_chunk: u64,
    cap: u64,
    enough: impl Fn(&[Value]) -> bool,
) -> std::io::Result<Tail> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut chunk = first_chunk.clamp(1, cap.max(1));
    loop {
        let start = size.saturating_sub(chunk);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        (&mut file).take(size - start).read_to_end(&mut bytes)?;
        let whole_lines = if start > 0 {
            without_first_line(&bytes)
        } else {
            &bytes
        };
        let entries = parse_lines(whole_lines);
        if start == 0 {
            return Ok(Tail {
                entries,
                capped: false,
            });
        }
        if chunk >= cap {
            return Ok(Tail {
                entries,
                capped: true,
            });
        }
        if enough(&entries) {
            return Ok(Tail {
                entries,
                capped: false,
            });
        }
        chunk = chunk.saturating_mul(2).min(cap);
    }
}

/// The bytes after the first newline: a chunk read from inside the file
/// starts in the middle of a line.
fn without_first_line(bytes: &[u8]) -> &[u8] {
    bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(&[], |at| &bytes[at + 1..])
}

fn parse_lines(bytes: &[u8]) -> Vec<Value> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect()
}

fn is_sidechain(entry: &Value) -> bool {
    entry["isSidechain"].as_bool() == Some(true)
}

fn entry_type(entry: &Value) -> &str {
    entry["type"].as_str().unwrap_or("")
}

/// The content blocks of an entry's message; a plain string is no blocks.
fn blocks(entry: &Value) -> &[Value] {
    entry
        .pointer("/message/content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// Input, cache-creation and cache-read tokens of the last main-chain
/// assistant entry that carries `message.usage`.
pub fn context_size(entries: &[Value]) -> Option<u64> {
    entries
        .iter()
        .rev()
        .filter(|entry| entry_type(entry) == "assistant" && !is_sidechain(entry))
        .find_map(|entry| {
            let usage = entry.pointer("/message/usage")?.as_object()?;
            let tokens = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
            Some(
                tokens("input_tokens")
                    + tokens("cache_creation_input_tokens")
                    + tokens("cache_read_input_tokens"),
            )
        })
}

/// The entries after the last person prompt.
#[derive(Debug)]
pub struct TurnWindow<'a> {
    /// The prompt's `uuid`, empty when the entry carries none.
    pub prompt_uuid: String,
    pub entries: &'a [Value],
}

/// A `user` entry that is a person's prompt and not a tool result or a
/// harness-injected message.
fn is_prompt(entry: &Value) -> bool {
    if entry_type(entry) != "user" || is_sidechain(entry) || entry["isMeta"].as_bool() == Some(true)
    {
        return false;
    }
    match entry.pointer("/message/content") {
        Some(Value::String(_)) => true,
        Some(Value::Array(parts)) => !parts
            .iter()
            .any(|part| part["type"].as_str() == Some("tool_result")),
        _ => false,
    }
}

/// The text of a user entry's prompt: a plain string or the first text block.
fn prompt_text(entry: &Value) -> Option<&str> {
    match entry.pointer("/message/content")? {
        Value::String(text) => Some(text),
        Value::Array(parts) => parts.iter().find_map(|part| part["text"].as_str()),
        _ => None,
    }
}

/// A prompt the harness injected: a task notification, or a message the
/// engine framed as `The <name> plugin sent a message:`.
pub fn is_injected(entry: &Value) -> bool {
    if entry_type(entry) != "user" {
        return false;
    }
    prompt_text(entry).is_some_and(|text| {
        text.starts_with("<task-notification>")
            || text
                .strip_prefix("The ")
                .and_then(|rest| rest.split_once(" plugin sent a message:"))
                .is_some_and(|(name, _)| !name.is_empty() && !name.contains('\n'))
    })
}

/// A prompt a person typed: a prompt that is not injected.
pub fn is_person_prompt(entry: &Value) -> bool {
    is_prompt(entry) && !is_injected(entry)
}

/// `None` when no prompt is within the entries read (the caller journals
/// `turn-too-long`). A turn opens at the last prompt of any kind: relais
/// plugin messages and task notifications start a turn too.
pub fn turn_window(entries: &[Value]) -> Option<TurnWindow<'_>> {
    let at = entries.iter().rposition(is_prompt)?;
    Some(TurnWindow {
        prompt_uuid: entries[at]["uuid"].as_str().unwrap_or("").to_string(),
        entries: &entries[at + 1..],
    })
}

/// A boundary the turn reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Boundary {
    /// What it was, in a few words.
    pub what: String,
    /// The `uuid` of the assistant entry whose Bash call it was found in,
    /// empty when the entry carries none.
    pub uuid: String,
    /// The Bash command it was found in.
    pub command: String,
    /// The `-C` chain of the git call, resolved against the payload cwd;
    /// empty when the call had none.
    pub dirs: Vec<PathBuf>,
}

/// The boundary commands among the Bash tool calls of the window.
pub fn boundaries(window: &TurnWindow<'_>, cwd: &Path) -> Vec<Boundary> {
    let mut found = Vec::new();
    for entry in window
        .entries
        .iter()
        .filter(|entry| entry_type(entry) == "assistant" && !is_sidechain(entry))
    {
        for block in blocks(entry) {
            if block["type"].as_str() != Some("tool_use") || block["name"].as_str() != Some("Bash")
            {
                continue;
            }
            let Some(command) = block.pointer("/input/command").and_then(Value::as_str) else {
                continue;
            };
            for segment in segments(command) {
                if let Some((what, dirs)) = classify(&segment, cwd) {
                    found.push(Boundary {
                        what: what.to_string(),
                        uuid: entry["uuid"].as_str().unwrap_or("").to_string(),
                        command: command.to_string(),
                        dirs,
                    });
                }
            }
        }
    }
    found
}

/// Split a shell command into simple commands of words, honouring quotes:
/// `;`, `&`, `|` and newlines end a command.
fn segments(command: &str) -> Vec<Vec<String>> {
    let mut all: Vec<Vec<String>> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for inner in chars.by_ref() {
                    if inner == '\'' {
                        break;
                    }
                    word.push(inner);
                }
            }
            '"' => {
                in_word = true;
                while let Some(inner) = chars.next() {
                    match inner {
                        '"' => break,
                        '\\' => word.extend(chars.next()),
                        other => word.push(other),
                    }
                }
            }
            '\\' => {
                in_word = true;
                word.extend(chars.next());
            }
            ';' | '&' | '|' | '\n' => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
                if !words.is_empty() {
                    all.push(std::mem::take(&mut words));
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            other => {
                in_word = true;
                word.push(other);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    if !words.is_empty() {
        all.push(words);
    }
    all
}

fn base_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// A `git` call: its global `-C` directories as written, its subcommand and
/// the words after it.
struct GitCall<'a> {
    directories: Vec<&'a str>,
    subcommand: &'a str,
    rest: &'a [String],
}

fn git_call(words: &[String]) -> Option<GitCall<'_>> {
    let start = words
        .iter()
        .position(|word| !word.contains('=') || word.starts_with('-'))?;
    if base_name(words.get(start)?) != "git" {
        return None;
    }
    let mut directories = Vec::new();
    let mut at = start + 1;
    while let Some(word) = words.get(at) {
        match word.as_str() {
            "-C" => {
                directories.push(words.get(at + 1)?.as_str());
                at += 2;
            }
            "-c" | "--git-dir" | "--work-tree" | "--namespace" => at += 2,
            option if option.starts_with('-') => at += 1,
            subcommand => {
                return Some(GitCall {
                    directories,
                    subcommand,
                    rest: &words[at + 1..],
                });
            }
        }
    }
    None
}

/// `base` joined with `directory`, `.` and `..` folded away lexically.
fn resolve(base: &Path, directory: &str) -> PathBuf {
    let joined = base.join(directory);
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other),
        }
    }
    resolved
}

/// Several `-C` flags chain as git does: each relative one against the last.
fn chain(cwd: &Path, directories: &[&str]) -> Vec<PathBuf> {
    if directories.is_empty() {
        return Vec::new();
    }
    vec![directories
        .iter()
        .fold(cwd.to_path_buf(), |base, directory| {
            resolve(&base, directory)
        })]
}

fn classify(words: &[String], cwd: &Path) -> Option<(&'static str, Vec<PathBuf>)> {
    if words.iter().any(|word| base_name(word) == "sweep.py")
        && words.iter().any(|word| word == "--apply")
    {
        return Some(("sweep.py --apply", Vec::new()));
    }
    let call = git_call(words)?;
    match call.subcommand {
        "worktree" => {
            let verb = call.rest.iter().find(|word| !word.starts_with('-'))?;
            (verb == "remove").then(|| ("worktree remove", chain(cwd, &call.directories)))
        }
        "commit" => {
            let subject = commit_subject(call.rest)?;
            (subject.starts_with("docs(plan): ") && !is_closing_plan_commit(&subject))
                .then(|| ("plan commit", chain(cwd, &call.directories)))
        }
        _ => None,
    }
}

/// The first line of the first `-m` message of a `git commit`.
fn commit_subject(rest: &[String]) -> Option<String> {
    let message = rest.iter().enumerate().find_map(|(at, word)| {
        if word == "-m" || word == "--message" {
            return rest.get(at + 1).cloned();
        }
        if let Some(attached) = word.strip_prefix("--message=") {
            return Some(attached.to_string());
        }
        let bundle = word.strip_prefix('-').filter(|b| !b.starts_with('-'))?;
        let position = bundle.find('m')?;
        if !bundle[..position].chars().all(char::is_alphabetic) {
            return None;
        }
        let attached = &bundle[position + 1..];
        if attached.is_empty() {
            rest.get(at + 1).cloned()
        } else {
            Some(attached.to_string())
        }
    })?;
    let body = match message.strip_prefix("$(cat <<") {
        Some(heredoc) => heredoc.split_once('\n')?.1.trim_start().to_string(),
        None => message,
    };
    body.lines().next().map(str::to_string)
}

/// A `docs(plan): …, done` or `…, phase N` commit closes a plan.
fn is_closing_plan_commit(subject: &str) -> bool {
    let subject = subject.trim_end();
    if subject.ends_with(", done") {
        return true;
    }
    subject
        .rsplit_once(", phase ")
        .is_some_and(|(_, number)| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
}

/// What the transcript shows of the relais runs this session started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelaisFlight {
    /// Starts not yet followed by a finished or exited message.
    pub pending: usize,
    /// `(run id, state)` of runs that finished waiting on this session's
    /// answer and no `mcp__relais__decide` has answered.
    pub waiting: Vec<(String, String)>,
}

impl RelaisFlight {
    pub fn in_flight(&self) -> bool {
        self.pending > 0 || !self.waiting.is_empty()
    }

    /// What one relais plugin message changes: a successor launched adds a
    /// run, a run finished or relais exited takes one off.
    fn apply(&mut self, message: &str) {
        use relais_messages::{
            EXITED, FINISHED, RUN_ID, SUCCESSOR, WAITING_STATES, WITHOUT_OUTCOME,
        };
        if message.contains(SUCCESSOR) {
            self.pending += 1;
        }
        if let Some(id) = word_after(message, RUN_ID).filter(|id| id.starts_with("run-")) {
            if let Some(state) = word_after(message, FINISHED) {
                self.pending = self.pending.saturating_sub(1);
                self.waiting.retain(|(waiting, _)| waiting != id);
                if WAITING_STATES.contains(&state) {
                    self.waiting.push((id.to_string(), state.to_string()));
                }
            }
        } else if message.contains(EXITED) && message.contains(WITHOUT_OUTCOME) {
            self.pending = self.pending.saturating_sub(1);
        }
    }
}

/// The relais wording this rule reads, from relais's
/// `claude-plugin/hooks/runs.ts`. It is relais's text, not a published
/// format: when relais rewords a phrase, a start or an end goes unseen. Each
/// phrase is read so that a miss errs towards a run in flight (an end not
/// recognised keeps the run pending, and the rule silent), except a start
/// not recognised, which the plan's Decision log names as the one contract
/// to recheck at each relais release.
mod relais_messages {
    /// The engine's frame around a message the relais plugin sent.
    pub const PLUGIN_MESSAGE_PREFIX: &str = "The relais plugin sent a message:\n";
    /// The start of an `mcp__relais__run` result that started a run.
    pub const STARTED_PREFIX: &str = "Started relais run";
    /// A plugin message launching a successor run.
    pub const SUCCESSOR: &str = "launches successor";
    /// Followed by the run id, in `relais run <id> finished: <state>`.
    pub const RUN_ID: &str = "relais run ";
    /// Followed by the run's final state.
    pub const FINISHED: &str = "finished: ";
    /// `relais exited [with code N] without an outcome`.
    pub const EXITED: &str = "relais exited";
    pub const WITHOUT_OUTCOME: &str = "without an outcome";
    /// States a finished run waits on this session in.
    pub const WAITING_STATES: [&str; 3] = ["needs_review", "needs_decision", "interrupted"];
}

fn result_text(block: &Value) -> String {
    match &block["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The word after `needle` (`run-65abc-1f2e`, `needs_decision`).
fn word_after<'a>(text: &'a str, needle: &str) -> Option<&'a str> {
    let rest = &text[text.find(needle)? + needle.len()..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(rest.len());
    Some(&rest[..end]).filter(|word| !word.is_empty())
}

/// Walks the entries in order and counts the relais runs still pending: a
/// `mcp__relais__run` result that starts `Started relais run` adds one, a
/// plugin message that `launches successor` adds one, and a `finished` or
/// `exited ... without an outcome` plugin message takes one off. A run that
/// finished in `needs_review`, `needs_decision` or `interrupted` waits until
/// a later `mcp__relais__decide` names it.
pub fn relais_flight(entries: &[Value]) -> RelaisFlight {
    let mut tool_names = std::collections::HashMap::new();
    let mut flight = RelaisFlight::default();
    for entry in entries {
        match entry_type(entry) {
            "assistant" => {
                for block in blocks(entry) {
                    if block["type"].as_str() != Some("tool_use") {
                        continue;
                    }
                    let name = block["name"].as_str().unwrap_or("");
                    if let Some(id) = block["id"].as_str() {
                        tool_names.insert(id.to_string(), name.to_string());
                    }
                    if name == "mcp__relais__decide" {
                        if let Some(run) = block["input"]["run"].as_str() {
                            flight.waiting.retain(|(waiting, _)| waiting != run);
                        }
                    }
                }
            }
            "user" => {
                if let Some(message) = entry
                    .pointer("/message/content")
                    .and_then(Value::as_str)
                    .and_then(|text| text.strip_prefix(relais_messages::PLUGIN_MESSAGE_PREFIX))
                {
                    flight.apply(message);
                    continue;
                }
                for block in blocks(entry) {
                    let started = block["type"].as_str() == Some("tool_result")
                        && block["tool_use_id"]
                            .as_str()
                            .and_then(|id| tool_names.get(id))
                            .is_some_and(|name| name == "mcp__relais__run")
                        && result_text(block)
                            .trim_start()
                            .starts_with(relais_messages::STARTED_PREFIX);
                    if started {
                        flight.pending += 1;
                    }
                }
            }
            _ => {}
        }
    }
    flight
}

/// How the repositories a boundary touched stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repositories {
    /// Every checked repository is clean, with its HEAD (empty when unborn).
    Clean {
        heads: Vec<(PathBuf, String)>,
    },
    NotClean {
        directory: PathBuf,
        reason: String,
    },
    /// The shared deadline passed.
    TimedOut,
}

fn run(directory: &Path, args: &[&str], deadline: Instant) -> Result<Vec<u8>, BoundedFailure> {
    bytes_bounded_detail(directory, args, None, &[], deadline)
}

fn run_text(directory: &Path, args: &[&str], deadline: Instant) -> Result<String, BoundedFailure> {
    run(directory, args, deadline).map(|b| String::from_utf8_lossy(&b).trim().to_string())
}

/// How one repository stands.
enum RepoCheck {
    /// Clean, with its HEAD (empty when unborn).
    Clean(String),
    NotClean(String),
    TimedOut,
}

/// The payload `cwd` when inside a work tree, and each existing `directory`:
/// a missing one, or one that is no work tree, counts as clean. A repository
/// is not clean when its status lists anything, or its branch has commits off
/// its upstream (its base when it has none) that touch a path outside
/// `docs/plans/`. Every git child shares `deadline`.
pub fn check_repositories(
    cwd: Option<&Path>,
    directories: &[PathBuf],
    deadline: Instant,
) -> Repositories {
    let mut heads: Vec<(PathBuf, String)> = Vec::new();
    for directory in cwd
        .into_iter()
        .chain(directories.iter().map(PathBuf::as_path))
    {
        if !directory.is_dir() {
            continue;
        }
        let top = match run_text(directory, &["rev-parse", "--show-toplevel"], deadline) {
            Ok(top) => PathBuf::from(top),
            Err(BoundedFailure::TimedOut) => return Repositories::TimedOut,
            Err(BoundedFailure::Exited { .. }) => continue,
        };
        if heads.iter().any(|(seen, _)| *seen == top) {
            continue;
        }
        match check_one(&top, deadline) {
            RepoCheck::Clean(head) => heads.push((top, head)),
            RepoCheck::NotClean(reason) => {
                return Repositories::NotClean {
                    directory: top,
                    reason,
                }
            }
            RepoCheck::TimedOut => return Repositories::TimedOut,
        }
    }
    Repositories::Clean { heads }
}

/// A git refusal is a reason the repository cannot be called clean, named
/// after the command git refused.
fn refused(what: &str, failure: BoundedFailure) -> RepoCheck {
    match failure {
        BoundedFailure::TimedOut => RepoCheck::TimedOut,
        BoundedFailure::Exited { .. } => RepoCheck::NotClean(format!("git {what} failed")),
    }
}

fn check_one(top: &Path, deadline: Instant) -> RepoCheck {
    let head = match run_text(top, &["rev-parse", "HEAD"], deadline) {
        Ok(head) => head,
        Err(BoundedFailure::TimedOut) => return RepoCheck::TimedOut,
        Err(BoundedFailure::Exited { .. }) => String::new(), // an unborn branch has no HEAD
    };
    let status = match run(top, &["status", "--porcelain", "-z"], deadline) {
        Ok(status) => status,
        Err(failure) => return refused("status", failure),
    };
    if !status.is_empty() {
        return RepoCheck::NotClean("uncommitted changes".to_string());
    }
    let names = match unpushed_names(top, "@{upstream}..HEAD", deadline) {
        Ok(names) => names,
        Err(BoundedFailure::TimedOut) => return RepoCheck::TimedOut,
        Err(BoundedFailure::Exited { .. }) => {
            let Some((base, _)) = amont_agent_guard::git::default_ref(top, deadline) else {
                return if Instant::now() >= deadline {
                    RepoCheck::TimedOut
                } else {
                    RepoCheck::NotClean("no upstream or base to compare with".to_string())
                };
            };
            match unpushed_names(top, &format!("{base}..HEAD"), deadline) {
                Ok(names) => names,
                Err(failure) => return refused("log", failure),
            }
        }
    };
    if names.iter().any(|name| !name.starts_with("docs/plans/")) {
        return RepoCheck::NotClean("unpushed commits outside docs/plans/".to_string());
    }
    RepoCheck::Clean(head)
}

/// The paths the commits of `range` touch, read NUL-separated: git quotes a
/// non-ASCII name unless told otherwise.
fn unpushed_names(
    top: &Path,
    range: &str,
    deadline: Instant,
) -> Result<Vec<String>, BoundedFailure> {
    let bytes = run(
        top,
        &["log", "--name-only", "-z", "--format=", range],
        deadline,
    )?;
    Ok(bytes
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect())
}

/// The key of one boundary: its command and the HEAD of each checked
/// repository. The same key again means the same boundary was already advised.
pub fn boundary_key(command: &str, heads: &[(PathBuf, String)]) -> String {
    let mut key = command.to_string();
    for (directory, head) in heads {
        key.push('\n');
        key.push_str(&directory.to_string_lossy());
        key.push(' ');
        key.push_str(head);
    }
    key
}

/// The one deadline every git child of a Stop shares.
pub const DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

fn state_dir() -> Option<PathBuf> {
    Some(journal::dir()?.join("clear-at-boundary"))
}

fn state_file(session: &str) -> Option<PathBuf> {
    if !amont_agent_guard::session_state::usable_id(session) {
        return None;
    }
    Some(state_dir()?.join(session))
}

/// A boundary reached while something was in flight, kept for a later Stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// The `uuid` of the transcript entry the boundary command was found in.
    pub uuid: String,
    pub command: String,
    pub what: String,
    pub dirs: Vec<PathBuf>,
}

/// What a session's state file holds.
#[derive(Debug, Default, PartialEq, Eq)]
struct State {
    /// The opening prompt's uuid of the turn advised in, empty when the rule
    /// only observed.
    turn: String,
    /// The key of the last boundary judged.
    key: String,
    /// The advices given in the session; a file in the old format reads 0.
    advices: u32,
    pending: Option<Pending>,
}

fn parse_pending(pending_json: &Value) -> Option<Pending> {
    let text = |name: &str| pending_json[name].as_str().map(str::to_string);
    Some(Pending {
        uuid: text("uuid")?,
        command: text("command")?,
        what: text("what")?,
        dirs: pending_json["dirs"]
            .as_array()?
            .iter()
            .filter_map(|dir| dir.as_str().map(PathBuf::from))
            .collect(),
    })
}

/// The file is JSON `{turn, advices, key, pending}`; the previous text
/// format, `<turn> <advices>\n<key>` or `<turn>\n<key>`, reads with no
/// pending boundary.
fn parse_state(text: &str) -> Option<State> {
    if text.starts_with('{') {
        let state_json: Value = serde_json::from_str(text).ok()?;
        return Some(State {
            turn: state_json["turn"].as_str()?.to_string(),
            key: state_json["key"].as_str()?.to_string(),
            advices: state_json["advices"]
                .as_u64()
                .and_then(|count| u32::try_from(count).ok())
                .unwrap_or(0),
            pending: parse_pending(&state_json["pending"]),
        });
    }
    let (head, key) = text.split_once('\n')?;
    let (turn, advices) = match head.split_once(' ') {
        Some((turn, count)) => (turn, count.parse().unwrap_or(0)),
        None => (head, 0),
    };
    Some(State {
        turn: turn.to_string(),
        key: key.to_string(),
        advices,
        pending: None,
    })
}

fn read_state(session: &str) -> Option<State> {
    parse_state(&std::fs::read_to_string(state_file(session)?).ok()?)
}

fn render_state(state: &State) -> String {
    let pending = state.pending.as_ref().map_or(Value::Null, |pending| {
        serde_json::json!({
            "uuid": pending.uuid,
            "command": pending.command,
            "what": pending.what,
            "dirs": pending.dirs.iter().map(|dir| dir.to_string_lossy()).collect::<Vec<_>>(),
        })
    });
    serde_json::json!({
        "turn": state.turn,
        "advices": state.advices,
        "key": state.key,
        "pending": pending,
    })
    .to_string()
}

fn write_state(session: &str, state: &State) -> std::io::Result<()> {
    let unusable = || std::io::Error::other("no usable state file for this session");
    let file = state_file(session).ok_or_else(unusable)?;
    let dir = file.parent().ok_or_else(unusable)?;
    std::fs::create_dir_all(dir)?;
    journal::private(dir, 0o700);
    write_atomic(&file, &render_state(state))
}

/// State files older than a week are nobody's session any more.
pub fn sweep() {
    let Some(dir) = state_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let week = std::time::Duration::from_secs(7 * 24 * 3600);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.elapsed().ok())
            .is_some_and(|age| age > week);
        if old {
            let _ = std::fs::remove_file(entry.path()); // a file already gone is swept
        }
    }
}

/// The rule already spoke in the turn this Stop ends: its state file holds
/// the opening person prompt's uuid, and the transcript still shows the same
/// prompt as the last one. `plan-phases-open` stays silent then, so the Stop
/// after the handoff Continue ends the turn.
pub fn spoke_this_turn(stop: &Stop) -> bool {
    let Some(State { turn, .. }) = read_state(&stop.session) else {
        return false;
    };
    if turn.is_empty() {
        return false;
    }
    let Some(transcript) = stop.transcript.as_deref() else {
        return false;
    };
    let Ok(tail) = read_tail_until(transcript, TAIL_FIRST_CHUNK, TAIL_CAP, |entries| {
        turn_window(entries).is_some()
    }) else {
        return false;
    };
    turn_window(&tail.entries).is_some_and(|window| window.prompt_uuid == turn)
}

/// A `source=clear` session start: the journal line the plan's Outcome (a)
/// counts.
pub fn on_session_start_clear(session: &str, cwd: &Path) {
    let repo = cwd.file_name().map_or_else(
        || "-".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    journal::record(&journal::Entry {
        rule: RULE.id,
        stance: "-",
        outcome: "watched",
        session,
        repo: &repo,
        mode: "-",
        excerpt: "session-start clear",
    });
}

fn has_waiting_line(message: &str) -> bool {
    message
        .lines()
        .any(|line| line.trim().starts_with("WAITING:"))
}

/// Silence without a journal line: what the payload alone shows.
fn payload_says_silence(stop: &Stop) -> bool {
    if stop.stop_hook_active
        || stop.permission_mode.as_deref() == Some("plan")
        || !amont_agent_guard::session_state::usable_id(&stop.session)
    {
        return true;
    }
    stop.last_message.as_deref().is_some_and(|message| {
        message.trim_end().ends_with('?') || has_waiting_line(message) || message.contains(MARKER)
    })
}

/// What the Stop shows about a boundary past [`SIZE`] tokens. `fact` gives
/// the plan's state (`plan_phases::open_phase`), and is called only once the
/// context is past the size, so a smaller Stop spawns no git for this rule.
/// `Working` is in flight, so the rule is silent. Every failure is silence.
/// At `deny` the answer is a Continue, at `advise` a note to the person
/// (except at `PlanOnly`, where `plan-phases-open` carries the plan on), at
/// `observe` silence.
pub fn on_stop(
    stop: &Stop,
    fact: impl FnOnce() -> PhaseFact,
    deadline: Instant,
    stance: impl FnOnce() -> Stance,
) -> Decision {
    if payload_says_silence(stop) {
        return Decision::Silent;
    }
    let Some(transcript) = stop.transcript.as_deref() else {
        return Decision::Silent;
    };
    let pending_uuid = read_state(&stop.session)
        .and_then(|state| state.pending)
        .map(|pending| pending.uuid);
    let Ok(tail) = read_tail_until(transcript, TAIL_FIRST_CHUNK, TAIL_CAP, |entries| {
        context_size(entries).is_some()
            && turn_window(entries).is_some()
            && pending_uuid
                .as_deref()
                .is_none_or(|uuid| position_of(entries, uuid).is_some())
    }) else {
        return Decision::Silent;
    };
    let Some(size) = context_size(&tail.entries).filter(|size| *size >= SIZE) else {
        return Decision::Silent;
    };
    let fact = fact();
    if fact == PhaseFact::Working {
        return Decision::Silent;
    }
    judge_boundary(stop, &tail, size, fact, deadline, stance()).unwrap_or(Decision::Silent)
}

/// The boundary a Stop judges: one this turn reached, or one kept pending
/// from an earlier Stop.
enum Candidate {
    Own(Pending),
    Resumed(Pending),
}

impl Candidate {
    fn boundary(&self) -> &Pending {
        match self {
            Candidate::Own(boundary) | Candidate::Resumed(boundary) => boundary,
        }
    }
}

fn position_of(entries: &[Value], uuid: &str) -> Option<usize> {
    if uuid.is_empty() {
        return None;
    }
    entries
        .iter()
        .position(|entry| entry["uuid"].as_str() == Some(uuid))
}

/// Whether the entries show work started: an Edit, Write or NotebookEdit, a
/// `git worktree add`, or an `mcp__relais__run`.
fn work_in(entries: &[Value]) -> bool {
    entries
        .iter()
        .filter(|entry| entry_type(entry) == "assistant" && !is_sidechain(entry))
        .flat_map(blocks)
        .filter(|block| block["type"].as_str() == Some("tool_use"))
        .any(|block| match block["name"].as_str().unwrap_or("") {
            "Edit" | "Write" | "NotebookEdit" | "mcp__relais__run" => true,
            "Bash" => block
                .pointer("/input/command")
                .and_then(Value::as_str)
                .is_some_and(|command| segments(command).iter().any(|words| adds_worktree(words))),
            _ => false,
        })
}

fn adds_worktree(words: &[String]) -> bool {
    git_call(words).is_some_and(|call| {
        call.subcommand == "worktree"
            && call
                .rest
                .iter()
                .find(|word| !word.starts_with('-'))
                .map(String::as_str)
                == Some("add")
    })
}

/// `None` is silence, journalled or not as each check decides.
fn judge_boundary(
    stop: &Stop,
    tail: &Tail,
    size: u64,
    fact: PhaseFact,
    deadline: Instant,
    stance: Stance,
) -> Option<Decision> {
    let cwd = stop.cwd.as_deref();
    let repo = cwd.and_then(Path::file_name).map_or_else(
        || "-".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let mode = stop.permission_mode.as_deref().unwrap_or("-");
    let capped = if tail.capped { " tail-capped" } else { "" };
    let record = |outcome: &str, what: &str| {
        journal::record(&journal::Entry {
            rule: RULE.id,
            stance: stance.as_str(),
            outcome,
            session: &stop.session,
            repo: &repo,
            mode,
            excerpt: &format!("{what}{capped}"),
        });
    };
    let watched = |what: &str| {
        record("watched", what);
        None
    };
    // A state file that cannot be written loses the boundary key and the
    // count: say so, and stay silent rather than advise without them.
    let persist = |state: &State| -> bool {
        let written = write_state(&stop.session, state).is_ok();
        if !written {
            record("watched", "state-unwritable");
        }
        written
    };

    if fact == PhaseFact::TimedOut {
        return watched("timeout");
    }
    let Some(window) = turn_window(&tail.entries) else {
        return watched("turn-too-long");
    };
    let mut state = read_state(&stop.session).unwrap_or_default();
    let found = boundaries(&window, cwd.unwrap_or(Path::new("")));
    let own = found.last().filter(|boundary| {
        state
            .pending
            .as_ref()
            .is_none_or(|pending| pending.uuid != boundary.uuid)
    });
    let candidate = match (own, state.pending.clone()) {
        (Some(boundary), _) => {
            let mut dirs: Vec<PathBuf> = Vec::new();
            for dir in found.iter().flat_map(|boundary| boundary.dirs.iter()) {
                if !dirs.contains(dir) {
                    dirs.push(dir.clone());
                }
            }
            Candidate::Own(Pending {
                uuid: boundary.uuid.clone(),
                command: boundary.command.clone(),
                what: boundary.what.clone(),
                dirs,
            })
        }
        (None, None) => return watched("no boundary"),
        (None, Some(pending)) => {
            let Some(at) = position_of(&tail.entries, &pending.uuid) else {
                state.pending = None;
                persist(&state);
                return watched("pending-lost");
            };
            let after = &tail.entries[at + 1..];
            if after.iter().any(is_person_prompt) {
                state.pending = None;
                persist(&state);
                return watched("no boundary");
            }
            if work_in(after) {
                return watched("pending-work");
            }
            Candidate::Resumed(pending)
        }
    };
    let hold = |state: &mut State, reason: &str| -> Option<Decision> {
        if let Candidate::Own(boundary) = &candidate {
            state.pending = Some(boundary.clone());
            if !persist(state) {
                return None;
            }
        }
        record("watched", &format!("pending {reason}"));
        None
    };
    if stop.background_busy {
        return hold(&mut state, "background");
    }
    let flight = relais_flight(&tail.entries);
    if let Some((id, relais_state)) = flight.waiting.first() {
        return hold(&mut state, &format!("relais {id} {relais_state}"));
    }
    if flight.pending > 0 {
        return hold(&mut state, &format!("relais pending {}", flight.pending));
    }
    let boundary = candidate.boundary();
    let heads = match check_repositories(cwd, &boundary.dirs, deadline) {
        Repositories::Clean { heads } => heads,
        Repositories::NotClean { reason, .. } => return hold(&mut state, &reason),
        Repositories::TimedOut => return watched("timeout"),
    };
    let key = boundary_key(&boundary.command, &heads);
    let advices = state.advices;
    if state.key == key {
        if state.pending.is_some() {
            state.pending = None;
            persist(&state);
        }
        return None;
    }
    if advices >= MAX_ADVICES_PER_SESSION {
        return watched("cap");
    }
    // At `advise` a note cannot carry the plan on, and a lone plan commit's
    // next step is the plan's: `plan-phases-open` speaks instead.
    let carried_by_plan = stance == Stance::Advise && fact == PhaseFact::PlanOnly;
    let counted = stance != Stance::Observe && !carried_by_plan;
    // Only a Continue asks for a handoff Stop, which `spoke_this_turn` lets end.
    let turn = if stance == Stance::Deny {
        window.prompt_uuid.as_str()
    } else {
        ""
    };
    let state = State {
        turn: turn.to_string(),
        key,
        advices: advices + u32::from(counted),
        pending: None,
    };
    if !persist(&state) {
        return None;
    }
    if carried_by_plan {
        return watched(&format!(
            "{} (advise, plan-phases-open carries the plan)",
            boundary.what
        ));
    }
    let fact = format!(
        "this turn reached a boundary ({}) with nothing in flight, and the context is at {}k tokens, past {}k.",
        boundary.what,
        size / 1000,
        SIZE / 1000
    );
    match stance {
        Stance::Observe => watched(&format!("{} (observe)", boundary.what)),
        Stance::Advise => {
            record("advised", &boundary.what);
            let remedy = "Say `hand off` to have the agent write the handoff, then /clear.";
            Some(Decision::UserNote(decision::phrase(RULE.id, &fact, remedy)))
        }
        Stance::Deny => {
            record("denied", &boundary.what);
            let remedy = format!(
                "The handoff is then complete when the plan or ticket states the next action and the turn ends with one line: \"{MARKER}; next: `<the exact first prompt>`\"."
            );
            Some(Decision::Continue(decision::phrase(
                RULE.id, &fact, &remedy,
            )))
        }
    }
}

#[cfg(test)]
pub(crate) mod testrepo {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A throwaway repository on branch `main`, removed on drop.
    pub(crate) struct TestRepo {
        directory: PathBuf,
    }

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    impl TestRepo {
        pub(crate) fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "amont-clear-boundary-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&directory).expect("create test repository directory");
            let repo = Self { directory };
            repo.git(&["init", "-q", "-b", "main"]);
            repo
        }

        pub(crate) fn path(&self) -> &Path {
            &self.directory
        }

        pub(crate) fn git(&self, args: &[&str]) {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.directory)
                .args([
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@example.com",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .expect("run git");
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        pub(crate) fn write(&self, relative: &str, content: &str) {
            let path = self.directory.join(relative);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("create parents");
            std::fs::write(path, content).expect("write file");
        }

        pub(crate) fn commit_file(&self, relative: &str, content: &str, message: &str) {
            self.write(relative, content);
            self.git(&["add", "--", relative]);
            self.git(&["commit", "-q", "-m", message]);
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory); // a leftover temp directory is harmless
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testrepo::TestRepo;
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn assistant(usage: Option<(u64, u64, u64)>, sidechain: bool, blocks: Value) -> Value {
        let mut message = json!({"role": "assistant", "content": blocks});
        if let Some((input, creation, read)) = usage {
            message["usage"] = json!({
                "input_tokens": input,
                "cache_creation_input_tokens": creation,
                "cache_read_input_tokens": read,
            });
        }
        json!({"type": "assistant", "isSidechain": sidechain, "message": message})
    }

    fn prompt(uuid: &str) -> Value {
        json!({"type": "user", "uuid": uuid, "message": {"role": "user", "content": "go"}})
    }

    fn bash(id: &str, command: &str) -> Value {
        assistant(
            None,
            false,
            json!([{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}}]),
        )
    }

    fn tool_use(id: &str, name: &str) -> Value {
        assistant(
            None,
            false,
            json!([{"type": "tool_use", "id": id, "name": name, "input": {}}]),
        )
    }

    fn tool_result(id: &str, text: &str) -> Value {
        json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": id, "content": [{"type": "text", "text": text}]}
        ]}})
    }

    fn found(commands: &[&str]) -> Vec<Boundary> {
        let mut entries = vec![prompt("p1")];
        for (at, command) in commands.iter().enumerate() {
            entries.push(bash(&format!("b{at}"), command));
        }
        let window = turn_window(&entries).expect("a window");
        boundaries(&window, Path::new("/work"))
    }

    fn write_lines(path: &Path, entries: &[Value]) {
        let text: String = entries.iter().map(|e| format!("{e}\n")).collect();
        std::fs::write(path, text).expect("write transcript");
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(20)
    }

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("cb-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("scratch directory");
        directory
    }

    #[test]
    fn the_tail_grows_by_chunks_and_drops_a_cut_first_line() {
        let directory = scratch("tail");
        let path = directory.join("t.jsonl");
        let filler = "x".repeat(200);
        let mut entries = vec![prompt("old")];
        for at in 0..40 {
            entries.push(bash(&format!("b{at}"), &filler));
        }
        entries.push(assistant(Some((1, 2, 3)), false, json!([])));
        write_lines(&path, &entries);
        let tail = read_tail_until(&path, 512, 1 << 20, |e| context_size(e).is_some())
            .expect("read the tail");
        assert!(!tail.capped);
        assert!(tail.entries.len() < entries.len(), "only a tail was read");
        assert_eq!(context_size(&tail.entries), Some(6));
        let everything = read_tail_until(&path, 512, 1 << 20, |_| false).expect("read all");
        assert_eq!(everything.entries.len(), entries.len());
        assert!(!everything.capped);
        std::fs::remove_dir_all(&directory).expect("clean up");
    }

    #[test]
    fn below_the_size_the_plan_state_is_never_asked_for() {
        let directory = scratch("lazy");
        let path = directory.join("t.jsonl");
        write_lines(
            &path,
            &[
                prompt("p1"),
                bash("b1", "git worktree remove ../x"),
                tool_result("b1", "ok"),
                assistant(Some((10, 0, 149_990)), false, json!([])),
            ],
        );
        let stop = Stop {
            session: "lazy-session".to_string(),
            cwd: Some(directory.clone()),
            permission_mode: None,
            last_message: None,
            background_busy: false,
            stop_hook_active: false,
            transcript: Some(path),
        };
        let asked = std::cell::Cell::new(false);
        let decision = on_stop(
            &stop,
            || {
                asked.set(true);
                PhaseFact::NoOpenPhase
            },
            deadline(),
            || Stance::Deny,
        );
        assert!(matches!(decision, Decision::Silent));
        assert!(!asked.get(), "open_phase was asked for at 150k");
        std::fs::remove_dir_all(&directory).expect("clean up");
    }

    #[test]
    fn the_cap_is_reported_when_the_file_start_is_not_reached() {
        let directory = scratch("cap");
        let path = directory.join("t.jsonl");
        let filler = "y".repeat(300);
        let entries: Vec<Value> = (0..30).map(|at| bash(&format!("b{at}"), &filler)).collect();
        write_lines(&path, &entries);
        let tail = read_tail_until(&path, 256, 1024, |_| false).expect("read");
        assert!(tail.capped);
        assert!(tail.entries.len() < 30);
        std::fs::remove_dir_all(&directory).expect("clean up");
    }

    #[test]
    fn a_sidechain_usage_after_the_main_chain_one_is_ignored() {
        let entries = vec![
            assistant(Some((10, 20, 30)), false, json!([])),
            assistant(Some((1_000_000, 0, 0)), true, json!([])),
        ];
        assert_eq!(context_size(&entries), Some(60));
    }

    #[test]
    fn the_size_sums_the_three_usage_fields_of_the_last_entry_with_usage() {
        let entries = vec![
            assistant(Some((5, 5, 5)), false, json!([])),
            assistant(Some((100_000, 50_000, 25_000)), false, json!([])),
            assistant(None, false, json!([])),
        ];
        assert_eq!(context_size(&entries), Some(175_000));
        assert_eq!(context_size(&[]), None);
    }

    #[test]
    fn no_person_prompt_in_the_tail_is_no_window() {
        let entries = vec![
            bash("b", "ls"),
            tool_result("b", "files"),
            assistant(Some((1, 1, 1)), false, json!([])),
        ];
        assert!(turn_window(&entries).is_none());
    }

    #[test]
    fn the_window_starts_after_the_last_prompt_that_is_no_tool_result() {
        let entries = vec![
            prompt("first"),
            bash("b", "ls"),
            prompt("second"),
            bash("c", "pwd"),
            tool_result("c", "/"),
        ];
        let window = turn_window(&entries).expect("a window");
        assert_eq!(window.prompt_uuid, "second");
        assert_eq!(window.entries.len(), 2);
    }

    #[test]
    fn every_boundary_shape_is_found_with_its_label() {
        let worktree = found(&["git worktree remove ../wt"]);
        assert_eq!(worktree.len(), 1);
        assert_eq!(worktree[0].what, "worktree remove");
        assert_eq!(found(&["git -C repo worktree remove --force x"]).len(), 1);
        assert_eq!(
            found(&["python3 ~/bin/sweep.py --prune --apply"])[0].what,
            "sweep.py --apply"
        );
        assert!(found(&["python3 sweep.py --dry-run"]).is_empty());
        assert!(found(&["python3 other.py --apply"]).is_empty());
        let plan = found(&["git commit -m \"docs(plan): add the plan\""]);
        assert_eq!(plan[0].what, "plan commit");
        let heredoc = found(&["git commit -m \"$(cat <<'EOF'\ndocs(plan): x\n\nbody\nEOF\n)\""]);
        assert_eq!(heredoc.len(), 1);
        let chained = found(&["cd x && git add . && git commit -am 'docs(plan): y'"]);
        assert_eq!(chained.len(), 1);
        assert!(found(&["git commit -m 'feat: x'"]).is_empty());
    }

    #[test]
    fn closing_plan_commits_are_not_boundaries() {
        assert!(found(&["git commit -m 'docs(plan): x, done'"]).is_empty());
        assert!(found(&["git commit -m 'docs(plan): x, phase 2'"]).is_empty());
        assert_eq!(found(&["git commit -m 'docs(plan): phases'"]).len(), 1);
    }

    #[test]
    fn dash_c_directories_resolve_against_the_cwd_and_chain() {
        let relative = found(&["git -C repo worktree remove w"]);
        assert_eq!(relative[0].dirs, vec![PathBuf::from("/work/repo")]);
        let absolute = found(&["git -C /abs worktree remove w"]);
        assert_eq!(absolute[0].dirs, vec![PathBuf::from("/abs")]);
        let chained = found(&["git -C repo -C ../other worktree remove w"]);
        assert_eq!(chained[0].dirs, vec![PathBuf::from("/work/other")]);
        let none = found(&["git worktree remove w"]);
        assert!(none[0].dirs.is_empty());
    }

    const STARTED: &str =
        "Started relais run for: /w/.relais/tasks/abc123.json (in /w/repo). Waiting.";

    fn plugin_message(text: &str) -> Value {
        json!({"type": "user", "uuid": "m", "message": {"role": "user",
            "content": format!("The relais plugin sent a message:\n{text}")}})
    }

    fn decide(run: &str) -> Value {
        assistant(
            None,
            false,
            json!([{"type": "tool_use", "id": "d", "name": "mcp__relais__decide", "input": {"run": run}}]),
        )
    }

    fn flight_after(extra: Vec<Value>) -> RelaisFlight {
        let mut entries = vec![
            prompt("p"),
            tool_use("r1", "mcp__relais__run"),
            tool_result("r1", STARTED),
        ];
        entries.extend(extra);
        relais_flight(&entries)
    }

    #[test]
    fn a_start_with_no_finished_message_is_in_flight() {
        let flight = flight_after(vec![]);
        assert_eq!(flight.pending, 1);
        assert!(flight.in_flight());
    }

    #[test]
    fn a_start_in_an_earlier_turn_is_in_flight() {
        assert!(flight_after(vec![prompt("later turn")]).in_flight());
    }

    #[test]
    fn a_finished_accepted_run_is_not_in_flight() {
        let flight = flight_after(vec![plugin_message(
            "relais run run-65abc-1f2e finished: accepted.",
        )]);
        assert!(!flight.in_flight());
    }

    #[test]
    fn a_run_that_needs_a_decision_is_in_flight_until_decided() {
        let finished = plugin_message("relais run run-65abc-1f2e finished: needs_decision (x)");
        let waiting = flight_after(vec![finished.clone()]);
        assert_eq!(
            waiting.waiting,
            vec![("run-65abc-1f2e".to_string(), "needs_decision".to_string())]
        );
        assert!(flight_after(vec![finished.clone(), decide("run-other")]).in_flight());
        assert!(!flight_after(vec![finished, decide("run-65abc-1f2e")]).in_flight());
    }

    #[test]
    fn a_cancelled_run_that_launches_a_successor_stays_in_flight() {
        let successor = plugin_message(
            "relais run run-65abc-1f2e finished: cancelled (scope_widened_for_tests). relais launches successor intent x.",
        );
        assert!(flight_after(vec![successor.clone()]).in_flight());
        let exited = plugin_message("relais exited with code 9 without an outcome.");
        assert!(!flight_after(vec![successor, exited]).in_flight());
    }

    #[test]
    fn a_refused_start_is_not_in_flight() {
        let entries = vec![
            prompt("p"),
            tool_use("r1", "mcp__relais__run"),
            tool_result("r1", "Nothing ran: the scope is empty."),
        ];
        assert!(!relais_flight(&entries).in_flight());
    }

    #[test]
    fn a_started_text_from_another_tool_does_not_count() {
        let entries = vec![tool_use("b", "Bash"), tool_result("b", STARTED)];
        assert!(!relais_flight(&entries).in_flight());
    }

    #[test]
    fn a_plans_only_unpushed_commit_with_a_non_ascii_path_is_clean() {
        let repo = TestRepo::new();
        repo.commit_file("README.md", "x", "init");
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.commit_file("docs/plans/é.md", "plan", "docs(plan): é");
        match check_repositories(Some(repo.path()), &[], deadline()) {
            Repositories::Clean { heads } => {
                assert_eq!(heads.len(), 1);
                assert_eq!(heads[0].1.len(), 40);
            }
            other => panic!("expected clean, got {other:?}"),
        }
    }

    #[test]
    fn a_dirty_tree_is_not_clean() {
        let repo = TestRepo::new();
        repo.commit_file("README.md", "x", "init");
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.write("scratch.txt", "dirty");
        assert!(matches!(
            check_repositories(Some(repo.path()), &[], deadline()),
            Repositories::NotClean { .. }
        ));
    }

    #[test]
    fn an_unpushed_non_plan_commit_is_not_clean() {
        let repo = TestRepo::new();
        repo.commit_file("README.md", "x", "init");
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.commit_file("src/lib.rs", "fn f() {}", "feat: f");
        assert!(matches!(
            check_repositories(Some(repo.path()), &[], deadline()),
            Repositories::NotClean { .. }
        ));
    }

    #[test]
    fn a_missing_directory_is_clean_and_a_passed_deadline_is_timed_out() {
        let missing = PathBuf::from("/nonexistent/removed-worktree");
        assert_eq!(
            check_repositories(None, &[missing], deadline()),
            Repositories::Clean { heads: vec![] }
        );
        let repo = TestRepo::new();
        repo.commit_file("README.md", "x", "init");
        assert_eq!(
            check_repositories(Some(repo.path()), &[], Instant::now()),
            Repositories::TimedOut
        );
    }

    #[test]
    fn the_key_is_the_command_and_each_head() {
        let heads = vec![(PathBuf::from("/r"), "abc".to_string())];
        assert_eq!(boundary_key("cmd", &heads), "cmd\n/r abc");
        assert_ne!(
            boundary_key("cmd", &heads),
            boundary_key("cmd", &[(PathBuf::from("/r"), "def".to_string())])
        );
    }

    #[test]
    fn injected_prompts_are_no_person_prompts_but_still_open_a_turn() {
        let notification = json!({"type": "user", "uuid": "n", "message": {"role": "user",
            "content": "<task-notification>\nx</task-notification>"}});
        let framed = json!({"type": "user", "uuid": "m", "message": {"role": "user",
            "content": [{"type": "text", "text": "The relais plugin sent a message:\nhi"}]}});
        assert!(is_injected(&notification) && is_injected(&framed));
        assert!(!is_person_prompt(&notification) && !is_person_prompt(&framed));
        assert!(is_person_prompt(&prompt("p")) && !is_injected(&prompt("p")));
        let entries = vec![prompt("p"), notification];
        assert_eq!(turn_window(&entries).expect("a window").prompt_uuid, "n");
    }

    #[test]
    fn the_state_reads_both_formats_and_round_trips_a_pending_boundary() {
        let old = parse_state("t1 2\nkey\nmore").expect("old format");
        assert_eq!(
            (old.turn.as_str(), old.advices, old.pending),
            ("t1", 2, None)
        );
        assert_eq!(old.key, "key\nmore");
        let older = parse_state("t1\nk").expect("older format");
        assert_eq!((older.advices, older.key.as_str()), (0, "k"));
        let state = State {
            turn: "t".into(),
            key: "k\nv".into(),
            advices: 1,
            pending: Some(Pending {
                uuid: "u".into(),
                command: "git worktree remove x".into(),
                what: "worktree remove".into(),
                dirs: vec![PathBuf::from("/r")],
            }),
        };
        assert_eq!(parse_state(&render_state(&state)), Some(state));
    }
}
