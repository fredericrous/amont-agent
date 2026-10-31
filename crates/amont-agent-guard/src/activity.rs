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

/// Past this many lines the file is cut back to [`KEPT_LINES`].
const CAP_LINES: usize = 4000;
const KEPT_LINES: usize = 2000;

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
    field
        .replace("%20", " ")
        .replace("%09", "\t")
        .replace("%0A", "\n")
        .replace("%0D", "\r")
        .replace("%25", "%")
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

/// The place of the last line of `text`, as its three encoded fields.
fn last_fields(text: &str) -> Option<[&str; 3]> {
    let line = text.lines().rev().find(|l| !l.trim().is_empty())?;
    let mut parts = line.split_whitespace();
    let _ts = parts.next()?;
    let fields = [parts.next()?, parts.next()?, parts.next()?];
    Some(fields)
}

/// Record that `session` is at `place` at `now`: a line when the place
/// differs from the last line's, otherwise only the modification time.
pub fn record(session: &str, place: &Place, now: SystemTime) -> Result<(), Failure> {
    use std::io::Write;
    let file = file_for(session).ok_or(Failure::NoSessionFile)?;
    let dir = file.parent().ok_or(Failure::NoSessionFile)?;
    std::fs::create_dir_all(dir).map_err(|_| Failure::DirectoryNotCreated)?;
    crate::journal::private(dir, 0o700);
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let fields = place.fields();
    let unchanged =
        last_fields(&text).is_some_and(|last| last.iter().zip(&fields).all(|(a, b)| a == b));
    let seconds = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut open = std::fs::OpenOptions::new();
    if unchanged {
        open.write(true);
    } else {
        open.create(true).append(true);
        if text.lines().count() > CAP_LINES {
            cut_back(&file, &text);
        }
    }
    let mut handle = open.open(&file).map_err(|_| Failure::NotWritten)?;
    if !unchanged {
        let line = format!("{seconds} {} {} {}\n", fields[0], fields[1], fields[2]);
        handle
            .write_all(line.as_bytes())
            .map_err(|_| Failure::NotWritten)?;
    }
    handle.set_modified(now).map_err(|_| Failure::NotWritten)
}

/// Keep the newest [`KEPT_LINES`] lines. A failed rewrite leaves the file as
/// it was: it only grows a little longer until the next call.
fn cut_back(file: &Path, text: &str) {
    let mut newest: Vec<&str> = text.lines().rev().take(KEPT_LINES).collect();
    newest.reverse();
    newest.push("");
    let _ = std::fs::write(file, newest.join("\n")); // a failed cut only leaves the file longer until the next call
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
            let last_seen = entry.metadata().ok()?.modified().ok()?;
            let text = std::fs::read_to_string(entry.path()).ok()?;
            let [repo, worktree, branch] = last_fields(&text)?;
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

/// Every other session whose latest line names `repo_identity`, freshest
/// first.
pub fn peers_in(session: &str, repo_identity: &Path) -> Vec<Peer> {
    let want = path_field(repo_identity);
    let mut found: Vec<Peer> = latest_all()
        .into_iter()
        .filter(|seen| seen.session != session && path_field(&seen.repo_identity) == want)
        .map(|seen| Peer {
            session: seen.session,
            worktree: seen.worktree,
            branch: seen.branch,
            last_seen: seen.last_seen,
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
                    _ => readable = false,
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

#[cfg(test)]
mod tests {
    use super::*;

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
