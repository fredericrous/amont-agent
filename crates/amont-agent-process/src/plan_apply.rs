//! `amont-agent plan-apply`: applies the `Patches:` a plan-review reviewer
//! handed back to the plan file, with no model
//! (docs/plans/2026-10-11-plan-review-edits-apply-without-opus.md).
//!
//! The patches come from the session transcript, as `plan-review record`
//! reads its reviewers; stdin names only the accepted findings. What matches
//! is written, what does not is listed and left for a person or a subagent,
//! and one `amont-agent-review-merge-v1` row per call goes to the plan-review
//! ledger. `--check` verifies the remainder afterwards.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::plan_review::{self, AGENT_PREFIX};
use crate::review_outcomes::{self, Launch};

pub const USAGE: &str = "\
usage: amont-agent plan-apply [--dry-run] [--transcript <path>] <plan.md>
       amont-agent plan-apply --check [--before <sha>] [--transcript <path>] <plan.md>
       amont-agent plan-apply --record-fallback [--transcript <path>] <plan.md>

Apply the Patches that plan-review reviewers returned to the plan, with no
model, and append one `amont-agent-review-merge-v1` row to
~/.claude/amont-agent/plan-review/outcomes.jsonl.

stdin: a JSON array of the accepted findings, each
  {\"role\": str, \"finding\": int}
or, to supply a patch the main session wrote itself,
  {\"role\": str, \"finding\": int,
   \"patch\": {\"old_string\": str, \"new_string\": str}}
Stdin on a terminal is a usage error.

The patches are read from the transcript (--transcript, or else the one under
~/.claude/projects that reviewed the plan): the latest completion of the role,
launch or SendMessage resume, that reviewed the plan body as it was BEFORE this
apply. Each old_string must occur exactly once in the plan, must not overlap
another patch, and must not be empty. A patch that does not match is listed,
never guessed.

  --dry-run          report what would change; write nothing
  --check            write nothing; pass when each new_string occurs once and
                     its old_string is gone. The body to resolve patches by is
                     --before <sha> (12 to 64 hex), else the latest `command`
                     row for this plan, else the current body
  --record-fallback  after a subagent applied the unmatched edits: find its
                     launch in the transcript by the <<<APPLY block its prompt
                     starts with, run the check on the findings on stdin and
                     write one `subagent` row (a rerun writes nothing)
  --transcript <p>   the transcript to read

stdout is data: one line per patch (`applied tui#1 <section>` or
`unmatched tui#3: <reason>`), then `applied A, unmatched U, sha <sha12>`. When
some patch is unmatched, a `next:` line follows and the last thing on stdout is
the block the fallback subagent's prompt must start with:
  <<<APPLY path=<plan> sha=<sha12 of the body before the apply>>>>
Diagnostics go to stderr.

Exit codes: 0 every patch applied (or verified); 3 some patch unmatched or not
verified (matched ones are still written); 1 file I/O, a concurrent change to
the plan (nothing written), or no transcript holds the reviewer's result;
2 usage error (including an empty old_string).
";

const MERGE_SCHEMA: &str = "amont-agent-review-merge-v1";
const FALLBACK_AGENT: &str = "general-purpose";
const APPLY_OPEN: &str = "<<<APPLY ";
const LINE: usize = 80;
const MIN_PREFIX: usize = 12;

enum Fail {
    Usage(String),
    Io(String),
}

type Done = Result<(String, u8), Fail>;

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Apply,
    DryRun,
    Check,
    Fallback,
}

struct Opts {
    plan: String,
    mode: Mode,
    before: Option<String>,
    transcript: Option<PathBuf>,
}

struct Finding {
    role: String,
    n: u64,
    patch: Option<(String, String)>,
}

struct Patch {
    label: String,
    old: String,
    new: String,
}

/// A reviewer's parsed patches `(finding, old, new)`, or why there are none.
type Parsed = Result<Vec<(u64, String, String)>, &'static str>;

/// What `gather` found for the accepted findings, in stdin order.
enum Item {
    Patch(Patch),
    Missing(String, String),
}

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
    let result = parse_args(&args).and_then(|opts| {
        let findings = read_findings()?;
        dispatch(&opts, &findings)
    });
    match result {
        Ok((said, code)) => (said, ExitCode::from(code)),
        Err(Fail::Usage(why)) => {
            eprint!("amont-agent: {why}\n\n{USAGE}");
            (String::new(), ExitCode::from(2))
        }
        Err(Fail::Io(why)) => {
            eprintln!("amont-agent: {why}");
            (String::new(), ExitCode::from(1))
        }
    }
}

fn parse_args(args: &[String]) -> Result<Opts, Fail> {
    let usage = |why: String| Fail::Usage(why);
    let mut mode = Mode::Apply;
    let mut set_mode = |m: Mode, flag: &str| {
        if mode != Mode::Apply {
            return Err(usage(format!(
                "{flag} cannot be combined with another mode"
            )));
        }
        mode = m;
        Ok(())
    };
    let mut plan: Option<String> = None;
    let mut before: Option<String> = None;
    let mut transcript: Option<PathBuf> = None;
    let mut operands_only = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if operands_only || !a.starts_with('-') || a == "-" {
            if plan.is_some() {
                return Err(usage(format!("unexpected argument `{a}`")));
            }
            plan = Some(a.clone());
            continue;
        }
        match a.as_str() {
            "--" => operands_only = true,
            "--dry-run" => set_mode(Mode::DryRun, a)?,
            "--check" => set_mode(Mode::Check, a)?,
            "--record-fallback" => set_mode(Mode::Fallback, a)?,
            "--before" | "--transcript" => {
                let v = it
                    .next()
                    .ok_or_else(|| usage(format!("{a} needs a value")))?;
                if a == "--before" {
                    before = Some(v.clone());
                } else {
                    transcript = Some(PathBuf::from(v));
                }
            }
            _ => {
                if let Some(v) = a.strip_prefix("--before=") {
                    before = Some(v.to_string());
                } else if let Some(v) = a.strip_prefix("--transcript=") {
                    transcript = Some(PathBuf::from(v));
                } else {
                    return Err(usage(format!("unexpected argument `{a}`")));
                }
            }
        }
    }
    let Some(plan) = plan else {
        return Err(usage("plan-apply needs a plan file".into()));
    };
    if let Some(b) = &before {
        if mode != Mode::Check {
            return Err(usage("--before is for --check".into()));
        }
        if b.len() < MIN_PREFIX || b.len() > 64 || !b.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(usage(format!(
                "--before takes {MIN_PREFIX} to 64 hex digits, not `{b}`"
            )));
        }
    }
    Ok(Opts {
        plan,
        mode,
        before: before.map(|b| b.to_ascii_lowercase()),
        transcript,
    })
}

fn read_findings() -> Result<Vec<Finding>, Fail> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(Fail::Usage(
            "the accepted findings come on stdin, not a terminal".into(),
        ));
    }
    let mut text = String::new();
    stdin
        .lock()
        .read_to_string(&mut text)
        .map_err(|e| Fail::Usage(format!("stdin cannot be read: {e}")))?;
    parse_findings(&text).map_err(Fail::Usage)
}

fn parse_findings(text: &str) -> Result<Vec<Finding>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("stdin is not JSON: {e}"))?;
    let list = v
        .as_array()
        .ok_or("stdin must be a JSON array of findings")?;
    let mut out = Vec::new();
    for (i, f) in list.iter().enumerate() {
        let role = f
            .get("role")
            .and_then(|r| r.as_str())
            .ok_or(format!("finding {i}: `role` must be a string"))?;
        let n = f
            .get("finding")
            .and_then(|n| n.as_u64())
            .ok_or(format!("finding {i}: `finding` must be a number"))?;
        let patch = match f.get("patch") {
            None | Some(serde_json::Value::Null) => None,
            Some(p) => {
                let s = |k: &str| p.get(k).and_then(|v| v.as_str()).map(str::to_string);
                match (s("old_string"), s("new_string")) {
                    (Some(o), Some(n)) => Some((o, n)),
                    _ => {
                        return Err(format!(
                            "finding {i}: `patch` needs string old_string and new_string"
                        ))
                    }
                }
            }
        };
        out.push(Finding {
            role: role.to_string(),
            n,
            patch,
        });
    }
    Ok(out)
}

fn dispatch(opts: &Opts, findings: &[Finding]) -> Done {
    let real = plan_review::real(Path::new(&opts.plan));
    match opts.mode {
        Mode::Apply | Mode::DryRun => apply(opts, &real, findings),
        Mode::Check => check(opts, &real, findings),
        Mode::Fallback => fallback(opts, &real, findings),
    }
}

// ---------------------------------------------------------------- patches

/// The patches a reviewer's result carries: the JSON after the last
/// `Patches:` and its opening fence. Parsed as the first JSON value of the
/// rest, and the closing fence ignored, because a patch to a fenced section
/// carries a fence inside its JSON string.
fn patches_in(result: &str) -> Parsed {
    const NONE: &str = "no patches block";
    const BAD: &str = "patches block is not JSON";
    let at = result.rfind("Patches:").ok_or(NONE)?;
    let after = result[at + "Patches:".len()..].trim_start();
    if !(after.starts_with("```") || after.starts_with("~~~")) {
        return Err(NONE);
    }
    let rest = after.split_once('\n').ok_or(NONE)?.1;
    let value = serde_json::Deserializer::from_str(rest)
        .into_iter::<serde_json::Value>()
        .next()
        .ok_or(BAD)?
        .map_err(|_| BAD)?;
    let list = value.as_array().ok_or(BAD)?;
    list.iter()
        .map(|p| {
            let s = |k: &str| p.get(k).and_then(|v| v.as_str()).map(str::to_string);
            Some((
                p.get("finding")?.as_u64()?,
                s("old_string")?,
                s("new_string")?,
            ))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(BAD)
}

/// The distinct body shas of `launches` that start with `prefix`.
fn resolve(launches: &[Launch], prefix: &str) -> Result<Option<String>, Fail> {
    let mut found: Vec<&str> = Vec::new();
    for l in launches.iter().filter(|l| l.sha.starts_with(prefix)) {
        if !found.contains(&l.sha.as_str()) {
            found.push(&l.sha);
        }
    }
    match found.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some((*one).to_string())),
        _ => Err(Fail::Io(format!(
            "ambiguous --before {prefix}: it starts {} reviewed bodies",
            found.len()
        ))),
    }
}

/// A round is a body: numbered in the order bodies first appear.
fn round_of(launches: &[Launch], sha: &str) -> Option<usize> {
    let mut seen: Vec<&str> = Vec::new();
    for l in launches {
        if !seen.contains(&l.sha.as_str()) {
            seen.push(&l.sha);
        }
    }
    seen.iter().position(|s| *s == sha).map(|i| i + 1)
}

fn load(opts: &Opts, real: &str, current: &str) -> Result<(PathBuf, Vec<Launch>), Fail> {
    match &opts.transcript {
        Some(t) => {
            let found = review_outcomes::launches_resumable(t, real)
                .map_err(|e| Fail::Io(format!("cannot read transcript {}: {e}", t.display())))?;
            Ok((t.clone(), found))
        }
        None => review_outcomes::discover(real, current, review_outcomes::launches_resumable)
            .ok_or_else(|| {
                Fail::Io(format!(
                    "no transcript from the last 7 days has a completed plan-review of {real}; name one with --transcript"
                ))
            }),
    }
}

/// The patches of the accepted findings, each role's from its latest
/// completion that reviewed `sha`.
fn gather(
    findings: &[Finding],
    launches: &[Launch],
    sha: Option<&str>,
    source: &str,
) -> Result<Vec<Item>, Fail> {
    let mut cache: BTreeMap<&str, Parsed> = BTreeMap::new();
    let mut items = Vec::new();
    for f in findings {
        let label = format!("{}#{}", f.role, f.n);
        if let Some((old, new)) = &f.patch {
            if old.is_empty() {
                return Err(Fail::Usage(format!("{label}: old_string is empty")));
            }
            items.push(Item::Patch(Patch {
                label,
                old: old.clone(),
                new: new.clone(),
            }));
            continue;
        }
        if !cache.contains_key(f.role.as_str()) {
            let done = sha.and_then(|sha| {
                launches
                    .iter()
                    .rev()
                    .find(|l| l.role == f.role && l.sha == sha)
            });
            let Some(done) = done else {
                return Err(Fail::Io(format!(
                    "no completed {AGENT_PREFIX}{} of body {} in {source}",
                    f.role,
                    sha.map_or("(unknown)", |s| &s[..MIN_PREFIX.min(s.len())])
                )));
            };
            let parsed = patches_in(&done.done.result);
            if let Err(why) = parsed {
                eprintln!(
                    "amont-agent: plan-apply: {} result has {}; apply its Edit lines by hand, then run plan-apply --check",
                    f.role,
                    why.replace("no patches block", "no Patches block")
                        .replace("patches block is not JSON", "a Patches block that is not JSON")
                );
            }
            cache.insert(&f.role, parsed);
        }
        match &cache[f.role.as_str()] {
            Err(why) => items.push(Item::Missing(label, (*why).to_string())),
            Ok(list) => {
                let mine: Vec<_> = list.iter().filter(|(n, _, _)| *n == f.n).collect();
                if mine.is_empty() {
                    items.push(Item::Missing(
                        label.clone(),
                        "no patch for this finding".into(),
                    ));
                }
                for (_, old, new) in mine {
                    if old.is_empty() {
                        return Err(Fail::Usage(format!("{label}: old_string is empty")));
                    }
                    items.push(Item::Patch(Patch {
                        label: label.clone(),
                        old: old.clone(),
                        new: new.clone(),
                    }));
                }
            }
        }
    }
    Ok(items)
}

// ---------------------------------------------------------------- matching

/// One patch that matched exactly once, in the coordinates of the text read.
struct Hit {
    start: usize,
    len: usize,
    new: String,
    section: String,
    outside: bool,
}

struct Judged {
    label: String,
    verdict: Result<Hit, String>,
}

fn flatten(s: &str) -> String {
    s.replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The nearest heading above `offset`, outside fenced code.
fn section_at(text: &str, offset: usize) -> String {
    let mut fence = crate::plan_text::Fence::default();
    let mut title = String::new();
    for line in text[..offset].lines() {
        if let Some(t) = fence.heading(line) {
            title = t.to_string();
        }
    }
    title
}

/// Judges every patch against `text`, in order. Offsets come from
/// `match_indices`/`str::find`, which are char boundaries, so every slice is
/// UTF-8 safe. Occurrences are counted non-overlapping. A match that overlaps
/// an earlier patch's range is unmatched; a 0-match is retried on a
/// whitespace-flattened copy only to say why, never to apply from it.
fn judge(text: &str, items: &[Item]) -> Vec<Judged> {
    let mut taken: Vec<(usize, usize, String)> = Vec::new();
    let mut out = Vec::new();
    for item in items {
        let (label, verdict) = match item {
            Item::Missing(label, why) => (label.clone(), Err(why.clone())),
            Item::Patch(p) => {
                let at: Vec<usize> = text.match_indices(&p.old).map(|(i, _)| i).collect();
                let verdict = match at.as_slice() {
                    [] if flatten(text).contains(&flatten(&p.old)) => {
                        Err("absent (whitespace)".to_string())
                    }
                    [] => Err("absent".to_string()),
                    [s] => {
                        let end = s + p.old.len();
                        match taken.iter().find(|(a, b, _)| *s < *b && end > *a) {
                            Some((_, _, by)) => Err(format!("overlaps {by}")),
                            None => {
                                taken.push((*s, end, p.label.clone()));
                                Ok(Hit {
                                    start: *s,
                                    len: p.old.len(),
                                    new: p.new.clone(),
                                    section: section_at(text, *s),
                                    outside: crate::plan_text::outside_body(text, *s),
                                })
                            }
                        }
                    }
                    many => Err(format!("{} occurrences", many.len())),
                };
                (p.label.clone(), verdict)
            }
        };
        out.push(Judged { label, verdict });
    }
    out
}

fn splice(text: &str, judged: &[Judged]) -> String {
    let mut hits: Vec<&Hit> = judged
        .iter()
        .filter_map(|j| j.verdict.as_ref().ok())
        .collect();
    hits.sort_by_key(|h| h.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for h in hits {
        out.push_str(&text[at..h.start]);
        out.push_str(&h.new);
        at = h.start + h.len;
    }
    out.push_str(&text[at..]);
    out
}

fn clip(s: String) -> String {
    if s.chars().count() <= LINE {
        return s;
    }
    let mut t: String = s.chars().take(LINE - 1).collect();
    t.push('…');
    t
}

/// The labels the apply step recorded as `overlaps` in its latest `command`
/// row for this plan — the row for `before` when one carries that sha,
/// else the latest. A fallback resolves two overlapping patches with one
/// merged edit, so neither reviewer's `new_string` can be expected in the
/// text; such a patch verifies on its `old_string` being gone.
fn overlapped(rows: &[serde_json::Value], plan: &str, before: Option<&str>) -> BTreeSet<String> {
    let commands: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|r| is_merge(r, plan))
        .filter(|r| r.get("mode").and_then(|m| m.as_str()) == Some("command"))
        .collect();
    let row = before
        .and_then(|sha| {
            commands
                .iter()
                .rev()
                .find(|r| r.get("body_sha_before").and_then(|s| s.as_str()) == Some(sha))
        })
        .or_else(|| commands.last())
        .copied();
    row.and_then(|r| r.get("unmatched")?.as_array())
        .into_iter()
        .flatten()
        .filter_map(|u| u.as_str())
        .filter_map(|u| {
            let (label, reason) = u.split_once(": ")?;
            reason.starts_with("overlaps ").then(|| label.to_string())
        })
        .collect()
}

/// A patch the check can verify: `new` once, `old` gone (unless `new`
/// contains `old`, when the absence cannot hold).
fn verify(text: &str, p: &Patch) -> Result<(), String> {
    if !p.new.is_empty() {
        let n = text.match_indices(&p.new).count();
        if n != 1 {
            return Err(format!("new_string occurs {n} times"));
        }
    }
    if !p.new.contains(&p.old) && text.contains(&p.old) {
        return Err("old_string is still present".into());
    }
    Ok(())
}

// ---------------------------------------------------------------- the ledger

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn store() -> Result<PathBuf, Fail> {
    let store = amont_agent_guard::journal::dir()
        .ok_or_else(|| Fail::Io("no home directory, so the ledger cannot be located".into()))?
        .join("plan-review");
    plan_review::private_dir(&store)
        .ok_or_else(|| Fail::Io(format!("cannot create {}", store.display())))?;
    Ok(store)
}

/// Every row of the ledger. A line that is not JSON is a torn append from
/// an interrupted writer: it is skipped and counted on stderr, never fatal,
/// as `review_outcomes::recorded` does.
fn rows(ledger: &Path) -> std::io::Result<Vec<serde_json::Value>> {
    match std::fs::read(ledger) {
        Ok(b) => {
            let text = String::from_utf8_lossy(&b);
            let mut torn = 0usize;
            let rows = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| serde_json::from_str(l).inspect_err(|_| torn += 1).ok())
                .collect();
            if torn > 0 {
                eprintln!(
                    "amont-agent: {torn} torn line(s) skipped in {}",
                    ledger.display()
                );
            }
            Ok(rows)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn is_merge(row: &serde_json::Value, plan: &str) -> bool {
    row.get("schema").and_then(|s| s.as_str()) == Some(MERGE_SCHEMA)
        && row.get("plan_path").and_then(|p| p.as_str()) == Some(plan)
}

fn append(ledger: &Path, row: &serde_json::Value) -> std::io::Result<()> {
    let mut f = review_outcomes::open_ledger(ledger)?;
    f.write_all(format!("{row}\n").as_bytes())?;
    f.flush()
}

fn read_plan(real: &Path) -> Result<String, Fail> {
    std::fs::read_to_string(real)
        .map_err(|e| Fail::Io(format!("cannot read {}: {e}", real.display())))
}

fn sha_of(text: &str) -> Result<String, Fail> {
    crate::plan_text::shas(text)
        .map(|s| s.sha)
        .map_err(|e| Fail::Io(e.to_string()))
}

fn unmatched_list(judged: &[Judged]) -> Vec<String> {
    judged
        .iter()
        .filter_map(|j| {
            j.verdict
                .as_ref()
                .err()
                .map(|why| format!("{}: {why}", j.label))
        })
        .collect()
}

// ---------------------------------------------------------------- apply

fn apply(opts: &Opts, real: &Path, findings: &[Finding]) -> Done {
    let dry = opts.mode == Mode::DryRun;
    let plan_path = real.to_string_lossy().into_owned();
    let store = store()?;
    let ledger = store.join("outcomes.jsonl");
    // Held until the row is written: the lock comes before the plan is read.
    let _lock = if dry {
        None
    } else {
        Some(
            review_outcomes::lock(&store.join("outcomes.jsonl.lock"))
                .map_err(|e| Fail::Io(format!("cannot lock the ledger: {e}")))?,
        )
    };
    let text = read_plan(real)?;
    let before = sha_of(&text)?;
    let (launches, source) = if findings.iter().any(|f| f.patch.is_none()) {
        let (t, l) = load(opts, &plan_path, &before)?;
        (l, t.display().to_string())
    } else {
        (Vec::new(), String::new())
    };
    let items = gather(findings, &launches, Some(&before), &source)?;
    let judged = judge(&text, &items);
    let applied = judged.iter().filter(|j| j.verdict.is_ok()).count();
    let unmatched = judged.len() - applied;

    let mut said = String::new();
    for j in &judged {
        said.push_str(&clip(match &j.verdict {
            Ok(h) if h.outside => {
                format!("applied {} outside the body ({})", j.label, h.section)
            }
            Ok(h) => format!("applied {} {}", j.label, h.section),
            Err(why) => format!("unmatched {}: {why}", j.label),
        }));
        said.push('\n');
    }

    let mut after = Some(before.clone());
    if applied > 0 {
        let new = splice(&text, &judged);
        after = crate::plan_text::shas(&new).ok().map(|s| s.sha);
        if after.is_none() {
            eprintln!("amont-agent: sha unavailable (the Markdown parser panicked)");
        }
        if !dry {
            // Another writer between our read and our rename would be lost.
            if read_plan(real)? != text {
                return Err(Fail::Io(
                    "plan changed under plan-apply; nothing written".into(),
                ));
            }
            amont_agent_guard::atomic::write_atomic(real, &new)
                .map_err(|e| Fail::Io(format!("cannot write {}: {e}", real.display())))?;
        }
    }
    if !dry {
        let row = serde_json::json!({
            "schema": MERGE_SCHEMA,
            "recorded_at": now(),
            "plan_path": plan_path,
            "body_sha_before": before,
            "body_sha_after": if applied > 0 { after.clone() } else { Some(before.clone()) },
            "round": round_of(&launches, &before),
            "accepted": findings.len(),
            "applied": applied,
            "unmatched": unmatched_list(&judged),
            "mode": "command",
        });
        append(&ledger, &row).map_err(|e| {
            Fail::Io(if applied > 0 {
                format!("plan written, merge row not: {e}")
            } else {
                format!("merge row not written: {e}")
            })
        })?;
    }
    let short = match &after {
        Some(s) => s[..MIN_PREFIX].to_string(),
        None => "unavailable".to_string(),
    };
    said.push_str(&format!(
        "applied {applied}, unmatched {unmatched}, sha {short}\n"
    ));
    if unmatched == 0 {
        return Ok((said, 0));
    }
    // A command the person copies: the path is never clipped, so this line,
    // like the APPLY block, may exceed LINE columns.
    said.push_str(&format!(
        "next: apply the unmatched edits, then run amont-agent plan-apply --check {}",
        opts.plan
    ));
    said.push_str(&format!(
        "\n\n{APPLY_OPEN}path={plan_path} sha={}>>>\n",
        &before[..MIN_PREFIX]
    ));
    Ok((said, 3))
}

// ---------------------------------------------------------------- check

/// Check lines and the counts, for `--check` and `--record-fallback`.
fn run_check(
    text: &str,
    items: &[Item],
    overlapped: &BTreeSet<String>,
) -> (String, Vec<String>, usize) {
    let mut said = String::new();
    let mut failed = Vec::new();
    let mut ok = 0;
    for item in items {
        let (label, verdict, note) = match item {
            Item::Missing(label, why) => (label, Err(why.clone()), ""),
            Item::Patch(p) if overlapped.contains(&p.label) => {
                // The merged edit holds neither new_string: only the old
                // text's absence can be asked of it.
                let verdict = if text.contains(&p.old) {
                    Err("old_string is still present (overlapped)".to_string())
                } else {
                    Ok(())
                };
                (&p.label, verdict, " (overlapped; old_string gone)")
            }
            Item::Patch(p) => (&p.label, verify(text, p), ""),
        };
        match verdict {
            Ok(()) => {
                ok += 1;
                said.push_str(&clip(format!("verified {label}{note}")));
            }
            Err(why) => {
                said.push_str(&clip(format!("unverified {label}: {why}")));
                failed.push(format!("{label}: {why}"));
            }
        }
        said.push('\n');
    }
    said.push_str(&format!("verified {ok}, unverified {}\n", failed.len()));
    (said, failed, ok)
}

fn check(opts: &Opts, real: &Path, findings: &[Finding]) -> Done {
    let plan_path = real.to_string_lossy().into_owned();
    let text = read_plan(real)?;
    let current = sha_of(&text)?;
    let (launches, source) = if findings.iter().any(|f| f.patch.is_none()) {
        let (t, l) = load(opts, &plan_path, &current)?;
        (l, t.display().to_string())
    } else {
        (Vec::new(), String::new())
    };
    let ledger = store()?.join("outcomes.jsonl");
    let recorded =
        rows(&ledger).map_err(|e| Fail::Io(format!("cannot read {}: {e}", ledger.display())))?;
    let prefix = match &opts.before {
        Some(b) => b.clone(),
        None => recorded
            .iter()
            .rev()
            .filter(|r| is_merge(r, &plan_path))
            .filter(|r| r.get("mode").and_then(|m| m.as_str()) == Some("command"))
            .find_map(|r| r.get("body_sha_before")?.as_str().map(str::to_string))
            .unwrap_or(current),
    };
    let sha = resolve(&launches, &prefix)?;
    let items = gather(findings, &launches, sha.as_deref(), &source)?;
    let overlapped = overlapped(&recorded, &plan_path, sha.as_deref());
    let (said, failed, _) = run_check(&text, &items, &overlapped);
    Ok((said, if failed.is_empty() { 0 } else { 3 }))
}

// ---------------------------------------------------------------- fallback

/// The path and sha prefix of the `<<<APPLY …>>>` block a prompt starts with.
fn apply_header(prompt: &str) -> Option<(String, String)> {
    let rest = prompt.trim_start().strip_prefix(APPLY_OPEN)?;
    let inner = &rest[..rest.find(">>>")?];
    let body = inner.strip_prefix("path=")?;
    let split = body.rfind(" sha=")?;
    Some((
        body[..split].to_string(),
        body[split + 5..].trim().to_string(),
    ))
}

fn fallback(opts: &Opts, real: &Path, findings: &[Finding]) -> Done {
    let plan_path = real.to_string_lossy().into_owned();
    let store = store()?;
    let ledger = store.join("outcomes.jsonl");
    let _lock = review_outcomes::lock(&store.join("outcomes.jsonl.lock"))
        .map_err(|e| Fail::Io(format!("cannot lock the ledger: {e}")))?;
    let text = read_plan(real)?;
    let current = sha_of(&text)?;
    let (transcript, launches) = load(opts, &plan_path, &current)?;
    let reader = BufReader::new(std::fs::File::open(&transcript).map_err(|e| {
        Fail::Io(format!(
            "cannot read transcript {}: {e}",
            transcript.display()
        ))
    })?);
    let found = plan_review::completed_agents(reader, FALLBACK_AGENT)
        .into_iter()
        .filter(|c| c.agent == FALLBACK_AGENT)
        .filter_map(|c| {
            let (path, sha) = apply_header(&c.prompt)?;
            (path == plan_path).then_some((c, sha))
        })
        .next_back();
    let Some((agent, prefix)) = found else {
        return Err(Fail::Io(format!(
            "{} has no completed fallback launch whose prompt starts with the {APPLY_OPEN}block for {plan_path}",
            transcript.display()
        )));
    };
    let Some(agent_id) = agent.agent_id.clone() else {
        return Err(Fail::Io(format!(
            "no agent id for the fallback subagent in {}",
            transcript.display()
        )));
    };
    let io = |e: std::io::Error| Fail::Io(format!("cannot read {}: {e}", ledger.display()));
    let seen = rows(&ledger).map_err(io)?;
    // The dedupe key is (schema, agent_id): an agent id names one launch,
    // whichever plan it applied.
    if seen.iter().any(|r| {
        r.get("schema").and_then(|s| s.as_str()) == Some(MERGE_SCHEMA)
            && r.get("agent_id").and_then(|a| a.as_str()) == Some(&agent_id)
    }) {
        eprintln!("already recorded the fallback of {agent_id}; nothing written");
        return Ok((String::new(), 0));
    }
    let sha = resolve(&launches, &prefix.to_ascii_lowercase())?;
    let items = gather(
        findings,
        &launches,
        sha.as_deref(),
        &transcript.display().to_string(),
    )?;
    let overlapped = overlapped(&seen, &plan_path, sha.as_deref());
    let (said, failed, ok) = run_check(&text, &items, &overlapped);
    let verified = failed.is_empty();
    let row = serde_json::json!({
        "schema": MERGE_SCHEMA,
        "recorded_at": now(),
        "plan_path": plan_path,
        "body_sha_before": sha,
        "body_sha_after": current,
        "round": sha.as_deref().and_then(|s| round_of(&launches, s)),
        "accepted": findings.len(),
        "applied": ok,
        "unmatched": failed,
        "mode": "subagent",
        "agent_id": agent_id,
        "model": agent.model,
        "tokens": agent.tokens,
        "seconds": agent.duration_ms.map(|ms| ms as f64 / 1000.0),
        "verified": verified,
    });
    append(&ledger, &row).map_err(|e| Fail::Io(format!("merge row not written: {e}")))?;
    eprintln!("recorded the fallback of {agent_id} (verified: {verified})");
    Ok((said, 0))
}
