//! The code-review profiles a plan declares (decisions ADR-0052, ADR-0061):
//! the `## Code review` section, the proposal `plan-panel --code-review`
//! prints from the files the plan's Phases name in each repository (from the
//! repository's areas when they name none), and the judge rules each profile
//! reviews.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::file_types;
use crate::plan_review::{locate, meta, root, seating, SeatSource};
use crate::plan_text::Fence;
use amont_agent_guard::rules::Stance;

const SECTION: &str = "Code review";
/// The names a rule's `**Reviewed by:**` line may carry.
const REVIEWER_NAMES: [&str; 8] = [
    "backend",
    "language",
    "platform",
    "unix",
    "tui",
    "react",
    "architect",
    "ui-design",
];
/// The profiles that are not `lang:<x>`, in the order a proposal lists them
/// (`backend` first, the languages after it).
const FIXED_PROFILES: [&str; 7] = [
    "backend",
    "platform",
    "unix",
    "tui",
    "react",
    "ui-design",
    "architect",
];

/// What the plan says about one repository's code review.
#[derive(Debug, PartialEq, Eq)]
pub enum Declared {
    Profiles {
        profiles: Vec<String>,
        /// The reason given for the profiles the proposal named and the plan
        /// left out, kept as text.
        left_out: Option<String>,
    },
    NoReview {
        reason: String,
    },
}

impl Declared {
    /// `backend, unix (reason)` or `none — reason`.
    pub fn describe(&self) -> String {
        match self {
            Declared::Profiles { profiles, left_out } => {
                let list = profiles.join(", ");
                match left_out {
                    Some(reason) => format!("{list} ({reason})"),
                    None => list,
                }
            }
            Declared::NoReview { reason } => format!("none — {reason}"),
        }
    }
}

/// The code profiles the panel's areas call for. `backend` always; the
/// product roles (`po`, `ux-research`, `game-ux`) have no code to read.
pub fn code_roles(areas: &BTreeSet<String>) -> BTreeSet<String> {
    let mut roles = BTreeSet::from(["backend".to_string()]);
    for area in areas {
        let add: &[&str] = match area.as_str() {
            "ops" => &["platform"],
            "cli" => &["unix", "tui"],
            "ui" => &["react", "ui-design"],
            "contract" | "large" => &["architect"],
            _ => &[],
        };
        roles.extend(add.iter().map(|s| s.to_string()));
        if area.starts_with("lang:") {
            roles.insert(area.clone());
        }
    }
    roles
}

/// What `plan-panel --code-review` proposes for one repository, in role
/// order; empty (`none`) when its areas carry no language.
pub fn proposed(areas: &BTreeSet<String>) -> Vec<String> {
    if !areas.iter().any(|a| a.starts_with("lang:")) {
        return Vec::new();
    }
    in_order(code_roles(areas))
}

/// Profiles in the order a proposal lists them: backend, the languages,
/// then [`FIXED_PROFILES`].
fn in_order(roles: BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    out.extend(roles.iter().filter(|r| *r == "backend").cloned());
    out.extend(roles.iter().filter(|r| r.starts_with("lang:")).cloned());
    for fixed in &FIXED_PROFILES[1..] {
        out.extend(roles.iter().filter(|r| *r == fixed).cloned());
    }
    out
}

/// Whether `profile` is one a plan may declare or `--profile` may name.
pub fn valid_profile(profile: &str) -> bool {
    FIXED_PROFILES.contains(&profile)
        || (profile.starts_with("lang:") && crate::plan_review::valid_class(profile))
}

/// The agent that reviews `profile` and the language it is told to read
/// (`--lang <x>`): `lang:<x>` is `code-review-language`, `ui-design` is
/// `interface-review`, every other profile `code-review-<role>`. `None` for
/// a name that is not a profile.
pub fn agent_for(profile: &str) -> Option<(String, Option<String>)> {
    if !valid_profile(profile) {
        return None;
    }
    Some(match profile.strip_prefix("lang:") {
        Some(language) => (
            "code-review-language".to_string(),
            Some(language.to_string()),
        ),
        None if profile == "ui-design" => ("interface-review".to_string(), None),
        None => (format!("code-review-{profile}"), None),
    })
}

/// The directory name a profile's passes are filed under (`lang:rust` is
/// `lang-rust`); `None` for a name that is not a profile.
pub fn slug(profile: &str) -> Option<String> {
    valid_profile(profile).then(|| profile.replace(':', "-"))
}

/// The value of each `Profile:` line in a review prompt's [`header`], in
/// order.
pub fn profile_lines(prompt: &str) -> Vec<&str> {
    header(prompt)
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Profile:"))
        .map(str::trim)
        .collect()
}

/// A review prompt's header: its text up to the first line that opens the
/// body. That line is a Markdown heading (`#` to `######` then a space), a
/// code fence (three or more backticks or tildes), or an unfenced diff
/// (`diff --git`, `@@`). An inlined brief starts with its `# … brief`
/// heading and an inlined diff is fenced or starts with `diff --git`, so
/// neither is in the header. A review's `Profile:` line and the review blocks
/// it binds are read from here only: the launch's own lines come first.
pub(crate) fn header(prompt: &str) -> &str {
    let mut end = 0;
    for line in prompt.split_inclusive('\n') {
        if opens_body(line.trim()) {
            break;
        }
        end += line.len();
    }
    &prompt[..end]
}

/// Whether a trimmed prompt line opens the body that follows the header.
fn opens_body(line: &str) -> bool {
    let rest = line.trim_start_matches('#');
    let hashes = line.len() - rest.len();
    let heading = (1..=6).contains(&hashes) && rest.starts_with(' ');
    heading
        || line.starts_with("```")
        || line.starts_with("~~~")
        || line.starts_with("diff --git")
        || line.starts_with("@@")
}

/// What `plan-panel --code-review` proposes for one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proposal {
    /// The profiles to declare, in [`in_order`]'s order.
    Profiles(Vec<String>),
    /// No code review, and why: the third field of a `none` line.
    NoReview(NoReviewReason),
}

/// Why a repository is proposed `none`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoReviewReason {
    /// Every file named in it is markdown (or a directory).
    MarkdownOnly,
    /// Files are named in it, none of a type a profile reviews (a shell
    /// script, a config file).
    NoReviewedFileType,
    /// No named path is attributed to it.
    NoFileNamed,
    /// Seated from areas, and its areas carry no language.
    NoLanguageArea,
    /// The change is a registered, fully reviewed block (ADR-0067): the
    /// short id of its fingerprint, `fp:` and 12 hex characters.
    CopyOf(String),
}

impl std::fmt::Display for NoReviewReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoReviewReason::MarkdownOnly => f.write_str("markdown only"),
            NoReviewReason::NoReviewedFileType => f.write_str("no reviewed file type"),
            NoReviewReason::NoFileNamed => f.write_str("no file named"),
            NoReviewReason::NoLanguageArea => f.write_str("no language area"),
            NoReviewReason::CopyOf(id) => write!(f, "copy of {id}"),
        }
    }
}

/// What the files attributed to one repository propose. A repository is
/// reviewed when its files earn a code class (a language, `ui`, `ops`); the
/// plan's declared classes then add theirs (`contract` → architect).
pub fn proposed_for_paths(
    paths: &[String],
    repo_areas: &BTreeSet<String>,
    adds: &[String],
) -> Proposal {
    if paths.is_empty() {
        return Proposal::NoReview(NoReviewReason::NoFileNamed);
    }
    let mut classes = file_types::classes(paths, repo_areas);
    let code = |class: &String| class.starts_with("lang:") || class == "ui" || class == "ops";
    if !classes.iter().any(code) {
        let markdown = paths
            .iter()
            .all(|p| p.ends_with(".md") || p.ends_with(".mdx") || p.ends_with('/'));
        return Proposal::NoReview(if markdown {
            NoReviewReason::MarkdownOnly
        } else {
            NoReviewReason::NoReviewedFileType
        });
    }
    classes.extend(adds.iter().cloned());
    Proposal::Profiles(in_order(code_roles(&classes)))
}

/// The proposal for every repository of the plan's machine comment, one
/// `<repo>\t<profiles>` line each, or `<repo>\tnone\t<reason>` (so `cut -f2`
/// reads `none`, and the line pastes as `- <repo>: none — <reason>`).
pub fn proposal(plan_text: &str) -> Result<String, String> {
    let machine = meta(plan_text)?;
    let seated = seating(&machine, plan_text)?;
    let mut out = String::new();
    for repo in &seated.repos {
        let proposed = match seated.source {
            SeatSource::Paths => proposed_for_paths(&repo.paths, &repo.areas, &machine.adds),
            SeatSource::Areas { .. } => match proposed(&repo.areas) {
                profiles if profiles.is_empty() => {
                    Proposal::NoReview(NoReviewReason::NoLanguageArea)
                }
                profiles => Proposal::Profiles(profiles),
            },
        };
        match proposed {
            Proposal::Profiles(profiles) => {
                out.push_str(&format!("{}\t{}\n", repo.name, profiles.join(",")))
            }
            Proposal::NoReview(reason) => out.push_str(&format!("{}\tnone\t{reason}\n", repo.name)),
        }
    }
    Ok(out)
}

/// What `plan-panel --code-review -C <dir>` makes of the branch at `dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchProposal {
    /// The repository's name, as a plan's machine comment spells it.
    pub repo: String,
    /// `Some(fp:<12 hex>)` when the branch is a registered, unrevoked block
    /// whose pin check passed and the hook enforces.
    pub copy_of: Option<String>,
    /// Lines for stderr: why a registered block is not proposed as a copy.
    pub notices: Vec<String>,
}

/// How long the proposal gives `git` to find the merge base and the diff.
const BRANCH_GIT_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

/// Look at the branch `base..HEAD` of the repository holding `dir`. A binary
/// diff, or a branch that is not a registered block, proposes nothing and says
/// nothing. A git or role failure says why on stderr and the profiles are
/// proposed. `hook` is the stance of the implementation-review hook: a copy
/// line is only verified when it denies. The pin check runs here without
/// writing a record.
pub fn branch_proposal(
    dir: &Path,
    base: Option<&str>,
    hook: Stance,
) -> Result<BranchProposal, String> {
    let repo = amont_agent_guard::push_target::toplevel(dir)
        .ok_or_else(|| format!("{} is not inside a git repository", dir.display()))?;
    let repo_name = crate::implementation_review::repo_name(&repo)
        .ok_or_else(|| "the repository has no name".to_string())?;
    let mut proposal = BranchProposal {
        repo: repo_name,
        copy_of: None,
        notices: Vec::new(),
    };
    let deadline = std::time::Instant::now() + BRANCH_GIT_BUDGET;
    let unproven = |mut proposal: BranchProposal, cause: String| {
        proposal.notices.push(format!(
            "amont-agent: plan-panel: {cause}; the profiles are proposed"
        ));
        Ok(proposal)
    };
    let base = match base {
        Some(base) => base.to_string(),
        None => match amont_agent_guard::git::default_ref(&repo, deadline) {
            Some((reference, _)) => reference,
            None => {
                return unproven(
                    proposal,
                    "no default branch of `origin` is known; pass --base <ref>".to_string(),
                )
            }
        },
    };
    let Some(merge_base) = amont_agent_guard::git::stdout_bounded(
        &repo,
        &["merge-base", base.as_str(), "HEAD"],
        deadline,
    ) else {
        return unproven(proposal, format!("`git merge-base {base} HEAD` failed"));
    };
    let fingerprint =
        match crate::block_fingerprint::fingerprint(&repo, &merge_base, "HEAD", deadline) {
            Ok(Some(fingerprint)) => fingerprint,
            Ok(None) => return Ok(proposal),
            Err(why) => return unproven(proposal, format!("git failed: {why}")),
        };
    let Some(state) = amont_agent_guard::journal::dir() else {
        return Ok(proposal);
    };
    let id = fingerprint.short();
    let needed = match crate::implementation_review::roles_needed(&repo, &merge_base, "HEAD") {
        Ok(needed) => needed,
        Err(why) => return unproven(proposal, format!("the role set failed: {why}")),
    };
    if crate::implementation_review::registered_cover(
        &repo,
        &state,
        fingerprint.full(),
        &id,
        &needed,
    )
    .is_err()
    {
        return Ok(proposal);
    }
    match hook {
        Stance::Deny => {}
        Stance::Observe | Stance::Advise => {
            proposal.notices.push(
                "amont-agent: plan-panel: the implementation-review hook does not enforce; the profiles are proposed"
                    .to_string(),
            );
            return Ok(proposal);
        }
    }
    let pin = crate::pin_check::check(&repo, &merge_base);
    match pin {
        crate::pin_check::Outcome::Passed { .. } => proposal.copy_of = Some(id),
        crate::pin_check::Outcome::Failed { why } => proposal.notices.push(format!(
            "amont-agent: plan-panel: {id} is a registered copy but the pin check failed: {why}; the profiles are proposed"
        )),
    }
    Ok(proposal)
}

/// The body's `## Code review` section: `Ok(None)` when there is none.
pub fn parse(plan_text: &str) -> Result<Option<BTreeMap<String, Declared>>, String> {
    let mut fence = Fence::default();
    let mut inside = false;
    let mut found = false;
    let mut declared = BTreeMap::new();
    for line in plan_text.lines() {
        if let Some(title) = fence.h2(line) {
            if inside {
                break;
            }
            inside = title == SECTION;
            found |= inside;
            continue;
        }
        if !inside {
            continue;
        }
        let Some(item) = line.trim_start().strip_prefix("- ") else {
            continue;
        };
        let (repo, value) = parse_line(item).map_err(|why| format!("`{}`: {why}", line.trim()))?;
        if declared.insert(repo.clone(), value).is_some() {
            return Err(format!("`{}`: `{repo}` is listed twice", line.trim()));
        }
    }
    Ok(found.then_some(declared))
}

fn parse_line(entry: &str) -> Result<(String, Declared), String> {
    let (repo, declaration) = entry
        .split_once(':')
        .ok_or("expected `- <repo>: <profiles>` or `- <repo>: none — <reason>`")?;
    let repo = repo.trim();
    if !crate::plan_review::valid_name(repo) {
        return Err(format!("`{repo}` is not a repository name"));
    }
    let declaration = declaration.trim();
    let none_rest = declaration.strip_prefix("none").filter(|rest| {
        rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == '—')
    });
    if let Some(rest) = none_rest {
        let rest = rest.trim_start();
        let reason = rest
            .strip_prefix('—')
            .or_else(|| rest.strip_prefix('-'))
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .ok_or("`none` needs a reason: `none — <reason>`")?;
        return Ok((
            repo.to_string(),
            Declared::NoReview {
                reason: reason.to_string(),
            },
        ));
    }
    let (list, left_out) = match (declaration.strip_suffix(')'), declaration.find('(')) {
        (Some(inner), Some(open)) => (
            &declaration[..open],
            Some(inner[open + 1..].trim().to_string()).filter(|r| !r.is_empty()),
        ),
        _ => (declaration, None),
    };
    let mut profiles: Vec<String> = Vec::new();
    for profile in list.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if !valid_profile(profile) {
            return Err(format!(
                "unknown profile `{profile}` (backend, platform, unix, tui, react, ui-design, architect or lang:<x>)"
            ));
        }
        if !profiles.iter().any(|p| p == profile) {
            profiles.push(profile.to_string());
        }
    }
    if profiles.is_empty() {
        return Err("the profile list is empty".into());
    }
    Ok((repo.to_string(), Declared::Profiles { profiles, left_out }))
}

/// Whether a plan file is a pointer: its front matter names a `canonical:`
/// plan in another repository.
pub(crate) fn is_pointer(plan_text: &str) -> bool {
    let (front, _) = crate::plans::split_front_matter(plan_text);
    crate::plans::field(front, "canonical").is_some_and(|canonical| !canonical.is_empty())
}

/// Whether a pointer plan carries its own `## Code review` section (one that
/// parses, or one that does not: either way it is the pointer's own).
pub(crate) fn has_own_section(plan_text: &str) -> bool {
    is_pointer(plan_text) && !matches!(parse(plan_text), Ok(None))
}

/// A plan's declaration text: the plan itself; for a pointer, its own
/// `## Code review` section when it has one (ADR-0052: it is versioned with
/// the branch), else the canonical file it names, read under the plan root.
/// `Err` names a canonical file that cannot be read.
pub(crate) fn declared_text(plan_text: &str) -> Result<String, String> {
    if has_own_section(plan_text) {
        return Ok(plan_text.to_string());
    }
    canonical_text(plan_text)
}

/// A pointer's own section must declare this repository: an empty one, or one
/// naming only other repositories, would drop the canonical's requirements
/// without stating any.
pub(crate) fn require_own_line(
    plan_text: &str,
    declared: &BTreeMap<String, Declared>,
    repo: &str,
) -> Result<(), String> {
    if has_own_section(plan_text) && !declared.contains_key(repo) {
        return Err(format!(
            "the pointer's own ## Code review section declares nothing for `{repo}`"
        ));
    }
    Ok(())
}

/// The canonical plan's text for a pointer (the plan itself otherwise), read
/// under the plan root's checkout.
pub(crate) fn canonical_text(plan_text: &str) -> Result<String, String> {
    let (front, _) = crate::plans::split_front_matter(plan_text);
    let Some(canonical) = crate::plans::field(front, "canonical").filter(|c| !c.is_empty()) else {
        return Ok(plan_text.to_string());
    };
    let Some((repo, path)) = canonical.split_once(':') else {
        return Ok(plan_text.to_string());
    };
    if !crate::plan_review::valid_name(repo) || path.split('/').any(|part| part == "..") {
        return Err(format!("canonical `{canonical}` is not a plan path"));
    }
    let plan_root = root().ok_or_else(|| {
        format!("canonical `{canonical}` cannot be read: no plan root could be determined")
    })?;
    let file = locate(&plan_root, repo).join(path);
    std::fs::read_to_string(&file)
        .map_err(|e| format!("canonical {} cannot be read: {e}", file.display()))
}

/// `Declared code review for <repo>: …` for the brief's plan section. A
/// declaration that cannot be read or parsed is an error, so the brief exits
/// 1 as `plan-panel` does, rather than describing a review nobody declared.
pub fn declared_line(repo: &str, plan_text: Option<&str>) -> Result<String, String> {
    let shown = match plan_text.map(|text| {
        declared_text(text)
            .and_then(|t| parse(&t))
            .and_then(|map| match map {
                Some(map) => require_own_line(text, &map, repo).map(|()| Some(map)),
                None => Ok(None),
            })
    }) {
        None | Some(Ok(None)) => "not declared".to_string(),
        Some(Ok(Some(map))) => map
            .get(repo)
            .map(Declared::describe)
            .unwrap_or_else(|| "not declared".to_string()),
        Some(Err(why)) => return Err(format!("the plan's ## Code review section: {why}")),
    };
    Ok(format!("Declared code review for {repo}: {shown}"))
}

/// Every active rule's `Reviewed by:` names are code-review profiles: the
/// drift guard between the rule bodies and [`REVIEWER_NAMES`], run on every
/// brief, not only a profile's.
pub fn check_reviewed_by(json: &[u8]) -> Result<(), String> {
    judge_rules(json, "").map(|_| ())
}

/// The text after `marker` on its line and the lines that continue it, up to
/// a blank line.
fn paragraph_after(body: &str, marker: &str) -> Option<String> {
    let mut lines = body.lines();
    let first = lines.by_ref().find_map(|l| l.split_once(marker))?.1;
    let mut text = first.to_string();
    for line in lines.take_while(|l| !l.trim().is_empty()) {
        text.push(' ');
        text.push_str(line);
    }
    Some(text)
}

/// The mechanism classes a `**Checked by:**` paragraph names: the first word
/// of each comma-separated entry outside parentheses.
fn checked_classes(body: &str) -> Vec<String> {
    let Some(text) = paragraph_after(body, "**Checked by:**") else {
        return Vec::new();
    };
    let mut classes = Vec::new();
    let (mut depth, mut entry) = (0usize, String::new());
    for ch in text.chars().chain(std::iter::once(',')) {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                let word: String = entry
                    .trim()
                    .chars()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .collect();
                if !word.is_empty() {
                    classes.push(word.to_ascii_lowercase());
                }
                entry.clear();
            }
            _ if depth == 0 => entry.push(ch),
            _ => {}
        }
    }
    classes
}

fn reviewed_by(body: &str) -> Option<Vec<String>> {
    let text = paragraph_after(body, "**Reviewed by:**")?;
    Some(
        text.split(',')
            .map(|n| n.trim().trim_end_matches('.').trim().to_string())
            .filter(|n| !n.is_empty())
            .collect(),
    )
}

fn names_role(name: &str, role: &str) -> bool {
    name == role || (name == "language" && role.starts_with("lang:"))
}

/// Every active rule's text whose checks are all `judge` and whose
/// `Reviewed by:` names `role`, from the `aval rules` JSON. A name outside
/// [`REVIEWER_NAMES`] is an error naming the rule.
fn judge_rules(json: &[u8], role: &str) -> Result<Vec<String>, String> {
    let document: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("aval rules is not JSON: {e}"))?;
    let rules = document
        .get("rules")
        .and_then(serde_json::Value::as_array)
        .ok_or("aval rules has no `rules` array")?;
    let mut out = Vec::new();
    for rule in rules {
        if rule.get("active").and_then(serde_json::Value::as_bool) == Some(false) {
            continue;
        }
        let field = |name: &str| rule.get(name).and_then(serde_json::Value::as_str);
        let (Some(id), Some(body)) = (field("id"), field("body")) else {
            continue;
        };
        let Some(names) = reviewed_by(body) else {
            continue;
        };
        if let Some(bad) = names.iter().find(|n| !REVIEWER_NAMES.contains(&n.as_str())) {
            return Err(format!(
                "rule `{id}`: `Reviewed by:` names `{bad}`, which is not a code-review profile"
            ));
        }
        let classes = checked_classes(body);
        let judge_only = !classes.is_empty() && classes.iter().all(|c| c == "judge");
        if judge_only && names.iter().any(|n| names_role(n, role)) {
            out.push(format!(
                "### {id}\n\n{}\n\n{body}\n",
                field("statement").unwrap_or_default()
            ));
        }
    }
    Ok(out)
}

/// The brief's `## Judge rules for <role>` section.
pub fn judge_section(json: &[u8], role: &str) -> Result<String, String> {
    let rules = judge_rules(json, role)?;
    let body = if rules.is_empty() {
        "(no judge rule names this profile)\n".to_string()
    } else {
        rules.join("\n")
    };
    Ok(format!("## Judge rules for {role}\n\n{body}\n"))
}

/// Reads a plan file for [`parse`].
pub fn read_plan(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn plan(section: &str) -> String {
        format!("# P\n\n## Context\n\nx\n\n## Code review\n\n{section}\n\n## Phases\n\n- [ ] one\n")
    }

    #[test]
    fn a_cli_in_rust_is_backend_language_unix_tui() {
        let areas = set(&["cli", "lang:rust"]);
        assert_eq!(
            code_roles(&areas),
            set(&["backend", "lang:rust", "unix", "tui"])
        );
        assert_eq!(proposed(&areas), ["backend", "lang:rust", "unix", "tui"]);
    }

    #[test]
    fn a_ui_is_react_and_ui_design_and_never_a_product_role() {
        assert_eq!(
            code_roles(&set(&["ui", "large", "ops", "lang:ts"])),
            set(&[
                "backend",
                "lang:ts",
                "react",
                "ui-design",
                "architect",
                "platform"
            ])
        );
        assert_eq!(
            proposed(&set(&["ui", "lang:ts"])),
            ["backend", "lang:ts", "react", "ui-design"]
        );
    }

    #[test]
    fn named_files_propose_the_profiles_of_their_types() {
        let paths = |ps: &[&str]| ps.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let none = BTreeSet::new();
        let cli = set(&["cli"]);
        let profiles = |ps: &[&str]| Proposal::Profiles(ps.iter().map(|p| p.to_string()).collect());
        assert_eq!(
            proposed_for_paths(&paths(&["web/Settings.tsx"]), &none, &[]),
            profiles(&["backend", "lang:typescript", "react", "ui-design"])
        );
        assert_eq!(
            proposed_for_paths(&paths(&["src/dispatch.rs"]), &cli, &["contract".into()]),
            profiles(&["backend", "lang:rust", "unix", "tui", "architect"])
        );
        assert_eq!(
            proposed_for_paths(&paths(&["apps/web/deployment.yaml"]), &none, &[]),
            profiles(&["backend", "platform"])
        );
        assert_eq!(
            proposed_for_paths(&paths(&["docs/plan-review.md", "CHANGELOG.md"]), &cli, &[]),
            Proposal::NoReview(NoReviewReason::MarkdownOnly)
        );
        assert_eq!(
            proposed_for_paths(&paths(&["scripts/check.sh"]), &cli, &[]),
            Proposal::NoReview(NoReviewReason::NoReviewedFileType)
        );
        assert_eq!(
            proposed_for_paths(&[], &cli, &[]),
            Proposal::NoReview(NoReviewReason::NoFileNamed)
        );
        assert_eq!(
            NoReviewReason::NoReviewedFileType.to_string(),
            "no reviewed file type"
        );
    }

    #[test]
    fn areas_with_no_language_propose_none() {
        assert!(proposed(&set(&["ui"])).is_empty());
        assert!(proposed(&set(&[])).is_empty());
    }

    #[test]
    fn a_list_parses_with_its_left_out_reason() {
        let text = plan(
            "- amont-agent: backend, lang:rust, unix (tui: no screens)\n- decisions: none — prose only",
        );
        let map = parse(&text).unwrap().unwrap();
        assert_eq!(
            map["amont-agent"],
            Declared::Profiles {
                profiles: vec!["backend".into(), "lang:rust".into(), "unix".into()],
                left_out: Some("tui: no screens".into()),
            }
        );
        assert_eq!(
            map["decisions"],
            Declared::NoReview {
                reason: "prose only".into()
            }
        );
        let plain = parse(&plan("- a: backend")).unwrap().unwrap();
        assert_eq!(plain["a"].describe(), "backend");
        let hyphen = parse(&plan("- a: none - generated")).unwrap().unwrap();
        assert_eq!(hyphen["a"].describe(), "none — generated");
    }

    #[test]
    fn no_section_is_none_and_a_fenced_one_is_ignored() {
        assert_eq!(parse("# P\n\n## Phases\n").unwrap(), None);
        let fenced = "# P\n\n```\n## Code review\n- a: backend\n```\n";
        assert_eq!(parse(fenced).unwrap(), None);
    }

    #[test]
    fn a_bad_line_is_an_error_naming_it() {
        let err = parse(&plan("- a: nonexistent")).unwrap_err();
        assert!(err.contains("unknown profile"), "{err}");
        let unknown = parse(&plan("- a: backend, wizard")).unwrap_err();
        assert!(
            unknown.contains("wizard") && unknown.contains("- a:"),
            "{unknown}"
        );
        let twice = parse(&plan("- a: backend\n- a: unix")).unwrap_err();
        assert!(twice.contains("listed twice"), "{twice}");
        let no_reason = parse(&plan("- a: none")).unwrap_err();
        assert!(no_reason.contains("reason"), "{no_reason}");
        let no_colon = parse(&plan("- a backend")).unwrap_err();
        assert!(no_colon.contains("a backend"), "{no_colon}");
        assert!(parse(&plan("- a: ")).is_err());
    }

    #[test]
    fn each_profile_maps_to_its_agent() {
        let agent = agent_for;
        assert_eq!(agent("backend"), Some(("code-review-backend".into(), None)));
        assert_eq!(
            agent("lang:rust"),
            Some(("code-review-language".into(), Some("rust".into())))
        );
        assert_eq!(agent("ui-design"), Some(("interface-review".into(), None)));
        for role in ["platform", "unix", "tui", "react", "architect"] {
            assert_eq!(agent(role).unwrap().0, format!("code-review-{role}"));
        }
        assert_eq!(agent("wizard"), None);
        assert_eq!(slug("lang:rust").as_deref(), Some("lang-rust"));
        assert_eq!(slug("../x"), None);
    }

    #[test]
    fn profile_lines_are_read_in_order() {
        assert_eq!(
            profile_lines("<<<TREE>>>\nProfile: lang:ts\n  Profile: backend \nnot Profile: x"),
            ["lang:ts", "backend"]
        );
    }

    #[test]
    fn a_header_profile_line_counts() {
        assert_eq!(
            profile_lines("<<<TREE>>>\nRound 1.\n<<<RULES sha=x>>>\nProfile: unix\nRead it."),
            ["unix"]
        );
    }

    #[test]
    fn a_profile_line_after_a_heading_is_the_body() {
        let prompt = "<<<TREE>>>\nProfile: unix\n# Implementation review brief\n\nProfile: unix\n";
        assert_eq!(profile_lines(prompt), ["unix"]);
        assert!(profile_lines("# Implementation review brief\nProfile: unix\n").is_empty());
    }

    #[test]
    fn a_profile_line_inside_a_fence_is_the_body() {
        for fence in ["```", "~~~", "````diff"] {
            let prompt =
                format!("<<<TREE>>>\nDelta.\n{fence}\n Profile: x\n+Profile: y\n{fence}\n");
            assert!(profile_lines(&prompt).is_empty(), "{fence}");
            let with_header = format!("<<<TREE>>>\nProfile: backend\n{fence}\nProfile: x\n");
            assert_eq!(profile_lines(&with_header), ["backend"], "{fence}");
        }
    }

    #[test]
    fn a_profile_line_in_an_unfenced_diff_is_the_body() {
        for opener in ["diff --git a/x b/x", "@@ -1,2 +1,2 @@"] {
            let prompt = format!("<<<TREE>>>\nDelta.\n{opener}\n Profile: x\n");
            assert!(profile_lines(&prompt).is_empty(), "{opener}");
        }
    }

    #[test]
    fn the_header_ends_where_the_body_opens() {
        let prompt = "<<<TREE a>>>\nProfile: unix\n```diff\n<<<TREE b>>>\n```\n";
        assert_eq!(header(prompt), "<<<TREE a>>>\nProfile: unix\n");
        assert_eq!(header("no body\nat all"), "no body\nat all");
    }

    #[test]
    fn a_pointer_with_its_own_section_declares_through_it() {
        let own = "---\ncanonical: decisions:docs/plans/p.md\n---\n# X\n\n## Code review\n\n- app: backend\n";
        assert!(has_own_section(own));
        assert_eq!(declared_text(own).as_deref(), Ok(own));
        let declared = parse(own).unwrap().unwrap();
        assert!(require_own_line(own, &declared, "app").is_ok());
        assert!(require_own_line(own, &declared, "other").is_err());
        // review-brief's line reports the same error, not "not declared".
        assert!(declared_line("other", Some(own)).is_err_and(|why| why.contains("`other`")));
        assert!(declared_line("app", Some(own)).is_ok());
        // An empty own section declares nothing for anyone: an error too.
        let empty = "---\ncanonical: decisions:docs/plans/p.md\n---\n# X\n\n## Code review\n\n";
        assert!(has_own_section(empty));
        assert!(declared_line("app", Some(empty)).is_err());
    }

    #[test]
    fn a_pointer_without_its_own_section_reads_the_canonical() {
        let bare = "---\ncanonical: decisions:docs/plans/p.md\n---\n# X\n";
        assert!(!has_own_section(bare));
        let plain = "# X\n\n## Code review\n\n- app: backend\n";
        assert!(!has_own_section(plain), "not a pointer");
        assert_eq!(declared_text(plain).as_deref(), Ok(plain));
    }

    #[test]
    fn a_hash_without_a_space_is_not_a_heading() {
        let prompt = "<<<TREE>>>\n#174 follow-up\n####### seven\nProfile: backend\n";
        assert_eq!(profile_lines(prompt), ["backend"]);
    }

    #[test]
    fn checked_by_classes_ignore_what_is_in_parentheses() {
        assert_eq!(checked_classes("**Checked by:** judge.\n"), ["judge"]);
        assert_eq!(
            checked_classes("**Checked by:** exact (a, b), judge (c, d).\n\nmore"),
            ["exact", "judge"]
        );
    }
}
