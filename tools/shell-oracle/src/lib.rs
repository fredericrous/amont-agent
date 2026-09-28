//! Shared plumbing for the shell-analysis oracle: calling the amont-agent
//! binary, reading its claims, the corpora, and running scripts under real
//! shells with stubbed network clients.
//!
//! The analyzer is exercised ONLY through `amont-agent analyze`; nothing here
//! links the crate. What the harness checks is the published contract in
//! `docs/analysis.md`, read off the JSON the binary prints.

pub mod gen;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

// ── the binary ──────────────────────────────────────────────────────────────

/// The repository root (two levels above this crate).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// `$AMONT_AGENT_BIN`, else `<repo>/target/debug/amont-agent`. Panics with
/// the fix when it is missing: this harness never builds the crate itself.
pub fn amont_bin() -> PathBuf {
    let p = match std::env::var_os("AMONT_AGENT_BIN") {
        Some(p) => PathBuf::from(p),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/amont-agent"),
    };
    assert!(
        p.is_file(),
        "amont-agent binary not found at {}.\n\
         Run `cargo build` at the repository root first, or point \
         AMONT_AGENT_BIN at a built binary.",
        p.display()
    );
    p
}

/// An upper bound as the binary prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upper {
    Finite(u128),
    Saturated,
    Uncapped,
    Unknown,
}

impl Upper {
    pub fn is_infinite(self) -> bool {
        !matches!(self, Upper::Finite(_))
    }
    pub fn plus(self, o: Upper) -> Upper {
        match (self, o) {
            (Upper::Finite(a), Upper::Finite(b)) => Upper::Finite(a.saturating_add(b)),
            // Order of precedence when mixing kinds is irrelevant to the
            // harness: any non-finite is ∞. Keep the "strongest" name.
            (Upper::Uncapped, _) | (_, Upper::Uncapped) => Upper::Uncapped,
            (Upper::Unknown, _) | (_, Upper::Unknown) => Upper::Unknown,
            _ => Upper::Saturated,
        }
    }
    pub fn admits(self, n: u128) -> bool {
        match self {
            Upper::Finite(u) => n <= u,
            _ => true,
        }
    }
}

impl std::fmt::Display for Upper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Upper::Finite(n) => write!(f, "{n}"),
            Upper::Saturated => f.write_str("saturated"),
            Upper::Uncapped => f.write_str("uncapped"),
            Upper::Unknown => f.write_str("unknown"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Contribution {
    pub program: String,
    pub span: (usize, usize),
    pub hosts: Vec<String>,
    pub unresolved: bool,
    pub local: bool,
    pub lower: u128,
    pub upper: Upper,
}

impl Contribution {
    /// The contract's "exact" destination: one host, nothing unresolved,
    /// nothing local. Only such a contribution's lower bound is a claim
    /// about a particular host; `hosts: [a, b]` with lower 1 says "a OR b".
    pub fn only_host(&self) -> Option<&str> {
        (self.hosts.len() == 1 && !self.unresolved && !self.local).then(|| self.hosts[0].as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Analysis {
    pub parsed: bool,
    pub incomplete: Option<String>,
    pub unknown: u64,
    pub contributions: Vec<Contribution>,
    pub raw: String,
}

/// The per-host interval the analysis claims.
///
/// lower: sum of lowers over contributions whose ONLY destination is `host`.
/// upper: sum of uppers over contributions that may reach `host` — those
/// naming it, and every unresolved one (it "may go anywhere").
pub fn claim(a: &Analysis, host: &str) -> (u128, Upper) {
    let mut lo = 0u128;
    let mut up = Upper::Finite(0);
    for c in &a.contributions {
        if c.only_host() == Some(host) {
            lo = lo.saturating_add(c.lower);
        }
        if c.unresolved || c.hosts.iter().any(|h| h == host) {
            up = up.plus(c.upper);
        }
    }
    (lo, up)
}

/// Every host the analysis names.
pub fn named_hosts(a: &Analysis) -> Vec<String> {
    let mut v: Vec<String> = a
        .contributions
        .iter()
        .flat_map(|c| c.hosts.iter().cloned())
        .collect();
    v.sort();
    v.dedup();
    v
}

fn parse_upper(v: &Value) -> Upper {
    match v {
        Value::Number(n) => Upper::Finite(
            n.as_u64()
                .map(u128::from)
                .or_else(|| n.to_string().parse().ok())
                .unwrap_or(u128::MAX),
        ),
        Value::String(s) if s == "saturated" => Upper::Saturated,
        Value::String(s) if s == "uncapped" => Upper::Uncapped,
        _ => Upper::Unknown,
    }
}

fn num(v: &Value) -> u128 {
    match v {
        Value::Number(n) => n
            .as_u64()
            .map(u128::from)
            .or_else(|| n.to_string().parse().ok())
            .unwrap_or(u128::MAX),
        _ => 0,
    }
}

/// Parse the JSON `analyze` prints.
pub fn parse_analysis(raw: &str) -> Result<Analysis, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("invalid JSON: {e}: {raw}"))?;
    let contributions = v["contributions"]
        .as_array()
        .ok_or_else(|| format!("no contributions array: {raw}"))?
        .iter()
        .map(|c| Contribution {
            program: c["program"].as_str().unwrap_or("").to_string(),
            span: (
                c["span"][0].as_u64().unwrap_or(0) as usize,
                c["span"][1].as_u64().unwrap_or(0) as usize,
            ),
            hosts: c["hosts"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|h| h.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            unresolved: c["unresolved"].as_bool().unwrap_or(true),
            local: c["local"].as_bool().unwrap_or(false),
            lower: num(&c["lower"]),
            upper: parse_upper(&c["upper"]),
        })
        .collect();
    Ok(Analysis {
        parsed: v["parsed"]
            .as_bool()
            .ok_or_else(|| format!("no `parsed`: {raw}"))?,
        incomplete: v["incomplete"].as_str().map(str::to_string),
        unknown: v["unknown"].as_u64().unwrap_or(0),
        contributions,
        raw: raw.to_string(),
    })
}

/// The raw result of running `analyze`: exit status, stdout, elapsed.
pub struct Run {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub elapsed: Duration,
}

pub fn analyze_raw(bin: &Path, dialect: &str, src: &str) -> Run {
    let t = Instant::now();
    let out = Command::new(bin)
        .args(["analyze", "--dialect", dialect, src])
        .stdin(Stdio::null())
        .output()
        .expect("spawn amont-agent");
    Run {
        status: out.status,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        elapsed: t.elapsed(),
    }
}

pub fn analyze(bin: &Path, dialect: &str, src: &str) -> Analysis {
    let r = analyze_raw(bin, dialect, src);
    assert!(
        r.status.success(),
        "analyze --dialect {dialect} failed ({}) on {src:?}: {}",
        r.status,
        r.stderr
    );
    parse_analysis(&r.stdout).unwrap_or_else(|e| panic!("{e}"))
}

// ── corpora ─────────────────────────────────────────────────────────────────

/// `src/corpus.rs::unescape`, byte for byte in behaviour: `\n`, `\r`, `\t`,
/// `\\` are escapes; any other backslash is kept verbatim.
pub fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// A command from a corpus, with where it came from.
#[derive(Debug, Clone)]
pub struct Sample {
    pub origin: String,
    pub command: String,
}

/// Every case in `tests/corpus/*.cases` (`label\tcommand`, `#` comments),
/// parsed the way `src/corpus.rs::parse` does.
pub fn corpus_cases() -> Vec<Sample> {
    let dir = repo_root().join("tests/corpus");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "cases"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).expect("read corpus");
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((_label, command)) = line.split_once('\t') else {
                continue;
            };
            out.push(Sample {
                origin: format!("{name}:{}", i + 1),
                command: unescape(command),
            });
        }
    }
    out
}

/// The string literals of `src/analysis/interp/tests.rs` (Appendix C) that
/// are commands: every `"…"` and `r#"…"#` literal that mentions a client,
/// minus `format!` templates. Read from the source at test time, so a case
/// added there is checked here without a copy to keep in step.
pub fn appendix_c() -> Vec<Sample> {
    let path = repo_root().join("src/analysis/interp/tests.rs");
    let src = std::fs::read_to_string(&path).expect("read interp tests");
    let mut out = Vec::new();
    for (line, lit) in rust_string_literals(&src) {
        let is_cmd = ["curl ", "wget ", "gh api", "; do "]
            .iter()
            .any(|k| lit.contains(k));
        let is_template = lit.contains("{body}") || lit.contains("{}") || lit.contains("{i}");
        if is_cmd && !is_template {
            out.push(Sample {
                origin: format!("interp/tests.rs:{line}"),
                command: lit,
            });
        }
    }
    out
}

/// A small scanner for Rust string literals: `"…"` with the common escapes
/// and `r"…"` / `r#"…"#`. Comments are skipped. Enough for a test file.
fn rust_string_literals(src: &str) -> Vec<(usize, String)> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let line_at = |i: usize| src[..i].matches('\n').count() + 1;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'\'' => {
                // char literal or lifetime: skip `'x'`, `'\n'`, `'a`
                if b.get(i + 1) == Some(&b'\\') {
                    i += 4;
                } else if b.get(i + 2) == Some(&b'\'') {
                    i += 3;
                } else {
                    i += 1;
                }
            }
            b'r' if (b.get(i + 1) == Some(&b'"') || b.get(i + 1) == Some(&b'#'))
                && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) =>
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let start = j + 1;
                let close = format!("\"{}", "#".repeat(hashes));
                let end = src[start..]
                    .find(&close)
                    .map(|e| start + e)
                    .unwrap_or(b.len());
                out.push((line_at(i), src[start..end].to_string()));
                i = end + close.len();
            }
            b'"' => {
                let at = i;
                let mut s = String::new();
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        match b.get(i + 1) {
                            Some(b'n') => s.push('\n'),
                            Some(b't') => s.push('\t'),
                            Some(b'r') => s.push('\r'),
                            Some(b'\\') => s.push('\\'),
                            Some(b'"') => s.push('"'),
                            Some(b'\'') => s.push('\''),
                            Some(b'\n') => {
                                // line continuation: skip leading whitespace
                                i += 2;
                                while i < b.len() && b[i].is_ascii_whitespace() {
                                    i += 1;
                                }
                                continue;
                            }
                            Some(&c) => s.push(c as char),
                            None => {}
                        }
                        i += 2;
                    } else {
                        let ch = src[i..].chars().next().unwrap();
                        s.push(ch);
                        i += ch.len_utf8();
                    }
                }
                i += 1;
                out.push((line_at(at), s));
            }
            _ => i += 1,
        }
    }
    out
}

// ── running scripts ─────────────────────────────────────────────────────────

/// A shell to run a script under, and the analyzer dialect that models it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
}

impl Shell {
    pub fn dialect(self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
        }
    }
    fn argv(self) -> &'static [&'static str] {
        match self {
            Shell::Bash => &["bash", "--noprofile", "--norc", "-c"],
            Shell::Zsh => &["zsh", "-f", "-c"],
        }
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// The shell exited by itself (any status).
    Exited(i32),
    /// The stub hit the transfer budget and killed the process group.
    Budget,
    /// The wall-clock limit killed it.
    Timeout,
}

#[derive(Debug, Clone)]
pub struct Observed {
    pub counts: BTreeMap<String, u128>,
    pub end: End,
}

impl Observed {
    pub fn total(&self) -> u128 {
        self.counts.values().sum()
    }
    pub fn finished(&self) -> bool {
        matches!(self.end, End::Exited(_))
    }
}

/// A scratch directory holding the stubs; removed on drop.
pub struct Sandbox {
    pub dir: PathBuf,
    pub bin: PathBuf,
    path_env: String,
    seq: std::cell::Cell<u64>,
}

static SANDBOX_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Sandbox {
    /// `stub`: the `oracle-stub` binary (CARGO_BIN_EXE_oracle-stub).
    pub fn new(stub: &Path) -> Sandbox {
        let n = SANDBOX_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("shell-oracle-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("sandbox dir");
        for p in ["curl", "wget", "gh"] {
            std::os::unix::fs::symlink(stub, bin.join(p)).expect("stub symlink");
        }
        let sys = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
        Sandbox {
            path_env: format!("{}:{sys}", bin.display()),
            dir,
            bin,
            seq: std::cell::Cell::new(0),
        }
    }

    /// Run `script` under `shell`; count the stub's log per host.
    pub fn run(&self, shell: Shell, script: &str, budget: usize, timeout: Duration) -> Observed {
        use std::os::unix::process::CommandExt;
        let k = self.seq.get();
        self.seq.set(k + 1);
        let work = self.dir.join(format!("w{k}"));
        std::fs::create_dir_all(&work).expect("work dir");
        let log = self.dir.join(format!("log{k}"));
        let argv = shell.argv();
        let mut child = Command::new(argv[0])
            .args(&argv[1..])
            .arg(script)
            .env_clear()
            .env("PATH", &self.path_env)
            .env("HOME", &work)
            .env("LC_ALL", "C")
            .env("ORACLE_LOG", &log)
            .env("ORACLE_BUDGET", budget.to_string())
            .current_dir(&work)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("spawn shell");
        let deadline = Instant::now() + timeout;
        let end = loop {
            if let Some(st) = child.try_wait().expect("wait") {
                break match st.code() {
                    Some(c) => End::Exited(c),
                    None => End::Budget,
                };
            }
            if Instant::now() > deadline {
                let _ = Command::new("kill")
                    .args(["-KILL", &format!("-{}", child.id())])
                    .status();
                let _ = child.wait();
                break End::Timeout;
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        // Give a killed group's stragglers a moment, then read.
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let mut counts = BTreeMap::new();
        for line in text.lines() {
            if let Some((_prog, host)) = line.split_once(' ') {
                *counts.entry(host.to_string()).or_insert(0) += 1;
            }
        }
        let lines: usize = text.lines().count();
        let end = match end {
            // Killed by a signal: the stub's budget kill is the only sender.
            End::Budget if lines > budget => End::Budget,
            End::Budget => End::Exited(-1),
            e => e,
        };
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_file(&log);
        Observed { counts, end }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// One disagreement between a run and a claim.
#[derive(Debug, Clone)]
pub struct Mismatch {
    pub dialect: String,
    pub shell: Shell,
    pub host: String,
    pub observed: u128,
    pub end: End,
    pub lower: u128,
    pub upper: Upper,
    pub why: String,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "--dialect {} vs {:?} ({:?}): host {} observed {} claimed [{}, {}] — {}",
            self.dialect,
            self.shell,
            self.end,
            self.host,
            self.observed,
            self.lower,
            self.upper,
            self.why
        )
    }
}

/// Check one run against one analysis. A run that did not finish (budget or
/// timeout) only bounds the upper claim: the lower bound talks about a
/// completed execution.
pub fn check(a: &Analysis, dialect: &str, shell: Shell, obs: &Observed) -> Vec<Mismatch> {
    let mut hosts: Vec<String> = named_hosts(a);
    hosts.extend(obs.counts.keys().cloned());
    hosts.sort();
    hosts.dedup();
    let mut out = Vec::new();
    let mk = |host: &str, lo, up, why: &str| Mismatch {
        dialect: dialect.to_string(),
        shell,
        host: host.to_string(),
        observed: obs.counts.get(host).copied().unwrap_or(0),
        end: obs.end,
        lower: lo,
        upper: up,
        why: why.to_string(),
    };
    for h in &hosts {
        let (lo, up) = claim(a, h);
        let n = obs.counts.get(h).copied().unwrap_or(0);
        if !up.admits(n) {
            out.push(mk(h, lo, up, "observed above the upper bound"));
        }
        if obs.finished() && n < lo {
            out.push(mk(h, lo, up, "observed below the lower bound"));
        }
    }
    if obs.finished() && obs.total() == 0 {
        for c in &a.contributions {
            if c.lower > 0 {
                out.push(mk(
                    c.hosts.first().map(String::as_str).unwrap_or("?"),
                    c.lower,
                    c.upper,
                    "zero requests made, but a contribution claims lower > 0",
                ));
            }
        }
    }
    out
}
