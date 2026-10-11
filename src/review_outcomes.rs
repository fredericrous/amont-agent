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
   \"section\": str (optional), \"area\": str (optional)}
They are attached to the rows written by this call for the highest round.

Exit 0 when the ledger is up to date, 1 when a reviewer has no agent id, no
transcript names the plan or the ledger cannot be written, 2 on a usage error
(including malformed findings).
";

const SCHEMA: &str = "amont-agent-review-outcome-v1";
const SEVERITIES: [&str; 4] = ["blocking", "high", "medium", "low"];
const DISPOSITIONS: [&str; 3] = ["accepted", "rejected", "decision"];
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
    if it.next().map(String::as_str) != Some("record") {
        return usage("plan-review takes one verb: record");
    }
    let mut plan: Option<&String> = None;
    let mut transcript: Option<&String> = None;
    let mut transcript_owned: Option<String> = None;
    let mut operands_only = false;
    while let Some(a) = it.next() {
        if !operands_only && a == "--" {
            operands_only = true;
        } else if !operands_only && a == "--transcript" {
            let Some(v) = it.next() else {
                return usage("--transcript needs a path");
            };
            transcript = Some(v);
        } else if let Some(v) = a.strip_prefix("--transcript=").filter(|_| !operands_only) {
            transcript_owned = Some(v.to_string());
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
    let transcript = transcript_owned.as_ref().or(transcript).map(PathBuf::from);
    match record(Path::new(plan), transcript, &findings) {
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
        for key in ["section", "area"] {
            if f.get(key).is_some_and(|v| !v.is_string() && !v.is_null()) {
                return Err(format!("finding {i}: `{key}` must be a string"));
            }
        }
    }
    Ok(list.clone())
}

/// One completed reviewer of the plan, with the body it reviewed.
struct Launch {
    role: String,
    sha: String,
    done: Completed,
}

/// The completed launches of `transcript` that reviewed `plan`, in launch
/// order, one per agent: a launch whose prompt carries several blocks for
/// the plan is one reviewer.
fn launches(transcript: &Path, plan: &str) -> std::io::Result<Vec<Launch>> {
    let reader = BufReader::new(std::fs::File::open(transcript)?);
    let mut out = Vec::new();
    for done in plan_review::completed_agents(reader, AGENT_PREFIX) {
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
fn discover(plan: &str, current: &str) -> Option<(PathBuf, Vec<Launch>)> {
    let projects = crate::settings::config_dir()?.join("projects");
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
            let found = match launches(&path, plan) {
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

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn json_list<'a>(items: impl IntoIterator<Item = &'a String>) -> serde_json::Value {
    serde_json::Value::Array(items.into_iter().map(|s| s.as_str().into()).collect())
}

/// An exclusive lock on the ledger's lock file, held until dropped. Two
/// `record` calls on one transcript would otherwise both read "not yet
/// recorded" and append the same reviewers twice.
struct Lock {
    _held: std::fs::File,
}

#[cfg(unix)]
fn lock(path: &Path) -> std::io::Result<Lock> {
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
    crate::journal::private(path, 0o600);
    if unsafe { flock(f.as_raw_fd(), LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Lock { _held: f })
}

// holds-until: two `record` calls run at once on Windows. No lock is taken
// there: `File::lock` needs Rust 1.89, above the 1.85 floor, and plan
// reviews run on unix machines. Ends when the floor reaches 1.89.
#[cfg(not(unix))]
fn lock(path: &Path) -> std::io::Result<Lock> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(Lock { _held: f })
}

/// Options for a file only its owner reads: the ledger and the bodies hold
/// what reviewers said about private plans.
fn private_options() -> std::fs::OpenOptions {
    #[allow(unused_mut)] // `mut` is used on unix only
    let mut o = std::fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o
}

/// The ledger, opened for appending; created when missing.
fn open_ledger(path: &Path) -> std::io::Result<std::fs::File> {
    let f = private_options().append(true).create(true).open(path)?;
    crate::journal::private(path, 0o600);
    Ok(f)
}

/// A saved body, created once: `AlreadyExists` when an earlier call wrote it.
fn create_body(path: &Path) -> std::io::Result<std::fs::File> {
    private_options().write(true).create_new(true).open(path)
}

/// What the ledger already holds: the agent ids on record, and the round-1
/// body an earlier call recorded for `plan`, if any. A torn line (a crash
/// mid-append) is skipped, not fatal.
fn recorded(path: &Path, plan: &str) -> std::io::Result<(BTreeSet<String>, Option<String>)> {
    let text = match std::fs::read(path) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(e) => return Err(e),
    };
    let rows: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let ids = rows
        .iter()
        .filter_map(|v| v.get("agent_id")?.as_str().map(str::to_string))
        .collect();
    let first = rows
        .iter()
        .filter(|v| v.get("plan_path").and_then(|p| p.as_str()) == Some(plan))
        .find_map(|v| v.get("first_round_sha")?.as_str().map(str::to_string));
    Ok((ids, first))
}

fn record(
    plan: &Path,
    transcript: Option<PathBuf>,
    findings: &[serde_json::Value],
) -> Result<(String, u8), String> {
    let text = std::fs::read_to_string(plan)
        .map_err(|e| format!("cannot read {}: {e}", plan.display()))?;
    let real = plan_review::real(plan);
    let real = real.to_string_lossy().into_owned();
    let plan_sha = plan_review::body_sha(&text);
    let (transcript, found) = match transcript {
        Some(t) => {
            let l = launches(&t, &real)
                .map_err(|e| format!("cannot read transcript {}: {e}", t.display()))?;
            (t, l)
        }
        None => discover(&real, &plan_sha).ok_or_else(|| {
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
    let store = crate::journal::dir()
        .ok_or("no home directory, so the ledger cannot be located")?
        .join("plan-review");
    let bodies = store.join("bodies");
    plan_review::private_dir(&store)
        .and_then(|()| plan_review::private_dir(&bodies))
        .ok_or_else(|| format!("cannot create {}", bodies.display()))?;
    let ledger = store.join("outcomes.jsonl");
    let _lock = lock(&store.join("outcomes.jsonl.lock")).map_err(io("cannot lock the ledger"))?;
    let (mut seen, first_on_record) =
        recorded(&ledger, &real).map_err(io(&format!("cannot read {}", ledger.display())))?;
    // Round 1 is the first body the LEDGER saw for this plan, so a review
    // that spans sessions keeps one round-1 body to diff against.
    // holds-until: `round` is still counted within this transcript; a plan
    // reviewed across sessions gets a second round 1 until rounds are
    // numbered from the ledger too.
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
            "body_lines": plan_review::canonical(body).lines().count(),
        })
    };

    let at = now();
    let mut buffer = String::new();
    let mut written: BTreeSet<usize> = BTreeSet::new();
    let mut count = 0;
    // The findings on stdin are this call's merge: they belong to the
    // highest round it writes, which is not always the transcript's top.
    let attach_round = found
        .iter()
        .filter(|l| {
            l.done
                .agent_id
                .as_ref()
                .is_some_and(|id| !seen.contains(id))
        })
        .map(|l| round_of(&l.sha))
        .max();
    for l in &found {
        let Some(id) = l.done.agent_id.as_ref() else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let round = round_of(&l.sha);
        let attach = Some(round) == attach_round;
        let mine: Vec<&serde_json::Value> = findings
            .iter()
            .filter(|f| attach && f.get("role").and_then(|r| r.as_str()) == Some(&l.role))
            .collect();
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
            "findings": mine,
        });
        buffer.push_str(&row.to_string());
        buffer.push('\n');
        written.insert(round);
        count += 1;
    }
    if count > 0 {
        let mut f =
            open_ledger(&ledger).map_err(io(&format!("cannot open {}", ledger.display())))?;
        f.write_all(buffer.as_bytes())
            .and_then(|()| f.flush())
            .map_err(io(&format!("cannot write {}", ledger.display())))?;
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
    let said = if count > 0 {
        format!("wrote {count} rows (round {round}, plan {})\n", &sha[..12])
    } else if missing {
        format!("no rows written for round {round} of {}\n", &sha[..12])
    } else {
        format!(
            "already recorded round {round} of {}; nothing written\n",
            &sha[..12]
        )
    };
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
