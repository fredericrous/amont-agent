//! `~/.claude/amont-agent/attestations/` is swept, except what a plan cites.
//!
//! That directory is where the worktree-task skill puts evidence (F3 records,
//! F7 screenshots and `guide.md`) and nothing ever removed anything from it.
//! A direct child directory is KEPT when its newest entry is younger than 30
//! days, when a Markdown file under a known repository's `docs/` (or
//! `~/.claude/plans`) cites it, or when it holds a `.keep`. Everything else is
//! renamed to `.trash-<unix-secs>-<name>` and deleted once the time in that
//! name is 7 days old.
//!
//! (docs/plans/2026-10-11-attestations-swept-except-cited.md)
//!
//! ## Failing safe
//!
//! Any read error during the scan for citations stops the removal of every
//! directory for that run: a directory that a plan cites must not go because
//! the plan could not be read. The purge of old `.trash-*` entries is not part
//! of that, because it was decided a week ago.
//!
//! ## One run at a time
//!
//! `attestations.sweeping` is created with `create_new`; the holder checks the
//! daily stamp `attestations.swept`, touches it BEFORE scanning (so a failed
//! scan does not rescan every session) and removes the lock at the end. A
//! lock older than an hour is a crashed run and is taken over by renaming it
//! to `attestations.sweeping.stale-<pid>`: of several processes only one
//! rename wins. The winner re-checks the renamed file's age, because a loser
//! of an earlier race may have renamed a LIVE lock created in between; under
//! an hour old, it puts it back and backs off.

use amont_agent_guard::journal::{self, Entry};
use amont_agent_guard::ui;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, UNIX_EPOCH};

pub const DAY: u64 = 86_400;
const KEEP_FOR: u64 = 30 * DAY;
const TRASH_FOR: u64 = 7 * DAY;
const LOCK_STALE: u64 = 3600;
const GIT_ROOTS_KEY: &str = r"^amont\.agent\.plan-review\..*\.path$";

/// Where the scan or the sweep could not read, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub path: String,
    pub err: String,
}

fn fail(path: &Path, e: &io::Error) -> Failure {
    Failure {
        path: path.display().to_string(),
        err: e.to_string(),
    }
}

/// Everything the sweep reads, so a test can point it at a scratch tree.
pub struct Env {
    /// `attestations/`.
    pub store: PathBuf,
    /// Beside the journal: the stamp and the lock live here.
    pub state: PathBuf,
    /// The directory repositories sit under; `Err` when it cannot be resolved.
    pub plan_root: Result<PathBuf, String>,
    /// `~/.claude/plans`.
    pub claude_plans: PathBuf,
    /// The configured `plan-review.<name>.path` roots. `None` asks git, which
    /// is the only subprocess the sweep runs; a test passes its own answer.
    pub git: Option<Result<Vec<PathBuf>, String>>,
}

impl Env {
    pub fn live() -> Option<Env> {
        let state = journal::dir()?;
        Some(Env {
            store: state.join("attestations"),
            plan_root: crate::plan_review::root().ok_or_else(|| "no home directory".to_string()),
            claude_plans: amont_agent_guard::settings::config_dir()?.join("plans"),
            state,
            git: None,
        })
    }
    fn stamp(&self) -> PathBuf {
        self.state.join("attestations.swept")
    }
    fn lock(&self) -> PathBuf {
        self.state.join("attestations.sweeping")
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------- citations

/// Every `attestations/<name>` in `text`: the maximal `[A-Za-z0-9._-]` run
/// after the slash, trailing dots (a sentence end) stripped. Plain
/// `str::find`; the full path `~/.claude/amont-agent/attestations/x` holds the
/// same text and counts.
fn citations(text: &str, mut found: impl FnMut(&str)) {
    const KEY: &str = "attestations/";
    let mut rest = text;
    while let Some(i) = rest.find(KEY) {
        let after = &rest[i + KEY.len()..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
            .unwrap_or(after.len());
        let name = after[..end].trim_end_matches('.');
        if !name.is_empty() {
            found(name);
        }
        rest = after;
    }
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Does a citation of `cited` keep the directory `dir`? Slugs match exactly; a
/// cited name of at least 7 hex characters also keeps a 40-hex directory that
/// starts with it, because plans cite short shas.
fn names_match(cited: &str, dir: &str) -> bool {
    cited == dir
        || (cited.len() >= 7
            && is_hex(cited)
            && dir.len() == 40
            && is_hex(dir)
            && dir.starts_with(cited))
}

type Cited = BTreeMap<String, String>;

fn sorted_entries(dir: &Path) -> Result<Vec<fs::DirEntry>, Failure> {
    let mut v = fs::read_dir(dir)
        .map_err(|e| fail(dir, &e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| fail(dir, &e))?;
    v.sort_by_key(|e| e.file_name());
    Ok(v)
}

fn scan_file(path: &Path, base: &Path, out: &mut Cited) -> Result<(), Failure> {
    let bytes = fs::read(path).map_err(|e| fail(path, &e))?;
    let text = String::from_utf8_lossy(&bytes);
    let shown = path
        .strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string();
    citations(&text, |name| {
        out.entry(name.to_string()).or_insert_with(|| shown.clone());
    });
    Ok(())
}

/// Every `*.md` below `dir`. Symlinks are not followed.
fn scan_docs(dir: &Path, base: &Path, out: &mut Cited) -> Result<(), Failure> {
    for e in sorted_entries(dir)? {
        let path = e.path();
        let ft = e.file_type().map_err(|err| fail(&path, &err))?;
        if ft.is_dir() {
            scan_docs(&path, base, out)?;
        } else if ft.is_file() && path.extension().is_some_and(|x| x == "md") {
            scan_file(&path, base, out)?;
        }
    }
    Ok(())
}

/// Scan `<root>/docs` when there is one.
fn scan_root(root: &Path, base: &Path, out: &mut Cited) -> Result<(), Failure> {
    let docs = root.join("docs");
    match fs::metadata(&docs) {
        Ok(m) if m.is_dir() => scan_docs(&docs, base, out),
        Ok(_) => Ok(()),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(())
        }
        Err(e) => Err(fail(&docs, &e)),
    }
}

/// The `amont.agent.plan-review.<name>.path` roots from global git config.
/// Exit 1 with no output is git saying no key matched; git not running, or
/// any other exit, is a failure.
fn git_roots() -> Result<Vec<PathBuf>, Failure> {
    let args = [
        "config",
        "--global",
        "--type=path",
        "--get-regexp",
        GIT_ROOTS_KEY,
    ];
    let bad = |err: String| Failure {
        path: "git config --global".to_string(),
        err,
    };
    let out = amont_agent_guard::git::output(&args).ok_or_else(|| bad("git did not run".into()))?;
    roots_from(out.code, &out.stdout, &out.stderr).map_err(bad)
}

/// What `git config --get-regexp` said, as roots: exit 0 lists them, exit 1
/// with nothing printed is no key matching (no configured roots), and
/// anything else is a failure.
fn roots_from(code: i32, stdout: &str, stderr: &str) -> Result<Vec<PathBuf>, String> {
    match code {
        0 => Ok(stdout
            .lines()
            .filter_map(|l| l.split_once(' ').map(|(_, v)| PathBuf::from(v.trim())))
            .collect()),
        1 if stdout.trim().is_empty() => Ok(Vec::new()),
        c => Err(format!("exit {c}: {}", stderr.trim())),
    }
}

fn scan(env: &Env) -> Result<Cited, Failure> {
    let mut out = Cited::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    match &env.plan_root {
        Err(why) => {
            return Err(Failure {
                path: "plan root".into(),
                err: why.clone(),
            })
        }
        Ok(root) => {
            for e in sorted_entries(root)? {
                let p = e.path();
                scan_root(&p, root, &mut out)?;
                seen.push(p);
            }
        }
    }
    let base = env.plan_root.as_deref().unwrap_or(Path::new(""));
    let configured = match &env.git {
        Some(Ok(v)) => v.clone(),
        Some(Err(why)) => {
            return Err(Failure {
                path: "git config --global".into(),
                err: why.clone(),
            })
        }
        None => git_roots()?,
    };
    for p in configured {
        if seen.contains(&p) {
            continue;
        }
        if let Err(e) = fs::metadata(&p) {
            return Err(fail(&p, &e));
        }
        scan_root(&p, base, &mut out)?;
        seen.push(p);
    }
    match fs::read_dir(&env.claude_plans) {
        Ok(rd) => {
            let mut files = rd
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| fail(&env.claude_plans, &e))?;
            files.sort_by_key(|e| e.file_name());
            for e in files {
                let p = e.path();
                let ft = e.file_type().map_err(|err| fail(&p, &err))?;
                if ft.is_file() && p.extension().is_some_and(|x| x == "md") {
                    scan_file(&p, &env.claude_plans, &mut out)?;
                }
            }
        }
        // No plans directory yet is no plans.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(fail(&env.claude_plans, &e)),
    }
    Ok(out)
}

// ------------------------------------------------------------- the store

/// What is known about one directory.
struct Probe {
    newest: u64,
    bytes: u64,
    /// An mtime that cannot be read or lies in the future: counts as recent.
    doubtful: bool,
}

fn walk(path: &Path, now: u64, p: &mut Probe) {
    let Ok(rd) = fs::read_dir(path) else {
        p.doubtful = true;
        return;
    };
    for e in rd {
        let Ok(e) = e else {
            p.doubtful = true;
            continue;
        };
        let child = e.path();
        let Ok(meta) = fs::symlink_metadata(&child) else {
            p.doubtful = true;
            continue;
        };
        note(&meta, now, p);
        if meta.is_dir() {
            walk(&child, now, p);
        } else {
            p.bytes += meta.len();
        }
    }
}

fn note(meta: &fs::Metadata, now: u64, p: &mut Probe) {
    match meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
    {
        Some(d) => {
            p.newest = p.newest.max(d.as_secs());
            if d.as_secs() > now {
                p.doubtful = true;
            }
        }
        None => p.doubtful = true,
    }
}

pub struct Row {
    pub name: String,
    pub age: u64,
    pub bytes: u64,
    pub remove: bool,
    pub reason: String,
    pub cited: bool,
}

fn rows(env: &Env, now: u64, cited: Result<&Cited, &Failure>) -> Result<Vec<Row>, Failure> {
    let entries = match sorted_entries(&env.store) {
        Ok(v) => v,
        Err(_) if !env.store.exists() => return Ok(Vec::new()),
        Err(f) => return Err(f),
    };
    let mut out = Vec::new();
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        // `DirEntry::file_type` does not follow a symlink.
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let path = e.path();
        let mut probe = Probe {
            newest: 0,
            bytes: 0,
            doubtful: false,
        };
        match fs::symlink_metadata(&path) {
            Ok(m) => note(&m, now, &mut probe),
            Err(_) => probe.doubtful = true,
        }
        walk(&path, now, &mut probe);
        let age = if probe.doubtful {
            0
        } else {
            now.saturating_sub(probe.newest)
        };
        let cite = cited.ok().and_then(|c| {
            c.iter()
                .find(|(k, _)| names_match(k, &name))
                .map(|(_, v)| v)
        });
        let keep_file = fs::symlink_metadata(path.join(".keep")).is_ok();
        let (remove, reason) = if probe.doubtful || age < KEEP_FOR {
            (
                false,
                format!("recent {}", amont_agent_guard::doctor::ago(age)),
            )
        } else if let Some(by) = cite {
            (false, format!("cited by {by}"))
        } else if keep_file {
            (false, ".keep".to_string())
        } else if let Err(f) = cited {
            (false, format!("held (scan failed: {})", f.path))
        } else {
            (
                true,
                format!("older than {}", amont_agent_guard::doctor::ago(KEEP_FOR)),
            )
        };
        out.push(Row {
            name,
            age,
            bytes: probe.bytes,
            remove,
            reason,
            cited: cite.is_some(),
        });
    }
    Ok(out)
}

pub struct Report {
    pub rows: Vec<Row>,
    pub failure: Option<Failure>,
    /// (name, bytes) of what moved to the trash.
    pub removed: Vec<(String, u64)>,
    /// (name, io error) of what could not.
    pub errors: Vec<(String, String)>,
    pub purged: usize,
}

/// Delete `.trash-<secs>-*` whose time is `TRASH_FOR` old. A name without a
/// readable time is left alone. Returns how many went, and (entry, io error)
/// for each that could not: this is the step that really deletes, so its
/// failures are reported, never dropped.
fn purge(store: &Path, now: u64) -> (usize, Vec<(String, String)>) {
    let mut errors = Vec::new();
    let rd = match fs::read_dir(store) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (0, errors),
        Err(e) => {
            errors.push((store.display().to_string(), e.to_string()));
            return (0, errors);
        }
    };
    let mut n = 0;
    for e in rd {
        let e = match e {
            Ok(e) => e,
            Err(err) => {
                errors.push((store.display().to_string(), err.to_string()));
                continue;
            }
        };
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(".trash-") else {
            continue;
        };
        let Some((secs, _)) = rest.split_once('-') else {
            continue;
        };
        let Ok(secs) = secs.parse::<u64>() else {
            continue;
        };
        if now.saturating_sub(secs) < TRASH_FOR {
            continue;
        }
        let path = e.path();
        let gone = match fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => fs::remove_dir_all(&path),
            Ok(_) => fs::remove_file(&path),
            Err(e) => Err(e),
        };
        match gone {
            Ok(()) => n += 1,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => errors.push((name, err.to_string())),
        }
    }
    (n, errors)
}

/// Scan, decide and (with `apply`) act. Without `apply` nothing is changed.
pub fn run(env: &Env, now: u64, apply: bool) -> Report {
    let scanned = scan(env);
    let (cited, failure) = match &scanned {
        Ok(c) => (Ok(c), None),
        Err(f) => (Err(f), Some(f.clone())),
    };
    let mut report = Report {
        rows: Vec::new(),
        failure,
        removed: Vec::new(),
        errors: Vec::new(),
        purged: 0,
    };
    match rows(env, now, cited) {
        Ok(r) => report.rows = r,
        Err(f) => {
            report.failure.get_or_insert(f);
        }
    }
    if !apply {
        return report;
    }
    let (purged, failed) = purge(&env.store, now);
    report.purged = purged;
    report.errors.extend(failed);
    if report.failure.is_some() {
        return report;
    }
    for row in report.rows.iter_mut().filter(|r| r.remove) {
        let to = env.store.join(format!(".trash-{now}-{}", row.name));
        match fs::rename(env.store.join(&row.name), to) {
            Ok(()) => report.removed.push((row.name.clone(), row.bytes)),
            // Someone got there first.
            Err(e) if e.kind() == io::ErrorKind::NotFound => row.remove = false,
            Err(e) => {
                row.remove = false;
                report.errors.push((row.name.clone(), e.to_string()));
            }
        }
    }
    report
}

fn record(outcome: &str, excerpt: &str) {
    journal::record(&Entry {
        rule: "-",
        stance: "-",
        outcome,
        session: "-",
        repo: "-",
        mode: "-",
        excerpt,
    });
}

/// One line per removal, one per run with what remains, and a failed scan last
/// so that it is the final word `doctor` reads.
fn journal_report(r: &Report) {
    for (name, bytes) in &r.removed {
        record("attestation-swept", &format!("{name} {bytes}"));
    }
    let gone: Vec<&str> = r.removed.iter().map(|(n, _)| n.as_str()).collect();
    let kept: Vec<&Row> = r
        .rows
        .iter()
        .filter(|x| !gone.contains(&x.name.as_str()))
        .collect();
    let bytes: u64 = kept.iter().map(|x| x.bytes).sum();
    record("attestations-kept", &format!("{} {bytes}", kept.len()));
    if let Some(f) = &r.failure {
        record("attestations-scan-failed", &f.path);
    }
}

// ---------------------------------------------------------------- locking

pub struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn age_of(path: &Path, now: u64) -> Option<u64> {
    let m = fs::symlink_metadata(path).ok()?.modified().ok()?;
    let secs = m.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(now.saturating_sub(secs))
}

fn create_lock(path: &Path) -> io::Result<Lock> {
    create_lock_file(path).map(|_| Lock(path.to_path_buf()))
}

/// Take the lock, or `None` when a live run holds it.
pub fn acquire(lock: &Path, now: u64) -> Option<Lock> {
    if let Some(dir) = lock.parent() {
        let _ = fs::create_dir_all(dir);
    }
    for _ in 0..2 {
        match create_lock(lock) {
            Ok(l) => return Some(l),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
        match age_of(lock, now) {
            // Vanished between the two calls: try again.
            None if !lock.exists() => continue,
            Some(age) if age >= LOCK_STALE => {}
            _ => return None,
        }
        return take_over(lock, now);
    }
    None
}

fn take_over(lock: &Path, now: u64) -> Option<Lock> {
    let mut name = lock.as_os_str().to_os_string();
    name.push(format!(".stale-{}", std::process::id()));
    let stale = PathBuf::from(name);
    // Only one rename of the same source can succeed.
    fs::rename(lock, &stale).ok()?;
    if age_of(&stale, now).is_some_and(|a| a < LOCK_STALE) {
        // A live lock, created after our age check. It is not put back: its
        // holder may have finished meanwhile, and a rename back would then
        // leave a lock nobody holds, blocking every run for an hour (CI
        // caught exactly that). The lock is advisory; the day's claim
        // (`claim_today`) is what keeps the daily run single.
        // holds-until: removal stays idempotent (a rename to the trash with
        // NotFound tolerated). Then a manual `--apply` overlapping the daily
        // run, which an advisory lock allows, is harmless; a removal that is
        // not would need a lock that cannot be dropped this way.
        let _ = fs::remove_file(&stale);
        return None;
    }
    let got = create_lock(lock).ok();
    let _ = fs::remove_file(&stale);
    got
}

fn touch(path: &Path, now: u64) -> io::Result<()> {
    let f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    f.set_modified(UNIX_EPOCH + Duration::from_secs(now))
}

fn stamp_fresh(env: &Env, now: u64) -> bool {
    age_of(&env.stamp(), now).is_some_and(|a| a < DAY)
}

/// Claim today's run: `attestations.swept.<day>` made with `create_new`, so
/// exactly one process per day gets it, whatever happened to the lock. The
/// lock is advisory: a takeover can drop a live holder's lock, and that
/// holder's `Drop` can then remove a successor's lock, so two processes can
/// hold "the" lock at once (the stale-lock test found two daily runs before
/// this claim existed). Older claims are removed.
fn claim_today(env: &Env, now: u64) -> bool {
    let day = now / DAY;
    let mine = format!("attestations.swept.{day}");
    if create_lock_file(&env.state.join(&mine)).is_err() {
        return false;
    }
    if let Ok(rd) = fs::read_dir(&env.state) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("attestations.swept.") && name != mine {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    true
}

fn create_lock_file(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// The daily sweep. `None`: nothing to do, or someone else is doing it.
pub fn sweep_at(env: &Env, now: u64) -> Option<Report> {
    if !env.store.is_dir() {
        return None;
    }
    // Not the check that decides (the day's claim does): a stat that keeps
    // the common SessionStart from creating and removing a lock file.
    if stamp_fresh(env, now) {
        return None;
    }
    let _lock = acquire(&env.lock(), now)?;
    if stamp_fresh(env, now) || !claim_today(env, now) {
        return None;
    }
    // Before the scan: a failed scan must not rescan every session.
    touch(&env.stamp(), now).ok()?;
    let report = run(env, now, true);
    journal_report(&report);
    Some(report)
}

/// `SessionStart`: sweep at most once a day, in a detached child. Every
/// failure is silent.
///
/// Measured on the real store (119 directories, 34 roots, load average
/// ~100): 0.4 s warm and 1.8 s cold, over the 300 ms the hook may cost. So
/// the session pays one stat and, once a day, one spawn; the child takes the
/// lock, checks the stamp again and journals as an inline run would.
pub fn sweep() {
    let Some(env) = Env::live() else { return };
    if !env.store.is_dir() || stamp_fresh(&env, now()) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["attestations", "--daily"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Its own group: a hook runner that kills ours on a timeout leaves it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let _ = cmd.spawn();
}

// ------------------------------------------------------------ the subcommand

const USAGE: &str = "\
usage: amont-agent attestations [--apply]

  lists each directory under ~/.claude/amont-agent/attestations, why it is
  kept or would be removed; --apply moves the removable ones to
  .trash-<secs>-<name>, deleted after 7 days

  --daily   internal: the once-a-day run SessionStart starts in the background
";

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

fn render(rows: &[Row], tty: bool) -> String {
    let mut ordered: Vec<&Row> = rows.iter().filter(|r| !r.remove).collect();
    ordered.extend(rows.iter().filter(|r| r.remove));
    let cells: Vec<[String; 5]> = ordered
        .iter()
        .map(|r| {
            [
                if r.remove { "remove" } else { "keep" }.to_string(),
                ui::sanitize(&r.name),
                amont_agent_guard::doctor::ago(r.age),
                mb(r.bytes),
                ui::sanitize(&r.reason),
            ]
        })
        .collect();
    let mut out = String::new();
    if !tty {
        for c in &cells {
            out.push_str(&c.join("\t"));
            out.push('\n');
        }
        return out;
    }
    let width = |i: usize| {
        cells
            .iter()
            .map(|c| c[i].chars().count())
            .max()
            .unwrap_or(0)
    };
    let (w0, w1, w2, w3) = (width(0), width(1), width(2), width(3));
    let fixed = w0 + w1 + w2 + w3 + 8;
    let room = 80usize.saturating_sub(fixed).max(12);
    for c in &cells {
        let mut reason = c[4].clone();
        if reason.chars().count() > room {
            reason = reason.chars().take(room - 1).collect::<String>() + "…";
        }
        out.push_str(&format!(
            "{:<w0$}  {:<w1$}  {:>w2$}  {:>w3$}  {reason}\n",
            c[0], c[1], c[2], c[3]
        ));
    }
    out
}

pub fn run_cli(args: &[OsString]) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        let _ = io::stdout().write_all(USAGE.as_bytes());
        return ExitCode::SUCCESS;
    }
    let mut apply = false;
    for a in args {
        match a.to_string_lossy().as_ref() {
            "--apply" => apply = true,
            // The child `sweep` spawns: the daily run, silent.
            "--daily" => {
                if let Some(env) = Env::live() {
                    let _ = sweep_at(&env, now());
                }
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                let _ = io::stdout().write_all(USAGE.as_bytes());
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!(
                    "amont-agent: unknown argument `{}`; try `amont-agent attestations --help`",
                    ui::sanitize(other)
                );
                return ExitCode::from(2);
            }
        }
    }
    let Some(env) = Env::live() else {
        eprintln!("amont-agent: no home directory");
        return ExitCode::FAILURE;
    };
    let now = now();
    let _lock = if apply {
        match acquire(&env.lock(), now) {
            Some(l) => Some(l),
            None => {
                eprintln!("amont-agent: a sweep is running");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    let report = run(&env, now, apply);
    if apply {
        journal_report(&report);
    }
    // Not `print!`: nothing on the hook path writes stdout but the emitter.
    let table = render(&report.rows, io::stdout().is_terminal());
    let _ = io::stdout().write_all(table.as_bytes());

    let mut code = ExitCode::SUCCESS;
    if let Some(f) = &report.failure {
        eprintln!(
            "amont-agent: {}: {}; no directory removed",
            ui::sanitize(&f.path),
            ui::sanitize(&f.err)
        );
        code = ExitCode::FAILURE;
    }
    for (name, err) in &report.errors {
        eprintln!("{} {}: {}", ui::error_sign(), ui::sanitize(name), err);
        code = ExitCode::FAILURE;
    }
    let total: u64 = report.rows.iter().map(|r| r.bytes).sum();
    let (n, bytes) = if apply {
        (
            report.removed.len(),
            report.removed.iter().map(|(_, b)| b).sum::<u64>(),
        )
    } else {
        (
            report.rows.iter().filter(|r| r.remove).count(),
            report
                .rows
                .iter()
                .filter(|r| r.remove)
                .map(|r| r.bytes)
                .sum(),
        )
    };
    eprintln!("{} directories, {}", report.rows.len(), mb(total));
    if apply {
        eprintln!("removed {n} ({}) to .trash-<secs>-<name>", mb(bytes));
    } else {
        eprintln!(
            "nothing removed; amont-agent attestations --apply removes {n} ({})",
            mb(bytes)
        );
    }
    code
}

// ------------------------------------------------------------------ doctor

pub struct Summary {
    pub dirs: usize,
    pub bytes: u64,
    pub oldest: u64,
    /// `None` when the scan failed.
    pub cited: Option<usize>,
    /// Seconds since the last daily sweep; `None` when there has not been one.
    pub swept_ago: Option<u64>,
}

pub fn summary(env: &Env, now: u64) -> Option<Summary> {
    if !env.store.is_dir() {
        return None;
    }
    let scanned = scan(env);
    let rows = rows(env, now, scanned.as_ref()).ok()?;
    Some(Summary {
        dirs: rows.len(),
        bytes: rows.iter().map(|r| r.bytes).sum(),
        oldest: rows.iter().map(|r| r.age).max().unwrap_or(0),
        cited: scanned
            .is_ok()
            .then(|| rows.iter().filter(|r| r.cited).count()),
        swept_ago: age_of(&env.stamp(), now),
    })
}

/// The path the most recent run could not read, when that run failed.
pub fn last_scan_failure() -> Option<String> {
    for p in journal::paths().into_iter().rev() {
        let Ok(text) = fs::read_to_string(&p) else {
            continue;
        };
        for line in text.lines().rev() {
            let mut f = line.splitn(9, ' ');
            let outcome = f.nth(4);
            match outcome {
                Some("attestations-scan-failed") => {
                    return Some(f.nth(3).unwrap_or("").to_string());
                }
                Some("attestations-kept") => return None,
                _ => {}
            }
        }
    }
    None
}

pub fn mb_text(bytes: u64) -> String {
    mb(bytes)
}

/// `amont-agent doctor`'s line for this store, which `main.rs` hands to
/// the doctor: how much it holds, and whether the daily sweep is getting
/// through. Silent when there is no store yet.
pub fn doctor_finding() -> Option<amont_agent_guard::doctor::Finding> {
    use amont_agent_guard::doctor::{ago, now, Finding};
    let env = Env::live()?;
    let t = now();
    let s = summary(&env, t)?;
    let swept = match s.swept_ago {
        Some(a) => format!("swept {} ago", ago(a)),
        None => "never swept".to_string(),
    };
    let cited = match s.cited {
        Some(k) => format!("{k} cited"),
        None => "cited unknown (scan failed)".to_string(),
    };
    let line = format!(
        "attestations: {} directories, {}, oldest {}; {cited}; {swept}",
        s.dirs,
        mb_text(s.bytes),
        ago(s.oldest),
    );
    if let Some(path) = last_scan_failure() {
        return Some(Finding::warn(
            line,
            format!(
                "the last sweep could not read {path} and removed nothing; \
                 run `amont-agent attestations` to see why"
            ),
        ));
    }
    if s.swept_ago.is_some_and(|a| a > 2 * DAY) {
        return Some(Finding::warn(
            line,
            "no SessionStart has swept for over 2 days; \
             run `amont-agent attestations` to see what it would do"
                .to_string(),
        ));
    }
    Some(Finding::good(line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    const NOW: u64 = 1_800_000_000;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Scratch {
            static N: AtomicU32 = AtomicU32::new(0);
            let d = std::env::temp_dir().join(format!(
                "amont-agent-att-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(d.join("state/attestations")).unwrap();
            fs::create_dir_all(d.join("root")).unwrap();
            fs::create_dir_all(d.join("claude/plans")).unwrap();
            Scratch(d)
        }
        fn env(&self) -> Env {
            Env {
                store: self.0.join("state/attestations"),
                state: self.0.join("state"),
                plan_root: Ok(self.0.join("root")),
                claude_plans: self.0.join("claude/plans"),
                git: Some(Ok(Vec::new())),
            }
        }
        fn store(&self) -> PathBuf {
            self.0.join("state/attestations")
        }
        /// A directory with one file, both `days` old.
        fn dir(&self, name: &str, days: u64) -> PathBuf {
            let d = self.store().join(name);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("a.txt"), "12345").unwrap();
            set(&d.join("a.txt"), NOW - days * DAY);
            set(&d, NOW - days * DAY);
            d
        }
        fn plan(&self, repo: &str, rel: &str, text: &str) {
            let f = self.0.join("root").join(repo).join(rel);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, text).unwrap();
        }
        fn names(&self) -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(self.store())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A directory opened so that its times can be set: Windows needs
    /// `FILE_FLAG_BACKUP_SEMANTICS` and write access to open one at all.
    fn open_dir(path: &Path) -> fs::File {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
            fs::OpenOptions::new()
                .write(true)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(path)
                .unwrap()
        }
        #[cfg(not(windows))]
        fs::File::open(path).unwrap()
    }

    fn set(path: &Path, secs: u64) {
        let t = UNIX_EPOCH + Duration::from_secs(secs);
        if path.is_dir() {
            open_dir(path).set_modified(t).unwrap();
        } else {
            fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(t)
                .unwrap();
        }
    }

    fn removed(r: &Report) -> Vec<&str> {
        let mut v: Vec<&str> = r.removed.iter().map(|(n, _)| n.as_str()).collect();
        v.sort();
        v
    }

    #[test]
    fn the_keep_rules_remove_only_what_nothing_keeps() {
        let s = Scratch::new();
        s.dir("old", 40);
        s.dir("fresh", 2);
        s.dir("cited", 40);
        let pinned = s.dir("pinned", 40);
        fs::write(pinned.join(".keep"), "").unwrap();
        set(&pinned, NOW - 40 * DAY);
        let deep = s.dir("deep", 40);
        fs::write(deep.join("new.png"), "x").unwrap();
        set(&deep.join("new.png"), NOW - 2 * DAY);
        fs::write(s.store().join("loose.txt"), "x").unwrap();
        set(&s.store().join("loose.txt"), NOW - 40 * DAY);
        let outside = s.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, s.store().join("link")).unwrap();
        s.plan("repo", "docs/plans/p.md", "see attestations/cited/guide.md");

        let r = run(&s.env(), NOW, true);
        assert_eq!(r.failure, None);
        assert_eq!(removed(&r), ["old"]);
        let mut want = vec![
            ".trash-1800000000-old",
            "cited",
            "deep",
            "fresh",
            "loose.txt",
            "pinned",
        ];
        if cfg!(unix) {
            want.push("link");
        }
        want.sort();
        assert_eq!(s.names(), want);
        assert!(outside.is_dir());
    }

    #[test]
    fn a_missing_plan_root_removes_nothing() {
        let s = Scratch::new();
        s.dir("old", 40);
        let mut env = s.env();
        env.plan_root = Ok(s.0.join("nowhere"));
        let r = run(&env, NOW, true);
        let f = r.failure.expect("scan failed");
        assert!(f.path.ends_with("nowhere"), "{f:?}");
        assert!(r.removed.is_empty());
        assert!(s.store().join("old").is_dir());
        assert!(r.rows[0].reason.starts_with("held (scan failed:"));
        assert!(!r.rows[0].remove);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_plan_removes_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let s = Scratch::new();
        s.dir("old", 40);
        s.plan("repo", "docs/plans/p.md", "x");
        let p = s.0.join("root/repo/docs/plans/p.md");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read(&p).is_ok() {
            return; // root ignores modes
        }
        let r = run(&s.env(), NOW, true);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(r.failure.is_some());
        assert!(r.removed.is_empty());
        assert!(s.store().join("old").is_dir());
    }

    #[test]
    fn a_missing_configured_root_holds_everything_and_a_present_one_is_scanned() {
        let s = Scratch::new();
        s.dir("by-dots", 40);
        s.dir("old", 40);
        let dots = s.0.join("dots");
        fs::create_dir_all(dots.join("docs/plans")).unwrap();
        fs::write(dots.join("docs/plans/x.md"), "attestations/by-dots").unwrap();

        let mut env = s.env();
        env.git = Some(Ok(vec![dots.clone()]));
        let r = run(&env, NOW, false);
        assert_eq!(r.failure, None);
        let by: Vec<_> = r.rows.iter().map(|x| (x.name.as_str(), x.remove)).collect();
        assert_eq!(by, [("by-dots", false), ("old", true)]);

        env.git = Some(Ok(vec![s.0.join("missing-dots")]));
        let r = run(&env, NOW, true);
        assert!(r.failure.unwrap().path.ends_with("missing-dots"));
        assert!(r.removed.is_empty());
    }

    #[test]
    fn citations_keep_the_exact_name() {
        let s = Scratch::new();
        for n in ["x", "v1.2", "m0-duro-web", "y", "z"] {
            s.dir(n, 40);
        }
        s.dir("unrelated", 40);
        s.plan(
            "r",
            "docs/a.md",
            "see attestations/x/guide.md and attestations/y.",
        );
        s.plan(
            "r",
            "docs/b.md",
            "v attestations/v1.2 then ~/.claude/amont-agent/attestations/z\nlast attestations/m0-duro-web",
        );
        let r = run(&s.env(), NOW, false);
        let gone: Vec<_> = r
            .rows
            .iter()
            .filter(|x| x.remove)
            .map(|x| &x.name[..])
            .collect();
        assert_eq!(gone, ["unrelated"]);
    }

    #[test]
    fn a_short_sha_keeps_the_long_one_and_a_slug_is_exact() {
        let s = Scratch::new();
        let long = format!("d78826f{}", "0".repeat(33));
        s.dir(&long, 40);
        s.dir("abc1234", 40);
        s.plan(
            "r",
            "docs/a.md",
            "attestations/d78826f and attestations/abc",
        );
        let r = run(&s.env(), NOW, false);
        let verdict: Vec<_> = r.rows.iter().map(|x| (x.name.clone(), x.remove)).collect();
        assert!(verdict.contains(&(long, false)), "{verdict:?}");
        assert!(
            verdict.contains(&("abc1234".to_string(), true)),
            "{verdict:?}"
        );
    }

    #[test]
    fn claude_plans_and_other_docs_directories_count() {
        let s = Scratch::new();
        s.dir("new-thing", 40);
        s.dir("noted", 40);
        fs::write(s.0.join("claude/plans/x.md"), "attestations/new-thing").unwrap();
        s.plan("r", "docs/notes/y.md", "attestations/noted");
        let r = run(&s.env(), NOW, false);
        assert!(r.rows.iter().all(|x| !x.remove));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_a_stale_directory_is_not_followed() {
        let s = Scratch::new();
        let d = s.dir("old", 40);
        let outside = s.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, d.join("l")).unwrap();
        set(&d, NOW - 40 * DAY);
        // The link's own mtime is real time and cannot be set without
        // following it, so the clock runs 40 days past whichever is later,
        // NOW or today: the test must not start failing on a calendar date.
        let later = NOW.max(now()) + 40 * DAY;
        let r = run(&s.env(), later, true);
        assert_eq!(removed(&r), ["old"]);
        assert!(outside.join("keep.txt").is_file());
    }

    #[test]
    fn git_exit_codes_map_to_roots_or_a_failure() {
        assert_eq!(
            roots_from(0, "amont.agent.plan-review.dots.path /h/dots\n", ""),
            Ok(vec![PathBuf::from("/h/dots")])
        );
        assert_eq!(roots_from(1, "", ""), Ok(vec![]), "no key matched");
        assert!(roots_from(1, "x y\n", "").is_err(), "exit 1 with output");
        assert!(roots_from(128, "", "fatal: bad config").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_trash_entry_that_cannot_be_deleted_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let s = Scratch::new();
        fs::create_dir_all(s.store()).unwrap();
        let t = s.store().join(format!(".trash-{}-x", NOW - 8 * DAY));
        fs::create_dir_all(t.join("inner")).unwrap();
        fs::write(t.join("inner/f"), "x").unwrap();
        fs::set_permissions(t.join("inner"), fs::Permissions::from_mode(0o500)).unwrap();
        // Root writes through 0o500, so there is nothing to test there.
        if fs::write(t.join("inner/probe"), "x").is_ok() {
            fs::set_permissions(t.join("inner"), fs::Permissions::from_mode(0o700)).unwrap();
            return;
        }
        let r = run(&s.env(), NOW, true);
        fs::set_permissions(t.join("inner"), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            r.errors.iter().any(|(n, _)| n.starts_with(".trash-")),
            "{:?}",
            r.errors
        );
    }

    #[test]
    fn a_future_mtime_counts_as_recent() {
        let s = Scratch::new();
        s.dir("odd", 0);
        set(&s.store().join("odd"), NOW + 10 * DAY);
        set(&s.store().join("odd/a.txt"), NOW + 10 * DAY);
        let r = run(&s.env(), NOW, true);
        assert!(r.removed.is_empty());
        assert!(r.rows[0].reason.starts_with("recent"));
    }

    #[test]
    fn the_trash_clock_is_in_the_name() {
        let s = Scratch::new();
        s.dir("old", 40);
        let t1 = run(&s.env(), NOW, true);
        assert_eq!(removed(&t1), ["old"]);
        let trash = s.store().join(format!(".trash-{NOW}-old"));
        assert!(trash.is_dir());
        fs::create_dir_all(s.store().join(".trash-x")).unwrap();
        fs::create_dir_all(s.store().join(".trash-abc-y")).unwrap();

        run(&s.env(), NOW + DAY, true);
        assert!(trash.is_dir(), "still there after one day");
        let r = run(&s.env(), NOW + 8 * DAY, true);
        assert!(!trash.exists(), "gone after eight");
        assert_eq!(r.purged, 1);
        assert!(s.store().join(".trash-x").is_dir());
        assert!(s.store().join(".trash-abc-y").is_dir());
    }

    #[test]
    fn a_failed_scan_still_purges_the_trash() {
        let s = Scratch::new();
        s.dir("old", 40);
        let t = s.store().join(format!(".trash-{}-x", NOW - 8 * DAY));
        fs::create_dir_all(&t).unwrap();
        let mut env = s.env();
        env.plan_root = Ok(s.0.join("nowhere"));
        let r = run(&env, NOW, true);
        assert!(r.failure.is_some());
        assert!(!t.exists());
        assert!(s.store().join("old").is_dir());
    }

    #[test]
    fn a_dry_run_changes_nothing() {
        let s = Scratch::new();
        s.dir("old", 40);
        let t = s.store().join(format!(".trash-{}-x", NOW - 8 * DAY));
        fs::create_dir_all(&t).unwrap();
        let r = run(&s.env(), NOW, false);
        assert!(r.rows[0].remove);
        assert!(t.exists());
        assert!(s.store().join("old").is_dir());
    }

    #[test]
    fn a_fresh_lock_blocks_the_sweep_and_a_two_hour_one_is_taken_over() {
        let s = Scratch::new();
        s.dir("old", 40);
        let lock = s.0.join("state/attestations.sweeping");
        fs::write(&lock, "").unwrap();
        set(&lock, NOW - 600);
        assert!(sweep_at(&s.env(), NOW).is_none());
        assert!(s.store().join("old").is_dir());
        assert!(lock.exists());

        set(&lock, NOW - 7200);
        let r = sweep_at(&s.env(), NOW).expect("took over");
        assert_eq!(removed(&r), ["old"]);
        assert!(!lock.exists(), "the lock is removed at the end");
        let leftovers: Vec<_> = fs::read_dir(s.0.join("state"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".stale-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn a_stale_rename_of_a_live_lock_backs_off_and_leaves_no_file() {
        let s = Scratch::new();
        let lock = s.0.join("state/attestations.sweeping");
        fs::write(&lock, "").unwrap();
        set(&lock, NOW - 60);
        // What a loser of an earlier race does: it saw an old lock, and by
        // the time it renames, the file there is a live one.
        assert!(take_over(&lock, NOW).is_none());
        assert!(!lock.exists(), "not put back: its holder may be gone");
        let stale = format!("{}.stale-{}", lock.display(), std::process::id());
        assert!(!Path::new(&stale).exists());
    }

    #[test]
    fn the_stamp_limits_the_sweep_to_once_a_day() {
        let s = Scratch::new();
        s.dir("old", 40);
        assert!(sweep_at(&s.env(), NOW).is_some());
        s.dir("old2", 40);
        assert!(sweep_at(&s.env(), NOW + 3600).is_none());
        assert!(s.store().join("old2").is_dir());
        assert!(sweep_at(&s.env(), NOW + DAY + 1).is_some());
    }

    #[test]
    fn a_scan_failure_still_advances_the_stamp() {
        let s = Scratch::new();
        s.dir("old", 40);
        let mut env = s.env();
        env.plan_root = Ok(s.0.join("nowhere"));
        let r = sweep_at(&env, NOW).unwrap();
        assert!(r.failure.is_some());
        assert!(sweep_at(&env, NOW + 60).is_none());
    }

    #[test]
    fn rendering_is_tab_separated_when_piped_and_sanitised() {
        // Rows, not directories: Windows refuses a control character in a
        // file name, and rendering never touches the file system.
        let row = |name: &str, remove: bool, reason: &str| Row {
            name: name.to_string(),
            age: if remove { 40 * DAY } else { DAY },
            bytes: 5,
            remove,
            reason: reason.to_string(),
            cited: false,
        };
        let rows = [
            row("bad\u{1b}[8mname", true, "older than 30d"),
            row("fresh", false, "recent 1d"),
        ];
        let r = Report {
            rows: rows.into(),
            failure: None,
            removed: Vec::new(),
            errors: Vec::new(),
            purged: 0,
        };
        let text = render(&r.rows, false);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("keep\tfresh\t"), "{text}");
        assert!(lines[1].starts_with("remove\tbad\\x1b[8mname\t"), "{text}");
        assert_eq!(lines[1].split('\t').count(), 5);
        let tty = render(&r.rows, true);
        assert!(tty.lines().all(|l| l.chars().count() <= 80), "{tty}");
    }
}
