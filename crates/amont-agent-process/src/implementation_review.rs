//! The implementation review (ADR-0022, `work.implementation-review`):
//! before the push of a branch that carries a plan, one independent
//! reviewer has read the diff against the plan and the active rules, and
//! its verdict is bound to the tree it read.
//!
//! A review is bound to the **content id** of the commit it read (attest
//! SPEC, "Content id"; not attest's canonical tree, which counts whether a
//! plan exists): the sha256 of `git ls-tree -r -z --full-tree <rev>`'s entries — mode,
//! type, object id and path, in git's own order — with every entry under
//! `docs/plans/` left out. So recording the review in the plan never makes
//! it stale, a later code change does, and the id is 64 hex whatever the
//! repository's object format. It is computed read-only: no index, no
//! object written, nothing to clean up if the hook is killed.
//!
//! The repository in a block is named by the basename of the parent of
//! `git rev-parse --git-common-dir`, so a worktree checked out as
//! `amont-agent-wt-x` still says `amont-agent`, and `tree-sha` and the
//! rule cannot disagree on the name.
//!
//! ## What the hook reads
//!
//! A push is judged by `confirm` (the rule is `rules::implementation_review`):
//! the pushed commit, the base it grew from (the remote's default branch,
//! never the branch's own tracking ref — once a branch is published its
//! plan commit is on the remote too, and a later push compared to the
//! tracking ref would show no `docs/plans/` change and skip the rule for
//! good), whether `docs/plans/` changed since that base, the content id,
//! and then a binding: a completed `implementation-review` agent in this
//! session's transcript whose block names this repository and tree, or a
//! pass remembered under `<state>/implementation-review/by-tree/`.
//!
//! A pass also holds for another tree with the same **branch patch** when
//! the repository's rule files are unchanged (ADR-0034,
//! `work.implementation-review-binding`): a conflict-free rebase onto a
//! moved `main` changes the tree, not the diff the reviewer read. The patch
//! is `git diff --binary <merge-base> <pushed> -- . ':!docs/plans'` hashed by
//! `git patch-id --verbatim`; the rules are `AGENTS.md`, `CLAUDE.md`,
//! `.adr.yaml`, `.adr/` and `.claude/` in the pushed tree. A pass is filed
//! under `by-patch/` too, and a hit there is journalled `reviewed-by-patch`.
//!
//! The verdict is read, not assumed: a review that said `rework` and got
//! no later delta on the same tree is not a pass. Only the hook writes a
//! pass file — from a transcript binding, or from the person's `Overrule`
//! on a marked question — and a Bash, Write or Edit under that directory
//! is refused by the hook itself, outside any stance, because under `deny`
//! a pass file is a trust boundary.

use std::path::{Component, Path, PathBuf};

use crate::candidates_block::Reviewed;
use crate::content_id::content_id;
use crate::plan_review::{completed_agents_resumable, Completed};
use crate::provenance::Review as Bound;
use crate::provenance::{ProfileHow, ProfileVerdict};
use amont_agent_guard::git;
use amont_agent_guard::journal;
use amont_agent_guard::rules::{Confirmed, Context, Stance};
use amont_agent_guard::sha256::{hex, sha256};
use amont_agent_guard::shell::Simple;

pub const RULE_ID: &str = "implementation-review";
/// The agent type the reviewer is launched as; a prefix, like the panel's.
const AGENT: &str = "implementation-review";
/// Opens a review block in the reviewer's prompt.
pub const BLOCK_OPEN: &str = "<<<TREE ";
const BLOCK_CLOSE: &str = ">>>";
/// The directory a review does not bind to: the plan's own record.
const PLANS_PATH: &str = "docs/plans";
/// The marker a person's overrule question carries.
const MARKER: &str = "[implementation-review ";
const OVERRULE: &str = "Overrule";
/// Follows the tree in a marker that overrules a code-review profile.
const PROFILE_KEY: &str = " profile=";

// ---------------------------------------------------------------- the tree

/// The files whose content a review was read against, besides the diff:
/// the repository's active rules.
const RULE_FILES: &[&[u8]] = &[b"AGENTS.md", b"CLAUDE.md", b".adr.yaml"];
const RULE_DIRS: &[&[u8]] = &[b".adr/", b".claude/"];

/// What a pass carries over to another tree: the branch patch and the
/// fingerprint of the rule files. Both must match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchKey {
    pub patch: String,
    pub rules: String,
}

/// The `git patch-id --verbatim` of the branch's diff since `merge_base`,
/// `docs/plans/` left out. `Err` on an empty diff or any failure: a key
/// that cannot be computed carries nothing over.
pub fn branch_patch(repo: &Path, merge_base: &str, pushed: &str) -> Result<String, String> {
    let diff = git::bytes_in(
        repo,
        &[
            "diff",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            merge_base,
            pushed,
            "--",
            ".",
            &format!(":!{PLANS_PATH}"),
        ],
        None,
    )
    .map_err(|e| format!("git could not be run: {e}"))?
    .ok_or("`git diff` against the base failed")?;
    if diff.is_empty() {
        return Err("the branch changes nothing outside docs/plans/".into());
    }
    let out = git::bytes_in(repo, &["patch-id", "--verbatim"], Some(&diff))
        .map_err(|e| format!("git could not be run: {e}"))?
        .ok_or("`git patch-id` failed")?;
    let out = String::from_utf8_lossy(&out);
    out.split_whitespace()
        .next()
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_string)
        .ok_or_else(|| "`git patch-id` printed no id".into())
}

/// The sha256 of the rule files' entries in `rev`'s tree.
pub fn rules_fingerprint(repo: &Path, rev: &str) -> Result<String, String> {
    let spec = format!("{rev}^{{tree}}");
    let entries = git::ls_tree_entries(repo, &spec)
        .map_err(|e| format!("git could not be run in {}: {e}", repo.display()))?
        .ok_or_else(|| format!("`{rev}` does not name a tree in {}", repo.display()))?;
    let mut bytes: Vec<u8> = Vec::new();
    for entry in entries {
        let path = entry
            .iter()
            .position(|c| *c == b'\t')
            .map(|i| &entry[i + 1..])
            .unwrap_or(&entry[..]);
        if RULE_FILES.contains(&path) || RULE_DIRS.iter().any(|d| path.starts_with(d)) {
            bytes.extend_from_slice(&entry);
            bytes.push(0);
        }
    }
    Ok(hex(&sha256(&bytes)))
}

/// Both halves of the key for `pushed`, or why either cannot be computed:
/// a miss, never a pass, and the caller says the reason when it holds.
pub fn patch_key(repo: &Path, merge_base: &str, pushed: &str) -> Result<PatchKey, String> {
    Ok(PatchKey {
        patch: branch_patch(repo, merge_base, pushed)?,
        rules: rules_fingerprint(repo, pushed)?,
    })
}

/// The repository's name for a block: the basename of the directory that
/// holds its common `.git`, whichever worktree `repo` is.
pub fn repo_name(repo: &Path) -> Option<String> {
    let common = git::stdout_in(repo, &["rev-parse", "--git-common-dir"])?;
    if common.is_empty() {
        return None;
    }
    let common = PathBuf::from(common);
    let common = if common.is_absolute() {
        common
    } else {
        repo.join(common)
    };
    let common = std::fs::canonicalize(&common).unwrap_or(common);
    common
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
}

/// The review block a reviewer's prompt carries.
pub fn block(repo: &str, sha: &str) -> String {
    format!("{BLOCK_OPEN}repo={repo} sha={sha}{BLOCK_CLOSE}")
}

/// The `(repo, sha)` of every review block in a prompt. A sha that is not
/// 64 hex is not a binding.
pub fn parse_blocks(prompt: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = prompt;
    while let Some(at) = rest.find(BLOCK_OPEN) {
        rest = &rest[at + BLOCK_OPEN.len()..];
        let Some(end) = rest.find(BLOCK_CLOSE) else {
            break;
        };
        let inner = &rest[..end];
        rest = &rest[end..];
        let Some(body) = inner.strip_prefix("repo=") else {
            continue;
        };
        let Some(split) = body.rfind(" sha=") else {
            continue;
        };
        let repo = body[..split].trim().to_string();
        let sha = body[split + 5..].trim().to_ascii_lowercase();
        if repo.is_empty() || !is_sha256(&sha) {
            continue;
        }
        out.push((repo, sha));
    }
    out
}

pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

// ---------------------------------------------------------------- the verdict

/// What a reviewer's result said, read from its first `Verdict:` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    ApproveWithChanges,
    Rework,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Approve => "approve",
            Verdict::ApproveWithChanges => "approve-with-changes",
            Verdict::Rework => "rework",
        }
    }
}

/// The verdict a result text states, or `None` when it states none the
/// contract knows — which is not a pass.
pub fn verdict_of(result: &str) -> Option<Verdict> {
    let line = result.lines().find(|l| l.contains("Verdict:"))?;
    let after = line.split_once("Verdict:")?.1;
    let word: String = after
        .trim()
        .trim_start_matches(['*', '`', '_', ' '])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase();
    match word.as_str() {
        "approve-with-changes" => Some(Verdict::ApproveWithChanges),
        "approve" | "approved" => Some(Verdict::Approve),
        "rework" => Some(Verdict::Rework),
        _ => None,
    }
}

/// The reviews this session completed, in launch order: what each bound
/// and what it said. A reviewer resumed with `SendMessage` on a new block is
/// a review of its own ([`completed_agents_resumable`]).
pub struct Review {
    pub repo: String,
    pub sha: String,
    pub verdict: Option<Verdict>,
    /// The sha of the prompt's `<<<RULES …>>>` line, when it has a valid one.
    pub rules_sha: Option<String>,
    /// The candidates the prompt listed and the result ruled on.
    pub candidates: Reviewed,
}

pub fn reviews(transcript: &Path) -> std::io::Result<Vec<Review>> {
    let file = std::fs::File::open(transcript)?;
    let reader = std::io::BufReader::new(file);
    Ok(completed_agents_resumable(reader, AGENT)
        .into_iter()
        .flat_map(|c: Completed| {
            let verdict = verdict_of(&c.result);
            let rules_sha = crate::review_brief::parse_rules_sha(&c.prompt);
            let candidates = Reviewed::read(c.agent_id.as_deref(), &c.prompt, &c.result);
            parse_blocks(&c.prompt)
                .into_iter()
                .map(move |(repo, sha)| Review {
                    repo,
                    sha,
                    verdict,
                    rules_sha: rules_sha.clone(),
                    candidates: candidates.clone(),
                })
        })
        .collect())
}

// ---------------------------------------------------------------- the store

fn store() -> Option<PathBuf> {
    Some(journal::dir()?.join(RULE_ID))
}

fn private_dir(dir: &Path) -> Option<()> {
    std::fs::create_dir_all(dir).ok()?;
    journal::private(dir, 0o700);
    Some(())
}

fn pass_file(repo: &str, sha: &str) -> Option<PathBuf> {
    if repo.contains('/') || repo.contains("..") || !is_sha256(sha) {
        return None;
    }
    Some(
        store()?
            .join("by-tree")
            .join(repo)
            .join(format!("{sha}.json")),
    )
}

/// The verdict a remembered pass carries (`approve`, `approve-with-changes`
/// or `overruled`), if this repository and tree have one.
pub fn remembered(repo: &str, sha: &str) -> Option<String> {
    remembered_pass(repo, sha).map(|pass| pass.verdict)
}

/// A remembered verdict and the rules line the passing review's prompt
/// carried.
struct Pass {
    verdict: String,
    prompt_rules: PromptRules,
    candidates: Reviewed,
}

/// What a passing review's prompt said about the rules it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptRules {
    /// The pass was remembered before the rules line was stored.
    NotRecorded,
    /// The prompt carried no rules line.
    Absent,
    /// The prompt's rules line, 64 hex.
    Sha(String),
}

/// How [`PromptRules::Absent`] is stored: the field is kept, and empty, so
/// it reads apart from a pass that predates the field.
const RULES_ABSENT_STORED: &str = "";

impl PromptRules {
    fn of_prompt(sha: Option<&str>) -> PromptRules {
        sha.map_or(PromptRules::Absent, |sha| PromptRules::Sha(sha.to_string()))
    }

    /// The stored `prompt_rules_sha`: `null` (not recorded), `""` (absent)
    /// or the sha.
    fn stored(&self) -> Option<&str> {
        match self {
            PromptRules::NotRecorded => None,
            PromptRules::Absent => Some(RULES_ABSENT_STORED),
            PromptRules::Sha(sha) => Some(sha),
        }
    }

    fn from_stored(stored: Option<&str>) -> PromptRules {
        match stored {
            None => PromptRules::NotRecorded,
            Some(RULES_ABSENT_STORED) => PromptRules::Absent,
            Some(sha) => PromptRules::Sha(sha.to_string()),
        }
    }
}

fn pass_of(stored_record: &serde_json::Value) -> Option<Pass> {
    Some(Pass {
        verdict: stored_record.get("verdict")?.as_str()?.to_string(),
        prompt_rules: PromptRules::from_stored(
            stored_record
                .get("prompt_rules_sha")
                .and_then(serde_json::Value::as_str),
        ),
        candidates: Reviewed::from_json(stored_record.get("candidates")),
    })
}

fn remembered_pass(repo: &str, sha: &str) -> Option<Pass> {
    let f = pass_file(repo, sha)?;
    pass_of(&serde_json::from_str(&std::fs::read_to_string(f).ok()?).ok()?)
}

/// Written only by the hook: from a transcript binding whose verdict is a
/// pass, or from the person's `Overrule` of a `rework`.
fn remember(
    repo: &str,
    sha: &str,
    verdict: &str,
    how: &str,
    prompt_rules: &PromptRules,
    candidates: &Reviewed,
) -> Option<()> {
    let f = pass_file(repo, sha)?;
    private_dir(f.parent()?)?;
    let body = serde_json::json!({
        "repo": repo,
        "tree": sha,
        "verdict": verdict,
        "agent": AGENT,
        "how": how,
        "prompt_rules_sha": prompt_rules.stored(),
        "candidates": candidates.to_json(),
    })
    .to_string();
    amont_agent_guard::atomic::write_atomic(&f, &body).ok()?;
    journal::private(&f, 0o600);
    Some(())
}

fn patch_file(repo: &str, patch: &str) -> Option<PathBuf> {
    if repo.contains('/')
        || repo.contains("..")
        || patch.is_empty()
        || !patch.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(
        store()?
            .join("by-patch")
            .join(repo)
            .join(format!("{patch}.json")),
    )
}

/// The pass remembered for this patch, when its rules match: the verdict
/// and the tree it was given for.
fn remembered_patch(repo: &str, key: &PatchKey) -> Option<(Pass, String)> {
    let f = patch_file(repo, &key.patch)?;
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f).ok()?).ok()?;
    if v.get("rules")?.as_str()? != key.rules {
        return None;
    }
    Some((pass_of(&v)?, v.get("tree")?.as_str()?.to_string()))
}

/// Files a pass under its patch too, so a conflict-free rebase keeps it.
fn remember_patch(
    repo: &str,
    key: &PatchKey,
    tree: &str,
    verdict: &str,
    how: &str,
    prompt_rules: &PromptRules,
    candidates: &Reviewed,
) -> Option<()> {
    let f = patch_file(repo, &key.patch)?;
    private_dir(f.parent()?)?;
    let body = serde_json::json!({
        "repo": repo,
        "patch": key.patch,
        "rules": key.rules,
        "tree": tree,
        "verdict": verdict,
        "agent": AGENT,
        "how": how,
        "prompt_rules_sha": prompt_rules.stored(),
        "candidates": candidates.to_json(),
    })
    .to_string();
    amont_agent_guard::atomic::write_atomic(&f, &body).ok()?;
    journal::private(&f, 0o600);
    Some(())
}

// The code-review profiles (ADR-0052).

/// A completed review by one code-review profile's agent (ADR-0052), bound
/// to the profile its launch prompt named.
pub struct ProfileReview {
    pub repo: String,
    pub sha: String,
    pub profile: String,
    pub agent: String,
    pub verdict: Option<Verdict>,
}

/// The code-review profile reviews this session completed, in launch order.
/// A launch counts for a profile only when its agent is the one the profile
/// maps to and its prompt's header (see [`crate::code_review::profile_lines`])
/// carries exactly one `Profile:` line naming that
/// profile. A resumed round inherits the profile of its launch and may omit
/// the line; one that names another profile counts for nothing.
pub fn profile_reviews(transcript: &Path) -> std::io::Result<Vec<ProfileReview>> {
    let bytes = std::fs::read(transcript)?;
    let mut completed = completed_agents_resumable(bytes.as_slice(), "code-review-");
    completed.extend(completed_agents_resumable(
        bytes.as_slice(),
        "interface-review",
    ));
    let mut launched: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for c in completed {
        let lines = crate::code_review::profile_lines(&c.prompt);
        let inherited = c.agent_id.as_ref().and_then(|id| launched.get(id));
        let profile = if c.resumed {
            match (lines.as_slice(), inherited) {
                ([], Some(profile)) => Some(profile.clone()),
                ([line], Some(profile)) if line == profile => Some(profile.clone()),
                _ => None,
            }
        } else {
            match lines.as_slice() {
                [line] => Some(line.to_string()),
                _ => None,
            }
        };
        let Some(profile) = profile.filter(|profile| {
            crate::code_review::agent_for(profile).is_some_and(|(agent, _)| agent == c.agent)
        }) else {
            continue;
        };
        if !c.resumed {
            if let Some(id) = &c.agent_id {
                launched.insert(id.clone(), profile.clone());
            }
        }
        let verdict = verdict_of(&c.result);
        out.extend(
            parse_blocks(&c.prompt)
                .into_iter()
                .map(|(repo, sha)| ProfileReview {
                    repo,
                    sha,
                    profile: profile.clone(),
                    agent: c.agent.clone(),
                    verdict,
                }),
        );
    }
    Ok(out)
}

fn profile_pass_file(profile: &str, repo: &str, sha: &str) -> Option<PathBuf> {
    if repo.contains('/') || repo.contains("..") || !is_sha256(sha) {
        return None;
    }
    Some(
        store()?
            .join("by-profile")
            .join(crate::code_review::slug(profile)?)
            .join("by-tree")
            .join(repo)
            .join(format!("{sha}.json")),
    )
}

fn profile_patch_file(profile: &str, repo: &str, patch: &str) -> Option<PathBuf> {
    if repo.contains('/')
        || repo.contains("..")
        || patch.is_empty()
        || !patch.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(
        store()?
            .join("by-profile")
            .join(crate::code_review::slug(profile)?)
            .join("by-patch")
            .join(repo)
            .join(format!("{patch}.json")),
    )
}

fn write_profile_pass(file: &Path, body: &serde_json::Value) -> Option<()> {
    private_dir(file.parent()?)?;
    amont_agent_guard::atomic::write_atomic(file, &body.to_string()).ok()?;
    journal::private(file, 0o600);
    Some(())
}

fn profile_record(profile: &str, repo: &str, sha: &str) -> Option<serde_json::Value> {
    let file = profile_pass_file(profile, repo, sha)?;
    serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
}

fn remembered_profile(profile: &str, repo: &str, sha: &str) -> Option<String> {
    Some(
        profile_record(profile, repo, sha)?
            .get("verdict")?
            .as_str()?
            .to_string(),
    )
}

/// How a remembered profile pass bound, from what it recorded.
fn remembered_how(profile: &str, repo: &str, sha: &str) -> ProfileHow {
    let record = profile_record(profile, repo, sha);
    let field = |key: &str| {
        record
            .as_ref()
            .and_then(|r| r.get(key))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let how = field("how");
    if field("verdict") == "overruled" {
        ProfileHow::Overruled
    } else if how.starts_with("carried-from") {
        ProfileHow::Carried
    } else if how.starts_with("patch") {
        ProfileHow::ByPatch
    } else {
        ProfileHow::Reviewed
    }
}

/// The content id a remembered profile pass read: what it recorded, else the
/// tree (a pass from before `read` was stored).
fn remembered_read(profile: &str, repo: &str, sha: &str) -> String {
    profile_record(profile, repo, sha)
        .and_then(|r| r.get("read")?.as_str().map(str::to_string))
        .unwrap_or_else(|| sha.to_string())
}

/// What a profile pass recorded about the review beyond its tree, so that a
/// later push can check whether it carries (ADR-0052): the commit the review
/// read, the plan that declared the profile and a digest of the repository's
/// parsed `## Code review` declaration. A pass without them never carries.
struct Basis {
    commit: String,
    plan: String,
    declaration: String,
}

fn remember_profile(
    profile: &str,
    repo: &str,
    sha: &str,
    verdict: &str,
    how: &str,
    read: &str,
    basis: Option<&Basis>,
) -> Option<()> {
    let mut body = serde_json::json!({
        "repo": repo,
        "tree": sha,
        "profile": profile,
        "verdict": verdict,
        "agent": crate::code_review::agent_for(profile)?.0,
        "how": how,
        "read": read,
    });
    if let Some(basis) = basis {
        body["commit"] = basis.commit.clone().into();
        body["plan"] = basis.plan.clone().into();
        body["declaration"] = basis.declaration.clone().into();
    }
    write_profile_pass(&profile_pass_file(profile, repo, sha)?, &body)
}

fn remembered_profile_patch(profile: &str, repo: &str, key: &PatchKey) -> Option<(String, String)> {
    let file = profile_patch_file(profile, repo, &key.patch)?;
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    if record.get("rules")?.as_str()? != key.rules {
        return None;
    }
    Some((
        record.get("verdict")?.as_str()?.to_string(),
        record.get("tree")?.as_str()?.to_string(),
    ))
}

fn remember_profile_patch(
    profile: &str,
    repo: &str,
    key: &PatchKey,
    tree: &str,
    verdict: &str,
    how: &str,
) -> Option<()> {
    write_profile_pass(
        &profile_patch_file(profile, repo, &key.patch)?,
        &serde_json::json!({
            "repo": repo,
            "patch": key.patch,
            "rules": key.rules,
            "tree": tree,
            "profile": profile,
            "verdict": verdict,
            "agent": crate::code_review::agent_for(profile)?.0,
            "how": how,
        }),
    )
}

/// Where a declared profile stands on the pushed tree.
enum Standing {
    /// How the pass bound, and the content id its review read.
    Passed(ProfileHow, String),
    /// The latest review of the tree said this (`rework`, or nothing the
    /// contract reads).
    Reworked(String, String),
    Missing,
}

/// The profiles the branch's plans declare for this repository.
struct Declaration {
    profiles: Vec<String>,
    /// The first plan that declares them.
    plan: String,
    /// Whether any plan has a `## Code review` section.
    any_section: bool,
    /// The sha256 of this repository's parsed declarations, in plan order.
    digest: String,
}

/// The per-profile half of the push judgement: every code-review profile
/// the branch's plans declare for this repository needs its own pass on the
/// pushed tree. A plan with no section requires none.
struct ProfileGate<'a> {
    repo: &'a Path,
    name: &'a str,
    tree: &'a str,
    key: Option<&'a PatchKey>,
    plans: &'a [String],
    pushed: &'a str,
    merge_base: &'a str,
    transcript: Option<&'a Path>,
    stance: Stance,
}

impl ProfileGate<'_> {
    fn declaration(&self) -> Result<Declaration, String> {
        let mut found = Declaration {
            profiles: Vec::new(),
            plan: String::new(),
            any_section: false,
            digest: String::new(),
        };
        let mut described = String::new();
        for path in self.plans {
            // A plan deleted on the branch is not there to declare anything.
            let Some(text) =
                git::stdout_in(self.repo, &["show", &format!("{}:{path}", self.pushed)])
            else {
                continue;
            };
            let declared = crate::code_review::declared_text(&text)
                .and_then(|text| crate::code_review::parse(&text))
                .map_err(|why| {
                    format!("the code review declared in {path} cannot be read: {why}")
                })?;
            let Some(declared) = declared else { continue };
            found.any_section = true;
            if let Some(line) = declared.get(self.name) {
                described.push_str(&line.describe());
                described.push('\n');
            }
            if let Some(crate::code_review::Declared::Profiles { profiles, .. }) =
                declared.get(self.name)
            {
                if found.plan.is_empty() {
                    found.plan = path.clone();
                }
                for profile in profiles {
                    if !found.profiles.contains(profile) {
                        found.profiles.push(profile.clone());
                    }
                }
            }
        }
        found.digest = hex(&sha256(described.as_bytes()));
        Ok(found)
    }

    /// The commits of the branch after the merge-base, newest first, each
    /// with its content id. Computed once per push and shared by every
    /// profile.
    fn branch(&self) -> Vec<(String, String)> {
        git::stdout_in(
            self.repo,
            &["rev-list", self.pushed, &format!("^{}", self.merge_base)],
        )
        .map(|out| {
            out.lines()
                .map(str::trim)
                .filter_map(|commit| {
                    let id = content_id(self.repo, commit).ok()?;
                    Some((commit.to_string(), id))
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// The basis a pass for the pushed tree records: the newest branch
    /// commit whose content id is the pushed tree, with the plan and
    /// declaration digest now in force. `None` when the commit cannot be
    /// found, so the pass cannot carry.
    fn basis(&self, declared: &Declaration, branch: &[(String, String)]) -> Option<Basis> {
        let (commit, _) = branch.iter().find(|(_, id)| id == self.tree)?;
        Some(Basis {
            commit: commit.clone(),
            plan: declared.plan.clone(),
            declaration: declared.digest.clone(),
        })
    }

    /// An `approve` this profile gave an earlier commit of the branch that
    /// still stands: the commit is in `pushed ^merge-base` (an ancestor of
    /// the pushed commit, strictly after the current merge-base, so a
    /// rebased branch or a merged plan's review never matches), and the
    /// pass recorded the same plan and declaration digest. Returns the
    /// content id the review read and the basis the carried pass keeps.
    fn carried(
        &self,
        profile: &str,
        declared: &Declaration,
        branch: &[(String, String)],
    ) -> Option<(String, Basis)> {
        for (_, id) in branch {
            if id == self.tree {
                continue;
            }
            let Some(record) = profile_record(profile, self.name, id) else {
                continue;
            };
            let field = |key: &str| record.get(key).and_then(serde_json::Value::as_str);
            let (Some("approve"), Some(reviewed), Some(plan), Some(digest)) = (
                field("verdict"),
                field("commit"),
                field("plan"),
                field("declaration"),
            ) else {
                continue;
            };
            let Some((_, reviewed_id)) = branch.iter().find(|(commit, _)| commit == reviewed)
            else {
                continue;
            };
            if plan == declared.plan && digest == declared.digest {
                return Some((
                    reviewed_id.clone(),
                    Basis {
                        commit: reviewed.to_string(),
                        plan: plan.to_string(),
                        declaration: digest.to_string(),
                    },
                ));
            }
        }
        None
    }

    fn standing<'r>(
        &self,
        profile: &str,
        declared: &Declaration,
        basis: Option<&Basis>,
        branch: &[(String, String)],
        reviews: &dyn Fn() -> &'r [ProfileReview],
    ) -> Standing {
        let (name, tree) = (self.name, self.tree);
        if let Some(verdict) = remembered_profile(profile, name, tree) {
            if let Some(key) = self.key {
                if remembered_profile_patch(profile, name, key).is_none() {
                    remember_profile_patch(profile, name, key, tree, &verdict, "tree");
                }
            }
            return Standing::Passed(
                remembered_how(profile, name, tree),
                remembered_read(profile, name, tree),
            );
        }
        if let Some((verdict, old_tree)) = self
            .key
            .and_then(|key| remembered_profile_patch(profile, name, key))
        {
            remember_profile(
                profile,
                name,
                tree,
                &verdict,
                &format!("patch {}", short(&old_tree)),
                // The review read `old_tree`, not this one: no commit here
                // is what it read, so this pass cannot carry.
                &old_tree,
                None,
            );
            let how = if verdict == "overruled" {
                ProfileHow::Overruled
            } else {
                ProfileHow::ByPatch
            };
            return Standing::Passed(how, old_tree);
        }
        let Some(latest) = reviews()
            .iter()
            .rev()
            .find(|r| r.repo == name && r.sha == tree && r.profile == profile)
        else {
            if let Some((reviewed_id, kept)) = self.carried(profile, declared, branch) {
                let stat = git::stdout_in(
                    self.repo,
                    &["diff", "--shortstat", &kept.commit, self.pushed],
                )
                .unwrap_or_default();
                remember_profile(
                    profile,
                    name,
                    tree,
                    "approve",
                    &format!("carried-from {}", &reviewed_id[..12]),
                    &reviewed_id,
                    Some(&kept),
                );
                note(
                    "profile-carried",
                    "-",
                    name,
                    &format!(
                        "tree={} profile={profile} carried-from {} {stat}",
                        short(tree),
                        &reviewed_id[..12]
                    ),
                );
                return Standing::Passed(ProfileHow::Carried, reviewed_id);
            }
            return Standing::Missing;
        };
        match latest.verdict {
            Some(verdict @ (Verdict::Approve | Verdict::ApproveWithChanges)) => {
                remember_profile(
                    profile,
                    name,
                    tree,
                    verdict.as_str(),
                    "transcript",
                    tree,
                    basis,
                );
                if let Some(key) = self.key {
                    remember_profile_patch(
                        profile,
                        name,
                        key,
                        tree,
                        verdict.as_str(),
                        "transcript",
                    );
                }
                Standing::Passed(ProfileHow::Reviewed, tree.to_string())
            }
            other => Standing::Reworked(
                latest.agent.clone(),
                other
                    .map(Verdict::as_str)
                    .unwrap_or("no verdict this guard can read")
                    .to_string(),
            ),
        }
    }

    /// What holds the push, when the implementation review has passed: the
    /// verdict of every declared profile, in declaration order, when each has
    /// a pass (empty when none is declared).
    fn hold(&self) -> Result<Vec<ProfileVerdict>, Judged> {
        let declared = match self.declaration() {
            Ok(declared) => declared,
            Err(why) if self.stance == Stance::Deny => {
                return Err(unknown(Some(self.tree), &why));
            }
            Err(why) => {
                note(
                    "profiles-unreadable",
                    "-",
                    self.name,
                    &format!("tree={} {why}", short(self.tree)),
                );
                return Ok(Vec::new());
            }
        };
        if declared.profiles.is_empty() {
            if !declared.any_section {
                note(
                    "undeclared-profiles",
                    "-",
                    self.name,
                    &format!("tree={}", short(self.tree)),
                );
            }
            return Ok(Vec::new());
        }
        let loaded: std::cell::OnceCell<Vec<ProfileReview>> = std::cell::OnceCell::new();
        let reviews = || -> &[ProfileReview] {
            loaded.get_or_init(|| {
                self.transcript
                    .and_then(|path| profile_reviews(path).ok())
                    .unwrap_or_default()
            })
        };
        let branch = self.branch();
        let basis = self.basis(&declared, &branch);
        let mut missing: Vec<String> = Vec::new();
        let mut reworked: Vec<String> = Vec::new();
        let mut passed: Vec<ProfileVerdict> = Vec::new();
        for profile in &declared.profiles {
            match self.standing(profile, &declared, basis.as_ref(), &branch, &reviews) {
                Standing::Passed(how, content) => passed.push(ProfileVerdict {
                    role: profile.clone(),
                    how,
                    content,
                }),
                Standing::Missing => {
                    missing.push(match crate::code_review::agent_for(profile) {
                        Some((agent, Some(language))) => format!("{agent} --lang {language}"),
                        Some((agent, None)) => agent,
                        None => profile.clone(),
                    });
                }
                Standing::Reworked(agent, verdict) => reworked.push(format!(
                    "the code review of {} tree {} by {agent} said {verdict}, and no delta followed.",
                    self.name,
                    short(self.tree)
                )),
            }
        }
        if missing.is_empty() && reworked.is_empty() {
            return Ok(passed);
        }
        let mut reason = reworked.join(" ");
        if !missing.is_empty() {
            if !reason.is_empty() {
                reason.push(' ');
            }
            reason.push_str(&format!(
                "missing code review of {} tree {}: {} (declared in {}). Launch them on this tree, or narrow ## Code review with a reason.",
                self.name,
                short(self.tree),
                missing.join(", "),
                declared.plan
            ));
        }
        Err(Judged::Profiles {
            reason,
            excerpt: format!(
                "tree={} review={} verdict=-",
                short(self.tree),
                if missing.is_empty() {
                    "profile-rework"
                } else {
                    "profile-missing"
                }
            ),
        })
    }
}

// ---------------------------------------------------------------- the push

/// What the hook found out about a push.
pub enum Judged {
    /// The rule has nothing to say: the reason is the journal's
    /// `unconfirmed` reason.
    Decline(&'static str),
    /// The rule speaks: the reason to say, and the journal excerpt.
    Say { reason: String, excerpt: String },
    /// A declared code-review profile has no pass: the reason names what to
    /// launch, so the rule's own remedy (the implementation review) is not
    /// added.
    Profiles { reason: String, excerpt: String },
    /// The agent the rule needs is not installed: said, never held.
    Gap { reason: String, excerpt: String },
}

/// Judge the push in `cmd`, whose clause runs in `cwd`.
pub fn judge_push(ctx: &Context, cmd: &Simple, stance: Stance) -> Judged {
    use amont_agent_guard::push_target::{self, Kind, Push};
    let (repo, targets) = match push_target::resolve_for(ctx, cmd) {
        Push::Resolved { repo, targets } => (repo, targets),
        Push::DryRun => return Judged::Decline("dry-run"),
        Push::DeleteOnly => return Judged::Decline("delete-only"),
        // amont refuses this land itself: nothing is published.
        Push::Refused(_) => return Judged::Decline("land-refused"),
        Push::Unresolvable { shape, .. } => {
            // Under `deny` an unreadable shape is held, so `--all` is not the
            // way around the gate; otherwise it is journalled and passes.
            return if stance == Stance::Deny {
                unknown(None, &format!("the push cannot be read: {shape}"))
            } else {
                Judged::Decline("unresolvable")
            };
        }
    };
    let Some(target) = targets.iter().find(|t| t.kind == Kind::Branch) else {
        return Judged::Decline("no-branch");
    };
    let pushed = target.src.as_str();
    // Commits some remote already has are not this push's work: seeding a
    // fork reaches here with no default branch on the fork, and used to ask
    // for a review of upstream's history.
    if git::nothing_new(&repo, pushed) {
        return Judged::Decline("nothing-new");
    }
    let Some(base) = default_base(&repo, &target.remote) else {
        return unknown(
            None,
            &format!(
                "no default branch of `{}` is known here (no {0}/HEAD, {0}/main or {0}/master)",
                target.remote
            ),
        );
    };
    let Some(merge_base) = git::stdout_in(&repo, &["merge-base", pushed, &base]) else {
        return unknown(
            None,
            &format!("`git merge-base {} {base}` failed", short(pushed)),
        );
    };
    let Some(changed) = git::stdout_in(
        &repo,
        &["diff", "--name-only", &merge_base, pushed, "--", PLANS_PATH],
    ) else {
        return unknown(None, "`git diff` against the base failed");
    };
    if changed.trim().is_empty() {
        return Judged::Decline("no-plan");
    }
    let tree = match content_id(&repo, pushed) {
        Ok(t) => t,
        Err(why) => return unknown(None, &why),
    };
    let Some(name) = repo_name(&repo) else {
        return unknown(Some(&tree), "the repository has no name");
    };
    // The branch patch, computed once: a pass found here or below is filed
    // under it, and a miss by tree is looked up by it.
    let key = patch_key(&repo, &merge_base, pushed);
    let plans: Vec<String> = changed
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    // Said when the push is held: no earlier review could carry over.
    let unkeyed = key
        .as_ref()
        .err()
        .map(|why| format!(" No earlier review could carry over by patch: {why}."))
        .unwrap_or_default();
    let key = key.ok();
    let push = crate::provenance::Push {
        repo: &repo,
        name: &name,
        pushed,
        patch: key.as_ref().map(|k| k.patch.as_str()),
        plans: &plans,
        target,
    };
    let gate = ProfileGate {
        repo: &repo,
        name: &name,
        tree: &tree,
        key: key.as_ref(),
        plans: &plans,
        pushed,
        merge_base: &merge_base,
        transcript: ctx.transcript,
        stance,
    };
    if let Some(Pass {
        verdict,
        prompt_rules,
        candidates,
    }) = remembered_pass(&name, &tree)
    {
        if let Some(k) = &key {
            if remembered_patch(&name, k).is_none() {
                remember_patch(
                    &name,
                    k,
                    &tree,
                    &verdict,
                    "tree",
                    &prompt_rules,
                    &candidates,
                );
            }
        }
        let profiles = match gate.hold() {
            Ok(profiles) => profiles,
            Err(held) => return held,
        };
        let overruled = verdict == "overruled";
        provenance(
            &push,
            if overruled {
                Bound::Overruled
            } else {
                Bound::Reviewed
            },
            &tree,
            &profiles,
        );
        if !overruled {
            check_remembered_rules(&repo, &name, &tree, &prompt_rules);
            check_candidates(&name, &tree, &candidates);
        }
        return Judged::Decline(if overruled { "overruled" } else { "reviewed" });
    }
    if let Some((pass, old_tree)) = key.as_ref().and_then(|k| remembered_patch(&name, k)) {
        let Pass {
            verdict,
            prompt_rules,
            candidates,
        } = pass;
        remember(
            &name,
            &tree,
            &verdict,
            &format!("patch {}", short(&old_tree)),
            &prompt_rules,
            &candidates,
        );
        let profiles = match gate.hold() {
            Ok(profiles) => profiles,
            Err(held) => return held,
        };
        let overruled = verdict == "overruled";
        provenance(
            &push,
            if overruled {
                Bound::Overruled
            } else {
                Bound::ByPatch
            },
            &old_tree,
            &profiles,
        );
        if !overruled {
            check_remembered_rules(&repo, &name, &tree, &prompt_rules);
            check_candidates(&name, &tree, &candidates);
        }
        return Judged::Decline(if overruled {
            "overruled-by-patch"
        } else {
            "reviewed-by-patch"
        });
    }
    let Some(transcript) = ctx.transcript else {
        return unknown(Some(&tree), "the payload carried no transcript_path");
    };
    let reviews = match reviews(transcript) {
        Ok(r) => r,
        Err(e) => {
            return unknown(
                Some(&tree),
                &format!(
                    "the transcript {} could not be read: {e}",
                    transcript.display()
                ),
            )
        }
    };
    let mine: Vec<&Review> = reviews.iter().filter(|r| r.repo == name).collect();
    if let Some(latest) = mine.iter().rev().find(|r| r.sha == tree) {
        return match latest.verdict {
            Some(Verdict::Rework) | None => Judged::Say {
                reason: format!(
                    "the implementation review of {name} tree {} said {}, and no delta followed.",
                    short(&tree),
                    latest
                        .verdict
                        .map(Verdict::as_str)
                        .unwrap_or("no verdict this guard can read")
                ),
                excerpt: format!(
                    "tree={} review=rework verdict={}",
                    short(&tree),
                    latest.verdict.map(Verdict::as_str).unwrap_or("-")
                ),
            },
            Some(v) => {
                let prompt_rules = PromptRules::of_prompt(latest.rules_sha.as_deref());
                let candidates = &latest.candidates;
                remember(
                    &name,
                    &tree,
                    v.as_str(),
                    "transcript",
                    &prompt_rules,
                    candidates,
                );
                if let Some(k) = &key {
                    remember_patch(
                        &name,
                        k,
                        &tree,
                        v.as_str(),
                        "transcript",
                        &prompt_rules,
                        candidates,
                    );
                }
                let profiles = match gate.hold() {
                    Ok(profiles) => profiles,
                    Err(held) => return held,
                };
                provenance(&push, Bound::Reviewed, &tree, &profiles);
                check_remembered_rules(&repo, &name, &tree, &prompt_rules);
                check_candidates(&name, &tree, candidates);
                Judged::Decline("reviewed")
            }
        };
    }
    if let Some(older) = mine.last() {
        return Judged::Say {
            reason: format!(
                "the implementation review of {name} read tree {}; this push carries tree {} (stale).{unkeyed}",
                short(&older.sha),
                short(&tree)
            ),
            excerpt: format!("tree={} review=stale verdict=-", short(&tree)),
        };
    }
    if !crate::agents::carried(crate::agents::Need::ImplementationReview, &repo)
        && !std::env::current_dir().is_ok_and(|cwd| {
            crate::agents::carried(crate::agents::Need::ImplementationReview, &cwd)
        })
    {
        return Judged::Gap {
            reason: crate::agents::gap_line(crate::agents::Need::ImplementationReview),
            excerpt: format!("tree={} review=gap verdict=-", short(&tree)),
        };
    }
    Judged::Say {
        reason: format!(
            "no implementation review of {name} tree {} is in this session.{unkeyed}",
            short(&tree)
        ),
        excerpt: format!("tree={} review=missing verdict=-", short(&tree)),
    }
}

/// The advise-only comparison of the rules a passing review read with the
/// rules now: journalled, never holding the push. `prompt_rules` is what the
/// pass remembered: `NotRecorded` (from before the line was stored) journals
/// `rules-unknown`; `Absent` and `Sha` are compared with `rules_sha` now.
fn check_remembered_rules(repo: &Path, name: &str, tree: &str, prompt_rules: &PromptRules) {
    let excerpt = match prompt_rules {
        PromptRules::NotRecorded => "review=rules-unknown",
        PromptRules::Absent => {
            crate::review_brief::rules_excerpt(None, &crate::review_brief::rules_sha(repo))
        }
        PromptRules::Sha(seen) => {
            crate::review_brief::rules_excerpt(Some(seen), &crate::review_brief::rules_sha(repo))
        }
    };
    note(
        "rules-checked",
        "-",
        name,
        &format!("tree={} {excerpt}", short(tree)),
    );
}

/// What a passing review did with the candidates adr-lint listed: one
/// `candidates-checked` note, and one ruling record per ruled candidate.
/// Never holds the push: a ruling that cannot be written is journalled.
fn check_candidates(name: &str, tree: &str, candidates: &Reviewed) {
    note(
        "candidates-checked",
        "-",
        name,
        &format!("tree={} {}", short(tree), candidates.excerpt()),
    );
    if let Err(why) = crate::candidates_block::append_rulings(name, tree, candidates) {
        note(
            "rulings-write-failed",
            "-",
            name,
            &format!("tree={} {why}", short(tree)),
        );
    }
}

/// Files the provenance record of a pass and journals what came of it. A
/// failure never changes the judgment.
fn provenance(
    push: &crate::provenance::Push,
    review: Bound,
    review_content: &str,
    profiles: &[ProfileVerdict],
) {
    use crate::provenance::Filing;
    match crate::provenance::file(push, review, review_content, profiles) {
        Filing::Deferred => note(
            "provenance-deferred",
            "-",
            push.name,
            "the preview has not stood",
        ),
        Filing::Written(file) => note(
            "provenance-written",
            "-",
            push.name,
            &file.display().to_string(),
        ),
        Filing::Kept(file) => note(
            "provenance-kept",
            "-",
            push.name,
            &file.display().to_string(),
        ),
        Filing::Failed(why) => note("provenance-write-failed", "-", push.name, &why),
    }
}

fn unknown(tree: Option<&str>, why: &str) -> Judged {
    Judged::Say {
        reason: format!("whether this push was reviewed cannot be told: {why}"),
        excerpt: format!(
            "tree={} review=unknown verdict=-",
            tree.map(short).unwrap_or("-")
        ),
    }
}

/// The remote's default branch as last fetched: `<remote>/HEAD`, else
/// `<remote>/main`, else `<remote>/master`. Never `HEAD`: with the pushed
/// commit as its own base every push would show no plan.
pub(crate) fn default_base(repo: &Path, remote: &str) -> Option<String> {
    [
        format!("refs/remotes/{remote}/HEAD"),
        format!("refs/remotes/{remote}/main"),
        format!("refs/remotes/{remote}/master"),
    ]
    .into_iter()
    .find(|r| {
        git::stdout_in(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{r}^{{commit}}"),
            ],
        )
        .is_some_and(|s| !s.is_empty())
    })
}

/// The rule's `confirm`, for a push: what to say, or why not.
pub fn confirm_push(ctx: &Context, cmd: &Simple, stance: Stance) -> Confirmed {
    match judge_push(ctx, cmd, stance) {
        Judged::Decline(why) => Confirmed::No(why),
        Judged::Say { reason, excerpt } => Confirmed::YesSaying {
            remedy: None,
            floor: None,
            ceiling: None,
            reason,
            excerpt,
        },
        Judged::Profiles { reason, excerpt } => Confirmed::YesSaying {
            remedy: Some(String::new()),
            floor: None,
            ceiling: None,
            reason,
            excerpt,
        },
        // Said at most as advice: the review cannot be had here, so refusing
        // the push would hold it for something nobody can supply.
        Judged::Gap { reason, excerpt } => Confirmed::YesSaying {
            remedy: Some(String::new()),
            floor: None,
            ceiling: Some(Stance::Advise),
            reason,
            excerpt,
        },
    }
}

// ---------------------------------------------------------------- the guards

/// Whether a path is inside the pass-file store, compared lexically (`.`
/// and `..` folded, no symlink followed) and, where the store exists, by
/// its canonical form too. Only the hook writes there.
pub fn guards_path(path: &Path) -> bool {
    HOOK_ONLY_DIRS
        .iter()
        .filter_map(|d| Some(journal::dir()?.join(d)))
        .any(|store| guards_store(path, &store))
}

fn guards_store(path: &Path, store: &Path) -> bool {
    let here = lexical(path);
    if here.starts_with(lexical(store)) {
        return true;
    }
    if let Ok(real) = std::fs::canonicalize(store) {
        if here.starts_with(&real) {
            return true;
        }
        // The path's own existing ancestor, resolved.
        if let Some(parent) = here.parent() {
            if let Ok(real_parent) = std::fs::canonicalize(parent) {
                return real_parent.starts_with(&real);
            }
        }
    }
    false
}

/// A path with `.` and `..` folded lexically.
pub fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Programs whose operand naming the store is taken as a write. A reader
/// (`ls`, `cat`, `find`, `grep`, `stat`, …) is not on this list, so looking
/// at the record is allowed; the list is the Bash half of a best-effort
/// guard, since a parser cannot see inside `python -c` beyond its words.
const WRITERS: &[&str] = &[
    "rm", "mv", "cp", "tee", "touch", "install", "truncate", "ln", "chmod", "chown", "mkdir",
    "rmdir", "dd", "sed", "perl", "ruby", "node", "deno", "python", "python3", "sh", "bash", "zsh",
    "dash", "fish", "sudo", "xargs", "rsync", "tar", "unzip", "chezmoi", "git",
];

/// The first clause of a command that would WRITE into the store: a redirect
/// into it (`> …/by-tree/…`), or a program on [`WRITERS`] with a word naming
/// it. A read — `ls`, `cat`, `find` of the pass files — passes: the record is
/// there to be looked at. Returns the clause's span.
pub fn store_clause(parsed: &amont_agent_guard::shell::Parsed) -> Option<std::ops::Range<usize>> {
    parsed
        .clauses()
        .iter()
        .find(|cmd| {
            let redirected = cmd
                .redirects
                .iter()
                .any(|(op, w)| op.contains('>') && names_store(&w.text));
            let writer = cmd
                .program()
                .map(|p| p.rsplit('/').next().unwrap_or(p))
                .is_some_and(|p| WRITERS.contains(&p) || p.starts_with("python"));
            redirected || (writer && cmd.words.iter().any(|w| names_store(&w.text)))
        })
        .map(|cmd| cmd.at..cmd.end)
}

/// Whether a shell word names the store, spelled with either separator:
/// Windows writes the same directory with backslashes.
pub fn names_store(word: &str) -> bool {
    let word = word.replace('\\', "/");
    HOOK_ONLY_DIRS.iter().any(|d| {
        word.contains(&format!("amont-agent/{d}/"))
            || word.contains(&format!("amont-agent/{d}")) && word.ends_with(d)
    })
}

/// The directories under the config dir that only the hook writes.
const HOOK_ONLY_DIRS: &[&str] = &[RULE_ID, "provenance"];

pub const GUARD_REASON: &str =
    "files under amont-agent/implementation-review/ and amont-agent/provenance/ are written by the hook alone: the pass files from a completed review in this session's transcript or the person's Overrule, the provenance records from a push the hook passed.";
pub const GUARD_REMEDY: &str =
    "Run the review (worktree-task F4b) or ask the person; never write the record by hand.";

// ---------------------------------------------------------------- the person

/// What a marked question names: the repository, the tree, and the code-review
/// profile when the marker carries `profile=<role>` (`None` is the
/// implementation review itself):
/// `[implementation-review <repo>@<sha64> profile=<role>]`, the repository
/// split from the tree at the last `@`.
pub fn marker(question: &str) -> Option<(String, String, Option<String>)> {
    let start = question.find(MARKER)? + MARKER.len();
    let end = start + question[start..].find(']')?;
    let inner = question[start..end].trim();
    let (named, profile) = match inner.rfind(PROFILE_KEY) {
        Some(at) => {
            let profile = inner[at + PROFILE_KEY.len()..].trim();
            if !crate::code_review::valid_profile(profile) {
                return None;
            }
            (inner[..at].trim(), Some(profile.to_string()))
        }
        None => (inner, None),
    };
    let at = named.rfind('@')?;
    let repo = named[..at].trim().to_string();
    let sha = named[at + 1..].trim().to_ascii_lowercase();
    (!repo.is_empty() && is_sha256(&sha)).then_some((repo, sha, profile))
}

fn asks_dir() -> Option<PathBuf> {
    Some(store()?.join("asks"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Before an `AskUserQuestion` runs: remember a marked question, and whether
/// it arrived pre-answered, so the answer can be trusted or not.
pub fn on_pre_ask(ask: &amont_agent_guard::payload::Ask) {
    let marked: Vec<(String, String, Option<String>)> =
        ask.questions.iter().filter_map(|q| marker(q)).collect();
    if marked.is_empty() || ask.tool_use_id.is_empty() || ask.tool_use_id.contains('/') {
        return;
    }
    let Some(d) = asks_dir() else { return };
    if private_dir(&d).is_none() {
        return;
    }
    let names: Vec<String> = marked
        .iter()
        .map(|(repo, sha, profile)| match profile {
            Some(profile) => format!("{repo}@{sha}{PROFILE_KEY}{profile}"),
            None => format!("{repo}@{sha}"),
        })
        .collect();
    let _ = amont_agent_guard::atomic::write_atomic(
        &d.join(&ask.tool_use_id),
        &format!(
            "{}\t{}\t{}\t{}\n",
            ask.session.replace(['\t', '\n'], " "),
            ask.prefilled,
            names.join(","),
            now_ms()
        ),
    );
    if ask.prefilled {
        note("prefilled", &ask.session, &marked[0].0, &names.join(","));
    }
}

/// After an `AskUserQuestion`: an `Overrule` of a `rework` this session's
/// transcript shows is remembered as a pass; anything else changes nothing.
/// Returns what the session must hear when an overrule bound nothing.
pub fn on_post_ask(ask: &amont_agent_guard::payload::Ask) -> Option<String> {
    if !ask.questions.iter().any(|q| marker(q).is_some()) {
        return None;
    }
    let recorded = asks_dir()
        .map(|d| d.join(&ask.tool_use_id))
        .filter(|_| !ask.tool_use_id.is_empty() && !ask.tool_use_id.contains('/'))
        .and_then(|p| {
            let t = std::fs::read_to_string(&p).ok();
            let _ = std::fs::remove_file(&p);
            t
        });
    let Some(recorded) = recorded else {
        note(
            "unrecorded",
            &ask.session,
            "-",
            "marked question with no PreToolUse record",
        );
        return None;
    };
    let f: Vec<&str> = recorded.trim_end().split('\t').collect();
    if f.len() != 4 || f[0] != ask.session || f[1] != "false" {
        return None;
    }
    let mut said: Vec<String> = Vec::new();
    for question in &ask.questions {
        let Some((repo, sha, profile)) = marker(question) else {
            continue;
        };
        let reviewed = match &profile {
            Some(profile) => format!("code review ({profile})"),
            None => "implementation review".to_string(),
        };
        let label = ask
            .answers
            .iter()
            .find(|(q, _)| q == question)
            .map(|(_, l)| l.as_str())
            .unwrap_or("");
        if label != OVERRULE {
            if !label.is_empty() {
                note(
                    "answered",
                    &ask.session,
                    &repo,
                    &format!("tree={} answer={}", short(&sha), label.to_ascii_lowercase()),
                );
            }
            continue;
        }
        let Some(transcript) = ask.transcript.as_deref() else {
            note(
                "unknown",
                &ask.session,
                &repo,
                &format!("tree={} overrule without a transcript", short(&sha)),
            );
            said.push(format!(
                "The answer Overrule bound NOTHING for {repo} tree {} ({reviewed}): the question's event carried no transcript, so the rework it overrules cannot be seen.",
                short(&sha)
            ));
            continue;
        };
        let reworked = match &profile {
            None => reviews(transcript)
                .map(|rs| {
                    rs.iter()
                        .rev()
                        .find(|r| r.repo == repo && r.sha == sha)
                        .is_some_and(|r| matches!(r.verdict, Some(Verdict::Rework) | None))
                })
                .unwrap_or(false),
            Some(profile) => profile_reviews(transcript)
                .map(|rs| {
                    rs.iter()
                        .rev()
                        .find(|r| r.repo == repo && r.sha == sha && r.profile == *profile)
                        .is_some_and(|r| matches!(r.verdict, Some(Verdict::Rework) | None))
                })
                .unwrap_or(false),
        };
        if !reworked {
            note(
                "unbound",
                &ask.session,
                &repo,
                &format!("tree={} overrule with no rework to overrule", short(&sha)),
            );
            said.push(format!(
                "The answer Overrule bound NOTHING for {repo} tree {} ({reviewed}): no completed {reviewed} of that tree said rework in this session.",
                short(&sha)
            ));
            continue;
        }
        match &profile {
            None => {
                remember(
                    &repo,
                    &sha,
                    "overruled",
                    "person",
                    &PromptRules::NotRecorded,
                    &Reviewed::not_recorded(),
                );
            }
            Some(profile) => {
                remember_profile(profile, &repo, &sha, "overruled", "person", &sha, None);
            }
        }
        note(
            "overruled",
            &ask.session,
            &repo,
            &format!("tree={} review=overruled verdict=rework", short(&sha)),
        );
    }
    (!said.is_empty()).then(|| said.join("\n\n"))
}

/// One journal line under the rule's id, for what happens away from a push.
fn note(outcome: &str, session: &str, repo: &str, excerpt: &str) {
    journal::record(&journal::Entry {
        rule: RULE_ID,
        stance: "-",
        outcome,
        session,
        repo,
        mode: "-",
        excerpt,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_id::content_id_of;

    /// A real session's launch of an implementation reviewer, resumed twice
    /// with `SendMessage` (2026-10-08), anonymised.
    const RESUMED: &str = include_str!("../../../tests/fixtures/sendmessage-resume.jsonl");
    const LAUNCH_TREE: &str = "6066ad987edfbbb3a514b6d9247b126a77a316db2e1350c226ffd14593a0ba35";
    const ROUND_2: &str = "b196a592b3b76cb75155b14491291a45602ae66c6f20ef18e4413a34169556ab";
    const ROUND_3: &str = "85f71e67db10c7737c55b27a3c5753b2617c58dec0d58e7ebae5085898cc12d7";

    /// The known answers attest's SPEC quotes under "Content id". Changing
    /// one is changing the definition: every stored review and approval
    /// would stop matching.
    #[test]
    fn the_content_id_gives_the_spec_known_answers() {
        let o1 = "1".repeat(40);
        let o2 = "2".repeat(40);
        let id = |recs: &[String]| {
            content_id_of(
                &recs
                    .iter()
                    .map(|r| r.as_bytes().to_vec())
                    .collect::<Vec<_>>(),
            )
        };
        // no records: the sha256 of the empty string
        assert_eq!(
            id(&[]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            id(&[format!("100644 blob {o1}\tREADME.md")]),
            "6f0214fffd79cb8eb5afb2283e46911f5f9022a25b3f339ae8a50a1ee223a0b2"
        );
        // every record under the top-level docs/plans/ is left out, a
        // symlink too; a near name and a nested docs/plans/ are kept
        let several = [
            format!("100644 blob {o1}\tREADME.md"),
            format!("100644 blob {o2}\tdocs/plans/a.md"),
            format!("100644 blob {o2}\tdocs/plans-old/a.md"),
            format!("120000 blob {o1}\tdocs/plans/link"),
            format!("100644 blob {o1}\tx/docs/plans/a.md"),
        ];
        let several_id = "0fca850e419d8a202129087fd90f00fe285c4b9cc63b42f4db6162e0a2b12eaf";
        assert_eq!(id(&several), several_id);
        // so a plan's existence does not count, unlike attest's canon(T)
        assert_eq!(
            id(&[several[0].clone(), several[2].clone(), several[4].clone()]),
            several_id
        );
        // a raw path with a newline: no quoting, no decoding
        assert_eq!(
            id(&[
                format!("100644 blob {o1}\tdocs/a\nb.md"),
                format!("100644 blob {o2}\tdocs/plans/c\nd.md"),
            ]),
            "88a9c5d4a7b3892be98a8492631b3c436954c2000a038b5d641bd054e76e19e0"
        );
    }

    fn reviews_of(text: &str) -> Vec<(String, Option<Verdict>)> {
        completed_agents_resumable(text.as_bytes(), AGENT)
            .into_iter()
            .flat_map(|c| {
                let v = verdict_of(&c.result);
                parse_blocks(&c.prompt)
                    .into_iter()
                    .map(move |(_, sha)| (sha, v))
            })
            .collect()
    }

    #[test]
    fn each_resumed_round_is_a_review_of_its_own_tree() {
        assert_eq!(
            reviews_of(RESUMED),
            vec![
                (LAUNCH_TREE.to_string(), Some(Verdict::ApproveWithChanges)),
                (ROUND_2.to_string(), Some(Verdict::Approve)),
                (ROUND_3.to_string(), Some(Verdict::Approve)),
            ],
            "the launch keeps its own verdict"
        );
    }

    #[test]
    fn the_shared_reader_counts_the_launch_alone() {
        // What the plan-review panel reads: a resume is not a launch.
        let launches = crate::plan_review::completed_agents(RESUMED.as_bytes(), AGENT);
        assert_eq!(launches.len(), 1);
        assert!(launches[0].prompt.contains(LAUNCH_TREE));
        assert_eq!(
            verdict_of(&launches[0].result),
            Some(Verdict::ApproveWithChanges),
            "the rounds' notifications do not overwrite the launch's"
        );
    }

    #[test]
    fn a_resume_of_an_agent_that_is_not_a_reviewer_does_not_count() {
        let other = RESUMED.replace(
            "\"subagent_type\":\"implementation-review\"",
            "\"subagent_type\":\"general-purpose\"",
        );
        assert!(reviews_of(&other).is_empty());
        // The message went to another agent than the one that resumed.
        let elsewhere = RESUMED.replace(
            "\"to\":\"aa339dbce3244a139\"",
            "\"to\":\"a0000000000000000\"",
        );
        assert_eq!(reviews_of(&elsewhere).len(), 1);
    }

    #[test]
    fn a_resumed_round_needs_its_own_agents_structured_notification() {
        // Another task's id on the round's notifications.
        let lines: Vec<String> = RESUMED
            .lines()
            .map(|l| {
                if l.contains("\"origin\":{\"kind\":\"task-notification\"") {
                    l.replace(
                        "<task-id>aa339dbce3244a139</task-id>",
                        "<task-id>a0000000000000000</task-id>",
                    )
                } else {
                    l.to_string()
                }
            })
            .collect();
        assert_eq!(reviews_of(&lines.join("\n")).len(), 1);

        // The round's notifications echoed in a tool result and typed by a
        // person: neither was queued by Claude Code.
        let forged: Vec<String> = RESUMED
            .lines()
            .map(|l| {
                if l.contains("\"origin\":{\"kind\":\"task-notification\"") {
                    l.replace(
                        "\"origin\":{\"kind\":\"task-notification\"",
                        "\"origin\":{\"kind\":\"human\"",
                    )
                } else {
                    l.to_string()
                }
            })
            .collect();
        assert_eq!(reviews_of(&forged.join("\n")).len(), 1);
    }

    #[test]
    fn a_block_round_trips_and_a_short_sha_does_not_bind() {
        let sha = "a".repeat(64);
        let b = block("amont-agent", &sha);
        assert_eq!(b, format!("<<<TREE repo=amont-agent sha={sha}>>>"));
        assert_eq!(
            parse_blocks(&format!("Round 1.\n{b}\nbrief")),
            vec![("amont-agent".to_string(), sha.clone())]
        );
        assert!(parse_blocks("<<<TREE repo=x sha=abc123>>>").is_empty());
        assert!(parse_blocks("<<<TREE sha=abc>>>").is_empty());
        assert!(parse_blocks(&format!("<<<TREE repo= sha={sha}>>>")).is_empty());
    }

    #[test]
    fn the_sha_is_lowercased() {
        let sha = "A".repeat(64);
        let got = parse_blocks(&block("r", &sha));
        assert_eq!(got[0].1, "a".repeat(64));
    }

    #[test]
    fn the_verdict_is_the_first_verdict_line() {
        assert_eq!(
            verdict_of("Verdict: approve\nFindings:"),
            Some(Verdict::Approve)
        );
        assert_eq!(
            verdict_of("**Verdict: approve-with-changes**\n"),
            Some(Verdict::ApproveWithChanges)
        );
        assert_eq!(
            verdict_of("preamble\nVerdict: rework"),
            Some(Verdict::Rework)
        );
        assert_eq!(verdict_of("no verdict here"), None);
        assert_eq!(verdict_of("Verdict: maybe"), None);
    }

    #[test]
    fn the_marker_splits_at_the_last_at_and_wants_64_hex() {
        let sha = "b".repeat(64);
        assert_eq!(
            marker(&format!("[implementation-review my@repo@{sha}] Overrule?")),
            Some(("my@repo".to_string(), sha.clone(), None))
        );
        assert_eq!(marker("[implementation-review repo@abc] x"), None);
        assert_eq!(marker(&format!("[implementation-review @{sha}]")), None);
        assert_eq!(marker("[preview a1] x"), None);
    }

    #[test]
    fn the_marker_names_a_profile_when_it_carries_one() {
        let sha = "c".repeat(64);
        assert_eq!(
            marker(&format!(
                "[implementation-review app@{sha} profile=backend] ?"
            )),
            Some(("app".to_string(), sha.clone(), Some("backend".to_string())))
        );
        assert_eq!(
            marker(&format!(
                "[implementation-review app@{sha} profile=lang:rust] ?"
            )),
            Some((
                "app".to_string(),
                sha.clone(),
                Some("lang:rust".to_string())
            ))
        );
        assert_eq!(
            marker(&format!(
                "[implementation-review app@{sha} profile=wizard] ?"
            )),
            None
        );
    }

    #[test]
    fn lexical_folds_dot_and_dotdot() {
        assert_eq!(lexical(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
    }

    #[test]
    fn a_read_of_the_store_passes_and_a_write_does_not() {
        use amont_agent_guard::shell::lex;
        let store = "~/.claude/amont-agent/implementation-review/by-tree/r/x.json";
        for read in [
            format!("ls {store}"),
            format!("cat {store} | head"),
            "find ~/.claude/amont-agent/implementation-review -type f".to_string(),
            format!("grep verdict {store}"),
        ] {
            assert!(store_clause(&lex(&read)).is_none(), "{read}");
        }
        for write in [
            format!("echo '{{}}' > {store}"),
            format!("printf x >> {store}"),
            format!("rm -f {store}"),
            format!("cp /tmp/x {store}"),
            format!("python3 -c 'open(\"{store}\",\"w\")'"),
            format!("tee {store} < /dev/null"),
            format!("cat x | sudo tee {store}"),
        ] {
            assert!(store_clause(&lex(&write)).is_some(), "{write}");
        }
    }

    #[test]
    fn a_word_names_the_store_by_its_directory() {
        assert!(names_store(
            "/Users/me/.claude/amont-agent/implementation-review/by-tree/r/x.json"
        ));
        assert!(names_store("~/.claude/amont-agent/implementation-review"));
        assert!(names_store(
            "C:\\Users\\me\\.claude\\amont-agent\\implementation-review\\by-tree\\r\\x.json"
        ));
        assert!(!names_store("~/.claude/amont-agent/journal.log"));
        assert!(!names_store("implementation-review"));
        // The outcome ledger lives beside the store, not in it.
        assert!(!names_store(
            "~/.claude/amont-agent/implementation-review-outcomes/outcomes.jsonl"
        ));
    }
}
