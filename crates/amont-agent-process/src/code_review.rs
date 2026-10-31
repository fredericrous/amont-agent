//! The code-review profiles a plan declares (decisions ADR-0052): the
//! `## Code review` section, the proposal `plan-panel --code-review` prints
//! from a repository's areas, and the judge rules each profile reviews.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::plan_review::{locate, meta, repo_areas, root};
use crate::plan_text::Fence;

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
            "large" => &["architect"],
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
    let roles = code_roles(areas);
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

/// The value of each `Profile:` line of a review prompt, in order.
pub fn profile_lines(prompt: &str) -> Vec<&str> {
    prompt
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Profile:"))
        .map(str::trim)
        .collect()
}

/// The proposal for every repository of the plan's machine comment, one
/// `<repo>\t<profiles|none>` line each.
pub fn proposal(plan_text: &str) -> Result<String, String> {
    let machine = meta(plan_text)?;
    let plan_root =
        root().ok_or("no plan root could be determined, so repos= cannot be resolved")?;
    let mut out = String::new();
    for name in &machine.repos {
        let dir = locate(&plan_root, name);
        let areas = repo_areas(&dir)?.ok_or_else(|| {
            format!(
                "repos= names `{name}`, which is not a repository at {}",
                dir.display()
            )
        })?;
        let profiles = proposed(&areas);
        let shown = if profiles.is_empty() {
            "none".to_string()
        } else {
            profiles.join(",")
        };
        out.push_str(&format!("{name}\t{shown}\n"));
    }
    Ok(out)
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

fn parse_line(item: &str) -> Result<(String, Declared), String> {
    let (repo, value) = item
        .split_once(':')
        .ok_or("expected `- <repo>: <profiles>` or `- <repo>: none — <reason>`")?;
    let repo = repo.trim();
    if !crate::plan_review::valid_name(repo) {
        return Err(format!("`{repo}` is not a repository name"));
    }
    let value = value.trim();
    let none_rest = value.strip_prefix("none").filter(|rest| {
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
    let (list, left_out) = match (value.strip_suffix(')'), value.find('(')) {
        (Some(inner), Some(open)) => (
            &value[..open],
            Some(inner[open + 1..].trim().to_string()).filter(|r| !r.is_empty()),
        ),
        _ => (value, None),
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

/// A plan's declaration text for the brief: the plan itself, or the
/// canonical file a pointer plan names, read under the plan root. `Err`
/// names a canonical file that cannot be read.
pub(crate) fn declared_text(plan_text: &str) -> Result<String, String> {
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
/// declaration that cannot be read or parsed is said, not shown as absent.
pub fn declared_line(repo: &str, plan_text: Option<&str>) -> String {
    let shown = match plan_text.map(|text| declared_text(text).and_then(|t| parse(&t))) {
        None | Some(Ok(None)) => "not declared".to_string(),
        Some(Ok(Some(map))) => map
            .get(repo)
            .map(Declared::describe)
            .unwrap_or_else(|| "not declared".to_string()),
        Some(Err(why)) => format!("unreadable ({why})"),
    };
    format!("Declared code review for {repo}: {shown}")
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
    fn checked_by_classes_ignore_what_is_in_parentheses() {
        assert_eq!(checked_classes("**Checked by:** judge.\n"), ["judge"]);
        assert_eq!(
            checked_classes("**Checked by:** exact (a, b), judge (c, d).\n\nmore"),
            ["exact", "judge"]
        );
    }
}
