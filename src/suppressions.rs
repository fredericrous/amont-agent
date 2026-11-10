//! `amont-agent suppressions --staged`: the commit gate for lint suppressions.
//!
//! It judges what the staged change adds, never the working tree, and never
//! what a file already held. Every failure to look is exit 2 (fail closed);
//! the only way past a blocking hit is a command-scoped
//! `git -c amont.agent.suppressions=allow commit`, which the person types and
//! the `suppressions-override` rule keeps from an agent.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use amont_agent_guard::rules::lint_suppression_added::{
    self as lint, FileChange, Hit, Origin, ReasonForm, RULE,
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
/// How long any one git call gets.
const GIT_DEADLINE: Duration = Duration::from_secs(30);

/// Whether the person's command line carried `amont.agent.suppressions=allow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Override {
    CommandScoped,
    Absent,
}

/// Why the staged change could not be fully looked at. `Display` is the line
/// printed on stderr.
#[derive(Debug)]
enum ScanError {
    /// A git call that failed, with its leading arguments and what it said.
    Git {
        command: String,
        detail: String,
    },
    BudgetExceeded,
    /// A `cat-file` answer shorter or odder than the names sent.
    IncompleteAnswer {
        answer: &'static str,
    },
    MissingName {
        name: String,
    },
    NewlineInPath,
    IndexHint {
        /// The variable that named it: `GREENGATE_INDEX_FILE` or 3.x's
        /// `AMONT_INDEX_FILE`.
        variable: &'static str,
        path: String,
    },
}

impl fmt::Display for ScanError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::Git { command, detail } => {
                write!(out, "suppressions: git {command} failed: {detail}")
            }
            ScanError::BudgetExceeded => {
                write!(
                    out,
                    "suppressions: scan budget exceeded (64 MiB); scan incomplete"
                )
            }
            ScanError::IncompleteAnswer { answer } => {
                write!(
                    out,
                    "suppressions: git cat-file failed: incomplete {answer} answer"
                )
            }
            ScanError::MissingName { name } => {
                write!(out, "suppressions: git cat-file failed: {name} missing")
            }
            ScanError::IndexHint { variable, path } => {
                write!(
                    out,
                    "suppressions: {variable} '{path}' is not an absolute path to an existing index file"
                )
            }
            ScanError::NewlineInPath => {
                write!(
                    out,
                    "suppressions: a staged path holds a newline; scan incomplete"
                )
            }
        }
    }
}

fn git_failed(command: &str, detail: impl Into<String>) -> ScanError {
    ScanError::Git {
        command: command.to_string(),
        detail: detail.into(),
    }
}

pub const HELP: &str = "\
usage: amont-agent suppressions --staged

Judge the staged change: a lint suppression it adds (net of those it removes
anywhere) blocks unless it carries a reason in its tool's own reason form.
Exit 0: nothing blocking. 1: a blocking hit. 2: usage error or git failure.
A command-scoped `git -c amont.agent.suppressions=allow commit` passes; the
same key in repository or global config is ignored.
`GREENGATE_INDEX_FILE` is a bound pair with `GREENGATE_INDEX_GIT_DIR`, the
absolute per-worktree `git rev-parse --absolute-git-dir` of the owner
(greengate 4.0 supplies both; greengate 3.x and amont >= 1.70.0 supply the
same pair as `AMONT_INDEX_FILE` and `AMONT_INDEX_GIT_DIR`, read only when
`GREENGATE_INDEX_FILE` is unset or empty). A non-empty index hint is honoured over `GIT_INDEX_FILE` only
when that owner, canonicalized, equals this command's own canonical
`git rev-parse --absolute-git-dir` (the per-worktree git-dir, never the common
dir or the index location). A hint whose owner is absent, empty, relative,
nonexistent or foreign is ignored with one stderr note and `GIT_INDEX_FILE`
(or the default index) is kept. A matching but missing, relative or non-file
index exits 2 unless the override applies; so does a failed identity query.
The selected index, an external alternate one included, is only read.
A wrapper that switches repositories must clear and recompute both hints.
`--staged` may be repeated.
";

/// Entry point. `git_index` is `GIT_INDEX_FILE` as it was before `main`
/// scrubbed the environment: git hands a hook a temporary index on
/// `commit -a`, `commit <path>` and `--only`. `handoff` is
/// `GREENGATE_INDEX_FILE` with its owner `GREENGATE_INDEX_GIT_DIR` (or 3.x's
/// `AMONT_INDEX_*` pair): when non-empty and
/// owned by this repository's worktree it must name an existing file by an
/// absolute path, and it takes precedence over `git_index`; otherwise the
/// direct Git-hook fallback stays.
pub fn run(args: &[OsString], handoff: IndexHandoff, git_index: Option<OsString>) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        emit(format_args!("{HELP}"));
        return ExitCode::SUCCESS;
    }
    if let Some(unknown) = args.iter().find(|a| *a != "--staged") {
        eprintln!(
            "amont-agent: suppressions: unknown argument '{}'\n",
            unknown.to_string_lossy()
        );
        eprint!("{HELP}");
        return ExitCode::from(2);
    }
    if args.is_empty() {
        eprintln!("amont-agent: suppressions needs --staged\n");
        eprint!("{HELP}");
        return ExitCode::from(2);
    }
    let allowed = command_scoped_allow(
        std::env::var("GIT_CONFIG_PARAMETERS").ok().as_deref(),
        &|name| std::env::var(name).ok(),
    );
    if allowed == Override::Absent {
        note_ignored_override();
    }
    let scanned =
        select_index(handoff, git_index).and_then(|index_file| scan(index_file.as_deref()));
    match scanned {
        Err(why) if allowed == Override::CommandScoped => {
            eprintln!("{why}");
            eprintln!("suppressions: allowed by command-scoped {OVERRIDE_KEY}=allow");
            ExitCode::SUCCESS
        }
        Err(why) if stance::resolve(&RULE) == Stance::Observe => {
            eprintln!("{why}");
            eprintln!("suppressions: stance observe, not blocking");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::from(2)
        }
        Ok(scan) => judge(&scan, allowed),
    }
}

/// `GREENGATE_INDEX_FILE` and `GREENGATE_INDEX_GIT_DIR` as `main` captured
/// them: the index the gate's commit check selected, and the absolute
/// per-worktree git-dir of the repository that owns it. The pair is one hint;
/// the index is honoured only where the owner is this command's own
/// repository. greengate 3.x and amont hand the same pair as `AMONT_INDEX_*`,
/// read only when the new file name is unset or empty, and always as a pair,
/// so one name of each never mixes.
#[derive(Debug, Default)]
pub struct IndexHandoff {
    pub index_file: Option<OsString>,
    pub owner_git_dir: Option<OsString>,
    /// The pair's variable names, for what stderr says of a bad hint.
    pub names: (&'static str, &'static str),
}

/// The pair names, greengate 4.0's first. The old pair goes at the census of
/// greengate's hard rename.
const INDEX_PAIRS: [(&str, &str); 2] = [
    ("GREENGATE_INDEX_FILE", "GREENGATE_INDEX_GIT_DIR"),
    ("AMONT_INDEX_FILE", "AMONT_INDEX_GIT_DIR"),
];

impl IndexHandoff {
    /// The first pair whose file variable is set and non-empty in this
    /// process's environment, else none.
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        INDEX_PAIRS
            .iter()
            .find_map(|&(file, dir)| {
                lookup(file)
                    .filter(|value| !value.is_empty())
                    .map(|index_file| Self {
                        index_file: Some(index_file),
                        owner_git_dir: lookup(dir),
                        names: (file, dir),
                    })
            })
            .unwrap_or_default()
    }
}

/// The index the scan reads. An absent or empty index hint keeps
/// `GIT_INDEX_FILE`. A non-empty one is honoured when its owning git-dir is
/// this command's own per-worktree git-dir, and then it must be an absolute
/// path to an existing file; an unbound or foreign hint is ignored with one
/// stderr note and `GIT_INDEX_FILE` is kept.
fn select_index(
    handoff: IndexHandoff,
    git_index: Option<OsString>,
) -> Result<Option<OsString>, ScanError> {
    let Some(index) = handoff.index_file.filter(|path| !path.is_empty()) else {
        return Ok(git_index);
    };
    let (variable, dir_variable) = handoff.names;
    let Some(owner) = usable_owner(handoff.owner_git_dir) else {
        note_ignored_hint(
            variable,
            &format!("{dir_variable} is absent, empty, relative or missing"),
        );
        return Ok(git_index);
    };
    if own_git_dir()? != owner {
        note_ignored_hint(variable, "it belongs to another repository");
        return Ok(git_index);
    }
    if Path::new(&index).is_absolute() && Path::new(&index).is_file() {
        Ok(Some(index))
    } else {
        Err(ScanError::IndexHint {
            variable,
            path: index.to_string_lossy().into_owned(),
        })
    }
}

/// The canonical owner git-dir, when the hint names an absolute existing one.
fn usable_owner(owner: Option<OsString>) -> Option<PathBuf> {
    let owner = PathBuf::from(owner.filter(|path| !path.is_empty())?);
    if !owner.is_absolute() {
        return None;
    }
    owner.canonicalize().ok()
}

fn note_ignored_hint(variable: &str, reason: &str) {
    eprintln!("suppressions: {variable} ignored: {reason}; keeping the direct index");
}

/// This command's own per-worktree git-dir, canonical, from git's bytes with
/// the one protocol newline removed.
fn own_git_dir() -> Result<PathBuf, ScanError> {
    let mut answer = checked(&["rev-parse", "--absolute-git-dir"], None, None, 1 << 16)?;
    if answer.last() == Some(&b'\n') {
        answer.pop();
    }
    let reported = path_from_git_bytes(answer)?;
    reported
        .canonicalize()
        .map_err(|error| git_failed("rev-parse", format!("{}: {error}", reported.display())))
}

#[cfg(unix)]
fn path_from_git_bytes(bytes: Vec<u8>) -> Result<PathBuf, ScanError> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
fn path_from_git_bytes(bytes: Vec<u8>) -> Result<PathBuf, ScanError> {
    String::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|_| git_failed("rev-parse", "git-dir is not valid UTF-8"))
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

fn judge(scan: &Scan, allowed: Override) -> ExitCode {
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
    if allowed == Override::CommandScoped {
        eprintln!(
            "suppressions: {} blocking hit(s) let through by command-scoped {OVERRIDE_KEY}=allow",
            scan.blocking.len()
        );
        return ExitCode::SUCCESS;
    }
    if stance::resolve(&RULE) == Stance::Observe {
        eprintln!("suppressions: stance observe, not blocking");
        return ExitCode::SUCCESS;
    }
    eprintln!("fix the cause, or stop and tell the person");
    eprintln!("for the person only (an agent's attempt is refused):");
    eprintln!("  git -c {OVERRIDE_KEY}=allow commit");
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
    match lint::reason_form(hit) {
        ReasonForm::Never => "blanket, never passes",
        ReasonForm::Attribute => "add reason = \"…\"",
        ReasonForm::AfterTag => "add the reason as text after the tag",
        ReasonForm::SlashSlash => "add // <why>",
        ReasonForm::Hash => "add # <why>",
        ReasonForm::TrailingDashes => "add -- <why>",
    }
}

/// Whether the command line itself said `amont.agent.suppressions=allow`:
/// `git -c` arrives in `GIT_CONFIG_PARAMETERS`, the environment form in
/// `GIT_CONFIG_COUNT` with `GIT_CONFIG_KEY_<n>` / `GIT_CONFIG_VALUE_<n>`.
/// The last value set wins.
fn command_scoped_allow(
    parameters: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Override {
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
    if last.is_some_and(|value| value.trim().eq_ignore_ascii_case("allow")) {
        Override::CommandScoped
    } else {
        Override::Absent
    }
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
        return; // a note about ignored config is a courtesy; the scan reports git failures itself
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

struct GitOutput {
    code: i32,
    stdout: Vec<u8>,
    stderr: String,
}

/// Run git in the current directory with `GIT_INDEX_FILE` as the hook was
/// given it. stdout is read under `ceiling` bytes; more kills git and is an
/// error, never a short answer. The call has [`GIT_DEADLINE`] to finish.
fn run_git(
    args: &[&str],
    input: Option<Vec<u8>>,
    index_file: Option<&OsStr>,
    ceiling: usize,
) -> Result<GitOutput, ScanError> {
    let command = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
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
    let deadline = Instant::now() + GIT_DEADLINE;
    let mut child = cmd
        .spawn()
        .map_err(|error| git_failed(&command, error.to_string()))?;
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        // Detached: a grandchild holding the pipe open must not hold this call.
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes); // git stopping early is read from its exit status
        });
    }
    let stderr_answer = child.stderr.take().map(|mut pipe| {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.by_ref().take(1 << 16).read_to_string(&mut text); // diagnostics only; a lossy read is enough
            let _ = sender.send(text); // a receiver past its deadline has gone
        });
        receiver
    });
    let stdout_answer = child.stdout.take().map(|pipe| {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let read = pipe.take(ceiling as u64 + 1).read_to_end(&mut bytes);
            let _ = sender.send((bytes, read.is_ok())); // a receiver past its deadline has gone
        });
        receiver
    });
    let timed_out = || git_failed(&command, "timed out");
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => return Err(git_failed(&command, error.to_string())),
        }
        if Instant::now() >= deadline {
            let _ = child.kill(); // the answer is already refused
            let _ = child.wait(); // reaping only
            return Err(timed_out());
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let (stdout, read_whole) = match stdout_answer {
        Some(receiver) => receiver
            .recv_timeout(remaining())
            .map_err(|_| timed_out())?,
        None => (Vec::new(), true),
    };
    if !read_whole || stdout.len() > ceiling {
        return Err(git_failed(
            &command,
            format!("output over its {ceiling}-byte ceiling"),
        ));
    }
    let stderr = match stderr_answer {
        Some(receiver) => receiver
            .recv_timeout(remaining())
            .map_err(|_| timed_out())?,
        None => String::new(),
    };
    let Some(code) = status.code() else {
        return Err(git_failed(&command, "killed by a signal"));
    };
    Ok(GitOutput {
        code,
        stdout,
        stderr,
    })
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

fn checked(
    args: &[&str],
    input: Option<Vec<u8>>,
    index_file: Option<&OsStr>,
    ceiling: usize,
) -> Result<Vec<u8>, ScanError> {
    let out = run_git(args, input, index_file, ceiling)?;
    if out.code != 0 {
        let command = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
        return Err(git_failed(&command, first_line(&out.stderr)));
    }
    Ok(out.stdout)
}

/// What `git diff --name-status` says happened to a path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Staged {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl Staged {
    /// `C` (a copy) is not asked for and is not accepted.
    fn from_letter(letter: u8) -> Option<Staged> {
        match letter {
            b'A' => Some(Staged::Added),
            b'M' => Some(Staged::Modified),
            b'D' => Some(Staged::Deleted),
            b'R' => Some(Staged::Renamed),
            _ => None,
        }
    }

    /// Whether HEAD holds the old path.
    fn has_before(self) -> bool {
        match self {
            Staged::Added => false,
            Staged::Modified | Staged::Deleted | Staged::Renamed => true,
        }
    }

    fn has_after(self) -> bool {
        match self {
            Staged::Deleted => false,
            Staged::Added | Staged::Modified | Staged::Renamed => true,
        }
    }
}

struct Entry {
    status: Staged,
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

fn parse_name_status(raw: &[u8]) -> Result<Vec<Entry>, ScanError> {
    let mut tokens = raw.split(|b| *b == 0).filter(|t| !t.is_empty());
    let mut entries = Vec::new();
    while let Some(token) = tokens.next() {
        let letter = token[0];
        let Some(status) = Staged::from_letter(letter) else {
            return Err(git_failed(
                "diff",
                format!("unexpected status {}", letter as char),
            ));
        };
        let mut next = || {
            tokens
                .next()
                .map(<[u8]>::to_vec)
                .ok_or_else(|| git_failed("diff", "truncated name-status"))
        };
        let (old, new) = match status {
            Staged::Renamed => {
                let old = next()?;
                let new = next()?;
                (old, new)
            }
            Staged::Added | Staged::Modified | Staged::Deleted => {
                let path = next()?;
                (path.clone(), path)
            }
        };
        entries.push(Entry { status, old, new });
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

fn scan(index_file: Option<&OsStr>) -> Result<Scan, ScanError> {
    let head = run_git(
        &["rev-parse", "--verify", "--quiet", "HEAD"],
        None,
        index_file,
        1 << 12,
    )?;
    let head_exists = match head.code {
        0 => true,
        1 => false,
        _ => return Err(git_failed("rev-parse", first_line(&head.stderr))),
    };
    let listing = checked(
        &[
            "diff",
            "--cached",
            "--name-status",
            "-z",
            "-M",
            "--diff-filter=AMRD",
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
        if head_exists && entry.status.has_before() {
            let mut name = b"HEAD:".to_vec();
            name.extend_from_slice(&entry.old);
            requests.push(Request {
                entry: entry_index,
                side: Side::Before,
                name,
            });
        }
        if entry.status.has_after() {
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
        return Err(ScanError::NewlineInPath);
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
        return Err(ScanError::BudgetExceeded);
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
        .map(|&i| FileChange {
            old_path: paths[i].0.as_path(),
            before: before[i].as_str(),
            new_path: paths[i].1.as_path(),
            after: after[i].as_str(),
        })
        .collect();

    let mut blocking = Vec::new();
    let mut kept = 0;
    for added in lint::net_added(&changes) {
        if added.reasoned {
            kept += 1;
        } else {
            blocking.push((added.path, added.hit));
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
fn blob_sizes(requests: &[Request], index_file: Option<&OsStr>) -> Result<Vec<u64>, ScanError> {
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
        return Err(ScanError::IncompleteAnswer {
            answer: "batch-check",
        });
    }
    let mut sizes = Vec::with_capacity(requests.len());
    for (request, line) in requests.iter().zip(lines) {
        if line.ends_with(" missing") {
            return Err(ScanError::MissingName {
                name: String::from_utf8_lossy(&request.name).into_owned(),
            });
        }
        let size = line
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| git_failed("cat-file", format!("unreadable answer `{line}`")))?;
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
) -> Result<Vec<String>, ScanError> {
    if reads.is_empty() {
        return Ok(Vec::new());
    }
    let input = names_input(reads.iter().map(|(r, _)| r.name.as_slice()));
    let ceiling = usize::try_from(total).unwrap_or(usize::MAX / 2)
        + input.len()
        + reads.len() * HEADER_ALLOWANCE
        + 1024;
    let out = checked(&["cat-file", "--batch"], Some(input), index_file, ceiling)?;
    let incomplete = || ScanError::IncompleteAnswer { answer: "batch" };
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

    fn handoff_of(vars: &[(&str, &str)]) -> IndexHandoff {
        IndexHandoff::from_lookup(|name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        })
    }

    #[test]
    fn the_new_index_pair_wins_and_the_old_pair_is_read_alone() {
        let new = handoff_of(&[
            ("GREENGATE_INDEX_FILE", "/n/index"),
            ("GREENGATE_INDEX_GIT_DIR", "/n/.git"),
            ("AMONT_INDEX_FILE", "/o/index"),
            ("AMONT_INDEX_GIT_DIR", "/o/.git"),
        ]);
        assert_eq!(new.index_file.as_deref(), Some("/n/index".as_ref()));
        assert_eq!(new.owner_git_dir.as_deref(), Some("/n/.git".as_ref()));

        let old = handoff_of(&[
            ("AMONT_INDEX_FILE", "/o/index"),
            ("AMONT_INDEX_GIT_DIR", "/o/.git"),
        ]);
        assert_eq!(old.index_file.as_deref(), Some("/o/index".as_ref()));
        assert_eq!(old.owner_git_dir.as_deref(), Some("/o/.git".as_ref()));

        assert_eq!(
            new.names,
            ("GREENGATE_INDEX_FILE", "GREENGATE_INDEX_GIT_DIR")
        );
        assert_eq!(old.names, ("AMONT_INDEX_FILE", "AMONT_INDEX_GIT_DIR"));

        // Never one name of each: the new file without its git-dir keeps none.
        let mixed = handoff_of(&[
            ("GREENGATE_INDEX_FILE", "/n/index"),
            ("AMONT_INDEX_GIT_DIR", "/o/.git"),
        ]);
        assert_eq!(mixed.owner_git_dir, None);

        // An empty new name is unset: the old pair is read.
        let cleared = handoff_of(&[
            ("GREENGATE_INDEX_FILE", ""),
            ("AMONT_INDEX_FILE", "/o/index"),
            ("AMONT_INDEX_GIT_DIR", "/o/.git"),
        ]);
        assert_eq!(cleared.index_file.as_deref(), Some("/o/index".as_ref()));
        assert_eq!(cleared.names.0, "AMONT_INDEX_FILE");

        assert!(handoff_of(&[]).index_file.is_none());
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn both_parameter_formats_are_read() {
        assert_eq!(
            command_scoped_allow(Some("'amont.agent.suppressions=allow'"), &no_env),
            Override::CommandScoped
        );
        assert_eq!(
            command_scoped_allow(Some("'amont.agent.suppressions'='allow'"), &no_env),
            Override::CommandScoped
        );
        assert_eq!(
            command_scoped_allow(Some("'amont.agent.suppressions=deny'"), &no_env),
            Override::Absent
        );
        assert_eq!(command_scoped_allow(None, &no_env), Override::Absent);
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
        assert_eq!(command_scoped_allow(None, &env), Override::CommandScoped);
    }
}
