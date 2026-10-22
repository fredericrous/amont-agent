//! `amont-agent plan-review record`: what each plan-review reviewer did,
//! kept so the panel can later be judged on its own record
//! (docs/plans/2026-10-11-plan-review-learns-model.md).
//!
//! The ledger is `<journal dir>/plan-review/outcomes.jsonl`, one JSON object
//! per completed reviewer, deduped by agent id. The plan body each round
//! reviewed is kept once under `plan-review/bodies/<sha>.md`, because the
//! plan file keeps changing and the ledger holds only the sha.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::plan_review::{self, Completed, AGENT_PREFIX};

pub const USAGE: &str = "\
usage: amont-agent plan-review record <plan.md> [--transcript <path>]
       amont-agent plan-review stats [--json] [--model <id>]

Append one row per completed plan-review reviewer of the plan to
~/.claude/amont-agent/plan-review/outcomes.jsonl, skipping agents already
recorded (by agent id), and save each reviewed plan body once under
plan-review/bodies/.

The reviewers come from the session transcript: --transcript, or else, of
the transcripts under ~/.claude/projects modified in the last 7 days that
reviewed this plan, the one that reviewed its current body, then the newest.

stdin: a JSON array of findings (empty stdin is `[]`):
  {\"role\": str, \"severity\": \"blocking\"|\"high\"|\"medium\"|\"low\",
   \"disposition\": \"accepted\"|\"rejected\"|\"decision\",
   \"n\": number (optional), \"section\": str (optional), \"area\": str (optional)}
They are attached to the rows written by this call for the highest round.

stats: one row per (role, model) of the plan-review and implementation-review
ledgers: reviews, findings by severity, accepted findings by severity (plan
review `accepted`, implementation review `fixed`), unresolved findings and
accepted-plus-unresolved high-or-blocking findings per review. A parsed
finding is `accepted` when a merge row of its body applied `role#n`, else
`unresolved`; a later disposition row wins. A row with 20 or more reviews and
under 0.05 per review is marked `retire-candidate` (ADR-0042). Stats also list
reviews that predate finding parsing, seats whose verdict has no parsed
finding, and plans whose hook record was skipped. --json prints one array;
--model keeps one model.

Exit 0 when the ledger is up to date, 1 when a reviewer has no agent id, no
transcript names the plan or the ledger cannot be written, 2 on a usage error
(including malformed findings).
";

const SCHEMA: &str = "amont-agent-review-outcome-v1";
const MERGE_SCHEMA: &str = "amont-agent-review-merge-v1";
const DISPOSITION_SCHEMA: &str = "amont-agent-review-disposition-v1";
pub(crate) const SEVERITIES: [&str; 4] = ["blocking", "high", "medium", "low"];
const DISPOSITIONS: [&str; 3] = ["accepted", "rejected", "decision"];
const LOCK_FILE: &str = "outcomes.jsonl.lock";
const WEEK: u64 = 7 * 86_400;
const BODY_TTL: u64 = 90 * 86_400;

/// Runs the verb and returns what it says on stdout, for `main` to print:
/// stdout is written only from there (`only_the_emitter_writes_to_stdout`).
pub fn run(args: &[OsString]) -> (String, ExitCode) {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return (USAGE.to_string(), ExitCode::SUCCESS);
    }
    let usage = |why: &str| {
        eprint!("amont-agent: {why}\n\n{USAGE}");
        (String::new(), ExitCode::from(2))
    };
    let mut it = args.iter();
    match it.next().map(String::as_str) {
        Some("record") => {}
        Some("stats") => return run_stats(&args[1..]),
        _ => return usage("plan-review takes one verb: record or stats"),
    }
    let mut plan: Option<&String> = None;
    // The last `--transcript` wins, in either form.
    let mut transcript: Option<String> = None;
    let mut operands_only = false;
    while let Some(a) = it.next() {
        if !operands_only && a == "--" {
            operands_only = true;
        } else if !operands_only && a == "--transcript" {
            let Some(v) = it.next() else {
                return usage("--transcript needs a path");
            };
            transcript = Some(v.clone());
        } else if let Some(v) = a.strip_prefix("--transcript=").filter(|_| !operands_only) {
            transcript = Some(v.to_string());
        } else if (!operands_only && a.starts_with('-') && a != "-") || plan.is_some() {
            return usage(&format!("unexpected argument `{a}`"));
        } else {
            plan = Some(a);
        }
    }
    let Some(plan) = plan else {
        return usage("record needs a plan file");
    };
    let mut stdin = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin) {
        return usage(&format!("stdin cannot be read: {e}"));
    }
    let findings = match parse_findings(&stdin) {
        Ok(f) => f,
        Err(why) => return usage(&why),
    };
    match record(Path::new(plan), transcript.map(PathBuf::from), &findings) {
        // A status line, not data: stderr (cli.basics.stdout-is-data-stderr-is-talk).
        Ok((said, code)) => {
            eprint!("{said}");
            (String::new(), ExitCode::from(code))
        }
        Err(why) => {
            eprintln!("amont-agent: {why}");
            (String::new(), ExitCode::from(1))
        }
    }
}

/// The findings on stdin, checked: an unknown severity would poison every
/// later aggregate, so it is refused here rather than stored.
fn parse_findings(text: &str) -> Result<Vec<serde_json::Value>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("stdin is not JSON: {e}"))?;
    let Some(list) = v.as_array() else {
        return Err("stdin must be a JSON array of findings".into());
    };
    for (i, f) in list.iter().enumerate() {
        let s = |k: &str| f.get(k).and_then(|v| v.as_str());
        if s("role").is_none() {
            return Err(format!("finding {i}: `role` must be a string"));
        }
        for (key, allowed) in [
            ("severity", &SEVERITIES[..]),
            ("disposition", &DISPOSITIONS[..]),
        ] {
            match s(key) {
                Some(v) if allowed.contains(&v) => {}
                other => {
                    return Err(format!(
                        "finding {i}: unknown {key} {}; one of {}",
                        other.unwrap_or("(missing)"),
                        allowed.join("|")
                    ))
                }
            }
        }
        if f.get("n").is_some_and(|v| v.as_u64().is_none()) {
            return Err(format!("finding {i}: `n` must be a number"));
        }
        for key in ["section", "area"] {
            if f.get(key).is_some_and(|v| !v.is_string() && !v.is_null()) {
                return Err(format!("finding {i}: `{key}` must be a string"));
            }
        }
    }
    Ok(list.clone())
}

/// One completed reviewer of the plan, with the body it reviewed.
pub(crate) struct Launch {
    pub(crate) role: String,
    pub(crate) sha: String,
    pub(crate) done: Completed,
}

/// The completed launches of `transcript` that reviewed `plan`, in launch
/// order: a launch whose prompt carries several blocks for the plan is one
/// reviewer. The rounds a reviewer was resumed for by `SendMessage` count as
/// launches of their own: `plan-apply` reads a round-2 reviewer's patches
/// from its resume, and `record` writes a row per (agent, body).
pub(crate) fn launches_resumable(transcript: &Path, plan: &str) -> std::io::Result<Vec<Launch>> {
    collect(transcript, plan, |r, p| {
        plan_review::completed_agents_resumable(r, p)
    })
}

type Reader = fn(BufReader<std::fs::File>, &str) -> Vec<Completed>;

fn collect(transcript: &Path, plan: &str, completed: Reader) -> std::io::Result<Vec<Launch>> {
    let reader = BufReader::new(std::fs::File::open(transcript)?);
    let mut out = Vec::new();
    for done in completed(reader, AGENT_PREFIX) {
        let found = plan_review::parse_blocks(&done.agent, &done.prompt)
            .into_iter()
            .find(|b| b.path == plan);
        if let Some(b) = found {
            out.push(Launch {
                role: b.role,
                sha: b.sha,
                done,
            });
        }
    }
    Ok(out)
}

/// The transcript to read when none is named: of those touched in the last
/// week that reviewed this plan, the one that reviewed its CURRENT body,
/// then the most recently written. The body comes first because a
/// long-lived session still being appended to can have reviewed an older
/// body days ago; its mtime says nothing about when the review ran.
pub(crate) fn discover(
    plan: &str,
    current: &str,
    load: fn(&Path, &str) -> std::io::Result<Vec<Launch>>,
) -> Option<(PathBuf, Vec<Launch>)> {
    let projects = amont_agent_guard::settings::config_dir()?.join("projects");
    let now = std::time::SystemTime::now();
    let mut best: Option<((bool, std::time::SystemTime), PathBuf, Vec<Launch>)> = None;
    let dirs = match std::fs::read_dir(&projects) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("amont-agent: cannot read {}: {e}", projects.display());
            return None;
        }
    };
    for dir in dirs.flatten() {
        let files = match std::fs::read_dir(dir.path()) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("amont-agent: skipped {}: {e}", dir.path().display());
                continue;
            }
        };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(modified) = f.metadata().ok().and_then(|m| m.modified().ok()) else {
                continue;
            };
            let age = now.duration_since(modified).map_or(0, |d| d.as_secs());
            // Nothing can outrank a best that reviewed the current body and
            // is newer: skip the read.
            if age > WEEK
                || best
                    .as_ref()
                    .is_some_and(|(k, _, _)| *k >= (true, modified))
            {
                continue;
            }
            let found = match load(&path, plan) {
                Ok(f) => Ok(f),
                Err(e) => {
                    eprintln!("amont-agent: skipped {}: {e}", path.display());
                    Err(e)
                }
            };
            if let Ok(found) = found {
                let key = (found.iter().any(|l| l.sha == current), modified);
                if !found.is_empty() && best.as_ref().is_none_or(|(k, _, _)| key > *k) {
                    best = Some((key, path, found));
                }
            }
        }
    }
    best.map(|(_, p, l)| (p, l))
}

/// The verdict a reviewer closed with: the first of the three words after
/// `Verdict`. `approve-with-changes` is tried before `approve`, which it
/// starts with.
fn verdict(result: &str) -> Option<&'static str> {
    let lower = result.to_lowercase();
    let after = &lower[lower.find("verdict")? + "verdict".len()..];
    (0..after.len())
        .filter(|&i| after.is_char_boundary(i))
        .find_map(|i| {
            ["rework", "approve-with-changes", "approve"]
                .into_iter()
                .find(|w| after[i..].starts_with(w))
        })
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// How serious a reviewer called a finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Severity {
    Blocking,
    High,
    Medium,
    Low,
}

impl Severity {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Severity::Blocking => "blocking",
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
        }
    }
}

/// The numbered findings of a reviewer's result: each `N. [severity]` line
/// between `Findings:` and `What I would measure:` / `Patches:` (or the end),
/// a leading `**` or spaces tolerated, with the count of numbered lines that
/// did not match. A line is skipped, never guessed.
pub(crate) fn findings_of(result: &str) -> (Vec<(u32, Severity)>, usize) {
    let heading = |l: &str| l.trim().trim_matches('*').trim().to_string();
    let mut found = Vec::new();
    let mut unparsed = 0;
    let mut inside = false;
    for line in result.lines() {
        let h = heading(line);
        if !inside {
            inside = h.starts_with("Findings:");
            continue;
        }
        if h.starts_with("What I would measure:") || h.starts_with("Patches:") {
            break;
        }
        let body = line.trim_start().trim_start_matches('*').trim_start();
        let digits = body.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 || !body[digits..].starts_with('.') {
            continue;
        }
        let parsed = body[..digits].parse::<u32>().ok().zip(
            body[digits + 1..]
                .trim_start()
                // `1. **[high]** …`: bold after the number, as reviewers write it.
                .trim_start_matches('*')
                .strip_prefix('[')
                .and_then(|r| r.split_once(']'))
                .and_then(|(w, _)| match w.trim().to_lowercase().as_str() {
                    "blocking" => Some(Severity::Blocking),
                    "high" => Some(Severity::High),
                    "medium" => Some(Severity::Medium),
                    "low" => Some(Severity::Low),
                    _ => None,
                }),
        );
        match parsed {
            Some(f) => found.push(f),
            None => unparsed += 1,
        }
    }
    (found, unparsed)
}

fn json_list<'a>(items: impl IntoIterator<Item = &'a String>) -> serde_json::Value {
    serde_json::Value::Array(items.into_iter().map(|s| s.as_str().into()).collect())
}

/// An exclusive lock on the ledger's lock file, held until dropped. Two
/// `record` calls on one transcript would otherwise both read "not yet
/// recorded" and append the same reviewers twice.
pub(crate) struct Lock {
    _held: std::fs::File,
}

#[cfg(unix)]
pub(crate) fn lock(path: &Path) -> std::io::Result<Lock> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn flock(fd: i32, op: i32) -> i32;
    }
    const LOCK_EX: i32 = 2;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    amont_agent_guard::journal::private(path, 0o600);
    if unsafe { flock(f.as_raw_fd(), LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Lock { _held: f })
}

// holds-until: two `record` calls run at once on Windows. No lock is taken
// there: `File::lock` needs Rust 1.89, above the 1.85 floor, and plan
// reviews run on unix machines. Ends when the floor reaches 1.89.
#[cfg(not(unix))]
pub(crate) fn lock(path: &Path) -> std::io::Result<Lock> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(Lock { _held: f })
}

/// The longest the PostToolUse hook waits for the ledger lock.
const HOOK_LOCK_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);
/// Pause between two non-blocking lock attempts.
const HOOK_LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(25);

/// Like [`lock`], but takes the lock with `LOCK_NB` and retries for at most
/// `budget`; `TimedOut` when another process still holds it.
#[cfg(unix)]
pub(crate) fn lock_within(path: &Path, budget: std::time::Duration) -> std::io::Result<Lock> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn flock(fd: i32, op: i32) -> i32;
    }
    const LOCK_EX_NB: i32 = 2 | 4;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    amont_agent_guard::journal::private(path, 0o600);
    let deadline = std::time::Instant::now() + budget;
    loop {
        if unsafe { flock(f.as_raw_fd(), LOCK_EX_NB) } == 0 {
            return Ok(Lock { _held: f });
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::WouldBlock {
            return Err(e);
        }
        if std::time::Instant::now() >= deadline {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(HOOK_LOCK_RETRY);
    }
}

// holds-until: see `lock`; no lock is taken off unix, so there is nothing to wait for.
#[cfg(not(unix))]
pub(crate) fn lock_within(path: &Path, _budget: std::time::Duration) -> std::io::Result<Lock> {
    lock(path)
}

const HOOK_RULE: &str = "plan-review-record";

fn hook_note(outcome: &str, session: &str, excerpt: &str) {
    amont_agent_guard::journal::record(&amont_agent_guard::journal::Entry {
        rule: HOOK_RULE,
        stance: "-",
        outcome,
        session,
        repo: "-",
        mode: "-",
        excerpt,
    });
}

/// PostToolUse on `ExitPlanMode`, approved or rejected: record the plan's
/// reviewers. It never fails the call: an error is journalled (`error`), and
/// a ledger lock still held after 2 s is journalled as one `skipped`, with
/// the body sha and path `plan-review stats` lists until a row for that body
/// exists (a manual `plan-review record` catches up).
pub fn on_plan_answered(ev: &amont_agent_guard::payload::PlanAnswered) {
    let Some(plan) = &ev.plan_file else { return };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let store = prepare_store()?;
        let held = match lock_within(&store.join(LOCK_FILE), HOOK_LOCK_BUDGET) {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                let sha = std::fs::read_to_string(plan)
                    .map(|t| crate::plan_text::body_sha(&t))
                    .unwrap_or_default();
                let real = plan_review::real(plan);
                hook_note(
                    "skipped",
                    &ev.session,
                    &format!("{} {}", sha.get(..12).unwrap_or("-"), real.display()),
                );
                return Ok(());
            }
            Err(e) => return Err(format!("cannot lock the ledger: {e}")),
        };
        let done = record_locked(&store, plan, ev.transcript.clone(), &[]).map(|_| ());
        drop(held);
        done
    }));
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(why)) => hook_note("error", &ev.session, &why),
        Err(_) => hook_note("error", &ev.session, "record panicked"),
    }
}

/// Options for a file only its owner reads: the ledger and the bodies hold
/// what reviewers said about private plans.
pub(crate) fn private_options() -> std::fs::OpenOptions {
    let o = std::fs::OpenOptions::new();
    #[cfg(unix)]
    let o = {
        use std::os::unix::fs::OpenOptionsExt;
        let mut o = o;
        o.mode(0o600);
        o
    };
    o
}

/// The ledger, opened for appending; created when missing.
pub(crate) fn open_ledger(path: &Path) -> std::io::Result<std::fs::File> {
    let f = private_options().append(true).create(true).open(path)?;
    amont_agent_guard::journal::private(path, 0o600);
    Ok(f)
}

/// A saved body, created once: `AlreadyExists` when an earlier call wrote it.
fn create_body(path: &Path) -> std::io::Result<std::fs::File> {
    private_options().write(true).create_new(true).open(path)
}

/// The rows of a ledger, in file order; a missing file is no rows. A torn
/// line (a crash mid-append) is skipped, not fatal. The one reader of every
/// outcomes ledger.
pub fn read_rows(path: &Path) -> std::io::Result<Vec<serde_json::Value>> {
    let text = match std::fs::read(path) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

type Recorded = (BTreeSet<(String, String)>, Option<String>);

/// What the ledger already holds: the (review agent id, body sha) pairs on
/// record, and the round-1 body an earlier call recorded for `plan`, if any.
/// A torn line (a crash mid-append) is skipped, not fatal.
fn recorded(path: &Path, plan: &str) -> std::io::Result<Recorded> {
    let rows = read_rows(path)?;
    // Only review rows: a merge row (`plan-apply`) has an `agent_id` too, and
    // must not make a reviewer look recorded.
    let ids = rows
        .iter()
        .filter(|v| v.get("schema").and_then(|s| s.as_str()) == Some(SCHEMA))
        .filter_map(|v| {
            Some((
                v.get("agent_id")?.as_str()?.to_string(),
                v.get("body_sha")?.as_str()?.to_string(),
            ))
        })
        .collect();
    let first = rows
        .iter()
        .filter(|v| v.get("plan_path").and_then(|p| p.as_str()) == Some(plan))
        .find_map(|v| v.get("first_round_sha")?.as_str().map(str::to_string));
    Ok((ids, first))
}

/// The plan-review store under the journal directory, with its `bodies`
/// directory, created private.
fn prepare_store() -> Result<PathBuf, String> {
    let store = amont_agent_guard::journal::dir()
        .ok_or("no home directory, so the ledger cannot be located")?
        .join("plan-review");
    let bodies = store.join("bodies");
    plan_review::private_dir(&store)
        .and_then(|()| plan_review::private_dir(&bodies))
        .ok_or_else(|| format!("cannot create {}", bodies.display()))?;
    Ok(store)
}

fn record(
    plan: &Path,
    transcript: Option<PathBuf>,
    findings: &[serde_json::Value],
) -> Result<(String, u8), String> {
    let store = prepare_store()?;
    let _lock = lock(&store.join(LOCK_FILE)).map_err(|e| format!("cannot lock the ledger: {e}"))?;
    record_locked(&store, plan, transcript, findings)
}

/// `record` with the ledger lock already held by the caller.
fn record_locked(
    store: &Path,
    plan: &Path,
    transcript: Option<PathBuf>,
    findings: &[serde_json::Value],
) -> Result<(String, u8), String> {
    let text = std::fs::read_to_string(plan)
        .map_err(|e| format!("cannot read {}: {e}", plan.display()))?;
    let real = plan_review::real(plan);
    let real = real.to_string_lossy().into_owned();
    let plan_sha = crate::plan_text::body_sha(&text);
    let (transcript, found) = match transcript {
        Some(t) => {
            let l = launches_resumable(&t, &real)
                .map_err(|e| format!("cannot read transcript {}: {e}", t.display()))?;
            (t, l)
        }
        None => discover(&real, &plan_sha, launches_resumable).ok_or_else(|| {
            format!(
                "no transcript from the last 7 days has a completed plan-review of {real}; name one with --transcript"
            )
        })?,
    };
    if found.is_empty() {
        return Err(format!(
            "{} has no completed plan-review of {real}",
            transcript.display()
        ));
    }
    // A round is a body: numbered in the order bodies first appear.
    let mut shas: Vec<&str> = Vec::new();
    for l in &found {
        if !shas.contains(&l.sha.as_str()) {
            shas.push(&l.sha);
        }
    }
    let round_of = |sha: &str| shas.iter().position(|s| *s == sha).map_or(0, |i| i + 1);
    let top = shas.len();

    let io = |what: &str| {
        let what = what.to_string();
        move |e: std::io::Error| format!("{what}: {e}")
    };
    let bodies = store.join("bodies");
    let ledger = store.join("outcomes.jsonl");
    let (mut seen, first_on_record) =
        recorded(&ledger, &real).map_err(io(&format!("cannot read {}", ledger.display())))?;
    // Round 1 is the first body the LEDGER saw for this plan, so a review
    // that spans sessions keeps one round-1 body to diff against.
    // holds-until: `round` is still counted within this transcript; a plan
    // reviewed across sessions gets a second round 1 until rounds are
    // numbered from the ledger too.
    let before = seen.clone();
    let first_round_sha = first_on_record.unwrap_or_else(|| shas[0].to_string());

    // A row describes the body its reviewer read: the current file when it
    // is that body, else the copy an earlier call saved. A body that was
    // never saved records null, not a guess from another body.
    let features = |sha: &str| {
        let saved = (plan_sha != sha)
            .then(|| {
                let file = bodies.join(format!("{sha}.md"));
                std::fs::read_to_string(&file)
                    .map_err(|e| {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            eprintln!("amont-agent: cannot read {}: {e}", file.display());
                        }
                    })
                    .ok()
            })
            .flatten();
        let body = match &saved {
            Some(s) => s.as_str(),
            None if plan_sha == sha => text.as_str(),
            None => return serde_json::Value::Null,
        };
        let meta = plan_review::meta(body);
        serde_json::json!({
            "areas": meta.as_ref().ok().and_then(plan_review::areas_of).map(|a| json_list(&a)).unwrap_or_else(|| serde_json::json!([])),
            "repos": meta.as_ref().map(|m| json_list(&m.repos)).unwrap_or_else(|_| serde_json::json!([])),
            "body_lines": crate::plan_text::canonical(body).lines().count(),
        })
    };

    let at = now();
    let mut buffer = String::new();
    let mut written: BTreeSet<usize> = BTreeSet::new();
    let mut written_roles: BTreeSet<&str> = BTreeSet::new();
    let mut count = 0;
    // The findings on stdin are this call's merge: they belong to the
    // highest round it writes, which is not always the transcript's top.
    let attach_round = found
        .iter()
        .filter(|l| {
            l.done
                .agent_id
                .as_ref()
                .is_some_and(|id| !seen.contains(&(id.clone(), l.sha.clone())))
        })
        .map(|l| round_of(&l.sha))
        .max();
    for l in &found {
        let Some(id) = l.done.agent_id.as_ref() else {
            continue;
        };
        if !seen.insert((id.clone(), l.sha.clone())) {
            continue;
        }
        let round = round_of(&l.sha);
        let attach = Some(round) == attach_round;
        let mine: Vec<&serde_json::Value> = findings
            .iter()
            .filter(|f| attach && f.get("role").and_then(|r| r.as_str()) == Some(&l.role))
            .collect();
        // Stdin wins for a role it names; else what the reviewer wrote.
        let (listed, unparsed) = if mine.is_empty() {
            let (parsed, unparsed) = findings_of(&l.done.result);
            let rows: Vec<serde_json::Value> = parsed
                .iter()
                .map(|(n, sev)| serde_json::json!({"n": n, "severity": sev.as_str()}))
                .collect();
            (rows, Some(unparsed))
        } else {
            (mine.iter().map(|f| (*f).clone()).collect(), None)
        };
        let row = serde_json::json!({
            "schema": SCHEMA,
            "recorded_at": at,
            "role": l.role,
            "agent_id": id,
            "model": l.done.model,
            "tokens": l.done.tokens,
            "seconds": l.done.duration_ms.map(|ms| ms as f64 / 1000.0),
            "round": round,
            "body_sha": l.sha,
            "first_round_sha": first_round_sha,
            "plan_path": real,
            "plan": features(&l.sha),
            "verdict": verdict(&l.done.result),
            "remit": json_list(&plan_review::remit(&l.role)),
            "findings": listed,
        });
        let mut row = row;
        if let Some(u) = unparsed {
            row["findings_unparsed"] = u.into();
        }
        buffer.push_str(&row.to_string());
        buffer.push('\n');
        written.insert(round);
        if attach {
            written_roles.insert(l.role.as_str());
        }
        count += 1;
    }
    if count > 0 {
        let mut f =
            open_ledger(&ledger).map_err(io(&format!("cannot open {}", ledger.display())))?;
        f.write_all(buffer.as_bytes())
            .and_then(|()| f.flush())
            .map_err(io(&format!("cannot write {}", ledger.display())))?;
    }

    // A stdin finding no new row took is a disposition for a reviewer already
    // on record: appended, never deduped.
    let mut dispositions = 0;
    if !findings.is_empty() {
        let mut buffer = String::new();
        for f in findings {
            let Some(role) = f.get("role").and_then(|r| r.as_str()) else {
                continue;
            };
            let taken = written_roles.contains(role);
            let target = found
                .iter()
                .filter(|l| l.role == role && l.done.agent_id.is_some())
                .filter(|l| {
                    let id = l.done.agent_id.clone().unwrap_or_default();
                    before.contains(&(id, l.sha.clone()))
                })
                .max_by_key(|l| round_of(&l.sha));
            let (false, Some(l)) = (taken, target) else {
                continue;
            };
            let row = serde_json::json!({
                "schema": DISPOSITION_SCHEMA,
                "recorded_at": at,
                "agent_id": l.done.agent_id,
                "body_sha": l.sha,
                "role": role,
                "n": f.get("n").cloned().unwrap_or(serde_json::Value::Null),
                "disposition": f.get("disposition"),
            });
            buffer.push_str(&row.to_string());
            buffer.push('\n');
            dispositions += 1;
        }
        if dispositions > 0 {
            let mut f =
                open_ledger(&ledger).map_err(io(&format!("cannot open {}", ledger.display())))?;
            f.write_all(buffer.as_bytes())
                .and_then(|()| f.flush())
                .map_err(io(&format!("cannot write {}", ledger.display())))?;
        }
    }

    for &round in &written {
        let sha = shas[round - 1];
        if plan_sha != sha {
            eprintln!("body of round {round} not saved: the plan changed since it was reviewed");
            continue;
        }
        match create_body(&bodies.join(format!("{sha}.md"))) {
            Ok(mut f) => {
                f.write_all(text.as_bytes())
                    .map_err(io("cannot save the plan body"))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("cannot save the plan body: {e}")),
        }
    }
    prune(&bodies);

    // Reported after the write: the rows that resolved are already safe.
    let mut missing = false;
    for l in found.iter().filter(|l| l.done.agent_id.is_none()) {
        eprintln!(
            "amont-agent: no agent id for {} in {}",
            l.role,
            transcript.display()
        );
        missing = true;
    }

    let (round, sha) = match written.iter().next_back() {
        Some(&r) => (r, shas[r - 1]),
        None => (top, shas[top - 1]),
    };
    let mut said = if count > 0 {
        format!("wrote {count} rows (round {round}, plan {})\n", &sha[..12])
    } else if missing {
        format!("no rows written for round {round} of {}\n", &sha[..12])
    } else {
        format!(
            "already recorded round {round} of {}; nothing written\n",
            &sha[..12]
        )
    };
    if dispositions > 0 {
        said.push_str(&format!("wrote {dispositions} disposition rows\n"));
    }
    Ok((said, u8::from(missing)))
}

/// Saved bodies are for looking back at a recent review, not an archive.
/// A failure is said on stderr and never fails the call: the rows are
/// already written, and a body left over is harmless.
fn prune(dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("amont-agent: bodies in {} not pruned: {e}", dir.display());
            return;
        }
    };
    let now = std::time::SystemTime::now();
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|d| d.as_secs() > BODY_TTL);
        if old {
            if let Err(why) = std::fs::remove_file(e.path()) {
                eprintln!("amont-agent: body {} not pruned: {why}", e.path().display());
            }
        }
    }
}

pub(crate) const IMPL_SCHEMA: &str = "amont-agent-impl-review-outcome-v1";
const IMPL_ROLE: &str = "implementation-review";
const RETIRE_MIN_REVIEWS: u64 = 20;
const RETIRE_BELOW: f64 = 0.05;

/// One (role, model) line of `plan-review stats`.
#[derive(Default, Debug, PartialEq)]
pub struct Stat {
    pub reviews: u64,
    pub found: [u64; 4],
    pub accepted: [u64; 4],
    /// Parsed findings no disposition row and no merge row settled.
    pub unresolved: [u64; 4],
    /// Reviews recorded before findings were parsed: `findings` empty and no
    /// `findings_unparsed`.
    pub predating: u64,
    /// Reviews that closed approve-with-changes or rework with no parsed
    /// finding, though the parser ran.
    pub verdict_without_findings: u64,
}

impl Stat {
    /// Accepted blocking and high findings per review.
    pub fn accepted_per_review(&self) -> f64 {
        if self.reviews == 0 {
            return 0.0;
        }
        (self.accepted[0] + self.accepted[1]) as f64 / self.reviews as f64
    }

    /// Accepted plus unresolved blocking and high findings per review: an
    /// unresolved finding is not evidence that the seat caught nothing.
    pub fn serious_per_review(&self) -> f64 {
        if self.reviews == 0 {
            return 0.0;
        }
        (self.accepted[0] + self.accepted[1] + self.unresolved[0] + self.unresolved[1]) as f64
            / self.reviews as f64
    }

    pub fn retire_candidate(&self) -> bool {
        self.reviews >= RETIRE_MIN_REVIEWS && self.serious_per_review() < RETIRE_BELOW
    }
}

/// What settles a parsed finding, read from the whole ledger so a merge row
/// or a disposition row appended after the review rows still counts.
struct Settled<'a> {
    /// The latest disposition row per (agent id, body sha, n).
    dispositions: std::collections::BTreeMap<(&'a str, &'a str, u64), &'a str>,
    /// The `role#n` labels merged into each body sha, over every merge row.
    applied: std::collections::BTreeMap<&'a str, BTreeSet<&'a str>>,
}

impl<'a> Settled<'a> {
    fn of(rows: &'a [serde_json::Value]) -> Settled<'a> {
        let mut out = Settled {
            dispositions: Default::default(),
            applied: Default::default(),
        };
        for r in rows {
            let text = |k: &str| r.get(k).and_then(|v| v.as_str());
            match text("schema") {
                Some(DISPOSITION_SCHEMA) => {
                    if let (Some(id), Some(sha), Some(n), Some(d)) = (
                        text("agent_id"),
                        text("body_sha"),
                        r.get("n").and_then(|n| n.as_u64()),
                        text("disposition"),
                    ) {
                        out.dispositions.insert((id, sha, n), d);
                    }
                }
                Some(MERGE_SCHEMA) => {
                    if let Some(sha) = text("body_sha_before") {
                        let labels = r
                            .get("applied_labels")
                            .and_then(|l| l.as_array())
                            .into_iter()
                            .flatten()
                            .filter_map(|l| l.as_str());
                        out.applied.entry(sha).or_default().extend(labels);
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The disposition of finding `f` of review row `r`: the one it was
    /// given (stdin), else the latest disposition row, else `accepted` when a
    /// merge of this row's body applied `role#n`, else `unresolved`.
    fn of_finding<'b>(&self, r: &'b serde_json::Value, f: &'b serde_json::Value) -> &'b str
    where
        'a: 'b,
    {
        if let Some(given) = f.get("disposition").and_then(|d| d.as_str()) {
            return given;
        }
        let text = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("");
        let Some(n) = f.get("n").and_then(|n| n.as_u64()) else {
            return "unresolved";
        };
        if let Some(d) = self
            .dispositions
            .get(&(text("agent_id"), text("body_sha"), n))
        {
            return d;
        }
        let label = format!("{}#{n}", text("role"));
        if self
            .applied
            .get(text("body_sha"))
            .is_some_and(|l| l.contains(label.as_str()))
        {
            "accepted"
        } else {
            "unresolved"
        }
    }
}

/// The rows of both ledgers folded to one [`Stat`] per (role, model), in key
/// order. Rows of another schema (merge rows) are not reviews.
pub fn tally(rows: &[serde_json::Value]) -> std::collections::BTreeMap<(String, String), Stat> {
    let settled = Settled::of(rows);
    let mut out: std::collections::BTreeMap<(String, String), Stat> = Default::default();
    for r in rows {
        let text = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("");
        let (role, accepted_word, plan_review) = match text("schema") {
            SCHEMA => (text("role").to_string(), "accepted", true),
            IMPL_SCHEMA => (IMPL_ROLE.to_string(), "fixed", false),
            _ => continue,
        };
        let stat = out.entry((role, text("model").to_string())).or_default();
        stat.reviews += 1;
        let list = r.get("findings").and_then(|f| f.as_array());
        if plan_review && list.is_none_or(|l| l.is_empty()) {
            if r.get("findings_unparsed").is_none() {
                stat.predating += 1;
            } else if ["approve-with-changes", "rework"].contains(&text("verdict")) {
                stat.verdict_without_findings += 1;
            }
        }
        for f in list.into_iter().flatten() {
            let Some(i) = f
                .get("severity")
                .and_then(|s| s.as_str())
                .and_then(|s| SEVERITIES.iter().position(|x| *x == s))
            else {
                continue;
            };
            stat.found[i] += 1;
            let disposition = if plan_review {
                settled.of_finding(r, f)
            } else {
                f.get("disposition").and_then(|d| d.as_str()).unwrap_or("")
            };
            if disposition == accepted_word {
                stat.accepted[i] += 1;
            } else if disposition == "unresolved" {
                stat.unresolved[i] += 1;
            }
        }
    }
    out
}

/// A plan whose `PostToolUse` record was skipped (ledger lock busy), read
/// from the journal.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Skip {
    pub ts: u64,
    /// The first 12 hex digits of the plan's body sha when it was skipped.
    pub sha: String,
    pub plan: String,
}

/// The journalled skips in `journal` text (`F ts rule stance outcome session
/// repo mode arm excerpt`), in file order.
pub fn skips_in(journal: &str) -> Vec<Skip> {
    journal
        .lines()
        .filter_map(|line| {
            let mut f = line.strip_prefix("F ")?.splitn(9, ' ');
            let ts = f.next()?.parse::<u64>().ok()?;
            let (rule, _stance, outcome) = (f.next()?, f.next()?, f.next()?);
            if rule != HOOK_RULE || outcome != "skipped" {
                return None;
            }
            let (_session, _repo, _mode, _arm) = (f.next()?, f.next()?, f.next()?, f.next()?);
            // The excerpt is `<sha12> <plan path>`.
            let (sha, plan) = f.next()?.split_once(' ')?;
            Some(Skip {
                ts,
                sha: sha.to_string(),
                plan: plan.trim().to_string(),
            })
        })
        .collect()
}

/// The skips no later `record` made up for: no review row for the plan holds
/// that body, and none was recorded for the plan since the skip. One per
/// (plan, body), the latest.
pub fn unrecorded_skips(rows: &[serde_json::Value], skips: &[Skip]) -> Vec<Skip> {
    let mut out: std::collections::BTreeMap<(String, String), Skip> = Default::default();
    for s in skips {
        let caught_up = rows.iter().any(|r| {
            let text = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("");
            text("schema") == SCHEMA
                && text("plan_path") == s.plan
                && (text("body_sha").starts_with(&s.sha)
                    || r.get("recorded_at").and_then(|t| t.as_u64()).unwrap_or(0) >= s.ts)
        });
        if !caught_up {
            out.insert((s.plan.clone(), s.sha.clone()), s.clone());
        }
    }
    out.into_values().collect()
}

/// How `plan-review stats` prints.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

/// `plan-review stats`: what the panel's seats caught, per role and model.
pub fn stats_report(rows: &[serde_json::Value], model: Option<&str>, format: Format) -> String {
    stats_report_with(rows, &[], model, format)
}

/// [`stats_report`] with the journalled skips, for the plans a record missed.
pub fn stats_report_with(
    rows: &[serde_json::Value],
    skips: &[Skip],
    model: Option<&str>,
    format: Format,
) -> String {
    let tallied = tally(rows);
    let shown: Vec<_> = tallied
        .iter()
        .filter(|((_, m), _)| model.is_none_or(|want| want == m))
        .collect();
    let missed = unrecorded_skips(rows, skips);
    if format == Format::Json {
        let mut list: Vec<serde_json::Value> = shown
            .iter()
            .map(|((role, model), s)| {
                let by = |a: &[u64; 4]| {
                    serde_json::json!({
                        "blocking": a[0], "high": a[1], "medium": a[2], "low": a[3]
                    })
                };
                serde_json::json!({
                    "role": role,
                    "model": model,
                    "reviews": s.reviews,
                    "findings": by(&s.found),
                    "accepted": by(&s.accepted),
                    "unresolved": by(&s.unresolved),
                    "accepted_high_or_blocking_per_review": s.accepted_per_review(),
                    "serious_per_review": s.serious_per_review(),
                    "predate_change": s.predating,
                    "verdict_without_findings": s.verdict_without_findings,
                    "retire_candidate": s.retire_candidate(),
                })
            })
            .collect();
        list.extend(missed.iter().map(|s| {
            serde_json::json!({"record_skipped": s.plan, "body_sha": s.sha, "skipped_at": s.ts})
        }));
        return format!("{}\n", serde_json::Value::Array(list));
    }
    let mut out = String::new();
    let sev = |a: &[u64; 4]| format!("{}/{}/{}/{}", a[0], a[1], a[2], a[3]);
    for ((role, model), s) in &shown {
        let model = if model.is_empty() { "-" } else { model };
        out.push_str(&format!(
            "{role}\t{model}\treviews={}\tfound={}\taccepted={}",
            s.reviews,
            sev(&s.found),
            sev(&s.accepted),
        ));
        if s.unresolved != [0; 4] {
            out.push_str(&format!("\tunresolved={}", sev(&s.unresolved)));
        }
        out.push_str(&format!("\tserious/review={:.2}", s.serious_per_review()));
        if s.predating > 0 {
            out.push_str(&format!("\tpredate-change={}", s.predating));
        }
        if s.retire_candidate() {
            out.push_str("\tretire-candidate");
        }
        out.push('\n');
    }
    for ((role, model), s) in &shown {
        if s.verdict_without_findings > 0 {
            let model = if model.is_empty() { "-" } else { model };
            out.push_str(&format!(
                "no-parsed-findings\t{role}\t{model}\t{} reviews closed approve-with-changes or rework with 0 parsed findings\n",
                s.verdict_without_findings
            ));
        }
    }
    for s in &missed {
        out.push_str(&format!(
            "record-skipped\t{}\tbody={}\tcatch up: amont-agent plan-review record {}\n",
            s.plan, s.sha, s.plan
        ));
    }
    out
}

/// The ledgers' path under the journal directory, for `store`.
fn ledger_path(store: &str) -> Result<PathBuf, String> {
    Ok(amont_agent_guard::journal::dir()
        .ok_or("no home directory, so the ledger cannot be located")?
        .join(store)
        .join("outcomes.jsonl"))
}

fn run_stats(args: &[String]) -> (String, ExitCode) {
    let usage = |why: &str| {
        eprint!("amont-agent: {why}\n\n{USAGE}");
        (String::new(), ExitCode::from(2))
    };
    let (mut json, mut model) = (false, None::<String>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--json" {
            json = true;
        } else if a == "--model" {
            let Some(v) = it.next() else {
                return usage("--model needs a model id");
            };
            model = Some(v.clone());
        } else if let Some(v) = a.strip_prefix("--model=") {
            model = Some(v.to_string());
        } else {
            return usage(&format!("unexpected argument `{a}`"));
        }
    }
    let mut rows = Vec::new();
    for store in ["plan-review", crate::impl_review_outcomes::STORE] {
        let read = ledger_path(store)
            .and_then(|p| read_rows(&p).map_err(|e| format!("cannot read {}: {e}", p.display())));
        match read {
            Ok(mut r) => rows.append(&mut r),
            Err(why) => {
                eprintln!("amont-agent: {why}");
                return (String::new(), ExitCode::from(1));
            }
        }
    }
    let journal: String = amont_agent_guard::journal::paths()
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect();
    (
        stats_report_with(
            &rows,
            &skips_in(&journal),
            model.as_deref(),
            if json { Format::Json } else { Format::Text },
        ),
        ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_contract_example_parses() {
        let r = "Verdict: approve-with-changes\n\n**Findings:**\n1. [blocking] Section A: x. Edit: y.\n   evidence 2. [low] not a finding\n**2. [high]** B\n3. [low] C\n\n**Patches:**\n````json\n[]\n````\n4. [high] after\n";
        assert_eq!(
            findings_of(r),
            (
                vec![
                    (1, Severity::Blocking),
                    (2, Severity::High),
                    (3, Severity::Low)
                ],
                0
            )
        );
    }

    #[test]
    fn a_malformed_numbered_line_is_counted_not_parsed() {
        let r = "Findings:\n1. [high] ok\n2. [severe] bad\n3. no severity\n";
        assert_eq!(findings_of(r), (vec![(1, Severity::High)], 2));
    }

    /// A reviewer on 2026-10-22 wrote its findings as `1. **[high] text**`:
    /// bold after the number. The live ledger counted both as unparsed.
    #[test]
    fn bold_after_the_number_parses() {
        let r = "Verdict: approve-with-changes\n\n**Findings:**\n\n\
                 1. **[high] A delta round that resumes the reviewer is never recorded.**\n\
                 2. **[high] The disposition is fixed at write time.**\n\n\
                 **What I would measure:**\n";
        assert_eq!(
            findings_of(r),
            (vec![(1, Severity::High), (2, Severity::High)], 0)
        );
    }

    #[test]
    fn none_gives_no_findings() {
        assert_eq!(
            findings_of("Verdict: approve\nFindings:\nNone.\n"),
            (vec![], 0)
        );
        assert_eq!(findings_of("no list at all"), (vec![], 0));
    }

    #[test]
    fn a_numbered_line_after_the_measure_heading_is_ignored() {
        let r = "Findings:\n1. [medium] a\nWhat I would measure:\n2. [high] b\n3. junk\n";
        assert_eq!(findings_of(r), (vec![(1, Severity::Medium)], 0));
    }
}
