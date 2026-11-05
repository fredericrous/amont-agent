//! `lint-suppression-added` — an edit that adds a lint suppression, or makes a
//! lint configuration looser.
//!
//! Measured over 99,810 tool calls (2026-10-06): 101 Edit/Write calls added a
//! suppression, 1.01 per 1,000 calls or about 1.7% of code edits. The agent
//! reaches for `#[allow(..)]`, `# type: ignore` or `eslint-disable` instead of
//! fixing the finding (`general.no-disabled-safety`).
//!
//! Like `file-reread`, `examine` never fires — the backtester sees no Edit or
//! Write calls — and the rule lives in the hook's file path. The hook calls
//! [`reconstruct`], the only function here that touches the disk, then the
//! pure [`examine_change`] and [`phrase`].
//!
//! ## What is compared
//!
//! Not lines: a **multiset of normalised markers**. A marker is
//! `(kind, code)`, for example `("type: ignore", "arg-type")`, with one marker
//! per code, so `noqa: E501` → `noqa: E501,F401` adds `F401` only. A config
//! setting is `(section, key, value)`. When a marker occurs `n` times before
//! and `m > n` times after, the occurrences after the first `n`, in line
//! order, are the hits. Editing code on a line that keeps its suppression, and
//! moving a suppression, are silent.
//!
//! **Known approximation:** the reported line is the first occurrence past
//! the before count in the *after* text. When the same marker already existed
//! elsewhere and the edit adds another, the line reported can be an earlier,
//! pre-existing occurrence, not the line that was typed.
//!
//! ## Known false negatives
//!
//! TOML, YAML and JSON are scanned by hand, line by line, with no new crate.
//! What the scanner does not handle is silent:
//!
//! - dotted TOML keys (`lints.clippy.x = "allow"` at the top level);
//! - inline tables (`x = { level = "allow" }`);
//! - YAML flow sequences (`disable: [a, b]`);
//! - markers only the fragment fallback sees: with no section context a bare
//!   `x = "allow"` is not under `[lints]`;
//! - a swap in one edit: the same marker removed in one place and added in
//!   another leaves the count unchanged;
//! - in a source file, a marker the inserted text does not name: an edit
//!   whose `new_string` or `content` carries none of the type's keywords is
//!   answered without a read ([`worth_rebuilding`]), so a marker assembled
//!   only by deleting the text between its halves is not seen;
//! - a Write over an existing file the hook cannot read (over the read cap,
//!   not UTF-8, an I/O error): what it held is unknown, so nothing is
//!   reported rather than every suppression it already had.
//!
//! A comment marker is required: a string literal that merely contains
//! `# noqa` or `eslint-disable` does not fire. Markdown and every other file
//! type are never examined.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::payload::Change;
use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "lint-suppression-added",
    prevents: "A lint suppression or a looser lint configuration added to silence a finding instead of fixing it",
    irreversible: false,
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        // `tools/suppression-rate.py --until 2026-10-06`: 101 Edit/Write
        // calls adding a suppression in 99,810 tool calls.
        per_1000: 1.01,
        measured: "2026-10-06",
        trend: Trend::Flat(6),
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(_parsed: &Parsed) -> Option<Finding> {
    None
}

/// Past this the file is not read and the fragments are examined instead: the
/// hook is waiting, and rebuilding costs about 22 ms per MiB (measured
/// 2026-10-06), so 256 KiB keeps the worst rebuild near 6 ms. Configuration
/// files, which need the whole file for their sections, are far smaller.
pub const MAX_READ: u64 = 256 * 1024;

/// One suppression, or one loosened setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    /// A comment marker's kind (`noqa`, `allow`, `eslint-disable-next-line`),
    /// or a config setting's key.
    pub kind: String,
    /// One code of the marker, `""` when it has none; or a setting's value.
    pub codes: String,
    /// 1-based, in the text the marker was found in.
    pub line: usize,
    /// Where the marker came from: a comment or a configuration setting.
    pub origin: Origin,
}

/// A comment marker, or a setting in a configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// How the comment reads, for the advice. Not part of its identity.
    Comment { shown: String },
    /// The setting's section, `""` for flat formats. Part of its identity.
    Setting { section: String },
}

pub type Hit = Marker;

impl Marker {
    fn key(&self) -> (String, String, String) {
        let section = match &self.origin {
            Origin::Comment { .. } => String::new(),
            Origin::Setting { section } => section.clone(),
        };
        (self.kind.clone(), self.codes.clone(), section)
    }
}

fn comment(kind: &str, code: &str, line: usize, shown: String) -> Marker {
    Marker {
        kind: kind.to_string(),
        codes: code.to_string(),
        line,
        origin: Origin::Comment { shown },
    }
}

fn setting(section: &str, key: &str, value: &str, line: usize) -> Marker {
    Marker {
        kind: key.to_string(),
        codes: value.to_string(),
        line,
        origin: Origin::Setting {
            section: section.to_string(),
        },
    }
}

/// One marker per code, or a single bare one.
fn per_code(
    out: &mut Vec<Marker>,
    kind: &str,
    codes: &[String],
    line: usize,
    show: impl Fn(&str) -> String,
) {
    if codes.is_empty() {
        out.push(comment(kind, "", line, show("")));
    }
    for code in codes {
        out.push(comment(kind, code, line, show(code)));
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum File {
    Python,
    Script,
    Rust,
    Go,
    Tsconfig,
    Eslint,
    PyrightJson,
    Pyproject,
    Ruff,
    Cargo,
    Golangci,
    Shell,
}

fn file_kind(path: &Path) -> Option<File> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if name.starts_with("tsconfig") && name.ends_with(".json") {
        return Some(File::Tsconfig);
    }
    if name.starts_with("eslint.config.") || name.starts_with(".eslintrc") {
        return Some(File::Eslint);
    }
    match name.as_str() {
        "pyrightconfig.json" => return Some(File::PyrightJson),
        "pyproject.toml" => return Some(File::Pyproject),
        "ruff.toml" | ".ruff.toml" => return Some(File::Ruff),
        "cargo.toml" => return Some(File::Cargo),
        ".golangci.yml" | ".golangci.yaml" => return Some(File::Golangci),
        _ => {}
    }
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "py" | "pyi" => Some(File::Python),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" | "vue" | "svelte" => {
            Some(File::Script)
        }
        "rs" => Some(File::Rust),
        "go" => Some(File::Go),
        "sh" | "bash" | "zsh" => Some(File::Shell),
        _ => None,
    }
}

/// Whether the change is worth rebuilding the file for. A source file can
/// gain a marker only if the text the change inserts names one, so an edit
/// whose inserted text carries none of the type's keywords is answered
/// without a read. A configuration file is always rebuilt: a loosened
/// setting needs its section, and these files are small.
pub fn worth_rebuilding(path: &Path, change: &Change) -> bool {
    let keywords: &[&str] = match file_kind(path) {
        None => return false,
        Some(File::Python) => &["noqa", "type:", "pyright", "ruff"],
        Some(File::Script) => &["eslint", "@ts-"],
        // Attribute forms only: `.expect(` is in most Rust edits.
        Some(File::Rust) => &["#[allow", "#![allow", "#[expect", "#![expect", "cfg_attr"],
        Some(File::Go) => &["nolint"],
        Some(File::Shell) => &["shellcheck"],
        Some(
            File::Tsconfig
            | File::Eslint
            | File::PyrightJson
            | File::Pyproject
            | File::Ruff
            | File::Cargo
            | File::Golangci,
        ) => return true,
    };
    let names_one = |text: &str| {
        let text = text.to_ascii_lowercase();
        keywords.iter().any(|k| text.contains(k))
    };
    match change {
        Change::Write { content } => names_one(content),
        Change::Edits(parts) => parts.iter().any(|p| names_one(&p.new)),
    }
}

/// Whether a lowercased Bash command names a suppression keyword of any
/// examined kind, or the name of an examined configuration file: the
/// no-process test that decides whether a working tree is worth looking at.
pub fn command_names_marker(lowercased: &str) -> bool {
    const WORDS: &[&str] = &[
        "noqa",
        "type:",
        "pyright",
        "ruff",
        "eslint",
        "@ts-",
        "#[allow",
        "#![allow",
        "#[expect",
        "#![expect",
        "cfg_attr",
        "nolint",
        "shellcheck",
        "tsconfig",
        ".eslintrc",
        "pyproject.toml",
        "cargo.toml",
        ".golangci.y",
    ];
    WORDS.iter().any(|w| lowercased.contains(w))
}

/// Whether an extensionless `path` is a shell script: its first line is a
/// `sh`, `bash` or `zsh` shebang, directly or through `env`.
pub fn shell_by_shebang(path: &Path, text: &str) -> bool {
    if path.file_name().is_none() || path.extension().is_some() {
        return false;
    }
    let first = text.lines().next().unwrap_or("");
    let Some(command) = first.strip_prefix("#!") else {
        return false;
    };
    let mut words = command.split_whitespace();
    let mut program = words.next().unwrap_or("");
    if program.rsplit('/').next() == Some("env") {
        program = words.find(|w| !w.starts_with('-')).unwrap_or("");
    }
    matches!(program.rsplit('/').next(), Some("sh" | "bash" | "zsh"))
}

/// Whether `path` is an examined kind by its name alone.
pub fn examined(path: &Path) -> bool {
    file_kind(path).is_some()
}

/// The file kind of `path` holding `text`: by name and extension, else a
/// shell shebang for an extensionless path.
fn kind_of(path: &Path, text: &str) -> Option<File> {
    file_kind(path).or_else(|| shell_by_shebang(path, text).then_some(File::Shell))
}

/// Lines split on `\n`, a trailing `\r` trimmed.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n').map(|l| l.trim_end_matches('\r'))
}

/// Every marker in `text`, in line order. Empty for a file type that is not
/// examined.
pub fn markers(path: &Path, text: &str) -> Vec<Marker> {
    let Some(file) = kind_of(path, text) else {
        return Vec::new();
    };
    let mut out = match file {
        File::Python => python(text),
        File::Script => script(text),
        File::Go => go(text),
        File::Rust => rust(text),
        File::Shell => shell(text),
        File::Tsconfig => json_lines(text, tsconfig_loose),
        File::PyrightJson => json_lines(text, pyright_loose),
        File::Eslint => json_lines(text, eslint_loose),
        File::Pyproject | File::Ruff | File::Cargo => toml(text, file),
        File::Golangci => golangci(text),
    };
    out.sort_by_key(|m| m.line);
    out
}

/// A marker's identity for counting: kind, code, section, and whether it
/// carries a reason (always false when reasons are not keyed).
type MarkerKey = (String, String, String, bool);

#[derive(Clone, Copy)]
enum Keying {
    /// Reason text is ignored: the Edit/Write rule denies a marker either way.
    Plain,
    /// A reasoned marker and a bare one are different markers.
    Reasoned,
}

fn keyed(marker: &Marker, has_reason: bool, keying: Keying) -> MarkerKey {
    let has_reason = match keying {
        Keying::Plain => false,
        Keying::Reasoned => has_reason,
    };
    let (kind, codes, section) = marker.key();
    (kind, codes, section, has_reason)
}

/// One file of a change: the old path and text, the new path and text. Each
/// side is classified by its own path; an added file has an empty `before`
/// and a deleted file an empty `after`, so that side holds no markers.
pub struct FileChange<'a> {
    pub old_path: &'a Path,
    pub before: &'a str,
    pub new_path: &'a Path,
    pub after: &'a str,
}

/// A marker a change adds, at its new path, with whether it carries a reason
/// in its tool's own form.
pub struct Added {
    pub path: PathBuf,
    pub hit: Hit,
    pub reasoned: bool,
}

fn net_added_keyed(changes: &[FileChange<'_>], keying: Keying) -> Vec<Added> {
    let mut had: HashMap<MarkerKey, usize> = HashMap::new();
    for change in changes {
        for m in markers(change.old_path, change.before) {
            let has_reason = reasoned(&m, from_line(change.before, m.line));
            *had.entry(keyed(&m, has_reason, keying)).or_insert(0) += 1;
        }
    }
    let mut now: Vec<(Added, MarkerKey)> = Vec::new();
    for change in changes {
        for m in markers(change.new_path, change.after) {
            let has_reason = reasoned(&m, from_line(change.after, m.line));
            let key = keyed(&m, has_reason, keying);
            now.push((
                Added {
                    path: change.new_path.to_path_buf(),
                    hit: m,
                    reasoned: has_reason,
                },
                key,
            ));
        }
    }
    // By the path's text, not its components, as the Python port sorts.
    now.sort_by(|a, b| {
        a.0.path
            .as_os_str()
            .cmp(b.0.path.as_os_str())
            .then(a.0.hit.line.cmp(&b.0.hit.line))
    });
    let mut seen: HashMap<MarkerKey, usize> = HashMap::new();
    now.into_iter()
        .filter_map(|(added, key)| {
            let had_count = had.get(&key).copied().unwrap_or(0);
            let seen_count = seen.entry(key).or_insert(0);
            *seen_count += 1;
            // holds-until: a move is told apart from a removal and an addition by path pairing
            (*seen_count > had_count).then_some(added)
        })
        .collect()
}

/// The markers a whole change adds, net of the ones it removes anywhere: counts
/// are summed over every file, keyed without the path and with whether the
/// marker carries a reason, so a marker moved between files cancels and
/// removing a reasoned marker never cancels an added bare one. Hits are
/// reported at their new path, in path then line order, each with its reasoned
/// bit. Pure.
pub fn net_added(changes: &[FileChange<'_>]) -> Vec<Added> {
    net_added_keyed(changes, Keying::Reasoned)
}

/// The markers `after` has more of than `before`. Pure.
pub fn examine_change(path: &Path, before: &str, after: &str) -> Vec<Hit> {
    let change = FileChange {
        old_path: path,
        before,
        new_path: path,
        after,
    };
    net_added_keyed(&[change], Keying::Plain)
        .into_iter()
        .map(|added| added.hit)
        .collect()
}

// ---- comment languages ------------------------------------------------------

/// `s` without a case-insensitive `prefix`.
fn after_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// A marker word ends here: nothing identifier-like follows.
fn boundary(tail: &str) -> bool {
    tail.chars()
        .next()
        .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'))
}

/// `[a, b]` right at the start of `tail`.
fn bracket_codes(tail: &str) -> Vec<String> {
    let Some(inner) = tail.strip_prefix('[') else {
        return Vec::new();
    };
    let inner = inner.split(']').next().unwrap_or("");
    inner
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect()
}

fn is_code(t: &str) -> bool {
    t.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && t.chars().all(|c| c.is_ascii_alphanumeric())
        && t.chars().any(|c| c.is_ascii_digit())
}

/// The codes after `noqa:`; stops at the first word that is not one.
fn noqa_codes(tail: &str) -> Vec<String> {
    let Some(codes) = tail.trim_start().strip_prefix(':') else {
        return Vec::new();
    };
    codes
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .take_while(|t| is_code(t))
        .map(|t| t.to_ascii_uppercase())
        .collect()
}

fn python(text: &str) -> Vec<Marker> {
    let mut out = Vec::new();
    let mut triple: Option<u8> = None;
    for (i, line) in lines(text).enumerate() {
        if let Some(c) = py_comment(line, &mut triple) {
            py_markers(c, i + 1, &mut out);
        }
    }
    out
}

/// The comment on this line, if a `#` falls outside every string. `triple`
/// carries an open `"""` or `'''` string from line to line.
fn py_comment<'a>(line: &'a str, triple: &mut Option<u8>) -> Option<&'a str> {
    let b = line.as_bytes();
    let mut single: Option<u8> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = *triple {
            if c == b'\\' {
                i += 2;
            } else if c == q && b[i..].starts_with(&[q, q, q]) {
                *triple = None;
                i += 3;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(q) = single {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                single = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'#' => return Some(&line[i + 1..]),
            b'"' | b'\'' => {
                if b[i..].starts_with(&[c, c, c]) {
                    *triple = Some(c);
                    i += 3;
                    continue;
                }
                single = Some(c);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn py_markers(comment_text: &str, line: usize, out: &mut Vec<Marker>) {
    for seg in comment_text.split('#') {
        let seg = seg.trim();
        if let Some(rest) = after_prefix(seg, "type:") {
            if let Some(tail) = after_prefix(rest.trim_start(), "ignore").filter(|t| boundary(t)) {
                per_code(out, "type: ignore", &bracket_codes(tail), line, |c| {
                    if c.is_empty() {
                        "# type: ignore".to_string()
                    } else {
                        format!("# type: ignore[{c}]")
                    }
                });
            }
        } else if let Some(rest) = after_prefix(seg, "pyright:") {
            let rest = rest.trim_start();
            if let Some(tail) = after_prefix(rest, "ignore").filter(|t| boundary(t)) {
                per_code(out, "pyright: ignore", &bracket_codes(tail), line, |c| {
                    if c.is_empty() {
                        "# pyright: ignore".to_string()
                    } else {
                        format!("# pyright: ignore[{c}]")
                    }
                });
            } else {
                for item in rest.split(',') {
                    let item = item.split_whitespace().collect::<Vec<_>>().join("");
                    let loose = match item.split_once('=') {
                        Some((k, v)) => pyright_loose(k, v),
                        None => item == "basic" || item == "off",
                    };
                    if loose {
                        out.push(comment(
                            "pyright",
                            &item,
                            line,
                            format!("# pyright: {item}"),
                        ));
                    }
                }
            }
        } else if let Some(tail) = after_prefix(seg, "ruff:")
            .and_then(|r| after_prefix(r.trim_start(), "noqa"))
            .filter(|t| boundary(t))
        {
            per_code(out, "ruff: noqa", &noqa_codes(tail), line, |c| {
                if c.is_empty() {
                    "# ruff: noqa".to_string()
                } else {
                    format!("# ruff: noqa: {c}")
                }
            });
        } else if let Some(tail) = after_prefix(seg, "noqa").filter(|t| boundary(t)) {
            per_code(out, "noqa", &noqa_codes(tail), line, |c| {
                if c.is_empty() {
                    "# noqa".to_string()
                } else {
                    format!("# noqa: {c}")
                }
            });
        }
    }
}

/// `(line, is_block, body)` of every comment outside a string, where `body` is
/// the text after `//` or `/*`, to the end of the line (or the `*/`).
fn c_comments(text: &str) -> Vec<(usize, bool, &str)> {
    enum S {
        Code,
        Str(u8),
        Block,
    }
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut state = S::Code;
    let mut line = 1;
    let mut i = 0;
    while i < b.len() {
        match state {
            S::Code => match b[i] {
                b'\n' => {
                    line += 1;
                    i += 1;
                }
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    let end = b[i..]
                        .iter()
                        .position(|&c| c == b'\n')
                        .map_or(b.len(), |p| i + p);
                    out.push((line, false, &text[i + 2..end]));
                    i = end;
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    let start = i + 2;
                    let mut j = start;
                    while j < b.len() && b[j] != b'\n' && !b[j..].starts_with(b"*/") {
                        j += 1;
                    }
                    out.push((line, true, &text[start..j]));
                    if b[j..].starts_with(b"*/") {
                        i = j + 2;
                    } else {
                        i = j;
                        state = S::Block;
                    }
                }
                q @ (b'"' | b'\'' | b'`') => {
                    state = S::Str(q);
                    i += 1;
                }
                _ => i += 1,
            },
            S::Str(q) => match b[i] {
                b'\\' => {
                    if b.get(i + 1) == Some(&b'\n') {
                        line += 1;
                    }
                    i += 2;
                }
                b'\n' => {
                    line += 1;
                    if q != b'`' {
                        state = S::Code;
                    }
                    i += 1;
                }
                c if c == q => {
                    state = S::Code;
                    i += 1;
                }
                _ => i += 1,
            },
            S::Block => {
                if b[i] == b'\n' {
                    line += 1;
                    i += 1;
                } else if b[i..].starts_with(b"*/") {
                    state = S::Code;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        }
    }
    out
}

fn script(text: &str) -> Vec<Marker> {
    let mut out = Vec::new();
    for (line, block, body) in c_comments(text) {
        let body = body.trim().trim_start_matches(['/', '*']).trim();
        let open = if block { "/*" } else { "//" };
        if let Some(rest) = body.strip_prefix("eslint-disable") {
            let (suffix, rest) = if let Some(r) = rest.strip_prefix("-next-line") {
                ("-next-line", r)
            } else if let Some(r) = rest.strip_prefix("-line") {
                ("-line", r)
            } else {
                ("", rest)
            };
            if !boundary(rest) {
                continue;
            }
            let kind = format!("eslint-disable{suffix}");
            let list = rest.split("--").next().unwrap_or("");
            let codes: Vec<String> = list
                .split(',')
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect();
            per_code(&mut out, &kind, &codes, line, |c| {
                let sp = if c.is_empty() { "" } else { " " };
                if block {
                    format!("{open} {kind}{sp}{c} */")
                } else {
                    format!("{open} {kind}{sp}{c}")
                }
            });
            continue;
        }
        for tag in ["@ts-ignore", "@ts-nocheck", "@ts-expect-error"] {
            if body.strip_prefix(tag).is_some_and(boundary) {
                let shown = if block {
                    format!("{open} {tag} */")
                } else {
                    format!("{open} {tag}")
                };
                out.push(comment(tag, "", line, shown));
            }
        }
    }
    out
}

/// The text after the first `#` that starts a shell comment: at the start of
/// the line or after whitespace, outside quotes.
fn shell_comment(line: &str) -> Option<&str> {
    let mut quote: Option<char> = None;
    let mut prev = ' ';
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '#' if prev.is_whitespace() => return Some(&line[i + 1..]),
                _ => {}
            },
        }
        prev = c;
    }
    None
}

/// The codes of a `shellcheck disable=..` comment and the text after the list.
fn shellcheck_disable(comment: &str) -> Option<(Vec<String>, &str)> {
    let rest = comment.trim_start().strip_prefix("shellcheck")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let list = rest.trim_start().strip_prefix("disable=")?;
    let end = list.find(char::is_whitespace).unwrap_or(list.len());
    let codes = list[..end]
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    Some((codes, &list[end..]))
}

fn shell(text: &str) -> Vec<Marker> {
    let mut out = Vec::new();
    for (i, line) in lines(text).enumerate() {
        let Some((codes, _)) = shell_comment(line).and_then(shellcheck_disable) else {
            continue;
        };
        for code in codes {
            out.push(comment(
                "shellcheck disable",
                &code,
                i + 1,
                format!("# shellcheck disable={code}"),
            ));
        }
    }
    out
}

fn go(text: &str) -> Vec<Marker> {
    let mut out = Vec::new();
    for (line, block, body) in c_comments(text) {
        let Some(rest) = body.strip_prefix("nolint").filter(|_| !block) else {
            continue;
        };
        if !boundary(rest) {
            continue;
        }
        let codes: Vec<String> = rest
            .strip_prefix(':')
            .map(|l| {
                l.split_whitespace()
                    .next()
                    .unwrap_or("")
                    .split(',')
                    .map(|c| c.trim().to_string())
                    .filter(|c| !c.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        per_code(&mut out, "nolint", &codes, line, |c| {
            if c.is_empty() {
                "//nolint".to_string()
            } else {
                format!("//nolint:{c}")
            }
        });
    }
    out
}

/// The comma-separated items of the parenthesis that opens right before `s`,
/// at its own nesting level; a `reason = ".."` item is not a lint.
fn paren_items(s: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut depth = 1;
    let mut quote = false;
    let mut prev = ' ';
    for c in s.chars() {
        if quote {
            cur.push(c);
            if c == '"' && prev != '\\' {
                quote = false;
            }
        } else {
            match c {
                '"' => {
                    quote = true;
                    cur.push(c);
                }
                '(' => {
                    depth += 1;
                    cur.push(c);
                }
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    cur.push(c);
                }
                ',' if depth == 1 => items.push(std::mem::take(&mut cur)),
                _ => cur.push(c),
            }
        }
        prev = c;
    }
    items.push(cur);
    items
        .into_iter()
        .map(|i| i.trim().to_string())
        .filter(|i| !i.is_empty() && !i.starts_with("reason"))
        .collect()
}

/// How many lines an attribute may span before its `]`: rustfmt wraps a long
/// `#[expect(lint, reason = "…")]` over a handful, never dozens.
const ATTRIBUTE_SPAN: usize = 40;

/// The attribute that opens `text` (`#[…]` or `#![…]`, after leading
/// whitespace), through its closing `]`, across lines; brackets inside a
/// string do not count. An attribute that does not close within
/// [`ATTRIBUTE_SPAN`] lines is its first line only. `None` when `text` does
/// not open an attribute.
fn attribute_text(text: &str) -> Option<&str> {
    let start = text.len() - text.trim_start().len();
    let opened = &text[start..];
    if !opened.starts_with("#[") && !opened.starts_with("#![") {
        return None;
    }
    let first_line = opened.split('\n').next().unwrap_or(opened);
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    let mut newlines = 0;
    for (i, c) in opened.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '[' => depth += 1,
            ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&opened[..=i]);
                }
            }
            '\n' => {
                newlines += 1;
                if newlines >= ATTRIBUTE_SPAN {
                    return Some(first_line);
                }
            }
            _ => {}
        }
    }
    Some(first_line)
}

/// `text` from the start of its 1-based `line` to the end.
pub fn from_line(text: &str, line: usize) -> &str {
    let mut rest = text;
    for _ in 1..line {
        match rest.split_once('\n') {
            Some((_, tail)) => rest = tail,
            None => return "",
        }
    }
    rest
}

fn rust(text: &str) -> Vec<Marker> {
    let mut out = Vec::new();
    for (i, line) in lines(text).enumerate() {
        if !line.trim_start().starts_with('#') {
            continue;
        }
        let attribute = attribute_text(from_line(text, i + 1)).unwrap_or(line);
        let Some(after_hash) = attribute.trim_start().strip_prefix('#') else {
            continue;
        };
        let (open, after_open) = match after_hash.strip_prefix('!') {
            Some(rest) => ("#![", rest),
            None => ("#[", after_hash),
        };
        let Some(inner) = after_open.strip_prefix('[').map(str::trim_start) else {
            continue;
        };
        for kind in ["allow", "expect"] {
            if let Some(args) = inner.strip_prefix(kind).and_then(|r| r.strip_prefix('(')) {
                per_code(&mut out, kind, &paren_items(args), i + 1, |c| {
                    format!("{open}{kind}({c})]")
                });
            }
        }
        if let Some(args) = inner.strip_prefix("cfg_attr(") {
            let mut found: Vec<(usize, &str)> = Vec::new();
            for kind in ["allow", "expect"] {
                let needle = format!("{kind}(");
                for (pos, _) in args.match_indices(&needle) {
                    if pos > 0 && matches!(args.as_bytes()[pos - 1], b' ' | b',') {
                        found.push((pos, kind));
                    }
                }
            }
            found.sort();
            for (pos, kind) in found {
                let lints = paren_items(&args[pos + kind.len() + 1..]);
                per_code(&mut out, kind, &lints, i + 1, |c| {
                    format!("{open}cfg_attr(.., {kind}({c}))]")
                });
            }
        }
    }
    out
}

/// Non-empty text once separators are trimmed.
fn has_text(s: &str) -> bool {
    !s.trim_matches(|c: char| c.is_whitespace() || c == ':' || c == '-' || c == '*' || c == '/')
        .is_empty()
}

/// The text after `]` when `tail` opens a bracket list, else `tail`.
fn after_brackets(tail: &str) -> &str {
    match tail.strip_prefix('[') {
        Some(inner) => inner.split_once(']').map_or("", |(_, rest)| rest),
        None => tail,
    }
}

/// `tail` without the `: A, B` code list `noqa` takes.
fn after_noqa_codes(tail: &str) -> &str {
    let mut rest = tail.trim_start().strip_prefix(':').unwrap_or(tail);
    loop {
        let t = rest.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
        let end = t
            .find(|c: char| c == ',' || c.is_whitespace())
            .unwrap_or(t.len());
        if end > 0 && is_code(&t[..end]) {
            rest = &t[end..];
        } else {
            return t;
        }
    }
}

/// Another marker, not a reason, when it follows a `#`.
fn starts_a_marker(seg: &str) -> bool {
    ["type:", "pyright:", "noqa", "ruff:"]
        .iter()
        .any(|p| after_prefix(seg, p).is_some())
}

fn python_reasoned(kind: &str, line_text: &str) -> bool {
    let Some(comment_text) = py_comment(line_text, &mut None) else {
        return false;
    };
    let segs: Vec<&str> = comment_text.split('#').map(str::trim).collect();
    segs.iter().enumerate().any(|(i, seg)| {
        let rest = match kind {
            "type: ignore" => after_prefix(seg, "type:")
                .and_then(|r| after_prefix(r.trim_start(), "ignore"))
                .filter(|t| boundary(t))
                .map(after_brackets),
            "pyright: ignore" => after_prefix(seg, "pyright:")
                .and_then(|r| after_prefix(r.trim_start(), "ignore"))
                .filter(|t| boundary(t))
                .map(after_brackets),
            _ => after_prefix(seg, "noqa")
                .filter(|t| boundary(t))
                .map(after_noqa_codes),
        };
        rest.is_some_and(|rest| {
            rest.trim_start()
                .strip_prefix("--")
                .is_some_and(|t| !t.trim().is_empty())
                || segs[i + 1..]
                    .iter()
                    .any(|s| !s.is_empty() && !starts_a_marker(s))
        })
    })
}

/// The bodies of the comments on one line, markers' leading `/` and `*` gone.
fn comment_bodies(line_text: &str) -> Vec<(bool, &str)> {
    c_comments(line_text)
        .into_iter()
        .map(|(_, block, body)| (block, body.trim().trim_start_matches(['/', '*']).trim()))
        .collect()
}

/// Whether the attribute that opens `text` carries a `reason = "<text>"`
/// item: inside the attribute only, outside its strings, as an item of its own
/// (after `(` or `,`), so a `// reason = "…"` comment after the `]` or a lint
/// whose name holds `reason` does not count.
fn rust_reasoned(text: &str) -> bool {
    let Some(attribute) = attribute_text(text) else {
        return false;
    };
    let mut quoted = false;
    let mut escaped = false;
    let mut previous = ' ';
    for (i, c) in attribute.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
            continue;
        }
        if c == '"' {
            quoted = true;
        } else if matches!(previous, '(' | ',') && reason_item(&attribute[i..]) {
            return true;
        }
        if !c.is_whitespace() {
            previous = c;
        }
    }
    false
}

/// `reason = "<non-empty>"` at the start of `item`.
fn reason_item(item: &str) -> bool {
    item.strip_prefix("reason")
        .map(str::trim_start)
        .and_then(|r| r.strip_prefix('='))
        .map(str::trim_start)
        .and_then(|r| r.strip_prefix('"'))
        .and_then(|r| r.split('"').next())
        .is_some_and(|why| !why.trim().is_empty())
}

/// The form in which a marker's tool takes a reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonForm {
    /// A Rust attribute's `reason = "…"` item.
    Attribute,
    /// Text after `--` (`noqa`, `type: ignore`, `eslint-disable`).
    TrailingDashes,
    /// Text after the tag itself (`@ts-expect-error`).
    AfterTag,
    /// A `// …` comment after the directive (`nolint`).
    SlashSlash,
    /// A `# …` comment after the directive (`shellcheck disable`).
    Hash,
    /// No form: a setting, a blanket marker, or a marker with no code.
    Never,
}

/// How `hit` can carry a reason. One answer for the check ([`reasoned`]) and
/// for the advice the commit gate prints.
pub fn reason_form(hit: &Hit) -> ReasonForm {
    if matches!(hit.origin, Origin::Setting { .. }) {
        return ReasonForm::Never;
    }
    let kind = hit.kind.as_str();
    match kind {
        "@ts-ignore" | "@ts-nocheck" | "pyright" | "ruff: noqa" => ReasonForm::Never,
        "@ts-expect-error" => ReasonForm::AfterTag,
        _ if hit.codes.is_empty() => ReasonForm::Never,
        "type: ignore" | "pyright: ignore" | "noqa" => ReasonForm::TrailingDashes,
        "allow" | "expect" => ReasonForm::Attribute,
        "nolint" => ReasonForm::SlashSlash,
        "shellcheck disable" => ReasonForm::Hash,
        _ if kind.starts_with("eslint-disable") => ReasonForm::TrailingDashes,
        _ => ReasonForm::Never,
    }
}

/// Whether the marker carries a reason in its tool's own reason form, read
/// from `text`: the file from the marker's line on. A comment marker's reason
/// is on that line; a Rust attribute's runs to its `]`, across lines. Never for a configuration
/// setting, a marker with no code, `@ts-ignore`, `@ts-nocheck`, or a
/// file-level `ruff: noqa` / `pyright:` comment. Pure.
pub fn reasoned(hit: &Hit, text: &str) -> bool {
    let line_text = lines(text).next().unwrap_or("");
    let kind = hit.kind.as_str();
    match reason_form(hit) {
        ReasonForm::Never => false,
        ReasonForm::AfterTag => comment_bodies(line_text).iter().any(|(_, body)| {
            body.strip_prefix(kind)
                .is_some_and(|rest| boundary(rest) && has_text(rest))
        }),
        ReasonForm::Attribute => rust_reasoned(text),
        ReasonForm::SlashSlash => comment_bodies(line_text).iter().any(|(block, body)| {
            !block
                && body.strip_prefix("nolint").is_some_and(|rest| {
                    let rest = rest.strip_prefix(':').unwrap_or(rest);
                    let after_list = rest.trim_start().split_once(char::is_whitespace);
                    after_list.is_some_and(|(_, tail)| {
                        tail.trim_start()
                            .strip_prefix("//")
                            .is_some_and(|reason| !reason.trim().is_empty())
                    })
                })
        }),
        ReasonForm::Hash => shell_comment(line_text)
            .and_then(shellcheck_disable)
            .is_some_and(|(_, after)| {
                after
                    .trim_start()
                    .strip_prefix('#')
                    .is_some_and(|reason| !reason.trim().is_empty())
            }),
        ReasonForm::TrailingDashes if kind.starts_with("eslint-disable") => {
            comment_bodies(line_text).iter().any(|(_, body)| {
                body.strip_prefix(kind).is_some_and(|rest| {
                    boundary(rest)
                        && rest
                            .split_once("--")
                            .is_some_and(|(_, why)| !why.trim().is_empty())
                })
            })
        }
        ReasonForm::TrailingDashes => python_reasoned(kind, line_text),
    }
}

// ---- configuration ------------------------------------------------------------

fn ident(c: char) -> bool {
    c.is_alphanumeric() || "_@/.-$".contains(c)
}

/// Every `key: value` on a line of JSON, JS or YAML, in order. A quoted key,
/// or a bare one; a value that opens an array gives its first element.
fn kv_pairs(line: &str) -> Vec<(String, String)> {
    let c: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let quoted = |from: usize| -> Option<(String, usize)> {
        let q = c[from];
        let end = (from + 1..c.len()).find(|&j| c[j] == q)?;
        Some((c[from + 1..end].iter().collect(), end + 1))
    };
    while i < c.len() {
        let (key, next) = if c[i] == '"' || c[i] == '\'' {
            match quoted(i) {
                Some(k) => k,
                None => break,
            }
        } else if ident(c[i]) {
            let end = (i..c.len()).find(|&j| !ident(c[j])).unwrap_or(c.len());
            (c[i..end].iter().collect(), end)
        } else {
            i += 1;
            continue;
        };
        let mut j = next;
        while j < c.len() && c[j].is_whitespace() {
            j += 1;
        }
        if j >= c.len() || c[j] != ':' {
            i = next;
            continue;
        }
        j += 1;
        while j < c.len() && c[j].is_whitespace() {
            j += 1;
        }
        if j < c.len() && c[j] == '[' {
            j += 1;
            while j < c.len() && c[j].is_whitespace() {
                j += 1;
            }
        }
        if j >= c.len() {
            break;
        }
        let (value, after) = if c[j] == '"' || c[j] == '\'' {
            quoted(j).unwrap_or_default()
        } else {
            let end = (j..c.len()).find(|&k| !ident(c[k])).unwrap_or(c.len());
            (c[j..end].iter().collect(), end)
        };
        out.push((key, value));
        i = after.max(j + 1);
    }
    out
}

fn json_lines(text: &str, loose: fn(&str, &str) -> bool) -> Vec<Marker> {
    let mut out = Vec::new();
    for (i, line) in lines(text).enumerate() {
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with('#') {
            continue;
        }
        for (k, v) in kv_pairs(line) {
            if loose(&k, &v) {
                out.push(setting("", &k, &v, i + 1));
            }
        }
    }
    out
}

fn tsconfig_loose(key: &str, value: &str) -> bool {
    (key.starts_with("strict") || key == "noImplicitAny") && value == "false"
}

fn pyright_loose(key: &str, value: &str) -> bool {
    (key.starts_with("report") && matches!(value, "none" | "false" | "warning" | "information"))
        || (key == "typeCheckingMode" && matches!(value, "off" | "basic"))
        || (key == "enableTypeIgnoreComments" && value == "true")
}

fn eslint_loose(key: &str, value: &str) -> bool {
    !matches!(key, "ecmaVersion" | "version") && matches!(value, "off" | "warn" | "0" | "1")
}

/// The line without a `#` comment, outside quotes.
fn strip_toml_comment(line: &str) -> &str {
    let mut quote: Option<u8> = None;
    for (i, &c) in line.as_bytes().iter().enumerate() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(c),
            (None, b'#') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// The quoted strings of an array's text, and whether its `]` was reached.
fn array_entries(s: &str) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => {
                out.push(std::mem::take(&mut cur));
                quote = None;
            }
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == ']' => return (out, true),
            None => {}
        }
    }
    (out, false)
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    match v.chars().next() {
        Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or("").to_string(),
        _ => v.split_whitespace().next().unwrap_or("").to_string(),
    }
}

fn split_kv(line: &str) -> Option<(String, &str)> {
    if line.starts_with(['"', '\'']) {
        let q = line.chars().next()?;
        let end = 1 + line[1..].find(q)?;
        let rest = line[end + 1..].trim_start().strip_prefix('=')?;
        return Some((line[1..end].to_string(), rest.trim()));
    }
    let (k, v) = line.split_once('=')?;
    Some((k.trim().to_string(), v.trim()))
}

fn ruff_section(file: File, section: &str) -> bool {
    match file {
        File::Ruff => true,
        File::Pyproject => section.starts_with("tool.ruff"),
        _ => false,
    }
}

fn lints_section(section: &str) -> bool {
    ["lints", "workspace.lints"]
        .iter()
        .any(|p| section == *p || section.strip_prefix(p).is_some_and(|r| r.starts_with('.')))
}

fn toml(text: &str, file: File) -> Vec<Marker> {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut open: Option<String> = None;
    let entry = |out: &mut Vec<Marker>, section: &str, key: &str, e: &str, line: usize| {
        let ignoring =
            matches!(key, "ignore" | "extend-ignore") || section.ends_with("per-file-ignores");
        if ruff_section(file, section) && ignoring {
            out.push(setting(section, key, e, line));
        }
    };
    for (i, raw) in lines(text).enumerate() {
        let line = strip_toml_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(key) = open.clone() {
            let (entries, closed) = array_entries(line);
            for e in entries {
                entry(&mut out, &section, &key, &e, i + 1);
            }
            if closed {
                open = None;
            }
            continue;
        }
        if let Some(head) = line.strip_prefix('[') {
            let head = head.trim_start_matches('[').trim_end_matches(']').trim();
            section = head.split('.').map(str::trim).collect::<Vec<_>>().join(".");
            continue;
        }
        let Some((key, val)) = split_kv(line) else {
            continue;
        };
        if let Some(rest) = val.strip_prefix('[') {
            let (entries, closed) = array_entries(rest);
            for e in entries {
                entry(&mut out, &section, &key, &e, i + 1);
            }
            if !closed {
                open = Some(key);
            }
            continue;
        }
        let v = unquote(val);
        let loose = match file {
            File::Pyproject => section == "tool.pyright" && pyright_loose(&key, &v),
            File::Cargo => lints_section(&section) && v == "allow",
            _ => false,
        };
        if loose {
            out.push(setting(&section, &key, &v, i + 1));
        }
    }
    out
}

fn golangci(text: &str) -> Vec<Marker> {
    let mut out = Vec::new();
    // The `key:` lines that opened a block, outermost first.
    let mut stack: Vec<(usize, String)> = Vec::new();
    for (i, raw) in lines(text).enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut indent = raw.len() - trimmed.len();
        let mut body = trimmed.split(" #").next().unwrap_or("").trim_end();
        if body == "-" || body.starts_with("- ") {
            while stack.last().is_some_and(|(ind, _)| *ind > indent) {
                stack.pop();
            }
            let item = body[1..].trim_start();
            let hit = stack
                .iter()
                .rev()
                .find(|(_, k)| k == "disable" || k.starts_with("exclude") || k == "exclusions")
                .map(|(_, k)| k.clone());
            if let Some(key) = hit.filter(|_| !item.is_empty()) {
                let section = stack.iter().map(|(_, k)| k.as_str()).collect::<Vec<_>>();
                out.push(setting(&section.join("."), &key, item, i + 1));
            }
            // `- path: x` opens a mapping whose keys sit past the dash.
            indent += body.len() - item.len();
            body = item;
        } else {
            while stack.last().is_some_and(|(ind, _)| *ind >= indent) {
                stack.pop();
            }
        }
        let Some((key, value)) = body.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches(['"', '\'']);
        let value = value.trim();
        while stack.last().is_some_and(|(ind, _)| *ind >= indent) {
            stack.pop();
        }
        if value.is_empty() {
            stack.push((indent, key.to_string()));
        } else if (key == "disable-all" && value == "true") || (key == "default" && value == "none")
        {
            let section = stack.iter().map(|(_, k)| k.as_str()).collect::<Vec<_>>();
            out.push(setting(&section.join("."), key, value, i + 1));
        }
    }
    out
}

// ---- rebuilding the change ----------------------------------------------------

/// Why the whole file could not be rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Missing,
    TooLarge,
    NotFile,
    NotUtf8,
    Io,
    NoMatch,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Missing => "missing",
            Reason::TooLarge => "too-large",
            Reason::NotFile => "not-file",
            Reason::NotUtf8 => "not-utf8",
            Reason::Io => "io",
            Reason::NoMatch => "no-match",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The whole file, before and after.
    Exact,
    /// The fragments the tool was handed; no section context, no line numbers.
    Fragment(Reason),
    /// The working tree against HEAD, after a Bash call.
    Bash,
}

#[derive(Debug)]
pub struct Rebuilt {
    pub before: String,
    pub after: String,
    pub mode: Mode,
}

/// The file on disk, read without ever blocking: `metadata` first, and only a
/// regular file within the limit is opened, so a FIFO or a device never is.
fn read_bounded(path: &Path) -> Result<String, Reason> {
    let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Reason::Missing,
        _ => Reason::Io,
    })?;
    if !meta.is_file() {
        return Err(Reason::NotFile);
    }
    if meta.len() > MAX_READ {
        return Err(Reason::TooLarge);
    }
    let file = std::fs::File::open(path).map_err(|_| Reason::Io)?;
    let mut bytes = Vec::new();
    file.take(MAX_READ + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Reason::Io)?;
    if bytes.len() as u64 > MAX_READ {
        return Err(Reason::TooLarge);
    }
    String::from_utf8(bytes).map_err(|_| Reason::NotUtf8)
}

/// Before and after the change, from disk where it can be read and from the
/// tool's own fragments where it cannot. Never denies, never panics.
pub fn reconstruct(path: &Path, change: &Change) -> Rebuilt {
    let disk = read_bounded(path);
    match change {
        Change::Write { content } => match disk {
            Ok(before) => Rebuilt {
                before: before.replace("\r\n", "\n"),
                after: content.clone(),
                mode: Mode::Exact,
            },
            // A new file has nothing before it.
            Err(Reason::Missing) => Rebuilt {
                before: String::new(),
                after: content.clone(),
                mode: Mode::Exact,
            },
            // The file exists but cannot be read, so what it held is unknown.
            // Comparing against nothing would report every suppression it
            // already had as added; comparing the content with itself is
            // silent, a known false negative.
            Err(why) => Rebuilt {
                before: content.clone(),
                after: content.clone(),
                mode: Mode::Fragment(why),
            },
        },
        Change::Edits(edits) => {
            let fragments = |why: Reason| Rebuilt {
                before: edits
                    .iter()
                    .map(|e| e.old.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                after: edits
                    .iter()
                    .map(|e| e.new.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                mode: Mode::Fragment(why),
            };
            let before = match disk {
                Ok(text) => text.replace("\r\n", "\n"),
                Err(why) => return fragments(why),
            };
            let mut after = before.clone();
            for e in edits {
                let old = e.old.replace("\r\n", "\n");
                if old.is_empty() || !after.contains(&old) {
                    return fragments(Reason::NoMatch);
                }
                let new = e.new.replace("\r\n", "\n");
                after = if e.replace_all {
                    after.replace(&old, &new)
                } else {
                    after.replacen(&old, &new, 1)
                };
            }
            Rebuilt {
                before,
                after,
                mode: Mode::Exact,
            }
        }
    }
}

// ---- what is said ---------------------------------------------------------------

const SHOWN: usize = 3;

const REMEDY_MARKER: &str = "Fix the finding in the code that raised it \
(general.no-disabled-safety). A suppression is allowed only for a cause outside the \
repository, named on the same line, in the tool's narrowest form. If you cannot fix it, \
stop and tell the person.";

const REMEDY_CONFIG: &str = "Fix the finding in the code that raised it \
(general.no-disabled-safety). Loosening a lint configuration needs its own commit and the \
person's agreement. If you cannot fix it, stop and tell the person.";

/// The advice, whole. `decision::phrase` joins reason and remedy with a space;
/// here the hits are a list and the remedy starts its own line, so the prefix
/// is the same and the join is a newline.
pub fn phrase(shown: &str, hits: &[Hit], mode: &Mode) -> String {
    let file = Path::new(shown)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(shown);
    let config = hits
        .first()
        .is_some_and(|h| matches!(h.origin, Origin::Setting { .. }));
    let n = hits.len();
    let mut text = if config {
        format!(
            "amont-agent/{}: this edit changes the lint configuration in {shown}:",
            RULE.id
        )
    } else {
        format!(
            "amont-agent/{}: this edit adds {n} lint suppression{} to {shown}:",
            RULE.id,
            if n == 1 { "" } else { "s" }
        )
    };
    for h in hits.iter().take(SHOWN) {
        let at = match mode {
            Mode::Exact | Mode::Bash => format!("line {}", h.line),
            Mode::Fragment(_) => "(line unknown)".to_string(),
        };
        let what = match &h.origin {
            Origin::Setting { section } => {
                let section = if section.is_empty() {
                    String::new()
                } else {
                    format!("[{section}] ")
                };
                format!("loosens {file} {section}{} = {}", h.kind, h.codes)
            }
            Origin::Comment { shown } => shown.clone(),
        };
        text.push_str(&format!("\n  {at}: {what}"));
    }
    if n > SHOWN {
        text.push_str(&format!("\n  and {} more", n - SHOWN));
    }
    text.push('\n');
    text.push_str(if config { REMEDY_CONFIG } else { REMEDY_MARKER });
    text
}

/// What a Bash call left in a working tree: the advice for `hits` found in
/// `repo`, one `path:line: marker` line each. The hook cannot tell which
/// command typed a hit, so the text does not claim that this one did.
pub fn bash_phrase(repo: &str, hits: &[(PathBuf, Hit)]) -> String {
    let config = hits
        .first()
        .is_some_and(|(_, h)| matches!(h.origin, Origin::Setting { .. }));
    let mut text = format!(
        "amont-agent/{}: the working tree of {repo} now holds {} lint suppression(s) its last commit does not:",
        RULE.id,
        hits.len()
    );
    for (path, h) in hits {
        let what = match &h.origin {
            Origin::Setting { section } => {
                let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let section = if section.is_empty() {
                    String::new()
                } else {
                    format!("[{section}] ")
                };
                format!("loosens {file} {section}{} = {}", h.kind, h.codes)
            }
            Origin::Comment { shown } => shown.clone(),
        };
        text.push_str(&format!("\n  {}:{}: {what}", path.display(), h.line));
    }
    text.push('\n');
    text.push_str(if config { REMEDY_CONFIG } else { REMEDY_MARKER });
    text
}

/// The journal excerpt: `<path>:<line> <kind>[<codes>] (+N) exact|fragment:<reason>`,
/// kept well inside the journal's 512-byte record by shortening the path from
/// the front.
pub fn excerpt(path: &str, hits: &[Hit], mode: &Mode) -> String {
    let Some(first) = hits.first() else {
        return path.to_string();
    };
    let line = match mode {
        Mode::Exact | Mode::Bash => first.line.to_string(),
        Mode::Fragment(_) => "?".to_string(),
    };
    let how = match mode {
        Mode::Exact => "exact".to_string(),
        Mode::Bash => "bash".to_string(),
        Mode::Fragment(why) => format!("fragment:{}", why.as_str()),
    };
    let codes: String = first.codes.chars().take(80).collect();
    let kind: String = first.kind.chars().take(60).collect();
    let tail = format!(":{line} {kind}[{codes}] (+{}) {how}", hits.len());
    let budget = 300usize.saturating_sub(tail.len());
    if path.len() <= budget {
        return format!("{path}{tail}");
    }
    let keep = budget.saturating_sub(3);
    let mut from = path.len() - keep.min(path.len());
    while !path.is_char_boundary(from) {
        from += 1;
    }
    format!("…{}{tail}", &path[from..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::EditPart;
    use std::path::PathBuf;

    /// `(line, kind[codes])` of what `after` adds to `before`.
    fn added(path: &str, before: &str, after: &str) -> Vec<(usize, String)> {
        examine_change(Path::new(path), before, after)
            .into_iter()
            .map(|h| (h.line, format!("{}[{}]", h.kind, h.codes)))
            .collect()
    }

    fn fires(path: &str, line: &str) -> bool {
        !added(path, "", line).is_empty()
    }

    fn silent(path: &str, before: &str, after: &str) {
        assert_eq!(added(path, before, after), vec![], "{path}: {after:?}");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("amont-agent-lsa-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn edit(old: &str, new: &str) -> Change {
        Change::Edits(vec![EditPart {
            old: old.to_string(),
            new: new.to_string(),
            replace_all: false,
        }])
    }

    // ---- the table, row by row ----

    #[test]
    fn python_rows() {
        for ok in [
            "x = f()  # type: ignore",
            "x = f()  # type: ignore[arg-type]",
            "import os  # noqa",
            "import os  # noqa: F401",
            "x = f()  # pyright: ignore[reportPrivateUsage]",
            "# pyright: basic",
            "# pyright: off",
            "# pyright: reportMissingImports=false",
            "# pyright: strict, reportFoo=none",
            "# ruff: noqa",
            "# ruff: noqa: F401",
        ] {
            assert!(fires("a/x.py", ok), "{ok}");
            assert!(fires("a/x.pyi", ok), "{ok} (pyi)");
        }
        for no in [
            "x = f()  # typed",
            "# pyright: strict",
            "# pyright: reportFoo=error",
            "# this is not a suppression",
            "# noqas",
            "x = 1  # type: int",
        ] {
            assert!(!fires("a/x.py", no), "{no}");
        }
        assert!(!fires("a/x.rb", "x = f()  # type: ignore"));
    }

    #[test]
    fn script_rows() {
        for ext in [
            "ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "vue", "svelte",
        ] {
            let path = format!("a/x.{ext}");
            for ok in [
                "// eslint-disable-next-line react-hooks/exhaustive-deps",
                "foo(); // eslint-disable-line no-console",
                "/* eslint-disable no-console */",
                "/* eslint-disable */",
                "// @ts-ignore",
                "// @ts-nocheck",
                "// @ts-expect-error: reason",
            ] {
                assert!(fires(&path, ok), "{path}: {ok}");
            }
            for no in ["// eslint-enable", "// just a comment", "// @ts-check"] {
                assert!(!fires(&path, no), "{path}: {no}");
            }
        }
        assert!(!fires("a/x.json", "// eslint-disable-next-line x"));
    }

    #[test]
    fn rust_rows() {
        for ok in [
            "#[allow(clippy::too_many_arguments)]",
            "#![allow(dead_code)]",
            "#[expect(unused)]",
            "#![expect(unused)]",
            "    #[allow(dead_code)]",
            "\t#[cfg_attr(test, allow(dead_code))]",
            "#[cfg_attr(feature = \"x\", expect(unused))]",
        ] {
            assert!(fires("src/x.rs", ok), "{ok}");
        }
        for no in [
            "#[derive(Debug)]",
            "#[deny(dead_code)]",
            "// #[allow(dead_code)]",
            "let a = 1; // #[allow(x)]",
        ] {
            assert!(!fires("src/x.rs", no), "{no}");
        }
    }

    #[test]
    fn an_indented_attribute_and_cfg_attr_fire() {
        let got = added(
            "x.rs",
            "",
            "impl A {\n    #[allow(dead_code)]\n    fn f() {}\n    #[cfg_attr(test, allow(unused, clippy::x))]\n}\n",
        );
        assert_eq!(
            got,
            vec![
                (2, "allow[dead_code]".to_string()),
                (4, "allow[unused]".to_string()),
                (4, "allow[clippy::x]".to_string()),
            ]
        );
    }

    #[test]
    fn go_rows() {
        assert!(fires("a.go", "x := f() //nolint"));
        assert!(fires("a.go", "x := f() //nolint:errcheck,gosec // why"));
        assert!(!fires("a.go", "// nolint is not the directive form"));
        assert!(!fires("a.go", "x := \"//nolint\""));
        assert_eq!(
            added("a.go", "", "f() //nolint:errcheck,gosec // why"),
            vec![(1, "nolint[errcheck]".into()), (1, "nolint[gosec]".into())]
        );
    }

    #[test]
    fn tsconfig_rows() {
        for name in [
            "tsconfig.json",
            "tsconfig.build.json",
            "sub/tsconfig.app.json",
        ] {
            for ok in [
                r#""strict": false"#,
                r#""noImplicitAny": false,"#,
                r#""strictNullChecks": false"#,
                r#""strictPropertyInitialization":false"#,
                r#"{"compilerOptions": {"strict": false}}"#,
            ] {
                assert!(fires(name, ok), "{name}: {ok}");
            }
            for no in [
                r#""strict": true"#,
                r#""target": "es2020""#,
                r#""noImplicitAny": true"#,
            ] {
                assert!(!fires(name, no), "{name}: {no}");
            }
        }
        assert!(!fires("package.json", r#""strict": false"#));
    }

    #[test]
    fn eslint_config_rows() {
        for name in [
            "eslint.config.js",
            "eslint.config.mjs",
            ".eslintrc",
            ".eslintrc.json",
            ".eslintrc.yml",
        ] {
            for ok in [
                r#""no-console": "off""#,
                "'no-console': 'warn',",
                "'react/x': 0,",
                "semi: 1,",
                r#""semi": ["warn", "always"]"#,
                "no-console: off",
            ] {
                assert!(fires(name, ok), "{name}: {ok}");
            }
            for no in [
                r#""no-console": "error""#,
                "'semi': 2,",
                r#""semi": ["error", "always"]"#,
            ] {
                assert!(!fires(name, no), "{name}: {no}");
            }
        }
    }

    #[test]
    fn pyright_rows() {
        for file in ["pyrightconfig.json", "sub/pyrightconfig.json"] {
            for ok in [
                r#""reportMissingImports": "none""#,
                r#""reportMissingImports": false,"#,
                r#""reportFoo": "warning""#,
                r#""reportFoo": "information""#,
                r#""typeCheckingMode": "off""#,
                r#""typeCheckingMode": "basic""#,
                r#""enableTypeIgnoreComments": true"#,
            ] {
                assert!(fires(file, ok), "{ok}");
            }
            for no in [
                r#""reportFoo": "error""#,
                r#""typeCheckingMode": "strict""#,
                r#""enableTypeIgnoreComments": false"#,
            ] {
                assert!(!fires(file, no), "{no}");
            }
        }
        let toml = "[tool.pyright]\ntypeCheckingMode = \"basic\"\nreportFoo = \"none\"\nenableTypeIgnoreComments = true\n";
        assert_eq!(added("pyproject.toml", "", toml).len(), 3);
        silent(
            "pyproject.toml",
            "",
            "[tool.pyright]\ntypeCheckingMode = \"strict\"\nreportFoo = \"error\"\n",
        );
    }

    #[test]
    fn ruff_rows() {
        let ruff = "[lint]\nignore = [\"E501\", \"F401\"]\n";
        for file in ["ruff.toml", ".ruff.toml"] {
            assert_eq!(added(file, "", ruff).len(), 2, "{file}");
        }
        let multi =
            "[lint]\nignore = [\n  \"E501\",\n  \"F401\", # why\n]\nselect = [\n  \"E\",\n]\n";
        assert_eq!(
            added("ruff.toml", "", multi),
            vec![(3, "ignore[E501]".into()), (4, "ignore[F401]".into())]
        );
        assert_eq!(
            added("ruff.toml", "", "extend-ignore = [\"D\"]\n"),
            vec![(1, "extend-ignore[D]".into())]
        );
        let per_file = "[lint.per-file-ignores]\n\"tests/*\" = [\"S101\"]\n";
        assert_eq!(
            added("ruff.toml", "", per_file),
            vec![(2, "tests/*[S101]".into())]
        );
        let pyproject = "[tool.ruff.lint]\nignore = [\"E501\"]\n";
        assert_eq!(added("pyproject.toml", "", pyproject).len(), 1);
        // An entry added inside an existing array.
        assert_eq!(
            added(
                "ruff.toml",
                "[lint]\nignore = [\"E501\"]\n",
                "[lint]\nignore = [\"E501\", \"F401\"]\n"
            ),
            vec![(2, "ignore[F401]".into())]
        );
        silent(
            "ruff.toml",
            "",
            "[lint]\nselect = [\"E\", \"F\"]\nignore = []\n",
        );
        silent("pyproject.toml", "", "[tool.other]\nignore = [\"x\"]\n");
    }

    #[test]
    fn cargo_rows() {
        let lints = "[lints.clippy]\ntoo_many_arguments = \"allow\"\npedantic = \"warn\"\n";
        assert_eq!(
            added("Cargo.toml", "", lints),
            vec![(2, "too_many_arguments[allow]".into())]
        );
        let ws = "[workspace.lints.rust]\nunused = \"allow\"\n";
        assert_eq!(added("Cargo.toml", "", ws).len(), 1);
        let table = "[lints.clippy.too_many_arguments]\nlevel = \"allow\"\npriority = -1\n";
        assert_eq!(
            added("Cargo.toml", "", table),
            vec![(2, "level[allow]".into())]
        );
        silent("Cargo.toml", "", "[lints.clippy]\nx = \"deny\"\n");
        silent("Cargo.toml", "", "[package]\nname = \"allow\"\n");
    }

    #[test]
    fn golangci_rows() {
        for file in [".golangci.yml", ".golangci.yaml"] {
            let yml = "linters:\n  disable:\n    - errcheck\n    - gosec\n";
            assert_eq!(added(file, "", yml).len(), 2, "{file}");
        }
        let yml = "issues:\n  exclude-rules:\n    - path: _test\\.go\n      linters:\n        - errcheck\n";
        assert_eq!(
            added(".golangci.yml", "", yml)
                .into_iter()
                .map(|(l, _)| l)
                .collect::<Vec<_>>(),
            vec![3, 5]
        );
        let v2 = "linters:\n  exclusions:\n    paths:\n      - vendor\n";
        assert_eq!(added(".golangci.yml", "", v2).len(), 1);
        assert_eq!(
            added(".golangci.yml", "", "linters:\n  disable-all: true\n"),
            vec![(2, "disable-all[true]".into())]
        );
        assert_eq!(
            added(".golangci.yml", "", "linters:\n  default: none\n"),
            vec![(2, "default[none]".into())]
        );
        // Same-indent sequence.
        assert_eq!(
            added(".golangci.yml", "", "linters:\n  disable:\n  - errcheck\n").len(),
            1
        );
        silent(
            ".golangci.yml",
            "",
            "linters:\n  enable:\n    - errcheck\n  default: standard\n",
        );
        silent(".golangci.yml", "", "linters:\n  disable-all: false\n");
    }

    // ---- what is added, and what is not ----

    #[test]
    fn a_line_edited_with_its_suppression_kept_is_silent() {
        silent(
            "x.py",
            "x = f(a)  # type: ignore[arg-type]\n",
            "x = f(b)  # type: ignore[arg-type]\n",
        );
        silent(
            "x.rs",
            "#[allow(dead_code)]\nfn f() {}\n",
            "#[allow(dead_code)]\nfn g() {}\n",
        );
    }

    #[test]
    fn a_moved_suppression_is_silent() {
        silent(
            "x.py",
            "a = 1  # noqa: E501\nb = 2\n",
            "a = 1\nb = 2  # noqa: E501\n",
        );
    }

    #[test]
    fn a_code_added_to_an_existing_suppression_fires_on_the_new_code_only() {
        assert_eq!(
            added(
                "x.py",
                "a = 1  # noqa: E501\n",
                "a = 1  # noqa: E501,F401\n"
            ),
            vec![(1, "noqa[F401]".to_string())]
        );
        assert_eq!(
            added(
                "x.ts",
                "// eslint-disable-next-line a\n",
                "// eslint-disable-next-line a, b\n"
            ),
            vec![(1, "eslint-disable-next-line[b]".to_string())]
        );
    }

    #[test]
    fn a_second_occurrence_is_a_hit_and_a_removal_is_not() {
        let before = "a  # noqa: E501\n";
        assert_eq!(
            added("x.py", before, "a  # noqa: E501\nb  # noqa: E501\n").len(),
            1
        );
        silent("x.py", "a  # noqa: E501\nb  # noqa: E501\n", before);
    }

    #[test]
    fn a_string_literal_is_not_a_suppression() {
        for line in [
            "x = \"# noqa\"",
            "x = '# type: ignore'",
            "x = \"a \\\" # noqa\"",
            "\"\"\"\n# noqa\n# type: ignore\n\"\"\"",
        ] {
            silent("x.py", "", line);
        }
        assert!(fires("x.py", "x = \"a\"  # noqa"));
        for line in [
            "let s = \"#[allow(dead_code)]\";",
            "let s = \"// eslint-disable\";",
            "log(\"  #[allow(x)]\");",
        ] {
            silent("x.rs", "", line);
        }
        silent("x.ts", "", "const s = \"// eslint-disable-next-line\";");
        silent("x.ts", "", "const s = `// @ts-ignore\n`;");
        silent("x.go", "", "s := \"//nolint\"");
    }

    #[test]
    fn a_markdown_file_is_never_examined() {
        for line in [
            "# noqa",
            "# type: ignore",
            "#[allow(dead_code)]",
            "// eslint-disable",
        ] {
            silent("README.md", "", line);
        }
        let noqa = Change::Write {
            content: "# noqa\n".to_string(),
        };
        assert!(!worth_rebuilding(Path::new("docs/x.md"), &noqa));
        assert!(!worth_rebuilding(Path::new("notes.txt"), &noqa));
        assert!(worth_rebuilding(Path::new("x.py"), &noqa));
    }

    #[test]
    fn a_source_edit_naming_no_keyword_is_not_rebuilt() {
        assert!(!worth_rebuilding(
            Path::new("x.py"),
            &edit("a = 1", "a = 2")
        ));
        assert!(!worth_rebuilding(Path::new("x.ts"), &edit("a", "b")));
        assert!(!worth_rebuilding(Path::new("x.rs"), &edit("a", "b")));
        assert!(!worth_rebuilding(
            Path::new("x.rs"),
            &edit("f()", "f().expect(\"allowed\")")
        ));
        assert!(!worth_rebuilding(Path::new("x.go"), &edit("a", "b")));
        // The keywords are matched without case: ruff reads `# NOQA` too.
        assert!(worth_rebuilding(Path::new("x.py"), &edit("a", "a  # NOQA")));
        assert!(worth_rebuilding(
            Path::new("x.ts"),
            &edit("a", "// @ts-ignore\na")
        ));
        assert!(worth_rebuilding(
            Path::new("x.rs"),
            &edit("", "#[allow(x)]")
        ));
        assert!(worth_rebuilding(
            Path::new("x.go"),
            &edit("a", "a //nolint")
        ));
    }

    #[test]
    fn a_configuration_file_is_always_rebuilt() {
        for name in [
            "pyproject.toml",
            "Cargo.toml",
            ".golangci.yml",
            "tsconfig.json",
            "eslint.config.js",
            "pyrightconfig.json",
            "ruff.toml",
        ] {
            assert!(worth_rebuilding(Path::new(name), &edit("a", "b")), "{name}");
        }
    }

    #[test]
    fn lines_split_on_lf_and_a_trailing_cr_is_trimmed() {
        assert_eq!(
            added("x.py", "a\r\n", "a\r\nb  # noqa\r\nc  # noqa\r\n"),
            vec![(2, "noqa[]".into()), (3, "noqa[]".into())]
        );
        silent("x.py", "a  # noqa\r\n", "a  # noqa\n");
    }

    #[test]
    fn a_crlf_pyproject_keeps_its_sections() {
        let before = "[tool.pyright]\r\ntypeCheckingMode = \"strict\"\r\n";
        let after = "[tool.pyright]\r\ntypeCheckingMode = \"basic\"\r\n";
        assert_eq!(
            added("pyproject.toml", before, after),
            vec![(2, "typeCheckingMode[basic]".into())]
        );
    }

    #[test]
    fn section_context_decides_pyproject_cargo_and_golangci() {
        let line = "typeCheckingMode = \"off\"\n";
        silent("pyproject.toml", "", &format!("[tool.black]\n{line}"));
        assert_eq!(
            added("pyproject.toml", "", &format!("[tool.pyright]\n{line}")).len(),
            1
        );
        silent("Cargo.toml", "", "[dependencies]\nfoo = \"allow\"\n");
        assert_eq!(
            added("Cargo.toml", "", "[lints.rust]\nfoo = \"allow\"\n").len(),
            1
        );
        silent(".golangci.yml", "", "linters:\n  enable:\n    - a\n");
        assert_eq!(
            added(".golangci.yml", "", "linters:\n  disable:\n    - a\n").len(),
            1
        );
        // The same setting under two sections is two settings.
        assert_eq!(
            added(
                "Cargo.toml",
                "[lints.rust]\nfoo = \"allow\"\n",
                "[lints.rust]\nfoo = \"allow\"\n[workspace.lints.rust]\nfoo = \"allow\"\n"
            ),
            vec![(4, "foo[allow]".into())]
        );
    }

    // ---- known false negatives, each asserting the documented behaviour ----

    #[test]
    fn a_dotted_toml_key_is_a_known_false_negative() {
        silent("Cargo.toml", "", "lints.clippy.x = \"allow\"\n");
        silent("ruff.toml", "", "lint.ignore = [\"E501\"]\n");
    }

    #[test]
    fn an_inline_table_is_a_known_false_negative() {
        silent(
            "Cargo.toml",
            "",
            "[lints.clippy]\nx = { level = \"allow\", priority = 1 }\n",
        );
        silent(
            "ruff.toml",
            "",
            "per-file-ignores = { \"a.py\" = [\"E501\"] }\n",
        );
    }

    #[test]
    fn a_yaml_flow_sequence_is_a_known_false_negative() {
        silent(
            ".golangci.yml",
            "",
            "linters:\n  disable: [errcheck, gosec]\n",
        );
    }

    #[test]
    fn a_fragment_has_no_section_context() {
        // Whole file: a hit. The fragment alone: no `[lints]` header.
        assert_eq!(
            added("Cargo.toml", "", "[lints.rust]\nx = \"allow\"\n").len(),
            1
        );
        silent("Cargo.toml", "", "x = \"allow\"\n");
        silent("pyproject.toml", "", "typeCheckingMode = \"off\"\n");
        silent(".golangci.yml", "", "- errcheck\n");
    }

    #[test]
    fn a_swap_in_one_edit_is_a_known_false_negative() {
        silent(
            "x.py",
            "a = 1  # noqa: E501\nb = 2\n",
            "a = 1\nb = 2  # noqa: E501\n",
        );
        silent(
            "x.py",
            "a = 1  # type: ignore[x]\nb = 2  # noqa\n",
            "a = 1  # noqa\nb = 2  # type: ignore[x]\n",
        );
    }

    /// The reported line is the first occurrence past the before count in
    /// the after text, which can be the older one.
    #[test]
    fn the_reported_line_can_be_an_earlier_preexisting_occurrence() {
        let before = "a  # noqa: E501\nb\n";
        let after = "a  # noqa: E501\nb  # noqa: E501\n";
        // One was already there, one is new: n = 1, m = 2. The occurrences
        // past the first n, in line order, are the hit: line 2.
        assert_eq!(added("x.py", before, after), vec![(2, "noqa[E501]".into())]);
        // The new one typed above the old one: still one hit, and it is
        // reported on the later line, which is the pre-existing text.
        let after = "z  # noqa: E501\na  # noqa: E501\nb\n";
        assert_eq!(added("x.py", before, after), vec![(2, "noqa[E501]".into())]);
    }

    // ---- reconstruct ----

    #[test]
    fn a_write_is_the_content_against_the_file_on_disk() {
        let dir = scratch("write");
        let f = dir.join("x.py");
        std::fs::write(&f, "a  # noqa\n").unwrap();
        let r = reconstruct(
            &f,
            &Change::Write {
                content: "b\n".into(),
            },
        );
        assert_eq!(
            (r.before.as_str(), r.after.as_str()),
            ("a  # noqa\n", "b\n")
        );
        assert_eq!(r.mode, Mode::Exact);
        let new = reconstruct(
            &dir.join("new.py"),
            &Change::Write {
                content: "b\n".into(),
            },
        );
        assert_eq!((new.before.as_str(), new.mode), ("", Mode::Exact));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_over_an_unreadable_file_reports_nothing() {
        let dir = scratch("write-unreadable");
        let content = "a  # noqa\nb  # type: ignore\n".to_string();
        let big = dir.join("big.py");
        std::fs::write(&big, vec![b'a'; MAX_READ as usize + 1]).unwrap();
        let binary = dir.join("bin.py");
        std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
        for (f, why) in [(&big, Reason::TooLarge), (&binary, Reason::NotUtf8)] {
            let write = Change::Write {
                content: content.clone(),
            };
            let r = reconstruct(f, &write);
            assert_eq!(r.mode, Mode::Fragment(why));
            assert!(examine_change(f, &r.before, &r.after).is_empty());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edit_is_applied_in_order_and_honours_replace_all() {
        let dir = scratch("edit");
        let f = dir.join("x.py");
        std::fs::write(&f, "a\na\nb\n").unwrap();
        let change = Change::Edits(vec![
            EditPart {
                old: "a".into(),
                new: "c".into(),
                replace_all: true,
            },
            EditPart {
                old: "c\nb".into(),
                new: "d".into(),
                replace_all: false,
            },
        ]);
        let r = reconstruct(&f, &change);
        assert_eq!(
            (r.before.as_str(), r.after.as_str()),
            ("a\na\nb\n", "c\nd\n")
        );
        assert_eq!(r.mode, Mode::Exact);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crlf_file_matches_an_lf_old_string() {
        let dir = scratch("crlf");
        let f = dir.join("pyproject.toml");
        std::fs::write(&f, "[tool.pyright]\r\ntypeCheckingMode = \"strict\"\r\n").unwrap();
        let r = reconstruct(&f, &edit("\"strict\"\n", "\"off\"\n"));
        assert_eq!(r.mode, Mode::Exact);
        assert_eq!(
            examine_change(&f, &r.before, &r.after)
                .iter()
                .map(|h| h.line)
                .collect::<Vec<_>>(),
            vec![2]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn fragment(path: &Path, change: &Change) -> (Reason, String, String) {
        let r = reconstruct(path, change);
        match r.mode {
            Mode::Fragment(why) => (why, r.before, r.after),
            Mode::Exact | Mode::Bash => panic!("expected a fragment for {}", path.display()),
        }
    }

    #[test]
    fn a_missing_file_falls_back_to_the_fragments() {
        let dir = scratch("missing");
        let (why, before, after) = fragment(&dir.join("nope.py"), &edit("a", "b  # noqa"));
        assert_eq!(
            (why, before.as_str(), after.as_str()),
            (Reason::Missing, "a", "b  # noqa")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_string_that_is_not_found_falls_back() {
        let dir = scratch("nomatch");
        let f = dir.join("x.py");
        std::fs::write(&f, "a\n").unwrap();
        let change = Change::Edits(vec![
            EditPart {
                old: "a".into(),
                new: "b".into(),
                replace_all: false,
            },
            EditPart {
                old: "zzz".into(),
                new: "c  # noqa".into(),
                replace_all: false,
            },
        ]);
        let (why, before, after) = fragment(&f, &change);
        assert_eq!(why, Reason::NoMatch);
        assert_eq!(
            (before.as_str(), after.as_str()),
            ("a\nzzz", "b\nc  # noqa")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn not_utf8_falls_back() {
        let dir = scratch("utf8");
        let f = dir.join("x.py");
        std::fs::write(&f, [0xff, 0xfe, b'a']).unwrap();
        assert_eq!(fragment(&f, &edit("a", "b")).0, Reason::NotUtf8);
        assert_eq!(
            fragment(
                &f,
                &Change::Write {
                    content: "x  # noqa".into()
                }
            )
            .0,
            Reason::NotUtf8
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_is_not_a_file() {
        let dir = scratch("dir");
        assert_eq!(fragment(&dir, &edit("a", "b")).0, Reason::NotFile);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_path_is_an_io_reason() {
        // A NUL in the name: `metadata` fails with something other than NotFound.
        let (why, _, _) = fragment(Path::new("a\0b.py"), &edit("a", "b"));
        assert_eq!(why, Reason::Io);
    }

    #[test]
    fn a_file_over_the_read_cap_is_not_read() {
        let dir = scratch("large");
        let f = dir.join("x.py");
        std::fs::write(&f, vec![b'a'; MAX_READ as usize + 1]).unwrap();
        assert_eq!(fragment(&f, &edit("a", "b")).0, Reason::TooLarge);
        std::fs::write(&f, vec![b'a'; MAX_READ as usize]).unwrap();
        assert_eq!(reconstruct(&f, &edit("a", "b")).mode, Mode::Exact);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_never_opened() {
        let dir = scratch("fifo");
        let fifo = dir.join("pipe.py");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(made.success(), "mkfifo {}", fifo.display());
        // Opening a FIFO with no writer would block; this returns.
        assert_eq!(fragment(&fifo, &edit("a", "b  # noqa")).0, Reason::NotFile);
        assert_eq!(
            fragment(
                &fifo,
                &Change::Write {
                    content: "x".into()
                }
            )
            .0,
            Reason::NotFile
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_edit_fragments_are_joined_with_newlines() {
        let dir = scratch("multi");
        let change = Change::Edits(vec![
            EditPart {
                old: "a".into(),
                new: "b".into(),
                replace_all: false,
            },
            EditPart {
                old: "c".into(),
                new: "d".into(),
                replace_all: false,
            },
        ]);
        let (_, before, after) = fragment(&dir.join("gone.py"), &change);
        assert_eq!((before.as_str(), after.as_str()), ("a\nc", "b\nd"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_reason_names_are_the_journals() {
        let names: Vec<&str> = [
            Reason::Missing,
            Reason::TooLarge,
            Reason::NotFile,
            Reason::NotUtf8,
            Reason::Io,
            Reason::NoMatch,
        ]
        .iter()
        .map(|r| r.as_str())
        .collect();
        assert_eq!(
            names,
            [
                "missing",
                "too-large",
                "not-file",
                "not-utf8",
                "io",
                "no-match"
            ]
        );
    }

    // ---- what is said ----

    #[test]
    fn the_whole_file_phrase_is_pinned() {
        let mut text = "a = 1\n".repeat(41);
        text.push_str("x  # type: ignore[arg-type]\n");
        text.push_str(&"b = 2\n".repeat(14));
        text.push_str("y  # noqa: E501\n");
        let hits = examine_change(Path::new("/abs/src/x.py"), "", &text);
        assert_eq!(
            phrase("/abs/src/x.py", &hits, &Mode::Exact),
            "amont-agent/lint-suppression-added: this edit adds 2 lint suppressions to /abs/src/x.py:\n  \
             line 42: # type: ignore[arg-type]\n  \
             line 57: # noqa: E501\n\
             Fix the finding in the code that raised it (general.no-disabled-safety). A suppression is allowed only for a cause outside the repository, named on the same line, in the tool's narrowest form. If you cannot fix it, stop and tell the person."
        );
    }

    #[test]
    fn the_fragment_phrase_is_pinned() {
        let hits = examine_change(Path::new("/abs/x.py"), "", "a  # noqa: E501\n");
        assert_eq!(
            phrase("/abs/x.py", &hits, &Mode::Fragment(Reason::Missing)),
            "amont-agent/lint-suppression-added: this edit adds 1 lint suppression to /abs/x.py:\n  \
             (line unknown): # noqa: E501\n\
             Fix the finding in the code that raised it (general.no-disabled-safety). A suppression is allowed only for a cause outside the repository, named on the same line, in the tool's narrowest form. If you cannot fix it, stop and tell the person."
        );
    }

    #[test]
    fn at_most_three_hits_are_shown_then_and_n_more() {
        let text = "a  # noqa\n".repeat(5);
        let hits = examine_change(Path::new("x.py"), "", &text);
        let said = phrase("x.py", &hits, &Mode::Exact);
        assert!(said.contains("adds 5 lint suppressions to x.py:"), "{said}");
        assert!(
            said.contains("  line 3: # noqa\n  and 2 more\nFix the finding"),
            "{said}"
        );
        assert!(!said.contains("line 4"), "{said}");
        let four = phrase("x.py", &hits[..4], &Mode::Exact);
        assert!(four.contains("  and 1 more\n"), "{four}");
        let three = phrase("x.py", &hits[..3], &Mode::Exact);
        assert!(!three.contains("more"), "{three}");
    }

    #[test]
    fn a_config_hit_says_loosens_and_asks_for_agreement() {
        let hits = examine_change(
            Path::new("/r/pyproject.toml"),
            "",
            "[tool.pyright]\ntypeCheckingMode = \"off\"\n",
        );
        assert_eq!(
            phrase("/r/pyproject.toml", &hits, &Mode::Exact),
            "amont-agent/lint-suppression-added: this edit changes the lint configuration in /r/pyproject.toml:\n  \
             line 2: loosens pyproject.toml [tool.pyright] typeCheckingMode = off\n\
             Fix the finding in the code that raised it (general.no-disabled-safety). Loosening a lint configuration needs its own commit and the person's agreement. If you cannot fix it, stop and tell the person."
        );
        let flat = examine_change(Path::new("tsconfig.json"), "", "\"strict\": false\n");
        assert!(phrase("tsconfig.json", &flat, &Mode::Exact)
            .contains("line 1: loosens tsconfig.json strict = false\n"));
    }

    #[test]
    fn the_journal_excerpt_is_pinned_and_bounded() {
        let hits = examine_change(Path::new("x.py"), "", "a  # noqa: E501\nb  # noqa\n");
        assert_eq!(
            excerpt("/abs/x.py", &hits, &Mode::Exact),
            "/abs/x.py:1 noqa[E501] (+2) exact"
        );
        assert_eq!(
            excerpt("/abs/x.py", &hits, &Mode::Fragment(Reason::NoMatch)),
            "/abs/x.py:? noqa[E501] (+2) fragment:no-match"
        );
        let long = format!("/{}/x.py", "é".repeat(400));
        let said = excerpt(&long, &hits, &Mode::Exact);
        assert!(said.len() <= 300, "{}", said.len());
        assert!(
            said.starts_with('…') && said.ends_with("x.py:1 noqa[E501] (+2) exact"),
            "{said}"
        );
    }

    fn reasons(path: &str, text: &str) -> Vec<bool> {
        markers(Path::new(path), text)
            .iter()
            .map(|m| reasoned(m, from_line(text, m.line)))
            .collect()
    }

    #[test]
    fn shell_markers_in_every_shell_extension() {
        for path in ["a/x.sh", "x.bash", "x.zsh"] {
            assert_eq!(
                added(path, "", "# shellcheck disable=SC1091,SC2086\n"),
                vec![
                    (1, "shellcheck disable[SC1091]".to_string()),
                    (1, "shellcheck disable[SC2086]".to_string())
                ],
                "{path}"
            );
            assert!(!fires(path, "# shellcheck source=lib.sh"), "{path}");
            assert!(!fires(path, "echo '# shellcheck disable=SC1'"), "{path}");
            assert!(!fires(path, "echo $# shellcheck disable=SC1"), "{path}");
        }
        assert!(!fires("x.txt", "# shellcheck disable=SC2086"));
    }

    #[test]
    fn an_extensionless_file_is_shell_by_its_shebang() {
        let body = "# shellcheck disable=SC2086\n";
        for head in [
            "#!/bin/sh",
            "#!/usr/bin/env bash",
            "#!/usr/bin/env -S zsh -e",
            "#!/bin/bash -eu",
        ] {
            let text = format!("{head}\n{body}");
            assert_eq!(added("bin/run", "", &text).len(), 1, "{head}");
        }
        assert!(added("bin/run", "", &format!("#!/usr/bin/python3\n{body}")).is_empty());
        assert!(added("bin/run", "", body).is_empty());
        assert!(added("bin/run.txt", "", &format!("#!/bin/sh\n{body}")).is_empty());
        assert!(!shell_by_shebang(Path::new("run"), "#!/bin/dash\n"));
    }

    #[test]
    fn the_shell_keyword_makes_a_shell_edit_worth_rebuilding() {
        let hit = edit("", "# shellcheck disable=SC2086");
        let miss = edit("", "echo hi");
        assert!(worth_rebuilding(Path::new("x.sh"), &hit));
        assert!(!worth_rebuilding(Path::new("x.sh"), &miss));
    }

    #[test]
    fn reasoned_rows_by_form() {
        let cases: &[(&str, &str, bool)] = &[
            (
                "x.ts",
                "// eslint-disable-next-line no-x -- upstream bug",
                true,
            ),
            ("x.ts", "/* eslint-disable no-x -- upstream bug */", true),
            ("x.ts", "// eslint-disable-next-line no-x", false),
            ("x.ts", "// eslint-disable-next-line no-x --", false),
            ("x.ts", "// @ts-expect-error: upstream typing", true),
            ("x.ts", "/* @ts-expect-error upstream typing */", true),
            ("x.ts", "// @ts-expect-error", false),
            (
                "x.rs",
                "#[allow(dead_code, reason = \"used by tests\")]",
                true,
            ),
            ("x.rs", "#[expect(unused, reason = \"upstream\")]", true),
            ("x.rs", "#[allow(dead_code)]", false),
            ("x.rs", "#[expect(unused, reason = \"\")]", false),
            // The reason must be an item of the attribute itself.
            ("x.rs", "#[allow(dead_code)] // reason = \"x\"", false),
            ("x.rs", "#[allow(clippy::reasonless)]", false),
            (
                "x.rs",
                "#[expect(clippy::reasonable, reason = \"upstream\")]",
                true,
            ),
            (
                "x.rs",
                "#[cfg_attr(test, expect(dead_code, reason = \"used by tests\"))]",
                true,
            ),
            ("x.py", "x  # noqa: F401 -- re-export", true),
            ("x.py", "x  # noqa: F401  # re-export", true),
            ("x.py", "x  # noqa: F401, E501 -- wide line", true),
            ("x.py", "x  # noqa: F401", false),
            ("x.py", "x  # noqa: F401 --", false),
            ("x.py", "x  # type: ignore[arg-type] -- stub missing", true),
            ("x.py", "x  # type: ignore[arg-type]  # stub missing", true),
            ("x.py", "x  # type: ignore[arg-type]", false),
            (
                "x.py",
                "x  # pyright: ignore[reportFoo] -- stub missing",
                true,
            ),
            (
                "x.py",
                "x  # pyright: ignore[reportFoo]  # stub missing",
                true,
            ),
            ("x.py", "x  # pyright: ignore[reportFoo]", false),
            (
                "x.py",
                "x  # noqa: F401  # pyright: ignore[reportFoo]",
                false,
            ),
            ("x.go", "f() //nolint:errcheck // best effort", true),
            ("x.go", "f() //nolint:errcheck", false),
            (
                "x.sh",
                "# shellcheck disable=SC2086 # word splitting wanted",
                true,
            ),
            (
                "x.sh",
                "# shellcheck disable=SC1091,SC2086 # sourced at runtime",
                true,
            ),
            ("x.sh", "# shellcheck disable=SC2086", false),
            ("x.sh", "# shellcheck disable=SC2086 #", false),
        ];
        for (path, text, want) in cases {
            let got = reasons(path, text);
            assert!(!got.is_empty(), "{path}: {text}: no marker");
            assert!(got.iter().all(|g| g == want), "{path}: {text}: {got:?}");
        }
    }

    #[test]
    fn never_reasoned_forms() {
        for (path, text) in [
            ("x.ts", "/* eslint-disable */ // why"),
            ("x.ts", "// eslint-disable-next-line -- why"),
            ("x.py", "x  # noqa -- why"),
            ("x.py", "x  # type: ignore -- why"),
            ("x.py", "x  # pyright: ignore -- why"),
            ("x.go", "f() //nolint // why"),
            ("x.ts", "// @ts-ignore: why"),
            ("x.ts", "// @ts-nocheck: why"),
            ("x.py", "# ruff: noqa: F401 -- why"),
            ("x.py", "# ruff: noqa -- why"),
            ("x.py", "# pyright: basic  # why"),
            ("x.py", "# pyright: reportFoo=none  # why"),
            ("tsconfig.json", "\"strict\": false // why"),
            ("Cargo.toml", "[lints.clippy]\nx = \"allow\" # why"),
            ("ruff.toml", "[lint]\nignore = [\"E501\"] # why"),
        ] {
            let got = reasons(path, text);
            assert!(!got.is_empty(), "{path}: {text}: no marker");
            assert!(got.iter().all(|g| !g), "{path}: {text}: {got:?}");
        }
    }

    fn net(changes: &[(&Path, &str, &Path, &str)]) -> Vec<(String, usize, String)> {
        let changes: Vec<FileChange<'_>> = changes
            .iter()
            .map(|&(old_path, before, new_path, after)| FileChange {
                old_path,
                before,
                new_path,
                after,
            })
            .collect();
        net_added(&changes)
            .into_iter()
            .map(|added| {
                (
                    added.path.display().to_string(),
                    added.hit.line,
                    format!("{}[{}]", added.hit.kind, added.hit.codes),
                )
            })
            .collect()
    }

    #[test]
    fn a_marker_moved_between_files_cancels() {
        let marker = "x = f()  # type: ignore[arg-type]\n";
        let got = net(&[
            (Path::new("a.py"), marker, Path::new("a.py"), ""),
            (Path::new("b.py"), "", Path::new("b.py"), marker),
        ]);
        assert_eq!(got, vec![]);
        // Delete plus add of a whole file.
        let got = net(&[
            (Path::new("a.py"), marker, Path::new("a.py"), ""),
            (Path::new("pkg/a.py"), "", Path::new("pkg/a.py"), marker),
        ]);
        assert_eq!(got, vec![]);
    }

    #[test]
    fn a_wrapped_attribute_is_one_marker_with_its_codes_and_reason() {
        // rustfmt's shape for a long attribute.
        let wrapped = "#[expect(\n    clippy::too_many_arguments,\n    reason = \"the wire format fixes the arity\"\n)]\nfn f() {}\n";
        let found = markers(Path::new("x.rs"), wrapped);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (
                found[0].kind.as_str(),
                found[0].codes.as_str(),
                found[0].line
            ),
            ("expect", "clippy::too_many_arguments", 1)
        );
        assert!(reasoned(&found[0], wrapped));
        // Added once, at its opening line; the gate keeps it as reasoned.
        assert_eq!(
            net(&[(Path::new("x.rs"), "", Path::new("x.rs"), wrapped)]),
            vec![(
                "x.rs".to_string(),
                1,
                "expect[clippy::too_many_arguments]".to_string()
            )]
        );
        let bare = "#[expect(\n    clippy::too_many_arguments,\n)]\nfn f() {}\n";
        let found = markers(Path::new("x.rs"), bare);
        assert_eq!(found.len(), 1);
        assert!(!reasoned(&found[0], bare));
        // A reason on a later item of another attribute is not this one's.
        let next = "#[allow(dead_code)]\n#[expect(unused, reason = \"x\")]\n";
        let found = markers(Path::new("x.rs"), next);
        assert!(!reasoned(&found[0], from_line(next, found[0].line)));
        assert!(reasoned(&found[1], from_line(next, found[1].line)));
    }

    #[test]
    fn hits_sort_by_the_paths_text_as_the_python_port_does() {
        let marker = "x = f()  # noqa: F401\n";
        let got = net(&[
            (Path::new("a/b.py"), "", Path::new("a/b.py"), marker),
            (Path::new("a.py"), "", Path::new("a.py"), marker),
        ]);
        let paths: Vec<_> = got.iter().map(|g| g.0.clone()).collect();
        assert_eq!(paths, vec!["a.py".to_string(), "a/b.py".to_string()]);
    }

    #[test]
    fn a_surplus_is_reported_at_the_new_path_in_path_then_line_order() {
        let got = net(&[
            (
                Path::new("z.py"),
                "",
                Path::new("z.py"),
                "a  # noqa: E501\n",
            ),
            (
                Path::new("old.py"),
                "",
                Path::new("b.py"),
                "x\n# noqa: F401\ny  # noqa: E501\n",
            ),
        ]);
        assert_eq!(
            got,
            vec![
                ("b.py".to_string(), 2, "noqa[F401]".to_string()),
                ("b.py".to_string(), 3, "noqa[E501]".to_string()),
                ("z.py".to_string(), 1, "noqa[E501]".to_string()),
            ]
        );
    }

    #[test]
    fn a_rename_is_classified_by_each_sides_own_path() {
        let bare = "x = f()  # noqa: F821\n";
        let got = net(&[(Path::new("a.txt"), bare, Path::new("a.py"), bare)]);
        assert_eq!(got, vec![("a.py".to_string(), 1, "noqa[F821]".to_string())]);
        let got = net(&[(Path::new("a.py"), bare, Path::new("a.txt"), bare)]);
        assert_eq!(got, vec![]);
    }

    #[test]
    fn removing_a_reasoned_marker_does_not_cancel_an_added_bare_one() {
        let reasoned_line = "x  # noqa: E501 -- generated table\n";
        let bare_line = "y  # noqa: E501\n";
        let got = net(&[
            (Path::new("a.py"), reasoned_line, Path::new("a.py"), ""),
            (Path::new("b.py"), "", Path::new("b.py"), bare_line),
        ]);
        assert_eq!(got, vec![("b.py".to_string(), 1, "noqa[E501]".to_string())]);
        // The same reason kept cancels.
        let got = net(&[
            (Path::new("a.py"), reasoned_line, Path::new("a.py"), ""),
            (Path::new("b.py"), "", Path::new("b.py"), reasoned_line),
        ]);
        assert_eq!(got, vec![]);
    }

    #[test]
    fn examine_change_ignores_reasons() {
        silent(
            "x.py",
            "a  # noqa: E501\n",
            "a  # noqa: E501 -- generated table\n",
        );
    }

    /// The shared fixture, through the fragment path. `tools/suppression-rate.py
    /// --self-test` classifies the same file.
    #[test]
    fn the_shared_fixture_classifies_through_the_fragment_path() {
        let text = include_str!("../../../../tests/fixtures/suppressions.txt");
        let mut checked = 0;
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.splitn(3, '\t');
            let (tag, file, sample) = (
                parts.next().unwrap(),
                parts
                    .next()
                    .unwrap_or_else(|| panic!("line {}: no file", n + 1)),
                parts
                    .next()
                    .unwrap_or_else(|| panic!("line {}: no sample", n + 1)),
            );
            let sample = sample.replace("\\n", "\n");
            let got = match tag {
                "+" | "-" => Some(!added(file, "", &sample).is_empty()),
                "r" | "b" => {
                    let hits = net_added(&[FileChange {
                        old_path: Path::new(""),
                        before: "",
                        new_path: Path::new(file),
                        after: &sample,
                    }]);
                    let reasons: Vec<bool> = hits.iter().map(|added| added.reasoned).collect();
                    let want = tag == "r";
                    Some(!reasons.is_empty() && reasons.iter().all(|r| *r == want))
                }
                "n+" | "n-" => {
                    let (old, new) = file.split_once('>').unwrap_or((file, file));
                    let (before, after) = sample.split_once("@@").unwrap_or(("", &sample));
                    let hits = net_added(&[FileChange {
                        old_path: Path::new(old),
                        before,
                        new_path: Path::new(new),
                        after,
                    }]);
                    Some(!hits.is_empty())
                }
                other => panic!("line {}: unknown tag {other}", n + 1),
            };
            let expected = matches!(tag, "+" | "r" | "b" | "n+");
            assert_eq!(got, Some(expected), "line {}: {line}", n + 1);
            checked += 1;
        }
        assert!(checked >= 40, "the fixture holds {checked} samples");
    }
}
