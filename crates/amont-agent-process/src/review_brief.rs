//! The implementation-review brief and the hash of the code rules it carries
//! (decisions ADR-0043, ADR-0044). One function hashes the `aval` output
//! ([`rules_sha`]); the brief, the CLI and the push check all call it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use amont_agent_guard::git;
use amont_agent_guard::sha256::{hex, sha256};

/// Opens the rules line in a reviewer's prompt.
pub const RULES_OPEN: &str = "<<<RULES sha=";
const RULES_CLOSE: &str = ">>>";
/// The full diff goes into the brief below this size.
const FULL_DIFF_LIMIT_BYTES: usize = 60 * 1024;
/// `aval` answers in well under a second; this bounds a hung one.
const AVAL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
const AVAL_POLL: std::time::Duration = std::time::Duration::from_millis(10);
const PLANS_DIR: &str = "docs/plans";
const RULES_ARGS: [&str; 7] = [
    "--json",
    "rules",
    "--level",
    "constraint",
    "--governs",
    "code",
    "--full",
];

/// What `aval` could not do: the command line and why.
#[derive(Debug, PartialEq, Eq)]
pub struct AvalFailed(pub String);

impl std::fmt::Display for AvalFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a brief could not be built.
#[derive(Debug, PartialEq, Eq)]
pub enum BriefError {
    Aval(AvalFailed),
    Repository(String),
}

impl std::fmt::Display for BriefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BriefError::Aval(why) => why.fmt(f),
            BriefError::Repository(why) => f.write_str(why),
        }
    }
}

impl From<AvalFailed> for BriefError {
    fn from(why: AvalFailed) -> Self {
        BriefError::Aval(why)
    }
}

/// What an `aval` call answered: how it exited and what it printed.
struct AvalAnswer {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: String,
}

/// The `aval` program to run: `AMONT_AGENT_AVAL` names another one (tests,
/// or an `aval` outside PATH), else `aval` from PATH.
fn aval_program() -> std::ffi::OsString {
    std::env::var_os("AMONT_AGENT_AVAL")
        .filter(|program| !program.is_empty())
        .unwrap_or_else(|| std::ffi::OsString::from("aval"))
}

/// Runs `aval` with a deadline: a hung `aval` is killed and reported, so the
/// push check that calls it never waits on it for long.
fn run_aval(dir: &Path, args: &[&str]) -> Result<AvalAnswer, AvalFailed> {
    use std::io::Read;
    let line = format!("aval {}", args.join(" "));
    let mut child = Command::new(aval_program())
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AvalFailed(format!("`{line}` could not run: {e}")))?;
    let mut pipe = child
        .stdout
        .take()
        .ok_or_else(|| AvalFailed(format!("`{line}` has no stdout")))?;
    let reader = std::thread::spawn(move || {
        let mut printed = Vec::new();
        pipe.read_to_end(&mut printed).map(|_| printed)
    });
    // stderr on its own thread too: a full pipe would otherwise stall aval.
    let mut err_pipe = child
        .stderr
        .take()
        .ok_or_else(|| AvalFailed(format!("`{line}` has no stderr")))?;
    let err_reader = std::thread::spawn(move || {
        let mut said = Vec::new();
        err_pipe.read_to_end(&mut said).map(|_| said)
    });
    let deadline = std::time::Instant::now() + AVAL_DEADLINE;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(AVAL_POLL);
            }
            Ok(None) => {
                let killed = child.kill().and_then(|()| child.wait());
                return Err(AvalFailed(format!(
                    "`{line}` timed out after {} s (kill: {killed:?})",
                    AVAL_DEADLINE.as_secs()
                )));
            }
            Err(e) => return Err(AvalFailed(format!("`{line}` could not be waited on: {e}"))),
        }
    };
    let stdout = reader
        .join()
        .map_err(|_| AvalFailed(format!("`{line}` output reader panicked")))?
        .map_err(|e| AvalFailed(format!("`{line}` output could not be read: {e}")))?;
    // What aval said goes into an error message only; an unreadable stderr says nothing.
    let stderr = err_reader
        .join()
        .ok()
        .and_then(Result::ok)
        .map(|said| String::from_utf8_lossy(&said).trim().to_string())
        .unwrap_or_default();
    Ok(AvalAnswer {
        status,
        stdout,
        stderr,
    })
}

fn aval(dir: &Path, args: &[&str]) -> Result<Vec<u8>, AvalFailed> {
    let answer = run_aval(dir, args)?;
    if !answer.status.success() {
        let line = format!("aval {}", args.join(" "));
        let said = if answer.stderr.is_empty() {
            String::new()
        } else {
            format!(": {}", answer.stderr.lines().next().unwrap_or_default())
        };
        return Err(AvalFailed(format!(
            "`{line}` exited {}{said}",
            answer.status
        )));
    }
    Ok(answer.stdout)
}

/// The code rules as `aval` printed them, and the hash of exactly those bytes.
pub struct Rules {
    pub json: Vec<u8>,
    pub sha: String,
}

/// Runs the rules command once and hashes its stdout: the only place the
/// rules hash is computed.
pub fn rules(dir: &Path) -> Result<Rules, AvalFailed> {
    let json = aval(dir, &RULES_ARGS)?;
    let sha = sha_of(&json);
    Ok(Rules { json, sha })
}

/// The lowercase sha256 hex of bytes.
pub fn sha_of(bytes: &[u8]) -> String {
    hex(&sha256(bytes))
}

/// The hash of the code rules `aval` reports for the repository at `dir`.
/// The only implementation: the brief, `rules-sha` and the push check call it.
pub fn rules_sha(dir: &Path) -> Result<String, AvalFailed> {
    rules(dir).map(|rules| rules.sha)
}

/// The rules line a reviewer's prompt carries.
pub fn rules_line(sha: &str) -> String {
    format!("{RULES_OPEN}{sha}{RULES_CLOSE}")
}

/// The sha of the first well-formed rules line in a prompt; a sha that is
/// not 64 hex is not a binding.
pub fn parse_rules_sha(prompt: &str) -> Option<String> {
    let mut rest = prompt;
    while let Some(at) = rest.find(RULES_OPEN) {
        rest = &rest[at + RULES_OPEN.len()..];
        // A mention in prose that is not a full line is skipped, not the end of the search.
        let Some(candidate) = rest.get(..64) else {
            continue;
        };
        if crate::implementation_review::is_sha256(candidate) && rest[64..].starts_with(RULES_CLOSE)
        {
            return Some(candidate.to_ascii_lowercase());
        }
    }
    None
}

/// The journal excerpt for how a reviewer's rules line compares with the
/// current rules. A match is journalled too (`rules-current`), so the
/// rollout can count the pushes the check passed.
pub fn rules_excerpt(
    prompt_sha: Option<&str>,
    current: &Result<String, AvalFailed>,
) -> &'static str {
    match (prompt_sha, current) {
        (_, Err(_)) => "review=rules-unknown",
        (None, Ok(_)) => "review=rules-missing",
        (Some(seen), Ok(now)) if seen != now => "review=rules-stale",
        (Some(_), Ok(_)) => "review=rules-current",
    }
}

/// `## <heading>` and everything up to the next `## ` heading, verbatim;
/// fenced lines never open or close a section.
pub fn section(text: &str, heading: &str) -> Option<String> {
    let wanted = format!("## {heading}");
    let mut out: Vec<&str> = Vec::new();
    let mut inside = false;
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        if !fenced && line.starts_with("## ") {
            if inside {
                break;
            }
            inside = line.trim_end() == wanted;
        }
        if inside {
            out.push(line);
        }
    }
    (!out.is_empty()).then(|| out.join("\n"))
}

/// Each rule as `## <id> [<level>]`, its statement, its body.
pub fn render_rules(json: &[u8]) -> Result<String, String> {
    let document: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("aval rules is not JSON: {e}"))?;
    let rules = document
        .get("rules")
        .and_then(serde_json::Value::as_array)
        .ok_or("aval rules has no `rules` array")?;
    // A rule the reviewer is bound to must have its text: a missing field is
    // an aval defect, never an empty rule.
    let text = |rule: &serde_json::Value, field: &str| -> Result<String, String> {
        rule.get(field)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                let id = rule
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("<no id>");
                format!("aval rules: rule `{id}` has no string `{field}`")
            })
    };
    let rendered = rules
        .iter()
        .filter(|rule| rule.get("active").and_then(serde_json::Value::as_bool) != Some(false))
        .map(|rule| {
            Ok(format!(
                "## {} [{}]\n\n{}\n\n{}\n",
                text(rule, "id")?,
                text(rule, "level")?,
                text(rule, "statement")?,
                text(rule, "body")?
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(rendered.join("\n"))
}

/// What the caller chose for a brief.
pub struct BriefRequest {
    pub dir: PathBuf,
    pub plan: Option<PathBuf>,
    pub base: Option<String>,
}

/// The brief, whole, or why it could not be built. Nothing is partial.
pub fn build(request: &BriefRequest) -> Result<String, BriefError> {
    let repo = amont_agent_guard::push_target::toplevel(&request.dir).ok_or_else(|| {
        BriefError::Repository(format!(
            "{} is not inside a git repository",
            request.dir.display()
        ))
    })?;
    let code_rules = rules(&repo)?;
    let rules_text =
        render_rules(&code_rules.json).map_err(|why| BriefError::Aval(AvalFailed(why)))?;
    let tree = crate::content_id::content_id(&repo, "HEAD").map_err(BriefError::Repository)?;
    let name = crate::implementation_review::repo_name(&repo)
        .ok_or_else(|| BriefError::Repository("the repository has no name".into()))?;
    let base = request
        .base
        .clone()
        .or_else(|| crate::implementation_review::default_base(&repo, "origin"))
        .ok_or_else(|| BriefError::Repository("no default branch of `origin` is known".into()))?;
    let merge_base = git::stdout_in(&repo, &["merge-base", &base, "HEAD"])
        .ok_or_else(|| BriefError::Repository(format!("`git merge-base {base} HEAD` failed")))?;
    let range = format!("{merge_base}...HEAD");
    let exclude_plans = format!(":!{PLANS_DIR}");

    let mut brief = String::new();
    brief.push_str("# Implementation review brief\n\n");
    brief.push_str(&crate::implementation_review::block(&name, &tree));
    brief.push('\n');
    brief.push_str(&rules_line(&code_rules.sha));
    brief.push_str("\n\n");

    let (plan_label, plan_text) = plan_of(&repo, request.plan.as_deref(), &merge_base)?;
    brief.push_str(&format!("## Plan\n\n{plan_label}\n\n"));
    let verification = plan_text
        .as_deref()
        .and_then(|t| section(t, "Verification"))
        .unwrap_or_else(|| missing_section(plan_text.as_deref(), "Verification"));
    brief.push_str(&format!("{verification}\n\n"));

    let stat = git::stdout_in(
        &repo,
        &["diff", &range, "--stat", "--", ".", &exclude_plans],
    )
    .ok_or_else(|| BriefError::Repository(format!("`git diff {range} --stat` failed")))?;
    brief.push_str(&format!("## Diff stat\n\n```\n{stat}\n```\n\n"));
    let diff = git::bytes_in(&repo, &["diff", &range, "--", ".", &exclude_plans], None)
        .ok()
        .flatten()
        .ok_or_else(|| BriefError::Repository(format!("`git diff {range}` failed")))?;
    if diff.len() < FULL_DIFF_LIMIT_BYTES {
        brief.push_str(&format!(
            "## Diff\n\n```diff\n{}\n```\n\n",
            String::from_utf8_lossy(&diff)
        ));
    } else {
        brief.push_str(
            "## Diff\n\nThe diff is 60 KB or more: read the changed files in the worktree.\n\n",
        );
    }

    brief.push_str(&format!("# Code rules (in full)\n\n{rules_text}\n"));

    let index = aval(&repo, &["rules", "--level", "constraint"])?;
    brief.push_str(&format!(
        "# Constraint rules\n\n{}\n\n",
        String::from_utf8_lossy(&index).trim_end()
    ));

    brief.push_str("# Decisions that apply to the changed paths\n\n");
    brief.push_str(&decisions(&repo, &range)?);

    let non_goals = plan_text
        .as_deref()
        .and_then(|t| section(t, "Non-goals"))
        .unwrap_or_else(|| missing_section(plan_text.as_deref(), "Non-goals"));
    brief.push_str(&format!("\n{non_goals}\n"));
    Ok(brief)
}

fn missing_section(plan_text: Option<&str>, heading: &str) -> String {
    match plan_text {
        Some(_) => format!("(the plan has no `## {heading}` section)"),
        None => "(no plan text was read)".into(),
    }
}

/// The plan's label for the brief and its text. `--plan` wins; else the
/// single plan changed since the merge-base; else the brief says why not.
fn plan_of(
    repo: &Path,
    chosen: Option<&Path>,
    merge_base: &str,
) -> Result<(String, Option<String>), BriefError> {
    let path = match chosen {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => repo.join(p),
        None => {
            let changed = git::stdout_in(
                repo,
                &["diff", "--name-only", merge_base, "HEAD", "--", PLANS_DIR],
            )
            .ok_or_else(|| {
                BriefError::Repository(format!(
                    "`git diff --name-only {merge_base} HEAD -- {PLANS_DIR}` failed"
                ))
            })?;
            let files: Vec<&str> = changed.lines().filter(|l| !l.trim().is_empty()).collect();
            return match files.as_slice() {
                [] => Ok((
                    format!("No plan under {PLANS_DIR}/ changed on this branch."),
                    None,
                )),
                [only] => Ok(match std::fs::read_to_string(repo.join(only)) {
                    Ok(text) => ((*only).to_string(), Some(text)),
                    Err(e) => (format!("{only} (could not be read: {e})"), None),
                }),
                many => Ok((
                    format!(
                        "Several plans under {PLANS_DIR}/ changed ({}); pass --plan.",
                        many.join(", ")
                    ),
                    None,
                )),
            };
        }
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| BriefError::Repository(format!("{}: {e}", path.display())))?;
    let label = chosen.map(|p| p.display().to_string()).unwrap_or_default();
    Ok((label, Some(text)))
}

/// `aval relevant` for every changed file, each key resolved.
fn decisions(repo: &Path, range: &str) -> Result<String, BriefError> {
    let changed = git::stdout_in(repo, &["diff", "--name-only", range])
        .ok_or_else(|| BriefError::Repository(format!("`git diff --name-only {range}` failed")))?;
    let files: Vec<&str> = changed.lines().filter(|l| !l.is_empty()).collect();
    let mut args: Vec<&str> = vec!["--json", "relevant", "--top", "20"];
    for file in &files {
        args.push("--path");
        args.push(file);
    }
    let relevant = aval(repo, &args)?;
    let relevance: serde_json::Value = serde_json::from_slice(&relevant).map_err(|e| {
        BriefError::Aval(AvalFailed(format!(
            "`aval --json relevant` printed output that is not JSON: {e}"
        )))
    })?;
    // Only an empty `keys` array means no decision applies; a missing one is an aval defect.
    let rows = relevance
        .get("keys")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            BriefError::Aval(AvalFailed(
                "`aval --json relevant` printed no `keys` array".to_string(),
            ))
        })?;
    let keys: Vec<String> = rows
        .iter()
        .filter_map(|row| row.get("key").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect();
    let mut out = String::new();
    for key in keys {
        // `undecided`, `retired` and `unknown` are answers that exit non-zero.
        let resolved = run_aval(repo, &["resolve", &key])?;
        if resolved.stdout.iter().all(u8::is_ascii_whitespace) {
            let said = resolved
                .stderr
                .lines()
                .next()
                .unwrap_or("nothing on stderr");
            return Err(BriefError::Aval(AvalFailed(format!(
                "`aval resolve {key}` printed nothing ({}): {said}",
                resolved.status
            ))));
        }
        out.push_str(&format!(
            "## {key}\n\n```\n{}\n```\n\n",
            String::from_utf8_lossy(&resolved.stdout).trim_end()
        ));
    }
    if out.is_empty() {
        out.push_str("(none)\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "6066ad987edfbbb3a514b6d9247b126a77a316db2e1350c226ffd14593a0ba35";

    #[test]
    fn the_hash_of_a_fixed_byte_string_is_the_known_sha256() {
        // FIPS 180-2 example: "abc".
        assert_eq!(
            sha_of(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_rules_line_round_trips_and_a_short_sha_is_ignored() {
        assert_eq!(
            parse_rules_sha(&format!("brief\n{}\nmore", rules_line(SHA))),
            Some(SHA.to_string())
        );
        assert_eq!(parse_rules_sha("<<<RULES sha=abc123>>>"), None);
        assert_eq!(parse_rules_sha("constraint statements only"), None);
    }

    #[test]
    fn the_three_outcomes_compare_the_prompt_with_the_current_rules() {
        let now: Result<String, AvalFailed> = Ok(SHA.to_string());
        assert_eq!(rules_excerpt(None, &now), "review=rules-missing");
        assert_eq!(
            rules_excerpt(Some(&"0".repeat(64)), &now),
            "review=rules-stale"
        );
        assert_eq!(rules_excerpt(Some(SHA), &now), "review=rules-current");
        let failed: Result<String, AvalFailed> = Err(AvalFailed("x".into()));
        assert_eq!(rules_excerpt(Some(SHA), &failed), "review=rules-unknown");
    }

    #[test]
    fn a_section_runs_to_the_next_heading_and_ignores_fenced_headings() {
        let plan = "# P\n## Verification\nrun it\n```\n## not a heading\n```\n## Non-goals\nnone\n";
        assert_eq!(
            section(plan, "Verification").as_deref(),
            Some("## Verification\nrun it\n```\n## not a heading\n```")
        );
        assert_eq!(
            section(plan, "Non-goals").as_deref(),
            Some("## Non-goals\nnone")
        );
        assert_eq!(section(plan, "Missing"), None);
    }

    #[test]
    fn rules_render_as_heading_statement_body() {
        let json = br#"{"rules":[{"id":"a.b","level":"constraint","statement":"S.","body":"B."}]}"#;
        assert_eq!(
            render_rules(json).unwrap(),
            "## a.b [constraint]\n\nS.\n\nB.\n"
        );
    }

    #[test]
    fn a_retired_rule_is_not_rendered() {
        let json = br#"{"rules":[{"id":"old","level":"constraint","statement":"S","body":"B","active":false},{"id":"new","level":"constraint","statement":"S","body":"B","active":true}]}"#;
        let text = render_rules(json).unwrap();
        assert!(
            text.contains("## new [constraint]") && !text.contains("old"),
            "{text}"
        );
    }

    #[test]
    fn a_rule_without_its_body_is_an_error_naming_it() {
        let json =
            br#"{"rules":[{"id":"x.y","level":"constraint","statement":"s","active":true}]}"#;
        let err = render_rules(json).unwrap_err();
        assert!(err.contains("x.y") && err.contains("body"), "{err}");
    }

    #[test]
    fn a_prose_mention_before_the_rules_line_does_not_hide_it() {
        let prompt = format!(
            "see {RULES_OPEN}<the sha here>>> below\n{}",
            rules_line(SHA)
        );
        assert_eq!(parse_rules_sha(&prompt).as_deref(), Some(SHA));
        let multibyte = format!("{RULES_OPEN}{}é>>>\n{}", "a".repeat(63), rules_line(SHA));
        assert_eq!(parse_rules_sha(&multibyte).as_deref(), Some(SHA));
    }
}
