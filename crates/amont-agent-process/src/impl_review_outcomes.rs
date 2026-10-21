//! `amont-agent implementation-review record`: what the implementation
//! review found and how each finding was settled, kept so the seat can be
//! judged on its own record (ADR-0042). One row per invocation is appended to
//! `<journal dir>/implementation-review-outcomes/outcomes.jsonl`. `plan-review stats`
//! reads it through [`crate::review_outcomes::read_rows`]. The ledger is
//! kept outside `implementation-review/`, which only the hook may write.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::review_outcomes::{
    lock, now, open_ledger, read_rows, IMPL_SCHEMA as SCHEMA, SEVERITIES,
};

pub const USAGE: &str = "\
usage: amont-agent implementation-review record <plan.md> --verdict <v>

Append one row to ~/.claude/amont-agent/implementation-review-outcomes/outcomes.jsonl
(schema amont-agent-impl-review-outcome-v1): recorded_at, tree (the content id
of HEAD, as `amont-agent tree-sha`), round (one more than the rows already
kept for this plan; a per-plan count), plan_path, model (the
resolvedModel of the completed implementation-review agent in the newest
transcript of this directory's session, or empty) and verdict. A remembered
pass for the tree that disagrees with --verdict is warned about on stderr.

--verdict approve|approve-with-changes|rework   the reviewer's verdict

stdin: a JSON array of findings (empty stdin is `[]`):
  {\"severity\": \"blocking\"|\"high\"|\"medium\"|\"low\",
   \"disposition\": \"fixed\"|\"deliberate\"|\"overruled\",
   \"section\": str (optional)}

Exit 0 when the row is appended, 1 when stdin, the tree or the ledger cannot
be read or written, 2 on a usage error (including malformed findings).
";

/// The directory under the journal dir that holds the ledger. It is not
/// `implementation-review/`: the hook alone writes there.
pub(crate) const STORE: &str = "implementation-review-outcomes";
const DISPOSITIONS: [&str; 3] = ["fixed", "deliberate", "overruled"];
const VERDICTS: [&str; 3] = ["approve", "approve-with-changes", "rework"];

/// Runs the verb; the text it returns is for `main` to print on stdout.
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
        return usage("implementation-review takes one verb: record");
    }
    let mut plan: Option<&String> = None;
    let mut verdict: Option<String> = None;
    while let Some(a) = it.next() {
        if a == "--verdict" {
            let Some(v) = it.next() else {
                return usage("--verdict needs a value");
            };
            verdict = Some(v.clone());
        } else if let Some(v) = a.strip_prefix("--verdict=") {
            verdict = Some(v.to_string());
        } else if (a.starts_with('-') && a != "-") || plan.is_some() {
            return usage(&format!("unexpected argument `{a}`"));
        } else {
            plan = Some(a);
        }
    }
    let Some(plan) = plan else {
        return usage("record needs a plan file");
    };
    let verdict = match verdict {
        Some(v) if VERDICTS.contains(&v.as_str()) => v,
        Some(v) => {
            return usage(&format!(
                "unknown verdict {v}; one of {}",
                VERDICTS.join("|")
            ))
        }
        None => return usage("record needs --verdict"),
    };
    let mut stdin = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin) {
        eprintln!("amont-agent: stdin cannot be read: {e}");
        return (String::new(), ExitCode::from(1));
    }
    let findings = match parse_findings(&stdin) {
        Ok(f) => f,
        Err(why) => return usage(&why),
    };
    match record(Path::new(plan), &verdict, findings) {
        Ok(said) => {
            eprint!("{said}");
            (String::new(), ExitCode::SUCCESS)
        }
        Err(why) => {
            eprintln!("amont-agent: {why}");
            (String::new(), ExitCode::from(1))
        }
    }
}

/// The findings on stdin, checked: an unknown severity or disposition would
/// poison every later aggregate, so it is refused before anything is written.
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
        if f.get("section")
            .is_some_and(|v| !v.is_string() && !v.is_null())
        {
            return Err(format!("finding {i}: `section` must be a string"));
        }
    }
    // Rebuilt from the validated keys alone: nothing else on stdin is kept.
    Ok(list
        .iter()
        .map(|f| {
            let mut out = serde_json::Map::new();
            for key in ["severity", "disposition"] {
                out.insert(key.into(), f[key].clone());
            }
            if let Some(section) = f.get("section").filter(|v| v.is_string()) {
                out.insert("section".into(), section.clone());
            }
            serde_json::Value::Object(out)
        })
        .collect())
}

fn record(plan: &Path, verdict: &str, findings: Vec<serde_json::Value>) -> Result<String, String> {
    let real = crate::plan_review::real(plan)
        .to_string_lossy()
        .into_owned();
    let cwd = std::env::current_dir().map_err(|e| format!("no working directory: {e}"))?;
    let repo = amont_agent_guard::push_target::toplevel(&cwd)
        .ok_or_else(|| format!("{} is not inside a git repository", cwd.display()))?;
    let tree = crate::content_id::content_id(&repo, "HEAD")?;
    let dir = amont_agent_guard::journal::dir()
        .ok_or("no home directory, so the ledger cannot be located")?
        .join(STORE);
    crate::plan_review::private_dir(&dir)
        .ok_or_else(|| format!("cannot create {}", dir.display()))?;
    let ledger = dir.join("outcomes.jsonl");
    let _lock = lock(&dir.join("outcomes.jsonl.lock"))
        .map_err(|e| format!("cannot lock the ledger: {e}"))?;
    let earlier = read_rows(&ledger)
        .map_err(|e| format!("cannot read {}: {e}", ledger.display()))?
        .iter()
        .filter(|r| r.get("schema").and_then(|s| s.as_str()) == Some(SCHEMA))
        .filter(|r| r.get("plan_path").and_then(|p| p.as_str()) == Some(real.as_str()))
        .count();
    let round = earlier + 1;
    let at = now();
    warn_on_disagreement(&repo, &tree, verdict);
    let row = serde_json::json!({
        "schema": SCHEMA,
        "recorded_at": at,
        "tree": tree,
        "round": round,
        "plan_path": real,
        "model": session_model(&cwd).unwrap_or_default(),
        "verdict": verdict,
        "findings": findings,
    });
    let mut f =
        open_ledger(&ledger).map_err(|e| format!("cannot open {}: {e}", ledger.display()))?;
    f.write_all(format!("{row}\n").as_bytes())
        .and_then(|()| f.flush())
        .map_err(|e| format!("cannot write {}: {e}", ledger.display()))?;
    Ok(format!(
        "wrote 1 row (round {round}, tree {})\n",
        &tree[..12]
    ))
}

/// Says on stderr when the pass remembered for this tree carries another
/// verdict than the one being recorded. A person's `overruled` stands for a
/// `rework` that was passed anyway.
fn warn_on_disagreement(repo: &Path, tree: &str, verdict: &str) {
    let Some(name) = crate::implementation_review::repo_name(repo) else {
        return;
    };
    let Some(remembered) = crate::implementation_review::remembered(&name, tree) else {
        return;
    };
    let same = remembered == verdict || (remembered == "overruled" && verdict == "rework");
    if !same {
        eprintln!(
            "amont-agent: the pass remembered for tree {} says {remembered}, not {verdict}",
            &tree[..12]
        );
    }
}

/// The `resolvedModel` of the last completed `implementation-review` agent
/// in the newest transcript of the session in `cwd`.
///
/// holds-until: Claude Code keeps a session's transcripts in
/// `<config dir>/projects/<cwd with every non-alphanumeric character as
/// `-`>/*.jsonl`. If that layout changes, the model is empty and a missing
/// directory is silent, but any other I/O error is said on stderr.
fn session_model(cwd: &Path) -> Option<String> {
    let name: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir: PathBuf = amont_agent_guard::settings::config_dir()?
        .join("projects")
        .join(name);
    let said = |what: &Path, e: std::io::Error| {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("amont-agent: cannot read {}: {e}", what.display());
        }
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            said(&dir, e);
            return None;
        }
    };
    let newest = entries
        .filter_map(|e| match e {
            Ok(e) => Some(e),
            Err(e) => {
                said(&dir, e);
                None
            }
        })
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .filter_map(|e| match e.metadata().and_then(|m| m.modified()) {
            Ok(t) => Some((t, e.path())),
            Err(err) => {
                said(&e.path(), err);
                None
            }
        })
        .max()?
        .1;
    let file = match std::fs::File::open(&newest) {
        Ok(f) => f,
        Err(e) => {
            said(&newest, e);
            return None;
        }
    };
    crate::plan_review::completed_agents(
        std::io::BufReader::new(file),
        crate::implementation_review::RULE_ID,
    )
    .into_iter()
    .filter_map(|c| c.model)
    .next_back()
}
