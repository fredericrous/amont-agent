//! `amont-agent quiet [--max-bytes N] -- <cmd…>`: run a command, keep all of
//! its output in a log file, and print one line on a pass or the failure on a
//! fail, with the command's real exit code.
//!
//! The reason it exists is `cargo test | tail`: a pipeline's status is the last
//! command's, so a failing test run exits 0, and `tail` cuts off the failure it
//! was meant to show. See docs/plans/2026-10-11-test-output-keeps-its-status.md
//! (the capture design is its "Shared rules") and docs/quiet.md.
//!
//! ## Capture
//!
//! The child's stdout and stderr share one pipe, so their order is kept. A
//! reader thread drains it continuously into `<journal dir>/logs/<ts>-<prog>.log.tmp`
//! (0600, so it can be tailed while the run is going) and feeds the excerpt
//! accumulator. When the child exits the file is renamed to `.log`. A storage
//! error (create, write, rename) stops the writing, never the draining: the
//! child must finish, and its status and excerpt are still printed.
//!
//! ## Pipe primitive
//!
//! `std::io::pipe` needs Rust 1.87 and this crate's floor is 1.85, so the pipe
//! is a four-line `extern "C"` wrapper, the way `main` already restores
//! SIGPIPE: std links libc and none of this is worth a dependency.

use std::ffi::{c_int, OsString};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::excerpt::{Accumulator, DEFAULT_BOUND, MAX_BOUND};

const USAGE: &str = "usage: amont-agent quiet [--max-bytes N[k]] -- <cmd…>\n";

/// Logs kept; the newest by name. A `*.tmp` is never counted or deleted.
const KEEP: usize = 50;

/// How long a run goes on before the running line is printed.
const RUNNING_AFTER_MS: u64 = 5000;

mod sys {
    use std::ffi::{c_int, c_short};

    #[repr(C)]
    pub struct PollFd {
        pub fd: c_int,
        pub events: c_short,
        pub revents: c_short,
    }

    #[cfg(target_os = "linux")]
    pub type Nfds = std::ffi::c_ulong;
    #[cfg(not(target_os = "linux"))]
    pub type Nfds = std::ffi::c_uint;

    extern "C" {
        pub fn pipe(fds: *mut c_int) -> c_int;
        pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
        pub fn poll(fds: *mut PollFd, n: Nfds, timeout: c_int) -> c_int;
        pub fn kill(pid: c_int, sig: c_int) -> c_int;
        pub fn signal(sig: c_int, handler: usize) -> usize;
        pub fn getpid() -> c_int;
    }

    pub const F_SETFD: c_int = 2;
    pub const FD_CLOEXEC: c_int = 1;
    pub const POLLIN: c_short = 1;
    pub const SIGINT: c_int = 2;
    pub const SIGPIPE: c_int = 13;
    pub const SIGTERM: c_int = 15;
    pub const SIG_IGN: usize = 1;
}

/// The process group of the child, for the signal handler to forward to.
static PGID: AtomicI32 = AtomicI32::new(0);
/// The last SIGINT/SIGTERM we received, 0 for none.
static RECEIVED: AtomicI32 = AtomicI32::new(0);

extern "C" fn forward(sig: c_int) {
    RECEIVED.store(sig, Ordering::SeqCst);
    let pgid = PGID.load(Ordering::SeqCst);
    if pgid > 0 {
        // `kill` is async-signal-safe; nothing else runs in here.
        unsafe {
            sys::kill(-pgid, sig);
        }
    }
}

struct Parsed {
    bound: usize,
    cmd: Vec<OsString>,
}

fn parse_bytes(text: &str) -> Option<usize> {
    let (digits, mult) = match text.strip_suffix(['k', 'K']) {
        Some(d) => (d, 1024),
        None => (text, 1),
    };
    let n: usize = digits.parse().ok()?;
    let bytes = n.checked_mul(mult)?;
    (1..=MAX_BOUND).contains(&bytes).then_some(bytes)
}

fn parse(args: &[OsString]) -> Result<Parsed, String> {
    let mut bound = DEFAULT_BOUND;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].to_string_lossy().into_owned();
        if a == "--" {
            let cmd = args[i + 1..].to_vec();
            if cmd.is_empty() {
                return Err("no command after `--`".into());
            }
            return Ok(Parsed { bound, cmd });
        }
        let value = if a == "--max-bytes" {
            i += 1;
            match args.get(i) {
                Some(v) => v.to_string_lossy().into_owned(),
                None => return Err("--max-bytes needs a value".into()),
            }
        } else if let Some(v) = a.strip_prefix("--max-bytes=") {
            v.to_string()
        } else {
            return Err(format!("unexpected `{a}` before `--`"));
        };
        bound = parse_bytes(&value).ok_or_else(|| {
            format!(
                "--max-bytes `{value}`: want 1 to {MAX_BOUND} bytes, or with a k suffix (48k at most)"
            )
        })?;
        i += 1;
    }
    Err("missing `--` before the command".into())
}

fn usage_error(why: &str) -> ExitCode {
    eprintln!("amont-agent quiet: {why}");
    eprint!("{USAGE}");
    ExitCode::from(2)
}

pub fn run(args: &[OsString]) -> ExitCode {
    let parsed = match parse(args) {
        Ok(p) => p,
        Err(why) => return usage_error(&why),
    };
    execute(parsed)
}

fn ci() -> bool {
    std::env::var("CI").is_ok_and(|v| !v.is_empty() && v != "false" && v != "0")
}

/// `20261011T101530.123Z`: fixed width, so names sort in time order.
fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let (y, m, d) = crate::civil::civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        now.subsec_millis()
    )
}

fn program_name(argv0: &OsString) -> String {
    let base = Path::new(argv0)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cmd".into());
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    if clean.is_empty() {
        "cmd".into()
    } else {
        clean
    }
}

struct LogFile {
    file: Option<File>,
    tmp: PathBuf,
    done: PathBuf,
    /// Why there is no file to write to, if there is none.
    err: Option<String>,
}

fn open_log(program: &str) -> LogFile {
    let fail = |why: String| LogFile {
        file: None,
        tmp: PathBuf::new(),
        done: PathBuf::new(),
        err: Some(why),
    };
    let Some(base) = crate::journal::dir() else {
        return fail("no config directory".into());
    };
    let dir = base.join("logs");
    if let Err(e) = fs::create_dir_all(&dir) {
        return fail(format!("{}: {e}", dir.display()));
    }
    crate::journal::private(&dir, 0o700);
    let name = format!("{}-{program}", timestamp());
    let done = dir.join(format!("{name}.log"));
    let tmp = dir.join(format!("{name}.log.tmp"));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
    {
        Ok(file) => LogFile {
            file: Some(file),
            tmp,
            done,
            err: None,
        },
        Err(e) => fail(format!("{}: {e}", tmp.display())),
    }
}

/// Keep the newest [`KEEP`] finished logs. `*.tmp` files are in-flight runs
/// (or one a killed wrapper left) and are never touched. Best-effort: a log
/// that cannot be listed or removed is left for the next run, and the result
/// this run prints does not depend on it.
fn prune(dir: &Path) {
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut logs: Vec<PathBuf> = read
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "log"))
        .collect();
    logs.sort();
    let excess = logs.len().saturating_sub(KEEP);
    for old in logs.into_iter().take(excess) {
        let _ = fs::remove_file(old);
    }
}

/// How long a finished run's entry stays for a watcher to see it.
const ENDED_KEEP_MS: u64 = 60_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn active_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("QUIET_ACTIVE_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let state = match std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => {
            PathBuf::from(std::env::var_os("HOME").filter(|d| !d.is_empty())?).join(".local/state")
        }
    };
    Some(state.join("quiet").join("active"))
}

fn pid_alive(pid: i64) -> bool {
    let Ok(pid) = c_int::try_from(pid) else {
        return true;
    };
    if pid <= 0 {
        return true;
    }
    // Signal 0 only checks. EPERM means the process exists; only ESRCH (3)
    // means it is gone.
    let gone = unsafe { sys::kill(pid, 0) } != 0
        && std::io::Error::last_os_error().raw_os_error() == Some(3);
    !gone
}

/// Remove entries of runs that died without finishing, and finished entries
/// older than [`ENDED_KEEP_MS`]. Best-effort; unreadable files stay.
fn sweep_active(dir: &Path, now: u64) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for path in read.filter_map(|e| e.ok().map(|e| e.path())) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let Some(name) = name else { continue };
        if name.starts_with('.') {
            // A temp file of an atomic write whose writer was killed before
            // the rename: nobody else removes it. An hour is far past any
            // write in flight.
            let old = fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() > 3600);
            if old && name.ends_with(".json") {
                let _ = fs::remove_file(&path);
            }
            continue;
        }
        if !name.ends_with(".json") {
            continue;
        }
        let Some(v) = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            continue;
        };
        let stale = match v.get("ended").and_then(|e| e.as_u64()) {
            Some(ended) => now.saturating_sub(ended) > ENDED_KEEP_MS,
            None => v
                .get("pid")
                .and_then(|p| p.as_i64())
                .is_some_and(|p| !pid_alive(p)),
        };
        if stale {
            let _ = fs::remove_file(&path);
        }
    }
}

/// The active-run entry of this run. It is written when the run starts and,
/// when this value is dropped (normal end, early return or panic), rewritten
/// with `exit` and `ended`. Every failure here is dropped: the entry is a
/// courtesy to a watcher and never changes the output or the exit code.
struct ActiveEntry {
    path: Option<PathBuf>,
    base: serde_json::Value,
    exit: Option<i32>,
    /// Where the log went when the run ended (`.log.tmp` renamed to `.log`):
    /// the ended entry names it, so the watcher's last drain reads the real
    /// file. Unset when the rename did not happen.
    final_log: Option<PathBuf>,
}

impl ActiveEntry {
    /// Write this run's entry. A run with no log file (`log` empty: it could
    /// not be created) writes none: there is nothing for a watcher to tail,
    /// and the run's own output still says `log not written`.
    fn start(program: &str, log: &Path) -> ActiveEntry {
        static N: AtomicU32 = AtomicU32::new(0);
        let none = ActiveEntry {
            path: None,
            base: serde_json::Value::Null,
            exit: None,
            final_log: None,
        };
        let Some(dir) = active_dir() else { return none };
        if log.as_os_str().is_empty() {
            return none;
        }
        let Ok(log) = std::path::absolute(log) else {
            return none;
        };
        if fs::create_dir_all(&dir).is_err() {
            return none;
        }
        crate::journal::private(&dir, 0o700);
        let now = now_ms();
        sweep_active(&dir, now);
        let cwd = std::env::current_dir().unwrap_or_default();
        let repo = crate::push_target::toplevel(&cwd).unwrap_or_else(|| cwd.clone());
        let pid = unsafe { sys::getpid() };
        let base = serde_json::json!({
            "v": 1,
            "tool": "amont-agent",
            "session": std::env::var("CLAUDE_CODE_SESSION_ID").ok(),
            "check": program,
            "repo": repo.to_string_lossy(),
            "log": log.to_string_lossy(),
            "pid": pid,
            "started": now,
        });
        let path = dir.join(format!("{pid}-{}.json", N.fetch_add(1, Ordering::SeqCst)));
        let mut entry = ActiveEntry {
            path: Some(path),
            base,
            exit: None,
            final_log: None,
        };
        if entry.write(&entry.base).is_err() {
            entry.path = None;
        }
        entry
    }

    fn write(&self, value: &serde_json::Value) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Err(std::io::Error::other("no active entry"));
        };
        crate::atomic::write_atomic(path, &value.to_string())
    }
}

impl Drop for ActiveEntry {
    fn drop(&mut self) {
        if self.path.is_none() {
            return;
        }
        let mut v = self.base.clone();
        if let Some(o) = v.as_object_mut() {
            o.insert("exit".into(), serde_json::json!(self.exit));
            o.insert("ended".into(), serde_json::json!(now_ms()));
            if let Some(log) = &self.final_log {
                o.insert("log".into(), serde_json::json!(log.to_string_lossy()));
            }
        }
        let _ = self.write(&v);
    }
}

fn make_pipe() -> std::io::Result<(File, OwnedFd)> {
    let mut fds = [0 as c_int; 2];
    if unsafe { sys::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    for fd in fds {
        // So a child never inherits the other end by accident; the ends it is
        // meant to have are dup2'ed onto 1 and 2, which clears the flag.
        unsafe {
            sys::fcntl(fd, sys::F_SETFD, sys::FD_CLOEXEC);
        }
    }
    Ok(unsafe { (File::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// What the reader thread owns while the child runs.
struct Capture {
    file: Option<File>,
    err: Option<String>,
    written: u64,
    #[cfg(feature = "test-hooks")]
    fail_after: Option<u64>,
    acc: Accumulator,
}

impl Capture {
    /// The part of `bytes` the log writer may still write, and whether the
    /// test injection cut it.
    #[cfg(feature = "test-hooks")]
    fn clip<'a>(&self, bytes: &'a [u8]) -> (&'a [u8], bool) {
        match self.fail_after {
            Some(limit) => {
                let left = limit.saturating_sub(self.written) as usize;
                if left < bytes.len() {
                    (&bytes[..left], true)
                } else {
                    (bytes, false)
                }
            }
            None => (bytes, false),
        }
    }

    #[cfg(not(feature = "test-hooks"))]
    fn clip<'a>(&self, bytes: &'a [u8]) -> (&'a [u8], bool) {
        (bytes, false)
    }

    fn take(&mut self, bytes: &[u8]) {
        self.acc.feed(bytes);
        if self.file.is_none() {
            return;
        }
        let (chunk, injected) = self.clip(bytes);
        let Some(file) = self.file.as_mut() else {
            return;
        };
        match file.write_all(chunk) {
            Ok(()) => self.written += chunk.len() as u64,
            Err(e) => self.err = Some(format!("write failed: {e}")),
        }
        if injected && self.err.is_none() {
            self.err = Some(format!(
                "injected write failure after {} bytes",
                self.written
            ));
        }
        if self.err.is_some() {
            // Stop writing; keep draining.
            self.file = None;
        }
    }
}

fn reader(source: File, mut cap: Capture, exited: Arc<AtomicBool>) -> Capture {
    let mut source = source;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let mut p = sys::PollFd {
            fd: source.as_raw_fd(),
            events: sys::POLLIN,
            revents: 0,
        };
        let n = unsafe { sys::poll(&mut p, 1, 50) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if n == 0 {
            // Nothing to read. Once the child is gone, whatever it wrote has
            // been drained; a backgrounded grandchild holding the pipe open
            // must not hold the result hostage.
            if exited.load(Ordering::SeqCst) {
                break;
            }
            continue;
        }
        match source.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => cap.take(&buf[..k]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    cap
}

#[cfg(feature = "test-hooks")]
fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.parse().ok()
}

#[cfg(feature = "test-hooks")]
fn running_after() -> u64 {
    env_u64("AMONT_AGENT_TEST_RUNNING_AFTER_MS").unwrap_or(RUNNING_AFTER_MS)
}

#[cfg(not(feature = "test-hooks"))]
fn running_after() -> u64 {
    RUNNING_AFTER_MS
}

fn exit_code(status: std::process::ExitStatus) -> u8 {
    match (status.code(), status.signal()) {
        (Some(c), _) => c as u8,
        (None, Some(s)) => 128u8.saturating_add(s as u8),
        _ => 1,
    }
}

fn execute(parsed: Parsed) -> ExitCode {
    let started = Instant::now();
    let program = program_name(&parsed.cmd[0]);

    unsafe {
        // EPIPE on our stdout is ignored: keep draining and logging, and exit
        // with the child's code. (`main` restores SIGPIPE's default.)
        sys::signal(sys::SIGPIPE, sys::SIG_IGN);
        let handler = forward as extern "C" fn(c_int) as *const () as usize;
        sys::signal(sys::SIGINT, handler);
        sys::signal(sys::SIGTERM, handler);
    }

    let mut log = open_log(&program);
    let mut active = ActiveEntry::start(&program, &log.tmp);

    let (read_end, write_end) = match make_pipe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("amont-agent quiet: cannot create a pipe: {e}");
            let _ = fs::remove_file(&log.tmp);
            return ExitCode::from(1);
        }
    };
    let write_clone = match write_end.try_clone() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("amont-agent quiet: cannot duplicate the pipe: {e}");
            let _ = fs::remove_file(&log.tmp);
            return ExitCode::from(1);
        }
    };

    let mut command = Command::new(&parsed.cmd[0]);
    command
        .args(&parsed.cmd[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::from(write_clone))
        .stderr(Stdio::from(write_end))
        .process_group(0);
    let spawned = command.spawn();
    // Drops our copies of the write end; the reader sees EOF when the child
    // (and anything it left running) closes theirs.
    drop(command);
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            let _ = fs::remove_file(&log.tmp);
            let name = parsed.cmd[0].to_string_lossy();
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!("amont-agent quiet: {name}: command not found");
                return ExitCode::from(127);
            }
            eprintln!("amont-agent quiet: {name}: {e}");
            return ExitCode::from(126);
        }
    };

    PGID.store(child.id() as i32, Ordering::SeqCst);
    let early = RECEIVED.load(Ordering::SeqCst);
    if early != 0 {
        unsafe {
            sys::kill(-(child.id() as i32), early);
        }
    }

    let exited = Arc::new(AtomicBool::new(false));

    // The running line, only when the run is still going after 5 s, so a quick
    // pass is exactly one line even under `2>&1`.
    {
        let exited = Arc::clone(&exited);
        let after = running_after();
        let line = if log.err.is_none() {
            format!("running {program}, log {}", log.tmp.display())
        } else {
            format!("running {program}")
        };
        std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_millis(after);
            while Instant::now() < until {
                if exited.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if !exited.load(Ordering::SeqCst) {
                eprintln!("{line}");
            }
        });
    }

    let cap = Capture {
        file: log.file.take(),
        err: log.err.clone(),
        written: 0,
        #[cfg(feature = "test-hooks")]
        fail_after: env_u64("AMONT_AGENT_TEST_LOG_FAIL_AFTER"),
        acc: Accumulator::new(parsed.bound),
    };
    let reader_exited = Arc::clone(&exited);
    let handle = std::thread::spawn(move || reader(read_end, cap, reader_exited));

    let status = child.wait();
    exited.store(true, Ordering::SeqCst);
    let mut cap = handle.join().unwrap_or_else(|_| Capture {
        file: None,
        err: Some("capture thread panicked".into()),
        written: 0,
        #[cfg(feature = "test-hooks")]
        fail_after: None,
        acc: Accumulator::new(parsed.bound),
    });
    let elapsed = started.elapsed();

    let status = match status {
        Ok(s) => s,
        Err(e) => {
            eprintln!("amont-agent quiet: waiting for the command failed: {e}");
            return ExitCode::from(1);
        }
    };
    let code = exit_code(status);
    active.exit = Some(i32::from(code));
    let interrupted = RECEIVED.load(Ordering::SeqCst) != 0;

    // Where the log ended up.
    enum Stored {
        Saved(PathBuf),
        RenameFailed(PathBuf),
        NotWritten(String),
    }
    let stored = if let Some(why) = cap.err.take() {
        // The file is a prefix of the run, not the run: do not leave it
        // looking like a log.
        if !log.tmp.as_os_str().is_empty() {
            let _ = fs::remove_file(&log.tmp);
        }
        Stored::NotWritten(why)
    } else {
        match fs::rename(&log.tmp, &log.done) {
            Ok(()) => {
                if let Some(dir) = log.done.parent() {
                    prune(dir);
                }
                active.final_log = Some(log.done.clone());
                Stored::Saved(log.done.clone())
            }
            Err(e) => {
                eprintln!("amont-agent quiet: cannot rename the log: {e}");
                Stored::RenameFailed(log.tmp.clone())
            }
        }
    };

    #[cfg(feature = "test-hooks")]
    if std::env::var_os("AMONT_AGENT_TEST_REPORT_PEAK").is_some() {
        eprintln!("accumulator peak {}", cap.acc.peak());
    }

    let secs = format!("{:.1}s", elapsed.as_secs_f64());
    let mut out = std::io::stdout().lock();
    // Every write below ignores its error. The case meant is EPIPE (a reader
    // such as `| head` gone, SIGPIPE ignored above); any other stdout error is
    // dropped with it, because the exit code must stay the child's whatever
    // happened to our own output. A closed stdout must not change
    // the exit code we return.
    let storage_line = |s: &Stored| match s {
        Stored::Saved(_) => None,
        Stored::RenameFailed(tmp) => Some(format!("log left at {}", tmp.display())),
        Stored::NotWritten(why) => Some(format!("log not written: {why}")),
    };

    if code == 0 && !interrupted {
        match &stored {
            Stored::Saved(p) => {
                let _ = writeln!(out, "ok {secs} log={}", p.display());
            }
            other => {
                let _ = writeln!(out, "ok {secs}");
                if let Some(l) = storage_line(other) {
                    let _ = writeln!(out, "{l}");
                }
            }
        }
        return ExitCode::from(code);
    }

    let _ = writeln!(out, "exit {code} {secs}");
    match (&stored, ci()) {
        (Stored::Saved(p), true) => {
            // CI: the log path vanishes with the runner, so print the log.
            if let Ok(mut f) = File::open(p) {
                let mut buf = vec![0u8; 64 * 1024];
                while let Ok(n) = f.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let _ = out.write_all(&buf[..n]);
                }
            }
        }
        (Stored::Saved(p), false) => {
            let text = cap.acc.finish(&p.display().to_string());
            let _ = out.write_all(text.as_bytes());
        }
        (Stored::RenameFailed(p), _) => {
            let text = cap.acc.finish(&p.display().to_string());
            let _ = out.write_all(text.as_bytes());
        }
        (Stored::NotWritten(_), _) => {
            let text = cap.acc.finish("(not written)");
            let _ = out.write_all(text.as_bytes());
        }
    }
    match &stored {
        Stored::Saved(p) if interrupted => {
            let _ = writeln!(out, "interrupted, full log: {}", p.display());
        }
        Stored::Saved(p) => {
            let _ = writeln!(out, "full log: {}", p.display());
        }
        other => {
            if interrupted {
                let _ = writeln!(out, "interrupted");
            }
            if let Some(l) = storage_line(other) {
                let _ = writeln!(out, "{l}");
            }
        }
    }
    let _ = out.flush();
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_bytes_range() {
        assert_eq!(parse_bytes("48k"), Some(48 * 1024));
        assert_eq!(parse_bytes("1"), Some(1));
        assert_eq!(parse_bytes("0"), None);
        assert_eq!(parse_bytes("49k"), None);
        assert_eq!(parse_bytes("x"), None);
        assert_eq!(parse_bytes("-1"), None);
    }

    #[test]
    fn missing_dashes_is_usage() {
        let a = |s: &[&str]| s.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(parse(&a(&["true"])).is_err());
        assert!(parse(&a(&["--"])).is_err());
        assert!(parse(&a(&["--max-bytes", "2k", "--", "true"])).is_ok());
    }
}
