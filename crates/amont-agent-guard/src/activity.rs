//! Which repository, worktree and branch each session edits — the one fact
//! `work-overlap` needs, and `docs/board.md` documents.
//!
//! One append-on-change file per session, `<journal dir>/sessions/<session>.activity`,
//! one line per change of place: `<ts> <repo_identity> <worktree_root> <branch>`,
//! whitespace-separated, `ts` in epoch seconds. Whitespace and `%` inside a
//! field are percent-encoded (`%20`, `%25`, ...), so a path with a space still
//! splits into four fields. The file's modification time, not `ts`, is the
//! session's last-seen time: a call that finds the same place as the last line
//! appends nothing and only moves the mtime.
//!
//! The place is resolved without spawning a process: walk up to `.git`, follow
//! a `gitdir:` file and its `commondir`, read `HEAD`. Everything here fails
//! toward silence, as `session_state` does.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A changed-place append to a file longer than this many bytes trims it.
const TRIM_ABOVE_BYTES: u64 = 256 * 1024;
/// A trim keeps the newest complete lines that fit in this many bytes,
/// the caller's new line included.
const TRIM_TARGET_BYTES: usize = 128 * 1024;
/// The last line is looked for in this many final bytes first.
const TAIL_WINDOW_BYTES: u64 = 4096;

/// Where a session works: the repository, the worktree and the branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The git common dir, shared by every worktree of a repository.
    pub repo_identity: PathBuf,
    pub worktree_root: PathBuf,
    /// The branch HEAD names, or `detached`.
    pub branch: String,
}

/// Another session seen in a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub session: String,
    pub worktree: PathBuf,
    pub branch: String,
    pub last_seen: SystemTime,
}

impl Peer {
    /// Seen less than `window` before `now`.
    pub fn is_live(&self, now: SystemTime, window: Duration) -> bool {
        now.duration_since(self.last_seen)
            .map_or(true, |age| age < window)
    }
}

fn encode(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    for c in field.chars() {
        match c {
            '%' => out.push_str("%25"),
            ' ' => out.push_str("%20"),
            '\t' => out.push_str("%09"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            other => out.push(other),
        }
    }
    out
}

fn decode(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut rest = field;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        let escape = &rest[at..];
        let (decoded, length) = if escape.starts_with("%25") {
            ('%', 3)
        } else if escape.starts_with("%20") {
            (' ', 3)
        } else if escape.starts_with("%09") {
            ('\t', 3)
        } else if escape.starts_with("%0A") {
            ('\n', 3)
        } else if escape.starts_with("%0D") {
            ('\r', 3)
        } else {
            ('%', 1)
        };
        out.push(decoded);
        rest = &escape[length..];
    }
    out.push_str(rest);
    out
}

fn path_field(path: &Path) -> String {
    encode(&path.to_string_lossy())
}

impl Place {
    /// The three fields of a line, encoded.
    fn fields(&self) -> [String; 3] {
        [
            path_field(&self.repo_identity),
            path_field(&self.worktree_root),
            encode(&self.branch),
        ]
    }

    /// The repository's name for a person: the directory holding `.git`, or
    /// `x` for `x.git`.
    pub fn repo_name(&self) -> String {
        repo_name_of(&self.repo_identity)
    }
}

/// See [`Place::repo_name`].
pub fn repo_name_of(repo_identity: &Path) -> String {
    let name_of = |path: &Path| {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    if repo_identity.file_name().is_some_and(|n| n == ".git") {
        return repo_identity.parent().map(name_of).unwrap_or_default();
    }
    let name = name_of(repo_identity);
    name.strip_suffix(".git")
        .map(str::to_string)
        .unwrap_or(name)
}

/// The place of the directory `dir` (which need not exist), `None` outside
/// any git repository. No process is spawned.
pub fn resolve(dir: &Path) -> Option<Place> {
    let start = crate::lsp::normal(dir);
    for ancestor in start.ancestors() {
        let dot_git = ancestor.join(".git");
        let Ok(meta) = std::fs::symlink_metadata(&dot_git) else {
            continue;
        };
        let git_dir = if meta.is_dir() {
            dot_git
        } else {
            let text = std::fs::read_to_string(&dot_git).ok()?;
            let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
            ancestor.join(target)
        };
        let common = match std::fs::read_to_string(git_dir.join("commondir")) {
            Ok(text) => git_dir.join(text.trim()),
            Err(_) => git_dir.clone(),
        };
        return Some(Place {
            repo_identity: crate::lsp::normal(&common),
            worktree_root: crate::lsp::normal(ancestor),
            branch: branch_of(&git_dir),
        });
    }
    None
}

fn branch_of(git_dir: &Path) -> String {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).unwrap_or_default();
    match head.trim().strip_prefix("ref:") {
        Some(reference) => {
            let reference = reference.trim();
            reference
                .strip_prefix("refs/heads/")
                .unwrap_or(reference)
                .to_string()
        }
        None => "detached".to_string(),
    }
}

fn sessions_dir() -> Option<PathBuf> {
    Some(crate::journal::dir()?.join("sessions"))
}

fn file_for(session: &str) -> Option<PathBuf> {
    if !crate::session_state::usable_id(session) {
        return None;
    }
    Some(sessions_dir()?.join(format!("{session}.activity")))
}

/// A record that was kept; `TrimFailed` says the oversized file could not be
/// trimmed and the line was appended to it instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    Whole,
    TrimFailed,
}

impl Recorded {
    pub const TRIM_FAILED_REASON: &'static str = "the activity file could not be trimmed";
}

/// Why the activity record could not be kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    NoSessionFile,
    DirectoryNotCreated,
    NotWritten,
}

impl Failure {
    pub fn as_str(self) -> &'static str {
        match self {
            Failure::NoSessionFile => "no usable session id or journal directory",
            Failure::DirectoryNotCreated => "the sessions directory could not be created",
            Failure::NotWritten => "the activity file could not be written",
        }
    }
}

/// The three encoded place fields of one activity line.
fn fields_of(line: &str) -> Option<[&str; 3]> {
    let mut parts = line.split_whitespace();
    let _ts = parts.next()?;
    Some([parts.next()?, parts.next()?, parts.next()?])
}

/// The last complete (newline-terminated) non-empty line of the bytes. An
/// unterminated tail, such as another process's append caught midway, is
/// ignored.
fn last_line_of(bytes: &[u8]) -> Option<String> {
    let complete_end = bytes.iter().rposition(|byte| *byte == b'\n')?;
    bytes[..complete_end]
        .split(|byte| *byte == b'\n')
        .rev()
        .find(|line| !line.iter().all(u8::is_ascii_whitespace))
        .map(|line| String::from_utf8_lossy(line).into_owned())
}

/// The last complete non-empty line of the file, read from at most its final
/// [`TAIL_WINDOW_BYTES`]; the whole file when that window holds none.
fn tail_line(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut handle = std::fs::File::open(path).ok()?;
    let length = handle.metadata().ok()?.len();
    let start = length.saturating_sub(TAIL_WINDOW_BYTES);
    handle.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    handle.read_to_end(&mut bytes).ok()?;
    let window_line = if start > 0 {
        // The window began mid-file: its first line may be cut.
        let after_first_newline = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        last_line_of(&bytes[after_first_newline..])
    } else {
        last_line_of(&bytes)
    };
    if let Some(line) = window_line {
        return Some(line);
    }
    if start == 0 {
        return None;
    }
    handle.seek(SeekFrom::Start(0)).ok()?;
    bytes.clear();
    handle.read_to_end(&mut bytes).ok()?;
    last_line_of(&bytes)
}

/// Record that `session` is at `place` at `now`: a line when the place
/// differs from the last line's, otherwise only the modification time. A
/// changed-place append to a file over [`TRIM_ABOVE_BYTES`] first trims it.
pub fn record(session: &str, place: &Place, now: SystemTime) -> Result<Recorded, Failure> {
    use std::io::Write;
    let file = file_for(session).ok_or(Failure::NoSessionFile)?;
    let dir = file.parent().ok_or(Failure::NoSessionFile)?;
    std::fs::create_dir_all(dir).map_err(|_| Failure::DirectoryNotCreated)?;
    crate::journal::private(dir, 0o700);
    let fields = place.fields();
    let last = tail_line(&file);
    let unchanged = last
        .as_deref()
        .and_then(fields_of)
        .is_some_and(|last| last.iter().zip(&fields).all(|(a, b)| a == b));
    if unchanged {
        let handle = std::fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .map_err(|_| Failure::NotWritten)?;
        handle.set_modified(now).map_err(|_| Failure::NotWritten)?;
        return Ok(Recorded::Whole);
    }
    let seconds = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let line = format!("{seconds} {} {} {}\n", fields[0], fields[1], fields[2]);
    let over = std::fs::metadata(&file).is_ok_and(|meta| meta.len() > TRIM_ABOVE_BYTES);
    let mut trim_failed = false;
    if over {
        // A failed trim leaves the file whole: it only grows until the next call.
        if trim_and_append(&file, &line, &|from, to| std::fs::rename(from, to)).is_ok() {
            let handle = std::fs::OpenOptions::new()
                .write(true)
                .open(&file)
                .map_err(|_| Failure::NotWritten)?;
            handle.set_modified(now).map_err(|_| Failure::NotWritten)?;
            return Ok(Recorded::Whole);
        }
        trim_failed = true;
    }
    let mut handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(|_| Failure::NotWritten)?;
    handle
        .write_all(line.as_bytes())
        .map_err(|_| Failure::NotWritten)?;
    handle.set_modified(now).map_err(|_| Failure::NotWritten)?;
    Ok(if trim_failed {
        Recorded::TrimFailed
    } else {
        Recorded::Whole
    })
}

/// Replace the file with its newest complete lines that fit in
/// [`TRIM_TARGET_BYTES`] together with `line`, then `line`. The new content is
/// written to `<file>.tmp.<pid>` (mode 0600) and `rename`d over the file; on
/// any failure the temp file is removed and the original is untouched. A line
/// another process appends between the read and the rename is lost.
fn trim_and_append(
    file: &Path,
    line: &str,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::io::Write;
    let old = std::fs::read(file)?;
    let complete_end = old
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |at| at + 1);
    let complete = &old[..complete_end];
    let budget = TRIM_TARGET_BYTES.saturating_sub(line.len());
    let mut kept_from = complete.len();
    while kept_from > 0 {
        let line_from = complete[..kept_from - 1]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        if complete.len() - line_from > budget {
            break;
        }
        kept_from = line_from;
    }
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp.{}", std::process::id()));
    let temp = file.with_file_name(name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options
        .open(&temp)
        .and_then(|mut handle| {
            handle.write_all(&complete[kept_from..])?;
            handle.write_all(line.as_bytes())
        })
        .and_then(|()| rename(&temp, file));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp); // the temp file may not exist; the original is what matters
    }
    written
}

/// A session's latest line, with the session's last-seen time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub session: String,
    pub repo_identity: PathBuf,
    pub worktree: PathBuf,
    pub branch: String,
    pub last_seen: SystemTime,
}

/// The latest line of every session's activity file. A superseded line never
/// appears: only the last line of each file is read. Read-only.
pub fn latest_all() -> Vec<Seen> {
    let Some(dir) = sessions_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let session = name.strip_suffix(".activity")?;
            let meta = entry.metadata().ok()?;
            let last_seen = meta.modified().ok()?;
            if !meta.is_file() {
                return None;
            }
            let line = tail_line(&entry.path())?;
            let [repo, worktree, branch] = fields_of(&line)?;
            Some(Seen {
                session: session.to_string(),
                repo_identity: PathBuf::from(decode(repo)),
                worktree: PathBuf::from(decode(worktree)),
                branch: decode(branch),
                last_seen,
            })
        })
        .collect()
}

/// Every other session last seen after `cutoff` whose latest line names
/// `repo_identity`, freshest first. A file is opened only when its name ends
/// `.activity`, it is a regular file and its modification time, read without
/// opening it, is after `cutoff`.
pub fn peers_in(session: &str, repo_identity: &Path, cutoff: SystemTime) -> Vec<Peer> {
    match sessions_dir() {
        Some(dir) => peers_in_dir(&dir, session, repo_identity, cutoff),
        None => Vec::new(),
    }
}

/// [`peers_in`] over the sessions directory `dir`.
fn peers_in_dir(dir: &Path, session: &str, repo_identity: &Path, cutoff: SystemTime) -> Vec<Peer> {
    let want = path_field(repo_identity);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let own = format!("{session}.activity");
    let mut found: Vec<Peer> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let peer = name.strip_suffix(".activity")?;
            if name == own {
                return None;
            }
            let meta = entry.metadata().ok()?;
            let last_seen = meta.modified().ok()?;
            if !meta.is_file() || last_seen <= cutoff {
                return None;
            }
            let line = tail_line(&entry.path())?;
            let [repo, worktree, branch] = fields_of(&line)?;
            if path_field(Path::new(&decode(repo))) != want {
                return None;
            }
            Some(Peer {
                session: peer.to_string(),
                worktree: PathBuf::from(decode(worktree)),
                branch: decode(branch),
                last_seen,
            })
        })
        .collect();
    found.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
    found
}

/// `5m`, `1h 20m`, `3d`: how long before `now` something was last seen.
pub fn ago(now: SystemTime, then: SystemTime) -> String {
    let secs = now.duration_since(then).map_or(0, |d| d.as_secs());
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => match (secs / 3600, secs % 3600 / 60) {
            (hours, 0) => format!("{hours}h"),
            (hours, minutes) => format!("{hours}h {minutes}m"),
        },
        _ => format!("{}d", secs / 86_400),
    }
}

/// What a git command in a Bash clause is for the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Records,
    Ignored,
}

fn verb_of(after_globals: &[crate::shell::Word], subcommand: &str) -> Verb {
    let loose: Vec<&str> = after_globals
        .iter()
        .filter(|w| !w.quoted)
        .map(|w| w.text.as_str())
        .take_while(|t| *t != "--")
        .collect();
    let has = |flags: &[&str]| loose.iter().any(|t| flags.contains(t));
    let records = match subcommand {
        "commit" | "push" | "tag" => true,
        "worktree" => loose.iter().find(|t| !t.starts_with('-')) == Some(&"add"),
        "checkout" => has(&["-b", "-B"]),
        "switch" => has(&["-c", "-C", "--create", "--force-create"]),
        _ => false,
    };
    if records {
        Verb::Records
    } else {
        Verb::Ignored
    }
}

/// The directories a Bash command's recording git clauses run in: the dir of
/// `git -C <dir>` (applied in order, relative ones against the clause's
/// directory), else the directory a leading `cd <dir> &&` or a subshell
/// `(cd <dir> && ...)` set, else `cwd`. A target that cannot be read
/// literally is left out.
pub fn bash_targets(command: &str, cwd: &Path) -> Vec<PathBuf> {
    use crate::shell::Resolved;
    if !command.contains("git") {
        return Vec::new();
    }
    let parsed = crate::shell::lex(command);
    let context = crate::rules::Context {
        cwd,
        parsed: &parsed,
        background: false,
        timeout_ms: None,
        tool_use_id: "",
        transcript: None,
        agent_transcript: None,
        command,
        platform: None,
        output: None,
        session: "",
        failed: false,
    };
    let mut targets: Vec<PathBuf> = Vec::new();
    for clause in parsed.judgeable() {
        if clause.program() != Some("git") {
            continue;
        }
        let mut dir = context.cwd_at(clause.at);
        let args = clause.args();
        let mut index = 0;
        let mut readable = true;
        while let Some(word) = args.get(index) {
            let text = word.text.as_str();
            if !text.starts_with('-') || word.quoted {
                break;
            }
            let valued = matches!(
                text,
                "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace"
            );
            if text == "-C" {
                match args.get(index + 1).map(|w| parsed.word_value(clause.at, w)) {
                    Some(Resolved::Text(path)) => dir = crate::rules::resolve(&dir, &path),
                    Some(Resolved::Variable | Resolved::Substitution) | None => readable = false,
                }
            }
            index += if valued { 2 } else { 1 };
        }
        let Some(subcommand) = args.get(index).map(|w| w.text.as_str()) else {
            continue;
        };
        let rest = args.get(index + 1..).unwrap_or_default();
        if readable && verb_of(rest, subcommand) == Verb::Records && !targets.contains(&dir) {
            targets.push(dir);
        }
    }
    targets
}

/// The default window a peer counts as live for.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(2 * 3600);

/// No activity file is older than this: the session sweep deletes it after a
/// week. A peer lookup can use it as its cutoff before the configured window
/// is known, which needs a `git config` read.
pub const HORIZON: Duration = Duration::from_secs(7 * 24 * 3600);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_file_last_written_before_the_cutoff_is_not_a_peer() {
        let dir = std::env::temp_dir().join(format!("amont-peers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); // a missing directory is the normal first run
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let repo = Path::new("/r/a/.git");
        let line = "1 /r/a/.git /r/a main\n";
        for (name, seconds) in [("old", 1_000), ("new", 9_000)] {
            let file = dir.join(format!("{name}.activity"));
            std::fs::write(&file, line).expect("write");
            let handle = std::fs::OpenOptions::new()
                .write(true)
                .open(&file)
                .expect("open");
            handle.set_modified(at(seconds)).expect("mtime");
        }
        let peers = peers_in_dir(&dir, "me", repo, at(5_000));
        let names: Vec<&str> = peers.iter().map(|peer| peer.session.as_str()).collect();
        assert_eq!(names, ["new"]);
        let _ = std::fs::remove_dir_all(&dir); // cleanup only; a leftover temp dir is harmless
    }

    #[test]
    fn an_unterminated_tail_is_not_a_line() {
        assert_eq!(last_line_of(b"1 a b c\n2 d e").as_deref(), Some("1 a b c"));
        assert_eq!(last_line_of(b"1 a b c\n").as_deref(), Some("1 a b c"));
        assert_eq!(last_line_of(b"no newline at all"), None);
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn a_field_with_spaces_or_percent_survives_the_round_trip() {
        for field in ["/a b/c", "100%", "x%20y", "tab\there"] {
            assert_eq!(decode(&encode(field)), field);
            assert!(!encode(field).contains(char::is_whitespace));
        }
    }

    fn old_decode(field: &str) -> String {
        field
            .replace("%20", " ")
            .replace("%09", "\t")
            .replace("%0A", "\n")
            .replace("%0D", "\r")
            .replace("%25", "%")
    }

    #[test]
    fn decode_matches_the_old_chained_replace_and_inverts_encode() {
        let alphabet = ["%", "2", "5", "0", "9", "A", "D", " ", "\t", "\n", "a"];
        let mut strings = vec![String::new()];
        let mut layer = vec![String::new()];
        for _ in 0..3 {
            layer = layer
                .iter()
                .flat_map(|prefix| alphabet.iter().map(move |c| format!("{prefix}{c}")))
                .collect();
            strings.extend(layer.iter().cloned());
        }
        for text in ["%2", "%41", "%%20", "%25", "%2525"] {
            strings.push(text.to_string());
        }
        for text in &strings {
            assert_eq!(decode(text), old_decode(text), "decode of {text:?}");
        }
        for text in &strings {
            assert_eq!(decode(&encode(text)), *text, "round trip of {text:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_rename_leaves_the_file_whole_and_no_temp_sibling() {
        let dir = std::env::temp_dir().join(format!("amont-agent-trim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let file = dir.join("s.activity");
        let original = b"1 /r /w main\n2 /r /w feat\n";
        std::fs::write(&file, original).expect("a seeded file");
        let outcome = trim_and_append(&file, "3 /r /w x\n", &|_, _| {
            Err(std::io::Error::other("rename refused"))
        });
        assert!(outcome.is_err());
        assert_eq!(std::fs::read(&file).expect("the file"), original);
        let names: Vec<String> = std::fs::read_dir(&dir)
            .expect("the directory")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["s.activity".to_string()]);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn ago_reads_in_the_largest_unit() {
        let then = at(1000);
        assert_eq!(ago(at(1030), then), "30s");
        assert_eq!(ago(at(1000 + 25 * 60), then), "25m");
        assert_eq!(ago(at(1000 + 3 * 3600), then), "3h");
        assert_eq!(ago(at(1000 + 3600 + 20 * 60), then), "1h 20m");
        assert_eq!(ago(at(1000 + 2 * 86_400), then), "2d");
    }

    #[test]
    fn repo_names_come_from_the_common_dir() {
        assert_eq!(
            repo_name_of(Path::new("/r/amont-agent/.git")),
            "amont-agent"
        );
        assert_eq!(repo_name_of(Path::new("/srv/x.git")), "x");
    }

    #[test]
    fn a_peer_is_live_inside_the_window_only() {
        let peer = Peer {
            session: "s".into(),
            worktree: PathBuf::from("/w"),
            branch: "b".into(),
            last_seen: at(10_000),
        };
        assert!(peer.is_live(at(10_000 + 3599), Duration::from_secs(3600)));
        assert!(!peer.is_live(at(10_000 + 3600), Duration::from_secs(3600)));
    }

    #[test]
    fn git_verbs_that_change_the_place_are_recorded_from_the_right_directory() {
        let cwd = Path::new("/session");
        let cases: [(&str, &[&str]); 11] = [
            ("git commit -m x", &["/session"]),
            ("git -C /r push origin main", &["/r"]),
            ("cd /r && git tag v1", &["/r"]),
            ("(cd /r && git commit -m y)", &["/r"]),
            ("git worktree add ../w -b x", &["/session"]),
            ("git checkout -b feat", &["/session"]),
            ("git switch -c feat", &["/session"]),
            ("git checkout main", &[]),
            ("git status", &[]),
            ("git commit -C HEAD", &["/session"]),
            ("echo git commit", &[]),
        ];
        for (command, want) in cases {
            let got = bash_targets(command, cwd);
            let want: Vec<PathBuf> = want.iter().map(PathBuf::from).collect();
            assert_eq!(got, want, "{command}");
        }
    }

    #[test]
    fn a_dash_c_value_after_the_subcommand_is_not_a_directory() {
        let got = bash_targets("git -C /a -C b commit -C HEAD", Path::new("/s"));
        assert_eq!(got, vec![PathBuf::from("/a/b")]);
    }
}
