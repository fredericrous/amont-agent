//! Whether the pages a question names were opened for the person
//! (`ask-unopened-page`, decisions ADR-0058, `work.page-shown-before-asked`).
//!
//! Two halves. [`pages_in`] finds the local pages a text names, narrowly and
//! literally; [`unopened`] reads the session transcript backwards and says
//! which of them no `amont-agent open` (or `preview register --open` that
//! reported `opened: true`) showed since the last answered question about
//! that page.
//!
//! A page is compared by its key: a `file://` URL with the path made
//! absolute and percent-encoded as `amont-agent open` does it, or the URL as
//! written. Fragments, queries, trailing slashes and `index.html` are part of
//! the key: they can name distinct views, and no HTTP route alias is
//! inferred.
//!
//! Only structured transcript entries are read: a `tool_use` matched by id to
//! a non-error `tool_result`. No free text is taken for an open; the one
//! result text read is the registration JSON of `preview register --open`.
//!
//! "Asked again since the last answered question" stands in for ADR-0058's
//! "the page changed since the person last saw it".
//! holds-until: `preview register` (or `amont-agent open`) records a content
//! hash of the page it opened, so a change can be told from a re-ask.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::payload::{self, Ask};
use crate::shell::{self, Connector, Parsed, Simple};
use crate::skill_window::READ_BACK;

const LOCAL_HOSTS: [&str; 4] = ["localhost", "127.0.0.1", "[::1]", "0.0.0.0"];

/// A local page a text names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// As the question wrote it.
    pub written: String,
    /// What it is compared by.
    pub key: String,
}

/// What the hook should do about a question.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No page is named, or every one was opened.
    Shown,
    /// These pages, as written, were not opened.
    Unopened(Vec<String>),
    /// The transcript cannot say; the pages are those named.
    Unknown { pages: Vec<String>, why: Blind },
}

/// Why the transcript cannot say whether a page was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blind {
    NoTranscript,
    Unreadable,
    LineInProgress,
    PastReadBack,
}

impl std::fmt::Display for Blind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Blind::NoTranscript => "the payload names no transcript",
            Blind::Unreadable => "the transcript could not be read",
            Blind::LineInProgress => "the transcript ends in a line still being written",
            Blind::PastReadBack => "the window is longer than the hook reads back",
        })
    }
}

/// The pages named by a question and its options.
pub fn pages_of_ask(ask: &Ask) -> Vec<Page> {
    distinct(
        ask.questions
            .iter()
            .chain(&ask.option_texts)
            .flat_map(|text| pages_in(text, &ask.cwd)),
    )
}

/// The pages `text` names, relative paths read against `cwd`.
pub fn pages_in(text: &str, cwd: &Path) -> Vec<Page> {
    let spaced = text.replace("](", " ");
    distinct(
        spaced
            .split(char::is_whitespace)
            .map(trim_token)
            .filter(|token| !token.is_empty() && is_page(token))
            .map(|token| Page {
                written: token.to_string(),
                key: key_of(token, cwd),
            }),
    )
}

/// The pages in order, each key once (the first spelling wins).
fn distinct(pages: impl Iterator<Item = Page>) -> Vec<Page> {
    let mut out: Vec<Page> = Vec::new();
    for page in pages {
        if !out.iter().any(|p| p.key == page.key) {
            out.push(page);
        }
    }
    out
}

fn trim_token(raw: &str) -> &str {
    let mut token = raw.trim_start_matches(['"', '\'', '`', '*', '<', '(', '{', '[']);
    while let Some(last) = token.chars().last() {
        let drop = match last {
            '"' | '\'' | '`' | '*' | '>' | ',' | ';' | '.' | ':' | '!' | '?' | '}' => true,
            ')' => token.matches('(').count() < token.matches(')').count(),
            ']' => token.matches('[').count() < token.matches(']').count(),
            _ => false,
        };
        if !drop {
            break;
        }
        token = &token[..token.len() - last.len_utf8()];
    }
    token
}

fn is_page(token: &str) -> bool {
    if let Some(at) = token.find("://") {
        let scheme = &token[..at];
        if scheme.is_empty()
            || !scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        {
            return false;
        }
        if scheme.eq_ignore_ascii_case("file") {
            return true;
        }
        return LOCAL_HOSTS.contains(&host_of(&token[at + 3..]).to_ascii_lowercase().as_str());
    }
    // A file name with a stem: a bare `.html` or a glob `*.html` names an
    // extension, not a page (a false deny in the 14-day backtest).
    let lower = token.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or("");
    let stem = name
        .strip_suffix(".html")
        .or_else(|| name.strip_suffix(".htm"))
        .unwrap_or("");
    !stem.is_empty() && !stem.contains(['*', '?', '<', '{'])
}

/// The host of `authority/path…`, without userinfo or port.
fn host_of(rest: &str) -> String {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or("");
    if authority.starts_with('[') {
        return match authority.find(']') {
            Some(end) => authority[..=end].to_string(),
            None => authority.to_string(),
        };
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host.to_string(),
        _ => authority.to_string(),
    }
}

/// What a page is compared by: a URL as written (a `file://` URL canonical),
/// a path made absolute against `cwd` and turned into a `file://` URL.
pub fn key_of(target: &str, cwd: &Path) -> String {
    if target
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("file://"))
    {
        let rest = &target[7..];
        let (path_part, suffix) = match rest.find(['?', '#']) {
            Some(at) => rest.split_at(at),
            None => (rest, ""),
        };
        let path = match path_part.find('/') {
            Some(0) => path_part,
            Some(slash) if path_part[..slash].eq_ignore_ascii_case("localhost") => {
                &path_part[slash..]
            }
            _ => return target.to_string(),
        };
        return format!("{}{suffix}", file_url(&decode(path)));
    }
    if target.contains("://") {
        return target.to_string();
    }
    let joined = if target.starts_with('/') || Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        cwd.join(target)
    };
    file_url(&joined.to_string_lossy())
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `file://` URL of an absolute path, split on the platform's separators
/// (`/`, and `\` too on Windows, so a joined `C:\repo\x.html` keys like the
/// opener's URL), `.` and `..` read lexically, with
/// every byte outside `[A-Za-z0-9/-_.~:]` percent-encoded. The encoding is the
/// one `amont-agent open` uses (`guide::file_url`); a test in the binary's
/// suite pins the two together, because the guard cannot depend on that crate.
fn file_url(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(std::path::is_separator) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let mut out = String::from("file://");
    for part in parts {
        out.push('/');
        for b in part.bytes() {
            if b.is_ascii_alphanumeric() || b"-_.~:".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    if out == "file://" {
        out.push('/');
    }
    out
}

/// Judge a question against its session transcript.
pub fn judge(ask: &Ask) -> Verdict {
    let pages = pages_of_ask(ask);
    if pages.is_empty() {
        return Verdict::Shown;
    }
    let written = || pages.iter().map(|p| p.written.clone()).collect();
    let Some(path) = &ask.transcript else {
        return Verdict::Unknown {
            pages: written(),
            why: Blind::NoTranscript,
        };
    };
    let Ok(file) = std::fs::File::open(path) else {
        return Verdict::Unknown {
            pages: written(),
            why: Blind::Unreadable,
        };
    };
    match unopened(file, &pages, &ask.tool_use_id) {
        Ok(missing) if missing.is_empty() => Verdict::Shown,
        Ok(missing) => Verdict::Unopened(
            missing
                .into_iter()
                .map(|i| pages[i].written.clone())
                .collect(),
        ),
        Err(why) => Verdict::Unknown {
            pages: written(),
            why,
        },
    }
}

/// The text a refusal gives: every page, one command each.
pub fn commands(pages: &[String]) -> String {
    pages
        .iter()
        .map(|p| {
            format!(
                concat!("  ", crate::bin!(), " open '{}'"),
                p.replace('\'', "'\\''")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Pending,
    Opened,
    Unopened,
}

/// Indices into `pages` with no opener in their window, read from `file`
/// backwards. `current` is the tool-use id of the question being judged.
/// `Err` when the transcript cannot say.
pub fn unopened(
    file: impl Read + Seek,
    pages: &[Page],
    current: &str,
) -> Result<Vec<usize>, Blind> {
    unopened_within(file, pages, current, READ_BACK)
}

fn unopened_within(
    file: impl Read + Seek,
    pages: &[Page],
    current: &str,
    budget: u64,
) -> Result<Vec<usize>, Blind> {
    let mut shown = Shown::new(pages);
    // Results are written after their calls, so a backwards scan meets the
    // result first and remembers it until the call turns up.
    let mut completed: HashMap<String, String> = HashMap::new();
    scan_backwards(file, budget, |raw| {
        let Ok(text) = std::str::from_utf8(raw) else {
            return false;
        };
        if !text.contains("\"tool_result\"") && !text.contains("\"tool_use\"") {
            return false;
        }
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(text) else {
            return false;
        };
        let cwd = Path::new(entry.get("cwd").and_then(|c| c.as_str()).unwrap_or(""));
        let blocks = entry
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array());
        for block in blocks.into_iter().flatten() {
            let id = block
                .get("tool_use_id")
                .or_else(|| block.get("id"))
                .and_then(|i| i.as_str())
                .unwrap_or("");
            match block.get("type").and_then(|t| t.as_str()) {
                Some("tool_result") => {
                    if block.get("is_error").and_then(|e| e.as_bool()) != Some(true) {
                        completed.insert(id.to_string(), result_text(block));
                    }
                }
                Some("tool_use") if id != current => {
                    let Some(tool_output) = completed.get(id) else {
                        continue;
                    };
                    let input = block.get("input");
                    match block.get("name").and_then(|n| n.as_str()) {
                        Some("Bash") => {
                            let command = input
                                .and_then(|i| i.get("command"))
                                .and_then(|c| c.as_str())
                                .unwrap_or("");
                            let keys = opened_keys(command, cwd, tool_output);
                            shown.settle(&keys, State::Opened);
                        }
                        Some("AskUserQuestion") => {
                            let keys = input.map(|i| asked_keys(i, cwd)).unwrap_or_default();
                            shown.settle(&keys, State::Unopened);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        shown.undecided() == 0
    })?;
    Ok(shown.not_opened())
}

/// The state of each page being judged, owned in one place.
struct Shown<'a> {
    pages: &'a [Page],
    states: Vec<State>,
}

impl<'a> Shown<'a> {
    fn new(pages: &'a [Page]) -> Shown<'a> {
        Shown {
            pages,
            states: vec![State::Pending; pages.len()],
        }
    }

    /// Settle every still-pending page whose key is in `keys`.
    fn settle(&mut self, keys: &[String], to: State) {
        for (state, page) in self.states.iter_mut().zip(self.pages) {
            if *state == State::Pending && keys.contains(&page.key) {
                *state = to;
            }
        }
    }

    fn undecided(&self) -> usize {
        self.states.iter().filter(|s| **s == State::Pending).count()
    }

    fn not_opened(&self) -> Vec<usize> {
        (0..self.states.len())
            .filter(|i| self.states[*i] != State::Opened)
            .collect()
    }
}

fn result_text(block: &serde_json::Value) -> String {
    let text = match block.get("content") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    // Only a registration's JSON is ever read back.
    if text.contains("\"opened\"") {
        text
    } else {
        String::new()
    }
}

fn asked_keys(input: &serde_json::Value, cwd: &Path) -> Vec<String> {
    let questions = input
        .get("questions")
        .and_then(|q| q.as_array())
        .into_iter()
        .flatten()
        .filter_map(|q| q.get("question").and_then(|t| t.as_str()))
        .map(str::to_string);
    distinct(
        questions
            .chain(payload::option_texts(input))
            .flat_map(|text| pages_in(&text, cwd)),
    )
    .into_iter()
    .map(|p| p.key)
    .collect()
}

/// The keys a successful Bash call showed the person.
fn opened_keys(command: &str, cwd: &Path, tool_output: &str) -> Vec<String> {
    if !command.contains(crate::names::BIN) && !command.contains(crate::names::ALIAS) {
        return Vec::new();
    }
    let Parsed::Clear(clauses) = shell::lex(command) else {
        return Vec::new();
    };
    if clauses
        .iter()
        .any(|c| c.opaque.is_some() || c.nested.is_some())
    {
        return Vec::new();
    }
    // A page counts as shown only in the two shapes where the command's own
    // output is what the transcript recorded: alone, or after one `cd … &&`.
    let shown_by = |clause: &Simple, base: &Path| -> Option<String> {
        if is_register_open(clause) {
            registered_page(tool_output, base)
        } else {
            open_target(clause).map(|target| key_of(target, base))
        }
    };
    match clauses.as_slice() {
        [only] if only.prev.is_none() && only.next.is_none() => {
            shown_by(only, cwd).into_iter().collect()
        }
        [cd, last]
            if cd.prev.is_none()
                && cd.next == Some(Connector::AndAnd)
                && last.prev == Some(Connector::AndAnd)
                && last.next.is_none() =>
        {
            let Some(dir) = cd_target(cd) else {
                return Vec::new();
            };
            shown_by(last, &cwd.join(dir)).into_iter().collect()
        }
        _ => Vec::new(),
    }
}

fn is_amont_agent(clause: &Simple) -> bool {
    clause
        .words
        .first()
        .is_some_and(|w| crate::names::is_ours_program(&w.text))
}

fn plain(clause: &Simple) -> bool {
    clause.redirects.is_empty()
        && !clause.heredoc
        && clause
            .words
            .iter()
            .all(|w| !w.expanded && !w.text.contains(['$', '`']))
}

fn is_register_open(clause: &Simple) -> bool {
    is_amont_agent(clause)
        && clause.words.get(1).is_some_and(|w| w.text == "preview")
        && clause.words.get(2).is_some_and(|w| w.text == "register")
        && clause.words.iter().any(|w| w.text == "--open")
}

/// `amont-agent open <target>`, exactly, with a target that is its own value.
fn open_target(clause: &Simple) -> Option<&str> {
    match clause.words.as_slice() {
        [_, verb, target]
            if is_amont_agent(clause)
                && plain(clause)
                && verb.text == "open"
                && !target.text.starts_with('~')
                && !target.text.is_empty() =>
        {
            Some(&target.text)
        }
        _ => None,
    }
}

/// `cd <dir>`, a literal directory.
fn cd_target(clause: &Simple) -> Option<&str> {
    match clause.words.as_slice() {
        [cd, dir] if cd.text == "cd" && plain(clause) && !dir.text.starts_with(['~', '-']) => {
            Some(&dir.text)
        }
        _ => None,
    }
}

/// The key of the `page` a registration reported as opened.
fn registered_page(tool_output: &str, cwd: &Path) -> Option<String> {
    let parsed = std::iter::once(tool_output)
        .chain(tool_output.lines())
        .map(str::trim)
        .filter(|l| l.starts_with('{'))
        .find_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())?;
    if parsed.get("opened").and_then(|o| o.as_bool()) != Some(true) {
        return None;
    }
    let page = parsed.get("page").and_then(|p| p.as_str())?;
    Some(key_of(page, cwd))
}

/// Feed `each` the transcript's lines from the last to the first until it
/// returns true. `Err` when the last line is still being written, or when the
/// scan would read further back than `budget` bytes.
fn scan_backwards(
    mut file: impl Read + Seek,
    budget: u64,
    mut each: impl FnMut(&[u8]) -> bool,
) -> Result<(), Blind> {
    const CHUNK: u64 = 256 * 1024;
    let mut pos = file.seek(SeekFrom::End(0)).map_err(|_| Blind::Unreadable)?;
    let end = pos;
    let mut carry: Vec<u8> = Vec::new();
    let mut first = true;
    loop {
        if end - pos > budget {
            return Err(Blind::PastReadBack);
        }
        let chunk_len = CHUNK.min(pos);
        pos -= chunk_len;
        let mut buf = vec![0u8; chunk_len as usize];
        file.seek(SeekFrom::Start(pos))
            .map_err(|_| Blind::Unreadable)?;
        file.read_exact(&mut buf).map_err(|_| Blind::Unreadable)?;
        buf.extend_from_slice(&carry);
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
                return Err(Blind::LineInProgress);
            }
            if each(raw) {
                return Ok(());
            }
        }
        if pos == 0 {
            return Ok(());
        }
        carry = buf[..cut].to_vec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<String> {
        pages_in(text, Path::new("/work/repo"))
            .into_iter()
            .map(|p| p.written)
            .collect()
    }

    #[test]
    fn local_pages_are_found_literally() {
        assert_eq!(
            keys("see docs/mockups/a/options.html, or http://localhost:3000/#/A."),
            ["docs/mockups/a/options.html", "http://localhost:3000/#/A"]
        );
        assert_eq!(
            keys("(http://[::1]:80/x) and file:///tmp/a.html"),
            ["http://[::1]:80/x", "file:///tmp/a.html"]
        );
        assert_eq!(
            keys("[the page](http://0.0.0.0:8/p)"),
            ["http://0.0.0.0:8/p"]
        );
    }

    #[test]
    fn remote_and_schemeless_hosts_are_not_pages() {
        assert!(keys("127.0.0.1:8080 and https://github.com/a/b and localhost:3000").is_empty());
        assert!(keys("http://localhost.evil.com/ x.htmlx").is_empty());
    }

    #[test]
    fn an_extension_alone_is_not_a_page() {
        assert!(keys("a local URL or `.html` file, any *.html or docs/*.htm").is_empty());
        assert_eq!(keys("docs/x.html"), ["docs/x.html"]);
    }

    #[test]
    fn a_path_and_its_file_url_share_a_key() {
        let cwd = Path::new("/work/my repo");
        assert_eq!(
            key_of("docs/../docs/a b.html", cwd),
            key_of("file:///work/my%20repo/docs/a%20b.html", cwd)
        );
        assert_eq!(key_of("x.html", cwd), "file:///work/my%20repo/x.html");
    }

    #[test]
    fn a_window_past_the_read_back_budget_cannot_be_told() {
        let page = pages_in("x.html", Path::new("/w"));
        let filler = format!("{{\"type\":\"system\",\"pad\":\"{}\"}}\n", "x".repeat(1000));
        let log = filler.repeat(600);
        let cursor = std::io::Cursor::new(log.into_bytes());
        assert!(unopened_within(cursor, &page, "now", 10).is_err());
    }
}
