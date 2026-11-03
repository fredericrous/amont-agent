//! `amont-agent suppressions --staged`: the commit gate for lint suppressions.
//!
//! It judges what the staged change adds, never the working tree, and never
//! what a file already held. Every failure to look is exit 2 (fail closed);
//! the only way past a blocking hit is a command-scoped
//! `git -c amont.agent.suppressions=allow commit`, which the person types and
//! the `suppressions-override` rule keeps from an agent.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use amont_agent_guard::rules::lint_suppression_added::{
    self as lint, FileChange, Hit, Origin, RULE,
};
use amont_agent_guard::rules::Stance;
use amont_agent_guard::stance;

const OVERRIDE_KEY: &str = "amont.agent.suppressions";
/// A blob over this is never read.
const MAX_BLOB: u64 = 4 * 1024 * 1024;
/// The sum of every requested blob; over it the scan is incomplete.
const BUDGET: u64 = 64 * 1024 * 1024;
/// Room for one `cat-file` header and its trailing newline.
const HEADER_ALLOWANCE: usize = 160;
const SKIPPED_NOTE: &str = "over 4 MiB";

pub const HELP: &str = "\
usage: amont-agent suppressions --staged

Judge the staged change: a lint suppression it adds (net of those it removes
anywhere) blocks unless it carries a reason in its tool's own reason form.
Exit 0: nothing blocking. 1: a blocking hit. 2: usage error or git failure.
A command-scoped `git -c amont.agent.suppressions=allow commit` passes; the
same key in repository or global config is ignored.
";

/// Entry point. `index_file` is `GIT_INDEX_FILE` as it was before `main`
/// scrubbed the environment: git hands a hook a temporary index on
/// `commit -a`, `commit <path>` and `--only`.
pub fn run(args: &[OsString], index_file: Option<OsString>) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        emit(format_args!("{HELP}"));
        return ExitCode::SUCCESS;
    }
    if args.len() != 1 || args[0] != "--staged" {
        eprintln!("amont-agent: suppressions takes exactly --staged\n");
        eprint!("{HELP}");
        return ExitCode::from(2);
    }
    let allowed = command_scoped_allow(
        std::env::var("GIT_CONFIG_PARAMETERS").ok().as_deref(),
        &|name| std::env::var(name).ok(),
    );
    if !allowed {
        note_ignored_override();
    }
    match scan(index_file.as_deref()) {
        Err(why) if allowed => {
            eprintln!("{why}");
            eprintln!("suppressions: allowed by command-scoped {OVERRIDE_KEY}=allow");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::from(2)
        }
        Ok(scan) => judge(&scan, allowed),
    }
}

/// The gate's stdout, written by the module that owns it rather than by a
/// print macro, so the hook path's one-writer check stays true.
fn emit(text: std::fmt::Arguments<'_>) {
    let _ = std::io::stdout().write_fmt(text); // a closed stdout leaves the exit status as the answer
}

struct Scan {
    files: usize,
    skipped: usize,
    blocking: Vec<(PathBuf, Hit)>,
    kept: usize,
}

fn judge(scan: &Scan, allowed: bool) -> ExitCode {
    if scan.blocking.is_empty() {
        let mut line = format!(
            "suppressions: {} files, 0 added ({} kept with a reason)",
            scan.files, scan.kept
        );
        if scan.skipped > 0 {
            line.push_str(&format!(", {} skipped ({SKIPPED_NOTE})", scan.skipped));
        }
        emit(format_args!("{line}\n"));
        return ExitCode::SUCCESS;
    }
    for (path, hit) in &scan.blocking {
        emit(format_args!(
            "{}:{}: {}: {}\n",
            path.display(),
            hit.line,
            shown(hit),
            fix(hit)
        ));
    }
    if allowed {
        eprintln!(
            "suppressions: {} blocking hit(s) let through by command-scoped {OVERRIDE_KEY}=allow",
            scan.blocking.len()
        );
        return ExitCode::SUCCESS;
    }
    emit(format_args!("fix the cause, or stop and tell the person\n"));
    emit(format_args!(
        "for the person only (an agent's attempt is refused):\n"
    ));
    emit(format_args!("  git -c {OVERRIDE_KEY}=allow commit\n"));
    if stance::resolve(&RULE) == Stance::Observe {
        eprintln!("suppressions: stance observe, not blocking");
        return ExitCode::SUCCESS;
    }
    ExitCode::from(1)
}

fn shown(hit: &Hit) -> String {
    match &hit.origin {
        Origin::Comment { shown } => shown.clone(),
        Origin::Setting { section } => {
            let section = if section.is_empty() {
                String::new()
            } else {
                format!("[{section}] ")
            };
            format!("loosens {section}{} = {}", hit.kind, hit.codes)
        }
    }
}

/// What turns a blocking marker into a passing one, in its tool's own form.
fn fix(hit: &Hit) -> &'static str {
    if matches!(hit.origin, Origin::Setting { .. }) {
        return "loosened lint configuration, never passes";
    }
    let kind = hit.kind.as_str();
    if hit.codes.is_empty()
        || matches!(
            kind,
            "@ts-ignore" | "@ts-nocheck" | "pyright" | "ruff: noqa"
        )
    {
        return "blanket, never passes";
    }
    match kind {
        "allow" | "expect" => "add reason = \"…\"",
        "@ts-expect-error" => "add the reason as text after the tag",
        "nolint" => "add // <why>",
        "shellcheck disable" => "add # <why>",
        _ => "add -- <why>",
    }
}

// ---- override ----------------------------------------------------------------

/// Whether the command line itself said `amont.agent.suppressions=allow`:
/// `git -c` arrives in `GIT_CONFIG_PARAMETERS`, the environment form in
/// `GIT_CONFIG_COUNT` with `GIT_CONFIG_KEY_<n>` / `GIT_CONFIG_VALUE_<n>`.
/// The last value set wins.
fn command_scoped_allow(parameters: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> bool {
    let mut last: Option<String> = None;
    for word in parameters.map(shell_words).unwrap_or_default() {
        if let Some((key, value)) = word.split_once('=') {
            if key.eq_ignore_ascii_case(OVERRIDE_KEY) {
                last = Some(value.to_string());
            }
        }
    }
    let count = env("GIT_CONFIG_COUNT")
        .and_then(|c| c.trim().parse::<usize>().ok())
        .unwrap_or(0);
    for n in 0..count {
        let key = env(&format!("GIT_CONFIG_KEY_{n}"));
        if key.is_some_and(|k| k.eq_ignore_ascii_case(OVERRIDE_KEY)) {
            last = env(&format!("GIT_CONFIG_VALUE_{n}"));
        }
    }
    last.is_some_and(|v| v.trim().eq_ignore_ascii_case("allow"))
}

/// Words of git's single-quote-escaped `GIT_CONFIG_PARAMETERS`
/// (`'key=value' 'key'='value'`).
fn shell_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '\'') => quoted = false,
            (true, c) => word.push(c),
            (false, '\'') => {
                quoted = true;
                started = true;
            }
            (false, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                    started = true;
                }
            }
            (false, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (false, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// A value in repository, global or system config is ignored; say so.
fn note_ignored_override() {
    let Ok(out) = run_git(
        &["config", "--show-scope", "--get-all", OVERRIDE_KEY],
        None,
        None,
        1 << 16,
    ) else {
        return;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((scope, value)) = line.split_once('\t') else {
            continue;
        };
        if scope != "command" {
            eprintln!(
                "suppressions: {OVERRIDE_KEY}={value} in {scope} config is ignored; only `git -c {OVERRIDE_KEY}=allow` counts"
            );
        }
    }
}

// ---- git ---------------------------------------------------------------------

struct GitOutput {
    code: i32,
    stdout: Vec<u8>,
    stderr: String,
}

/// Run git in the current directory with `GIT_INDEX_FILE` as the hook was
/// given it. stdout is read under `ceiling` bytes; more kills git and is an
/// error, never a short answer.
fn run_git(
    args: &[&str],
    input: Option<Vec<u8>>,
    index_file: Option<&OsStr>,
    ceiling: usize,
) -> Result<GitOutput, String> {
    let label = format!(
        "git {}",
        args.iter().take(2).copied().collect::<Vec<_>>().join(" ")
    );
    let mut cmd = Command::new("git");
    cmd.args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(index) = index_file {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("suppressions: {label} failed: {e}"))?;
    let writer = match (input, child.stdin.take()) {
        (Some(bytes), Some(mut stdin)) => Some(std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes); // git stopping early is read from its exit status
        })),
        _ => None,
    };
    let stderr_reader = child.stderr.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = s.by_ref().take(1 << 16).read_to_string(&mut text); // diagnostics only; a lossy read is enough
            text
        })
    });
    let mut stdout = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        let read = pipe.take(ceiling as u64 + 1).read_to_end(&mut stdout);
        if read.is_err() || stdout.len() > ceiling {
            let _ = child.kill(); // the answer is already refused
            let _ = child.wait(); // reaping only
            return Err(format!(
                "suppressions: {label} failed: output over its {ceiling}-byte ceiling"
            ));
        }
    }
    let status = child
        .wait()
        .map_err(|e| format!("suppressions: {label} failed: {e}"))?;
    if let Some(writer) = writer {
        let _ = writer.join(); // a panicked writer shows as a short git answer
    }
    let stderr = stderr_reader
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let Some(code) = status.code() else {
        return Err(format!("suppressions: {label} failed: killed by a signal"));
    };
    Ok(GitOutput {
        code,
        stdout,
        stderr,
    })
}

fn checked(
    args: &[&str],
    input: Option<Vec<u8>>,
    index_file: Option<&OsStr>,
    ceiling: usize,
) -> Result<Vec<u8>, String> {
    let out = run_git(args, input, index_file, ceiling)?;
    if out.code != 0 {
        let first = out.stderr.lines().next().unwrap_or("").trim();
        return Err(format!(
            "suppressions: git {} failed: {first}",
            args.iter().take(2).copied().collect::<Vec<_>>().join(" ")
        ));
    }
    Ok(out.stdout)
}

// ---- the staged change -------------------------------------------------------

struct Entry {
    status: u8,
    old: Vec<u8>,
    new: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Before,
    After,
}

struct Request {
    entry: usize,
    side: Side,
    name: Vec<u8>,
}

fn parse_name_status(raw: &[u8]) -> Result<Vec<Entry>, String> {
    let mut tokens = raw.split(|b| *b == 0).filter(|t| !t.is_empty());
    let mut entries = Vec::new();
    while let Some(status) = tokens.next() {
        let letter = status[0];
        let mut next = || {
            tokens
                .next()
                .map(<[u8]>::to_vec)
                .ok_or_else(|| "suppressions: git diff failed: truncated name-status".to_string())
        };
        let (old, new) = match letter {
            b'R' | b'C' => {
                let old = next()?;
                let new = next()?;
                (old, new)
            }
            b'A' | b'M' | b'D' => {
                let path = next()?;
                (path.clone(), path)
            }
            other => {
                return Err(format!(
                    "suppressions: git diff failed: unexpected status {}",
                    other as char
                ));
            }
        };
        entries.push(Entry {
            status: letter,
            old,
            new,
        });
    }
    Ok(entries)
}

fn path_of(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

/// An entry is worth reading when either side is an examined kind or has no
/// extension (a shell script can only be told by its content).
fn wanted(entry: &Entry) -> bool {
    [&entry.old, &entry.new].iter().any(|bytes| {
        let path = path_of(bytes);
        lint::examined(&path) || path.extension().is_none()
    })
}

fn scan(index_file: Option<&OsStr>) -> Result<Scan, String> {
    let head = run_git(
        &["rev-parse", "--verify", "--quiet", "HEAD"],
        None,
        index_file,
        1 << 12,
    )?;
    let head_exists = match head.code {
        0 => true,
        1 => false,
        _ => {
            let first = head.stderr.lines().next().unwrap_or("").trim().to_string();
            return Err(format!("suppressions: git rev-parse failed: {first}"));
        }
    };
    let listing = checked(
        &[
            "diff",
            "--cached",
            "--name-status",
            "-z",
            "-M",
            "--diff-filter=ACMRD",
        ],
        None,
        index_file,
        64 * 1024 * 1024,
    )?;
    let entries: Vec<Entry> = parse_name_status(&listing)?
        .into_iter()
        .filter(wanted)
        .collect();

    let mut requests = Vec::new();
    for (entry_index, entry) in entries.iter().enumerate() {
        if head_exists && matches!(entry.status, b'M' | b'R' | b'D') {
            let mut name = b"HEAD:".to_vec();
            name.extend_from_slice(&entry.old);
            requests.push(Request {
                entry: entry_index,
                side: Side::Before,
                name,
            });
        }
        if entry.status != b'D' {
            let mut name = b":".to_vec();
            name.extend_from_slice(&entry.new);
            requests.push(Request {
                entry: entry_index,
                side: Side::After,
                name,
            });
        }
    }
    if requests.iter().any(|r| r.name.contains(&b'\n')) {
        return Err("suppressions: a staged path holds a newline; scan incomplete".to_string());
    }

    let sizes = blob_sizes(&requests, index_file)?;
    let mut skipped_entries = vec![false; entries.len()];
    for (request, size) in requests.iter().zip(&sizes) {
        if *size > MAX_BLOB {
            skipped_entries[request.entry] = true;
        }
    }
    for (entry, skipped) in entries.iter().zip(&skipped_entries) {
        if *skipped {
            eprintln!(
                "suppressions: skipped {} ({SKIPPED_NOTE})",
                path_of(&entry.new).display()
            );
        }
    }
    let wanted_reads: Vec<(&Request, u64)> = requests
        .iter()
        .zip(sizes.iter().copied())
        .filter(|(r, _)| !skipped_entries[r.entry])
        .collect();
    let total: u64 = wanted_reads.iter().map(|(_, size)| *size).sum();
    if total > BUDGET {
        return Err("suppressions: scan budget exceeded (64 MiB); scan incomplete".to_string());
    }
    let texts = read_blobs(&wanted_reads, total, index_file)?;

    let mut before = vec![String::new(); entries.len()];
    let mut after = vec![String::new(); entries.len()];
    for ((request, _), text) in wanted_reads.iter().zip(texts) {
        match request.side {
            Side::Before => before[request.entry] = text,
            Side::After => after[request.entry] = text,
        }
    }
    let kept_entries: Vec<usize> = (0..entries.len())
        .filter(|i| !skipped_entries[*i])
        .collect();
    let paths: Vec<(PathBuf, PathBuf)> = entries
        .iter()
        .map(|e| (path_of(&e.old), path_of(&e.new)))
        .collect();
    let changes: Vec<FileChange<'_>> = kept_entries
        .iter()
        .map(|&i| {
            (
                paths[i].0.as_path(),
                before[i].as_str(),
                paths[i].1.as_path(),
                after[i].as_str(),
            )
        })
        .collect();
    let hits = lint::net_added(&changes);

    let after_of: HashMap<&Path, &str> = kept_entries
        .iter()
        .map(|&i| (paths[i].1.as_path(), after[i].as_str()))
        .collect();
    let mut blocking = Vec::new();
    let mut kept = 0;
    for (path, hit) in hits {
        let text = after_of.get(path.as_path()).copied().unwrap_or("");
        if lint::reasoned(&hit, lint::from_line(text, hit.line)) {
            kept += 1;
        } else {
            blocking.push((path, hit));
        }
    }
    Ok(Scan {
        files: kept_entries.len(),
        skipped: skipped_entries.iter().filter(|s| **s).count(),
        blocking,
        kept,
    })
}

/// One `cat-file --batch-check` for every requested name. A name git reports
/// missing is an error: the diff said it exists.
fn blob_sizes(requests: &[Request], index_file: Option<&OsStr>) -> Result<Vec<u64>, String> {
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let input = names_input(requests.iter().map(|r| r.name.as_slice()));
    let ceiling = input.len() + requests.len() * HEADER_ALLOWANCE + 1024;
    let out = checked(
        &["cat-file", "--batch-check"],
        Some(input),
        index_file,
        ceiling,
    )?;
    let text = String::from_utf8_lossy(&out);
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() != requests.len() {
        return Err("suppressions: git cat-file failed: incomplete batch-check answer".to_string());
    }
    let mut sizes = Vec::with_capacity(requests.len());
    for (request, line) in requests.iter().zip(lines) {
        if line.ends_with(" missing") {
            return Err(format!(
                "suppressions: git cat-file failed: {} missing",
                String::from_utf8_lossy(&request.name)
            ));
        }
        let size = line
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| {
                format!("suppressions: git cat-file failed: unreadable answer `{line}`")
            })?;
        sizes.push(size);
    }
    Ok(sizes)
}

fn names_input<'a>(names: impl Iterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut input = Vec::new();
    for name in names {
        input.extend_from_slice(name);
        input.push(b'\n');
    }
    input
}

/// The bounded content read. The ceiling covers payloads and protocol
/// headers; anything beyond, or short of, what the sizes promised is exit 2.
fn read_blobs(
    reads: &[(&Request, u64)],
    total: u64,
    index_file: Option<&OsStr>,
) -> Result<Vec<String>, String> {
    if reads.is_empty() {
        return Ok(Vec::new());
    }
    let input = names_input(reads.iter().map(|(r, _)| r.name.as_slice()));
    let ceiling = usize::try_from(total).unwrap_or(usize::MAX / 2)
        + input.len()
        + reads.len() * HEADER_ALLOWANCE
        + 1024;
    let out = checked(&["cat-file", "--batch"], Some(input), index_file, ceiling)?;
    let incomplete = || "suppressions: git cat-file failed: incomplete batch answer".to_string();
    let mut at = 0;
    let mut texts = Vec::with_capacity(reads.len());
    for (_, size) in reads {
        let newline = out[at..]
            .iter()
            .position(|b| *b == b'\n')
            .ok_or_else(incomplete)?;
        let header = String::from_utf8_lossy(&out[at..at + newline]).into_owned();
        at += newline + 1;
        let announced = header
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(incomplete)?;
        if announced != *size {
            return Err(incomplete());
        }
        let length = usize::try_from(announced).map_err(|_| incomplete())?;
        if at + length + 1 > out.len() {
            return Err(incomplete());
        }
        texts.push(String::from_utf8_lossy(&out[at..at + length]).into_owned());
        at += length + 1;
    }
    Ok(texts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn both_parameter_formats_are_read() {
        assert!(command_scoped_allow(
            Some("'amont.agent.suppressions=allow'"),
            &no_env
        ));
        assert!(command_scoped_allow(
            Some("'amont.agent.suppressions'='allow'"),
            &no_env
        ));
        assert!(!command_scoped_allow(
            Some("'amont.agent.suppressions=deny'"),
            &no_env
        ));
        assert!(!command_scoped_allow(None, &no_env));
    }

    #[test]
    fn the_numbered_environment_form_is_read() {
        let env = |name: &str| match name {
            "GIT_CONFIG_COUNT" => Some("2".to_string()),
            "GIT_CONFIG_KEY_1" => Some("amont.agent.suppressions".to_string()),
            "GIT_CONFIG_VALUE_1" => Some("allow".to_string()),
            "GIT_CONFIG_KEY_0" => Some("user.name".to_string()),
            "GIT_CONFIG_VALUE_0" => Some("x".to_string()),
            _ => None,
        };
        assert!(command_scoped_allow(None, &env));
    }
}
