//! The mechanical pin check of a copied block (decisions ADR-0067): the block
//! carries one marker line among its added lines,
//!
//! ```text
//! # release-pin: <owner/repo> <tag-template> <asset-template> <ENV_KEY>
//! ```
//!
//! The check reads the pin `<ENV_KEY>` from the same file at HEAD, expands
//! `{version}` in the two templates with it, and asks `gh release view` whether
//! that release lists the asset. `plan-panel` and `review-brief` call the same
//! function; only `review-brief` writes the record the hook reads.

use amont_agent_guard::names::BIN;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::review_brief::{run_bounded_for, AvalFailed};

const MARKER: &str = "# release-pin:";
const VERSION_SLOT: &str = "{version}";
const GH_MISSING: &str = "gh not found; install GitHub CLI and run gh auth login";
/// The child must never wait for a prompt or print an update notice.
const GH_ENV: [(&str, &str); 2] = [("GH_PROMPT_DISABLED", "1"), ("GH_NO_UPDATE_NOTIFIER", "1")];
const GH_DEADLINE: Duration = Duration::from_secs(20);
const DIFF_BUDGET: Duration = Duration::from_secs(10);

/// What the check found: whether the asset is listed, and why when it is not
/// (or which release was confirmed when it is).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The release lists the asset; `release` says which.
    Passed { release: String },
    /// The check did not confirm it, for this reason.
    Failed { why: String },
}

impl Outcome {
    fn failed(why: impl Into<String>) -> Self {
        Outcome::Failed { why: why.into() }
    }

    /// Whether the asset was confirmed.
    pub fn is_passed(&self) -> bool {
        matches!(self, Outcome::Passed { .. })
    }

    /// The release confirmed, or the reason it was not.
    pub fn detail(&self) -> &str {
        match self {
            Outcome::Passed { release } => release,
            Outcome::Failed { why } => why,
        }
    }
}

/// The five fields of a marker line and the file that holds it.
#[derive(Debug, PartialEq, Eq)]
struct Marker {
    path: String,
    repo: String,
    tag_template: String,
    asset_template: String,
    key: String,
}

/// Whether `AMONT_AGENT_GH` names the program to run.
pub fn gh_override_set() -> bool {
    std::env::var_os("AMONT_AGENT_GH").is_some_and(|program| !program.is_empty())
}

/// The `gh` to run, or the failure when none can be found.
fn gh_program() -> Result<std::ffi::OsString, String> {
    if let Some(program) = std::env::var_os("AMONT_AGENT_GH").filter(|p| !p.is_empty()) {
        return Ok(program);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let found = std::env::split_paths(&path).any(|dir| dir.join("gh").is_file());
    if found {
        Ok(std::ffi::OsString::from("gh"))
    } else {
        Err(GH_MISSING.to_string())
    }
}

/// The deadline of the `gh` call; `AMONT_AGENT_GH_DEADLINE_MS` shortens it
/// for tests, like the other tool overrides.
fn gh_deadline() -> Duration {
    std::env::var("AMONT_AGENT_GH_DEADLINE_MS")
        .ok()
        .and_then(|millis| millis.parse::<u64>().ok())
        .map_or(GH_DEADLINE, Duration::from_millis)
}

/// The added lines of a `git diff -U0` text, each with the path of its file.
fn added_lines(diff: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(diff);
    let mut out = Vec::new();
    let (mut path, mut header, mut binary) = (String::new(), false, false);
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            (path, header, binary) = (String::new(), true, false);
        } else if header {
            if let Some(name) = line.strip_prefix("+++ b/") {
                path = name.to_string();
            } else if line.starts_with("@@") {
                header = false;
            } else if line.starts_with("GIT binary patch") {
                (header, binary) = (false, true);
            }
        } else if !binary {
            if let Some(added) = line.strip_prefix('+') {
                out.push((path.clone(), added.to_string()));
            }
        }
    }
    out
}

fn marker_in(diff: &[u8]) -> Result<Marker, String> {
    let mut found: Vec<Marker> = Vec::new();
    for (path, line) in added_lines(diff) {
        let Some(rest) = line.trim().strip_prefix(MARKER) else {
            continue;
        };
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let [repo, tag_template, asset_template, key] = fields.as_slice() else {
            return Err(format!(
                "the `{MARKER}` line in {path} needs <owner/repo> <tag-template> <asset-template> <ENV_KEY>"
            ));
        };
        found.push(Marker {
            path,
            repo: (*repo).to_string(),
            tag_template: (*tag_template).to_string(),
            asset_template: (*asset_template).to_string(),
            key: (*key).to_string(),
        });
    }
    match found.len() {
        0 => Err(format!("no `{MARKER}` line among the added lines")),
        1 => Ok(found.remove(0)),
        count => Err(format!(
            "{count} `{MARKER}` lines among the added lines; the block carries one"
        )),
    }
}

/// The value of `key` in a file's text: `KEY: value`, `KEY=value`, with an
/// optional list dash or `export`, quotes and trailing comment dropped.
fn value_of(file: &str, key: &str) -> Option<String> {
    file.lines().find_map(|line| {
        let line = line.trim_start();
        let line = line.strip_prefix("- ").unwrap_or(line);
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let rest = line.strip_prefix(key)?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix([':', '='])?;
        let rest = rest.split(" #").next().unwrap_or(rest).trim();
        let value = rest.trim_matches(['"', '\'']);
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Run the check for the branch `merge_base..HEAD` of `repo`. Prints one line
/// on stderr before it calls `gh`.
pub fn check(repo: &Path, merge_base: &str) -> Outcome {
    let diff = match crate::block_fingerprint::diff_of(
        repo,
        merge_base,
        "HEAD",
        Instant::now() + DIFF_BUDGET,
    ) {
        Ok(diff) => diff,
        Err(why) => return Outcome::failed(format!("git failed: {why}")),
    };
    let marker = match marker_in(&diff) {
        Ok(marker) => marker,
        Err(why) => return Outcome::failed(why),
    };
    let Some(file) = amont_agent_guard::git::stdout_bounded(
        repo,
        &["show", &format!("HEAD:{}", marker.path)],
        Instant::now() + DIFF_BUDGET,
    ) else {
        return Outcome::failed(format!("{} cannot be read at HEAD", marker.path));
    };
    let Some(version) = value_of(&file, &marker.key) else {
        return Outcome::failed(format!("{} has no value for {}", marker.path, marker.key));
    };
    let tag = marker.tag_template.replace(VERSION_SLOT, &version);
    let asset = marker.asset_template.replace(VERSION_SLOT, &version);
    let program = match gh_program() {
        Ok(program) => program,
        Err(why) => return Outcome::failed(why),
    };
    eprintln!(
        "{BIN}: pin check: gh release view {tag} --repo {} (asset {asset})",
        marker.repo
    );
    let args = [
        "release",
        "view",
        tag.as_str(),
        "--repo",
        marker.repo.as_str(),
        "--json",
        "assets",
    ];
    let answer = match run_bounded_for(&program, "gh", repo, &args, &GH_ENV, gh_deadline()) {
        Ok(answer) => answer,
        Err(AvalFailed::NotInstalled) => return Outcome::failed(GH_MISSING),
        Err(AvalFailed::Failed(why)) => return Outcome::failed(why),
    };
    if !answer.status.success() {
        let said = answer.stderr.lines().next().unwrap_or_default();
        let exit = answer
            .status
            .code()
            .map_or_else(|| "killed by a signal".to_string(), |code| code.to_string());
        return Outcome::failed(format!(
            "gh release view {tag} failed (exit {exit}): {said}"
        ));
    }
    let listed = serde_json::from_slice::<serde_json::Value>(&answer.stdout)
        .ok()
        .and_then(|value| {
            value.get("assets").and_then(|assets| {
                assets.as_array().map(|assets| {
                    assets
                        .iter()
                        .any(|a| a.get("name").and_then(|n| n.as_str()) == Some(asset.as_str()))
                })
            })
        });
    match listed {
        Some(true) => Outcome::Passed {
            release: format!("{} release {tag} lists {asset}", marker.repo),
        },
        Some(false) => Outcome::failed(format!(
            "{} release {tag} does not list {asset}",
            marker.repo
        )),
        None => Outcome::failed(format!("`gh release view {tag}` printed no asset list")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/.github/ci.yml b/.github/ci.yml\n--- a/.github/ci.yml\n+++ b/.github/ci.yml\n@@ -1,0 +2,2 @@\n+  # release-pin: o/r v{version} tool-{version}.tgz TOOL_VERSION\n+  run: x\n";

    #[test]
    fn marker_is_read_from_added_lines() {
        let marker = marker_in(DIFF.as_bytes()).expect("marker");
        assert_eq!(marker.path, ".github/ci.yml");
        assert_eq!(marker.repo, "o/r");
        assert_eq!(marker.key, "TOOL_VERSION");
    }

    #[test]
    fn a_missing_or_doubled_marker_is_an_error() {
        assert!(marker_in(b"diff --git a/x b/x\n@@ -0,0 +1 @@\n+x\n").is_err());
        let twice = format!("{DIFF}+# release-pin: a/b t a K\n");
        assert!(marker_in(twice.as_bytes()).is_err());
    }

    #[test]
    fn values_are_read_from_yaml_and_env_lines() {
        assert_eq!(
            value_of("  TOOL: \"1.2\" # c\n", "TOOL"),
            Some("1.2".into())
        );
        assert_eq!(value_of("export TOOL=3\n", "TOOL"), Some("3".into()));
        assert_eq!(value_of("TOOLS: 3\n", "TOOL"), None);
    }
}
