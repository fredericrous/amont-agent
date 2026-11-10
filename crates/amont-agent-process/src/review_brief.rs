//! The implementation-review brief and the hash of the code rules it carries
//! (decisions ADR-0043, ADR-0044, ADR-0065: a profile's brief carries its own
//! judge rules and an id-and-title index of the rest). One function hashes the `aval` output
//! ([`rules_sha`]); the brief, the CLI and the push check all call it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::file_types;

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

/// What `aval` could not do: it is not installed, or the command line and why
/// it failed.
#[derive(Debug, PartialEq, Eq)]
pub enum AvalFailed {
    /// `AMONT_AGENT_AVAL` is unset and no PATH entry is named `aval` at all.
    NotInstalled,
    Failed(String),
}

impl std::fmt::Display for AvalFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AvalFailed::NotInstalled => f.write_str("aval is not installed"),
            AvalFailed::Failed(why) => f.write_str(why),
        }
    }
}

/// Why a brief could not be built.
#[derive(Debug, PartialEq, Eq)]
pub enum BriefError {
    Aval(AvalFailed),
    Repository(String),
    /// A delta brief was refused: the full brief is the answer.
    DeltaRefused(DeltaRefusal),
}

impl std::fmt::Display for BriefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BriefError::Aval(why) => why.fmt(f),
            BriefError::Repository(why) => f.write_str(why),
            BriefError::DeltaRefused(why) => why.fmt(f),
        }
    }
}

/// Why `review-brief --interdiff` refused. A delta brief is never cut: each
/// refusal names the brief to build instead.
#[derive(Debug, PartialEq, Eq)]
pub enum DeltaRefusal {
    /// The given head, or its merge-base with the base, cannot be read here.
    UnreadableHead(String),
    /// The rule files changed since the given head.
    RulesChanged(String),
    /// This role's last passed review of the branch read another head.
    OtherHead {
        recorded: String,
        profile: Option<String>,
    },
    /// What changed is 60 KB or more.
    TooLarge,
}

impl std::fmt::Display for DeltaRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let full = "run `amont-agent review-brief` without --interdiff for the full brief";
        match self {
            DeltaRefusal::UnreadableHead(head) => write!(
                f,
                "`{head}` is not a commit this repository can read: fetch it, or {full}"
            ),
            DeltaRefusal::RulesChanged(head) => {
                write!(f, "the rule files changed since `{head}`: {full}")
            }
            DeltaRefusal::OtherHead { recorded, profile } => {
                let short = recorded.get(..12).unwrap_or(recorded);
                let role = profile
                    .as_deref()
                    .map(|p| format!(" --profile {p}"))
                    .unwrap_or_default();
                write!(
                    f,
                    "this branch's last passed review read `{short}`: run `amont-agent review-brief --interdiff {short}{role}`, or {full}"
                )
            }
            DeltaRefusal::TooLarge => write!(f, "what changed is 60 KB or more: {full}"),
        }
    }
}

impl From<AvalFailed> for BriefError {
    fn from(why: AvalFailed) -> Self {
        BriefError::Aval(why)
    }
}

/// What an `aval` call answered: how it exited and what it printed.
pub(crate) struct AvalAnswer {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: String,
}

/// The `aval` program to run: `AMONT_AGENT_AVAL` names another one (tests,
/// or an `aval` outside PATH), else `aval` from PATH.
fn aval_program() -> std::ffi::OsString {
    std::env::var_os("AMONT_AGENT_AVAL")
        .filter(|program| !program.is_empty())
        .unwrap_or_else(|| std::ffi::OsString::from("aval"))
}

/// The `adr-lint` program to run: `AMONT_AGENT_ADR_LINT` names another one,
/// else `adr-lint` from PATH.
fn adr_lint_program() -> std::ffi::OsString {
    std::env::var_os("AMONT_AGENT_ADR_LINT")
        .filter(|program| !program.is_empty())
        .unwrap_or_else(|| std::ffi::OsString::from("adr-lint"))
}

/// True only when no PATH entry holds anything named `aval` (with the
/// platform's executable suffixes). A non-executable file, a dangling symlink
/// or a lookup that fails for another reason is a candidate, not absence: the
/// spawn then says what is wrong.
fn no_aval_on_path() -> bool {
    let mut names = vec![std::ffi::OsString::from("aval")];
    if cfg!(windows) {
        let suffixes = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        names.extend(
            suffixes
                .split(';')
                .filter(|suffix| !suffix.is_empty())
                .map(|suffix| std::ffi::OsString::from(format!("aval{suffix}"))),
        );
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).all(|dir| {
        names.iter().all(|name| {
            matches!(
                std::fs::symlink_metadata(dir.join(name)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound
            )
        })
    })
}

/// Runs `aval` with a deadline: a hung `aval` is killed and reported, so the
/// push check that calls it never waits on it for long.
fn run_aval(dir: &Path, args: &[&str]) -> Result<AvalAnswer, AvalFailed> {
    if std::env::var_os("AMONT_AGENT_AVAL").is_none_or(|program| program.is_empty())
        && no_aval_on_path()
    {
        return Err(AvalFailed::NotInstalled);
    }
    run_bounded(&aval_program(), "aval", dir, args)
}

/// Runs a program with stdin from /dev/null and a deadline; `name` is how
/// an error spells the command.
fn run_bounded(
    program: &std::ffi::OsStr,
    name: &str,
    dir: &Path,
    args: &[&str],
) -> Result<AvalAnswer, AvalFailed> {
    run_bounded_for(program, name, dir, args, &[], AVAL_DEADLINE)
}

/// [`run_bounded`] with the caller's own deadline and extra child environment.
pub(crate) fn run_bounded_for(
    program: &std::ffi::OsStr,
    name: &str,
    dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    limit: std::time::Duration,
) -> Result<AvalAnswer, AvalFailed> {
    use std::io::Read;
    let line = format!("{name} {}", args.join(" "));
    let mut child = Command::new(program)
        .args(args)
        .envs(envs.iter().copied())
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AvalFailed::Failed(format!("`{line}` could not run: {e}")))?;
    let mut pipe = child
        .stdout
        .take()
        .ok_or_else(|| AvalFailed::Failed(format!("`{line}` has no stdout")))?;
    let reader = std::thread::spawn(move || {
        let mut printed = Vec::new();
        pipe.read_to_end(&mut printed).map(|_| printed)
    });
    // stderr on its own thread too: a full pipe would otherwise stall aval.
    let mut err_pipe = child
        .stderr
        .take()
        .ok_or_else(|| AvalFailed::Failed(format!("`{line}` has no stderr")))?;
    let err_reader = std::thread::spawn(move || {
        let mut said = Vec::new();
        err_pipe.read_to_end(&mut said).map(|_| said)
    });
    let deadline = std::time::Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(AVAL_POLL);
            }
            Ok(None) => {
                let killed = child.kill().and_then(|()| child.wait());
                return Err(AvalFailed::Failed(format!(
                    "`{line}` timed out after {} s (kill: {killed:?})",
                    limit.as_secs()
                )));
            }
            Err(e) => {
                return Err(AvalFailed::Failed(format!(
                    "`{line}` could not be waited on: {e}"
                )))
            }
        }
    };
    let stdout = reader
        .join()
        .map_err(|_| AvalFailed::Failed(format!("`{line}` output reader panicked")))?
        .map_err(|e| AvalFailed::Failed(format!("`{line}` output could not be read: {e}")))?;
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
        return Err(AvalFailed::Failed(format!(
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

/// The `.adr.yaml` that marks a repository with an aval corpus.
const CORPUS_FILE: &str = ".adr.yaml";

/// The candidates section and block for the brief: what adr-lint printed, or
/// why it could not.
enum Candidates {
    Answered(crate::candidates_block::Document),
    Unavailable(String),
}

/// Asks adr-lint for the candidates the branch adds since `merge_base`.
/// Advisory input: every failure is an `Unavailable` with its cause.
fn candidates(repo: &Path, merge_base: &str) -> Candidates {
    let args = ["candidates", "--base", merge_base, "--json", "--max", "40"];
    let answer = match run_bounded(&adr_lint_program(), "adr-lint", repo, &args) {
        Ok(answer) => answer,
        Err(why) => return Candidates::Unavailable(why.to_string()),
    };
    if !answer.status.success() {
        let said = answer.stderr.lines().next().unwrap_or_default();
        return Candidates::Unavailable(format!(
            "`adr-lint candidates` exited {}: {said}",
            answer.status
        ));
    }
    match crate::candidates_block::parse_document(&answer.stdout) {
        Ok(document) => Candidates::Answered(document),
        Err(why) => Candidates::Unavailable(why),
    }
}

/// A brief and the lines for stderr that go with it.
pub struct Brief {
    pub text: String,
    /// One line each, for the caller to say on stderr.
    pub notices: Vec<String>,
}

/// What the caller chose for a brief.
pub struct BriefRequest {
    pub dir: PathBuf,
    pub plan: Option<PathBuf>,
    pub base: Option<String>,
    /// A code-review profile (`backend`, `lang:rust`, …): adds the judge
    /// rules that role reviews.
    pub profile: Option<String>,
    /// The head an earlier review read: the brief is then the headers and
    /// only what changed since it, not the whole review again.
    pub interdiff: Option<String>,
}

/// The brief, whole, or why it could not be built. Nothing is partial.
pub fn build(request: &BriefRequest) -> Result<Brief, BriefError> {
    let repo = amont_agent_guard::push_target::toplevel(&request.dir).ok_or_else(|| {
        BriefError::Repository(format!(
            "{} is not inside a git repository",
            request.dir.display()
        ))
    })?;
    let has_corpus = repo.join(CORPUS_FILE).is_file();
    let aval_missing;
    let mut judge_section = None;
    let code_rules = if has_corpus {
        match rules(&repo) {
            Ok(code_rules) => {
                aval_missing = false;
                let text = render_rules(&code_rules.json)
                    .map_err(|why| BriefError::Aval(AvalFailed::Failed(why)))?;
                crate::code_review::check_reviewed_by(&code_rules.json)
                    .map_err(BriefError::Repository)?;
                if let Some(profile) = &request.profile {
                    judge_section = Some(
                        crate::code_review::judge_section(&code_rules.json, profile)
                            .map_err(BriefError::Repository)?,
                    );
                }
                Some((code_rules.sha, text))
            }
            Err(AvalFailed::NotInstalled) => {
                aval_missing = true;
                None
            }
            Err(failed) => return Err(failed.into()),
        }
    } else {
        aval_missing = false;
        None
    };
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
    let pin = match &request.profile {
        None => pin_section(&repo, &merge_base, &tree),
        Some(_) => None,
    };
    if let Some(old) = &request.interdiff {
        let mut delta = interdiff_brief(&Delta {
            repo: &repo,
            name: &name,
            tree: &tree,
            base: &base,
            merge_base: &merge_base,
            old,
            rules_sha: code_rules.as_ref().map(|(sha, _)| sha.as_str()),
            profile: request.profile.as_deref(),
        })?;
        if let Some((section, notice)) = pin {
            delta.text.push_str(&section);
            delta.notices.push(notice);
        }
        return Ok(delta);
    }
    let range = format!("{merge_base}...HEAD");
    let exclude_plans = format!(":!{PLANS_DIR}");

    let mut brief = String::new();
    brief.push_str("# Implementation review brief\n\n");
    brief.push_str(&crate::implementation_review::block(&name, &tree));
    brief.push('\n');
    if let Some((sha, _)) = &code_rules {
        brief.push_str(&rules_line(sha));
        brief.push('\n');
    }
    // Candidates are implementation-review's alone (ADR-0052): one ruling
    // per id, so a profile's brief carries neither the block nor the list.
    let found = match &request.profile {
        None => Some(candidates(&repo, &merge_base)),
        Some(_) => None,
    };
    match &found {
        Some(Candidates::Answered(document)) => {
            brief.push_str(&crate::candidates_block::block(document));
            brief.push_str("\n\n");
        }
        Some(Candidates::Unavailable(_)) => {
            brief.push_str(crate::candidates_block::UNAVAILABLE);
            brief.push_str("\n\n");
        }
        None => brief.push('\n'),
    }
    if let Some(profile) = &request.profile {
        brief.push_str(&format!("Profile: {profile}\n\n"));
    }

    let (plan_label, plan_text) = plan_of(&repo, request.plan.as_deref(), &merge_base)?;
    brief.push_str(&format!("## Plan\n\n{plan_label}\n\n"));
    if request.profile.is_none() {
        brief.push_str(
            &crate::code_review::declared_line(&name, plan_text.as_deref())
                .map_err(BriefError::Repository)?,
        );
        brief.push_str("\n\n");
    }
    let verification = plan_text
        .as_deref()
        .and_then(|t| section(t, "Verification"))
        .unwrap_or_else(|| missing_section(plan_text.as_deref(), "Verification"));
    brief.push_str(&format!("{verification}\n\n"));

    // A profile whose file types filter the diff reads its own part of it
    // (ADR-0061, `work.review-slices`); the stat stays whole, so what it did
    // not read is in sight. Renames read as a deletion and an addition, so
    // each language sees its own side of a move.
    let selection = select(&repo, &[&range], request.profile.as_deref())?;
    let mut stat_args = vec!["diff"];
    if selection.is_some() {
        stat_args.push("--no-renames");
    }
    stat_args.extend([range.as_str(), "--stat", "--", ".", &exclude_plans]);
    let stat = git::stdout_in(&repo, &stat_args)
        .ok_or_else(|| BriefError::Repository(format!("`git diff {range} --stat` failed")))?;
    brief.push_str(&format!("## Diff stat\n\n```\n{stat}\n```\n\n"));
    let mut notices = Vec::new();
    let diff = match &selection {
        None => git::bytes_in(&repo, &["diff", &range, "--", ".", &exclude_plans], None)
            .ok()
            .flatten()
            .ok_or_else(|| BriefError::Repository(format!("`git diff {range}` failed")))?,
        Some(chosen) => {
            let patch = filtered_patch(
                &repo,
                &["diff", "--no-renames"],
                &[&range],
                &chosen.kept,
                argv_budget(),
            )?;
            notices.push(chosen.notice(patch.len()));
            patch
        }
    };
    if selection.as_ref().is_some_and(|c| c.kept.is_empty()) {
        brief.push_str(&format!(
            "## Diff\n\nNo changed file is of {}'s types: this brief carries no patch.\n\n",
            request.profile.as_deref().unwrap_or_default()
        ));
    } else if diff.len() < FULL_DIFF_LIMIT_BYTES {
        brief.push_str(&format!(
            "## Diff\n\n```diff\n{}\n```\n\n",
            String::from_utf8_lossy(&diff)
        ));
    } else {
        brief.push_str(
            "## Diff\n\nThe diff is 60 KB or more: read the changed files in the worktree.\n\n",
        );
    }
    if let Some(chosen) = &selection {
        brief.push_str(&chosen.not_in_brief());
    }

    let section_text = match &found {
        Some(Candidates::Answered(document)) => Some(crate::candidates_block::section(document)),
        Some(Candidates::Unavailable(cause)) => {
            Some(crate::candidates_block::unavailable_section(cause))
        }
        None => None,
    };
    if let Some(section_text) = section_text {
        brief.push_str(&format!("{section_text}\n"));
    }

    if let Some(Candidates::Unavailable(cause)) = &found {
        notices.push(format!("adr-lint's candidates are unavailable: {cause}"));
    }
    match &code_rules {
        Some((_, rules_text)) => {
            let index = aval(&repo, &["rules", "--level", "constraint"])?;
            let index = String::from_utf8_lossy(&index);
            match &judge_section {
                // A profile seat reads the rules it judges in full and an id
                // index of the rest (ADR-0065): implementation-review reads
                // every code rule, so none loses its reader.
                Some(section) => {
                    brief.push_str(&format!("{section}\n"));
                    brief.push_str(&format!(
                        "# Constraint rules (index)\n\n\
                         The other constraints, by id and title. Implementation-review \
                         reads every code rule in full. Fetch one with the `aval_rule` \
                         tool or `aval rule <id>` only when your review needs it.\n\n{}\n\n",
                        constraint_index(&index)
                    ));
                }
                None => {
                    brief.push_str(&format!("# Code rules (in full)\n\n{rules_text}\n"));
                    brief.push_str(&format!("# Constraint rules\n\n{}\n\n", index.trim_end()));
                }
            }

            brief.push_str("# Decisions that apply to the changed paths\n\n");
            brief.push_str(&decisions(&repo, &range)?);
        }
        None if aval_missing => {
            if let Some(profile) = &request.profile {
                brief.push_str(&format!(
                    "## Judge rules for {profile}\n\nNot read: aval is not installed.\n\n"
                ));
            }
            brief.push_str(
                "# Code rules\n\nThis repository has an aval corpus (.adr.yaml), but aval is not installed, so its decided code rules were not read. Review against the plan; do not treat the code as unconstrained.\n",
            );
            notices.push(format!(
                "{} has {CORPUS_FILE} but aval is not installed: the brief carries no decided code rules; install it (brew install fredericrous/tap/aval) or set AMONT_AGENT_AVAL",
                repo.display()
            ));
        }
        None => {
            if let Some(profile) = &request.profile {
                brief.push_str(&format!(
                    "## Judge rules for {profile}\n\nNone: this repository has no aval corpus.\n\n"
                ));
            }
            brief.push_str(&format!(
                "# Code rules\n\nThis repository has no aval corpus (no {CORPUS_FILE}), so no decided code rules apply.\n"
            ));
            notices.push(format!(
                "{} has no {CORPUS_FILE}: the brief carries no decided code rules",
                repo.display()
            ));
        }
    }

    // A profile reads a slice of the work: what the plan is for and how it
    // behaves is what that slice is judged against.
    let mut closing = Vec::new();
    if request.profile.is_some() {
        closing.extend(["Goal", "Behaviour"]);
    }
    closing.push("Non-goals");
    for heading in closing {
        let text = plan_text
            .as_deref()
            .and_then(|t| section(t, heading))
            .unwrap_or_else(|| missing_section(plan_text.as_deref(), heading));
        brief.push_str(&format!("\n{text}\n"));
    }
    if let Some((section, notice)) = pin {
        brief.push_str(&section);
        notices.push(notice);
    }
    Ok(Brief {
        text: brief,
        notices,
    })
}

/// How long the pin check gives `git diff` to fingerprint the branch.
const PIN_FINGERPRINT_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

/// The "Pin check" section and its stderr notice for a branch whose
/// fingerprint is a registered block. The check's result is also written as
/// the latest pin record of this tree, a pass or a failure, where the hook
/// reads it. `None` when the branch is not a registered block.
fn pin_section(repo: &Path, merge_base: &str, tree: &str) -> Option<(String, String)> {
    let fingerprint = crate::block_fingerprint::fingerprint(
        repo,
        merge_base,
        "HEAD",
        std::time::Instant::now() + PIN_FINGERPRINT_BUDGET,
    )
    .ok()??;
    let base = amont_agent_guard::journal::dir()?;
    if crate::block_registry::entries(&base, fingerprint.full()).is_empty() {
        return None;
    }
    let outcome = crate::pin_check::check(repo, merge_base);
    let record = crate::block_registry::Pin {
        passed: outcome.passed,
        detail: outcome.detail.clone(),
        gh_override_set: crate::pin_check::gh_override_set(),
    };
    let recorded = match crate::block_registry::write_pin(&base, fingerprint.full(), tree, &record)
    {
        Ok(()) => String::new(),
        Err(why) => format!(" (the record could not be written: {why})"),
    };
    let verdict = if outcome.passed { "passed" } else { "FAILED" };
    let section = format!(
        "\n## Pin check\n\n{}: {verdict}: {}{recorded}\n",
        fingerprint.short(),
        outcome.detail
    );
    let notice = format!(
        "pin check {verdict} for {}: {}{recorded}",
        fingerprint.short(),
        outcome.detail
    );
    Some((section, notice))
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
        BriefError::Aval(AvalFailed::Failed(format!(
            "`aval --json relevant` printed output that is not JSON: {e}"
        )))
    })?;
    // Only an empty `keys` array means no decision applies; a missing one is an aval defect.
    let rows = relevance
        .get("keys")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            BriefError::Aval(AvalFailed::Failed(
                "`aval --json relevant` printed no `keys` array".to_string(),
            ))
        })?;
    let mut keys: Vec<String> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let key = row
            .get("key")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                BriefError::Aval(AvalFailed::Failed(format!(
                    "`aval --json relevant` row {index} of `keys` has no string `key`"
                )))
            })?;
        keys.push(key.to_string());
    }
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
            return Err(BriefError::Aval(AvalFailed::Failed(format!(
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

/// What a delta brief is built from.
struct Delta<'a> {
    repo: &'a Path,
    name: &'a str,
    tree: &'a str,
    base: &'a str,
    merge_base: &'a str,
    /// The head the earlier review read, as given.
    old: &'a str,
    rules_sha: Option<&'a str>,
    profile: Option<&'a str>,
}

/// The diff options every patch of a delta brief is taken with, so a
/// person's diff configuration never changes its bytes.
const DIFF_PINNED: [&str; 5] = [
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

/// The delta brief after a rebase or a small fix: the same headers as the
/// full brief, then what changed since the head the earlier review read,
/// `docs/plans/` left out: the plain diff when that head is an ancestor of
/// HEAD, else the diff between the two branch patches. (`git range-diff`
/// lists a commit added on top without its patch, so it would hide a fix.)
/// The plan, the full diff and the rules are not repeated.
///
/// Refused ([`DeltaRefusal`]) when the head or its merge-base cannot be
/// read, when the rule files changed since it, when this role's last pass
/// of the branch read another head, or when what changed is 60 KB or more.
fn interdiff_brief(delta: &Delta) -> Result<Brief, BriefError> {
    let Delta {
        repo,
        name,
        tree,
        base,
        merge_base,
        old,
        rules_sha,
        profile,
    } = *delta;
    let refuse = |why: DeltaRefusal| Err(BriefError::DeltaRefused(why));
    let Some(old_head) = git::stdout_in(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{old}^{{commit}}"),
        ],
    ) else {
        return refuse(DeltaRefusal::UnreadableHead(old.to_string()));
    };
    let Some(old_base) = git::stdout_in(repo, &["merge-base", &old_head, base]) else {
        return refuse(DeltaRefusal::UnreadableHead(old.to_string()));
    };
    let rules = |rev: &str| {
        crate::implementation_review::rules_fingerprint(repo, rev).map_err(BriefError::Repository)
    };
    if rules(&old_head)? != rules("HEAD")? {
        return refuse(DeltaRefusal::RulesChanged(old.to_string()));
    }
    let mut notices = Vec::new();
    let recorded = crate::implementation_review::branch_ref(repo)
        .and_then(|dst| crate::implementation_review::reviewed_head(name, profile, &dst));
    match recorded {
        Some(pass) if pass.head != old_head => {
            return refuse(DeltaRefusal::OtherHead {
                recorded: pass.head,
                profile: profile.map(str::to_string),
            })
        }
        Some(_) => {}
        None => notices.push(format!(
            "no passed review of this branch is recorded here; `{old}` is taken as given"
        )),
    }
    let exclude_plans = format!(":!{PLANS_DIR}");
    // Each patch is filtered on its own range's paths, so a rebased delta
    // compares the profile's part of each branch patch.
    let mut omitted: BTreeSet<String> = BTreeSet::new();
    let mut kept: BTreeSet<String> = BTreeSet::new();
    let filtered = profile.is_some_and(|p| file_types::profile_reads(p).is_some());
    let mut patch = |from: &str, to: &str| -> Result<Vec<u8>, BriefError> {
        let mut args = vec!["diff"];
        args.extend(DIFF_PINNED);
        match select(repo, &[from, to], profile)? {
            None => {
                args.extend([from, to, "--", ".", &exclude_plans]);
                git::bytes_in(repo, &args, None)
                    .ok()
                    .flatten()
                    .ok_or_else(|| BriefError::Repository(format!("`git diff {from} {to}` failed")))
            }
            Some(chosen) => {
                args.push("--no-renames");
                kept.extend(chosen.kept.iter().cloned());
                omitted.extend(chosen.omitted.iter().cloned());
                filtered_patch(repo, &args, &[from, to], &chosen.kept, argv_budget())
            }
        }
    };
    let on_top =
        git::stdout_in(repo, &["merge-base", "--is-ancestor", &old_head, "HEAD"]).is_some();
    let (shape, interdiff) = if on_top {
        ("the diff from it to HEAD", patch(&old_head, "HEAD")?)
    } else {
        let before = patch(&old_base, &old_head)?;
        let after = patch(merge_base, "HEAD")?;
        let between = diff_of(&before, &after).map_err(|e| {
            BriefError::Repository(format!("the two branch patches could not be compared: {e}"))
        })?;
        ("the diff between its branch patch and HEAD's", between)
    };
    if interdiff.len() >= FULL_DIFF_LIMIT_BYTES {
        return refuse(DeltaRefusal::TooLarge);
    }

    let mut brief = String::new();
    brief.push_str("# Implementation review delta brief\n\n");
    brief.push_str(&crate::implementation_review::block(name, tree));
    brief.push('\n');
    if let Some(sha) = rules_sha {
        brief.push_str(&rules_line(sha));
        brief.push('\n');
    }
    match profile {
        None => match candidates(repo, merge_base) {
            Candidates::Answered(document) => {
                brief.push_str(&crate::candidates_block::block(&document));
                brief.push_str("\n\n");
            }
            Candidates::Unavailable(why) => {
                brief.push_str(crate::candidates_block::UNAVAILABLE);
                brief.push_str("\n\n");
                notices.push(format!("candidates unavailable: {why}"));
            }
        },
        Some(role) => brief.push_str(&format!("\nProfile: {role}\n\n")),
    }
    let left_out = match profile {
        Some(role) if filtered => {
            format!("docs/plans and the files outside {role}'s types left out")
        }
        _ => "docs/plans left out".to_string(),
    };
    brief.push_str(&format!(
        "## Interdiff since {old}\n\n\
         The earlier review read `{old_head}` on base `{old_base}`. This is \
         {shape} (base `{merge_base}`), {left_out}. Your prompt \
         carries `Delta.` and your round-1 findings: judge what changed \
         against them. Everything else is what the earlier review read.\n\n",
    ));
    if filtered && kept.is_empty() {
        brief.push_str(&format!(
            "No changed file is of {}'s types: this delta carries no patch.\n",
            profile.unwrap_or_default()
        ));
    } else {
        brief.push_str(&format!(
            "```diff\n{}\n```\n",
            String::from_utf8_lossy(&interdiff)
        ));
    }
    if filtered {
        notices.push(format!(
            "{}: {} of {} changed files in this delta ({} bytes of patch)",
            profile.unwrap_or_default(),
            kept.len(),
            kept.len() + omitted.len(),
            interdiff.len()
        ));
    }
    if filtered && !omitted.is_empty() {
        brief.push_str(&format!(
            "\n{}",
            not_in_brief(profile.unwrap_or_default(), omitted.iter())
        ));
    }
    crate::implementation_review::note(
        "interdiff-brief",
        "-",
        name,
        &format!(
            "head={} bytes={} shape={}{}",
            old_head.get(..12).unwrap_or(&old_head),
            interdiff.len(),
            if on_top { "on-top" } else { "rebased" },
            if filtered {
                format!(" kept={} omitted={}", kept.len(), omitted.len())
            } else {
                String::new()
            }
        ),
    );
    Ok(Brief {
        text: brief,
        notices,
    })
}

/// What a profile reads of a range when its file types filter the diff:
/// the changed paths it keeps and the ones it leaves out.
struct Selection {
    profile: String,
    kept: Vec<String>,
    omitted: Vec<String>,
}

impl Selection {
    fn notice(&self, bytes: usize) -> String {
        format!(
            "{}: {} of {} changed files in this brief ({bytes} bytes of patch)",
            self.profile,
            self.kept.len(),
            self.kept.len() + self.omitted.len()
        )
    }

    fn not_in_brief(&self) -> String {
        if self.omitted.is_empty() {
            return String::new();
        }
        not_in_brief(&self.profile, self.omitted.iter())
    }
}

/// `## Not in this brief`: the changed files outside a profile's types.
fn not_in_brief<'a>(profile: &str, paths: impl Iterator<Item = &'a String>) -> String {
    let mut out = format!(
        "## Not in this brief\n\nChanged, and not of {profile}'s types, so their patch is not here (the stat lists them):\n\n"
    );
    for p in paths {
        out.push_str(&format!("- `{p}`\n"));
    }
    out.push('\n');
    out
}

/// The paths `revs` changes (one `a...b` argument or two revisions), a
/// rename read as a deletion and an addition, docs/plans left out by prefix
/// rather than by pathspec magic, so `GIT_LITERAL_PATHSPECS` cannot bring
/// the plans back. Second: the names that are not UTF-8, which a literal
/// pathspec could not give back to git; they are listed, never in a patch.
fn changed_paths(repo: &Path, revs: &[&str]) -> Result<(Vec<String>, Vec<String>), BriefError> {
    let mut args = vec!["diff", "--no-renames", "--name-only", "-z"];
    args.extend(revs);
    let out = run_git(
        repo,
        &args,
        &format!("`git diff --name-only {}`", revs.join(" ")),
    )?;
    let plans = format!("{PLANS_DIR}/");
    let (mut readable, mut unreadable) = (Vec::new(), Vec::new());
    for name in out.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        match std::str::from_utf8(name) {
            Ok(path) if path.starts_with(&plans) => {}
            Ok(path) => readable.push(path.to_string()),
            Err(_) => unreadable.push(format!(
                "{} (not UTF-8: not in this patch)",
                String::from_utf8_lossy(name)
            )),
        }
    }
    Ok((readable, unreadable))
}

/// One git call's stdout, or an error that says which call failed and
/// whether git could not start or exited non-zero.
fn run_git(repo: &Path, args: &[&str], what: &str) -> Result<Vec<u8>, BriefError> {
    match git::bytes_in(repo, args, None) {
        Ok(Some(out)) => Ok(out),
        Ok(None) => Err(BriefError::Repository(format!("{what} failed"))),
        Err(e) => Err(BriefError::Repository(format!(
            "{what} could not be run: {e}"
        ))),
    }
}

/// `None`: no profile, or one that reads the whole diff (backend, architect,
/// unix, tui). Else the paths [`file_types::profile_reads`] keeps.
fn select(
    repo: &Path,
    revs: &[&str],
    profile: Option<&str>,
) -> Result<Option<Selection>, BriefError> {
    let Some(profile) = profile else {
        return Ok(None);
    };
    let Some(reads) = file_types::profile_reads(profile) else {
        return Ok(None);
    };
    let (readable, unreadable) = changed_paths(repo, revs)?;
    let (kept, mut omitted): (Vec<String>, Vec<String>) =
        readable.into_iter().partition(|p| reads(p));
    omitted.extend(unreadable);
    Ok(Some(Selection {
        profile: profile.to_string(),
        kept,
        omitted,
    }))
}

/// The bytes one git call may spend on pathspecs, the environment taken
/// off: far under any system's `ARG_MAX` (1 MiB on macOS, 2 MiB on Linux),
/// which counts the environment too. `git diff` has no
/// `--pathspec-from-file`, so a long list is split into batches.
const ARGV_BUDGET: usize = 128 * 1024;
const ARGV_FLOOR: usize = 16 * 1024;

fn argv_budget() -> usize {
    let env: usize = std::env::vars_os()
        .map(|(k, v)| k.len() + v.len() + 2)
        .sum();
    ARGV_BUDGET.saturating_sub(env).max(ARGV_FLOOR)
}

/// `paths` in order, split so each batch's arguments fit `budget` bytes
/// (each costs its bytes, a NUL and a pointer); every path in exactly one
/// batch, and a path longer than the budget alone in its own.
fn batches(paths: &[String], budget: usize) -> Vec<&[String]> {
    let mut out = Vec::new();
    let (mut start, mut used) = (0, 0);
    for (i, p) in paths.iter().enumerate() {
        let cost = p.len() + 1 + std::mem::size_of::<usize>();
        if i > start && used + cost > budget {
            out.push(&paths[start..i]);
            (start, used) = (i, 0);
        }
        used += cost;
    }
    if start < paths.len() {
        out.push(&paths[start..]);
    }
    out
}

/// The patch of `revs` restricted to `kept`, batch after batch in git's
/// path order, so no file appears twice. Pathspecs are literal
/// (`--literal-pathspecs`): a name is never read as a glob or as magic.
/// Nothing kept is an empty patch, never the unrestricted diff.
fn filtered_patch(
    repo: &Path,
    command: &[&str],
    revs: &[&str],
    kept: &[String],
    budget: usize,
) -> Result<Vec<u8>, BriefError> {
    let mut out = Vec::new();
    let all = batches(kept, budget);
    for (i, batch) in all.iter().enumerate() {
        let mut args = vec!["--literal-pathspecs"];
        args.extend(command);
        args.extend(revs);
        args.push("--");
        args.extend(batch.iter().map(String::as_str));
        let what = format!(
            "`git diff {}` (batch {} of {}, {} paths)",
            revs.join(" "),
            i + 1,
            all.len(),
            batch.len()
        );
        out.extend(run_git(repo, &args, &what)?);
    }
    Ok(out)
}

/// The unified diff from one branch patch to another: what a rebase changed
/// in what the reviewer read. `git diff --no-index` over two files in a
/// directory made for this call alone (mode 0700, refused if it exists).
fn diff_of(before: &[u8], after: &[u8]) -> std::io::Result<Vec<u8>> {
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    let dir = std::env::temp_dir().join(format!(
        "amont-agent-interdiff-{}-{call}-{nanos}",
        std::process::id()
    ));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(&dir)?;
    let compared = (|| {
        std::fs::write(dir.join("reviewed.patch"), before)?;
        std::fs::write(dir.join("pushed.patch"), after)?;
        let mut args = vec!["diff", "--no-index"];
        args.extend(DIFF_PINNED);
        args.extend(["reviewed.patch", "pushed.patch"]);
        std::process::Command::new("git")
            .args(&args)
            .current_dir(&dir)
            .output()
    })();
    let _ = std::fs::remove_dir_all(&dir); // best effort: this call's own temp dir
    let out = compared?;
    // `--no-index` exits 1 when the files differ: that is the answer.
    match out.status.code() {
        Some(0 | 1) => Ok(out.stdout),
        _ => Err(std::io::Error::other(format!(
            "`git diff --no-index` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// One line per rule of `aval rules --level constraint` (`constraint <id>
/// <statement>`): the id and a title, the statement's first 72 characters
/// cut at a word. Lines that are not a constraint row are left out.
fn constraint_index(index: &str) -> String {
    index
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            if words.next() != Some("constraint") {
                return None;
            }
            let id = words.next()?;
            let statement = words.collect::<Vec<_>>().join(" ");
            let title = if statement.chars().count() <= 72 {
                statement
            } else {
                let cut: String = statement.chars().take(72).collect();
                format!(
                    "{}…",
                    cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head)
                )
            };
            Some(format!("- {id}: {title}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_constraint_index_keeps_ids_and_short_titles_of_constraint_rows() {
        let long = "word ".repeat(30);
        let index = super::constraint_index(&format!(
            "constraint errors.typed   A typed error.\nheader line\nconstraint names.x {long}\n"
        ));
        let lines: Vec<&str> = index.lines().collect();
        assert_eq!(lines[0], "- errors.typed: A typed error.");
        assert!(
            lines[1].starts_with("- names.x: word") && lines[1].ends_with('…'),
            "{index}"
        );
        assert!(lines[1].chars().count() < 90, "{index}");
        assert_eq!(lines.len(), 2, "a non-constraint row is left out: {index}");
    }

    /// Forced into one batch per path, the joined patch is byte for byte
    /// the one diff of the same paths: same order, no file twice.
    #[test]
    fn a_patch_in_batches_is_the_one_diff() {
        use std::process::Command;
        let repo = std::env::temp_dir().join(format!(
            "amont-agent-batches-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&repo); // a leftover of an earlier run, or none
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
            out.stdout
        };
        git(&["init", "-q", "-b", "main"]);
        // `d[a].ts` is a glob that also matches `da.ts`: read as a pattern,
        // its batch would carry `da.ts` a second time. Legal on Windows,
        // unlike `*`. In git's byte order, as the whole diff lists them.
        let names = ["a.ts", "b c.ts", "d[a].ts", "da.ts", "z.ts"];
        for (round, text) in [(0, "one"), (1, "two")] {
            for name in names {
                std::fs::write(repo.join(name), format!("{text} {name}\n")).unwrap();
            }
            git(&["add", "-A"]);
            git(&["commit", "-qm", &round.to_string()]);
        }
        let kept: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        let joined = super::filtered_patch(
            &repo,
            &["diff", "--no-renames"],
            &["HEAD~1", "HEAD"],
            &kept,
            1,
        )
        .unwrap();
        let mut whole = vec![
            "--literal-pathspecs",
            "diff",
            "--no-renames",
            "HEAD~1",
            "HEAD",
            "--",
        ];
        whole.extend(names);
        assert_eq!(
            String::from_utf8_lossy(&joined),
            String::from_utf8_lossy(&git(&whole))
        );
        assert_eq!(joined.windows(10).filter(|w| w == b"diff --git").count(), 5);
        let empty = super::filtered_patch(&repo, &["diff"], &["HEAD~1", "HEAD"], &[], 1).unwrap();
        assert!(
            empty.is_empty(),
            "nothing kept is no patch, never the whole diff"
        );
        let _ = std::fs::remove_dir_all(&repo); // scratch under the temp dir: the OS reclaims it
    }

    #[test]
    fn batches_keep_order_put_each_path_once_and_fit_the_budget() {
        let paths: Vec<String> = (0..50).map(|i| format!("src/file_{i:03}.ts")).collect();
        let cost = |p: &String| p.len() + 1 + std::mem::size_of::<usize>();
        let budget = 10 * cost(&paths[0]) + 3;
        let got = super::batches(&paths, budget);
        assert_eq!(got.len(), 5, "{got:?}");
        assert!(got
            .iter()
            .all(|b| b.iter().map(cost).sum::<usize>() <= budget));
        assert_eq!(got.concat(), paths);
        let long = vec!["x".repeat(500), "y.ts".to_string()];
        assert_eq!(super::batches(&long, 100).len(), 2);
        assert!(super::batches(&[], 100).is_empty());
    }

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
        let failed: Result<String, AvalFailed> = Err(AvalFailed::Failed("x".into()));
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
