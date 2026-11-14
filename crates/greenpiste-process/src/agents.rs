//! Whether the agents a review rule needs are installed anywhere Claude Code
//! loads agents from.
//!
//! `plan-review-panel` and `implementation-review` hold a tool call until a
//! reviewer agent has run. On a machine where no such agent exists the hold
//! can never be satisfied, so the guards say how to get the agents
//! (`grh install workflow`) instead of refusing. Any one source carrying them
//! keeps the hold.
//!
//! Sources: `$CLAUDE_CONFIG_DIR` or `~/.claude` `agents/`, the repository's
//! `.claude/agents/` (the repository root holding the working directory), and the
//! `agents/` of every installed plugin.

use std::path::{Path, PathBuf};

/// The agents one rule needs.
#[derive(Clone, Copy)]
pub enum Need {
    /// Any `plan-review-*` agent.
    PlanPanel,
    /// The `implementation-review` agent.
    ImplementationReview,
    /// One code-review profile's agent, by file stem (see [`CODE_REVIEW_AGENTS`]).
    CodeReview(&'static str),
}

/// The agent file stem of each code-review profile a plan may declare
/// (`lang:<x>` all map to `code-review-language`, `ui-design` to
/// `interface-review`); `code_review::agent_for` is the mapping this lists.
const CODE_REVIEW_AGENTS: [&str; 8] = [
    "code-review-backend",
    "code-review-language",
    "code-review-platform",
    "code-review-unix",
    "code-review-tui",
    "code-review-react",
    "code-review-architect",
    "interface-review",
];

/// The extension of an agent file; the stem is the agent's name.
const AGENT_EXTENSION: &str = ".md";

impl Need {
    fn matches(self, file_name: &str) -> bool {
        let Some(stem) = file_name.strip_suffix(AGENT_EXTENSION) else {
            return false;
        };
        match self {
            Need::PlanPanel => stem.starts_with("plan-review-"),
            Need::ImplementationReview => stem == "implementation-review",
            Need::CodeReview(agent) => stem == agent,
        }
    }
}

/// The one line a guard says when the agents are not installed.
pub fn gap_line(need: Need) -> String {
    let what = match need {
        Need::PlanPanel => "no `plan-review-*` agent".to_string(),
        Need::ImplementationReview => "no `implementation-review` agent".to_string(),
        Need::CodeReview(agent) => format!("no `{agent}` agent"),
    };
    format!(
        "{what} is installed (looked in the Claude config agents/, .claude/agents/ and installed \
         plugins), so this check cannot be satisfied here: run `grh install workflow`."
    )
}

/// One doctor gap per review agent no source carries (the implementation
/// review, then each code-review profile's), for the repository holding
/// `project`; none when all are installed.
pub fn review_gaps(project: &Path) -> Vec<greenpiste_guard::doctor::Finding> {
    let needs = std::iter::once(Need::ImplementationReview).chain(
        CODE_REVIEW_AGENTS
            .iter()
            .map(|agent| Need::CodeReview(agent)),
    );
    needs
        .filter(|need| !carried(*need, project))
        .map(|need| {
            let line = gap_line(need);
            greenpiste_guard::doctor::Finding::warn(
                line.split(" is installed")
                    .next()
                    .unwrap_or(&line)
                    .to_string(),
                line,
            )
        })
        .collect()
}

/// Whether any source carries an agent `need` names. Evidence that cannot be
/// read counts as carried: missing evidence is not permission.
pub fn carried(need: Need, project: &Path) -> bool {
    carried_in(
        need,
        greenpiste_guard::settings::config_dir().as_deref(),
        project,
    )
}

fn carried_in(need: Need, config_dir: Option<&Path>, project: &Path) -> bool {
    match agent_dirs_in(config_dir, project) {
        Some(dirs) => dirs.iter().any(|dir| dir_has(dir, need)),
        None => true,
    }
}

/// Every `agents/` directory Claude Code loads agents from, for the
/// repository holding `project`: the config directory, installed plugins,
/// the repository. `None` when where the agents would be cannot be known.
pub fn agent_dirs(project: &Path) -> Option<Vec<PathBuf>> {
    agent_dirs_in(greenpiste_guard::settings::config_dir().as_deref(), project)
}

/// The files an agent named `agent` could be loaded from, in load order:
/// one candidate per directory of [`agent_dirs`]. `None` when the
/// directories cannot be known. A file may not exist.
pub fn agent_files(project: &Path, agent: &str) -> Option<Vec<PathBuf>> {
    let file_name = format!("{agent}{AGENT_EXTENSION}");
    Some(
        agent_dirs(project)?
            .into_iter()
            .map(|dir| dir.join(&file_name))
            .collect(),
    )
}

fn agent_dirs_in(config_dir: Option<&Path>, project: &Path) -> Option<Vec<PathBuf>> {
    // Where the agents would be is unknown without a config directory.
    let config = config_dir?;
    let mut dirs: Vec<PathBuf> = vec![config.join("agents")];
    dirs.extend(
        plugin_roots(config)?
            .into_iter()
            .map(|root| root.join("agents")),
    );
    dirs.push(repository_root(project).join(".claude").join("agents"));
    Some(dirs)
}

/// The repository root holding `project`, else `project` itself: the walk
/// stops there so a home directory above the repository is not a source.
fn repository_root(project: &Path) -> PathBuf {
    greenpiste_guard::lsp::git_root(project).unwrap_or_else(|| project.to_path_buf())
}

/// Whether `dir` holds an agent `need` names. A directory that is absent
/// holds none; one that cannot be read may, so it counts as carried.
fn dir_has(dir: &Path, need: Need) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => return e.kind() != std::io::ErrorKind::NotFound,
    };
    // An entry that cannot be read may be the agent: it counts as carried.
    entries.into_iter().any(|e| match e {
        Ok(e) => e.file_name().to_str().is_some_and(|n| need.matches(n)),
        Err(_) => true,
    })
}

/// The directories inside `dir`, or `None` when `dir` exists and cannot be
/// listed: a cache level that cannot be read may hold the agents.
fn subdirs(dir: &Path) -> Option<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    let mut out = Vec::new();
    for e in entries {
        let path = e.ok()?.path();
        if path.is_dir() {
            out.push(path);
        }
    }
    Some(out)
}

/// The install directory of every plugin: those `installed_plugins.json`
/// names, and the version directories of the plugin cache. `None` when the
/// manifest exists and cannot be understood.
fn plugin_roots(config: &Path) -> Option<Vec<PathBuf>> {
    use greenpiste_guard::lsp::{installed_plugins, InstalledPlugins};
    let mut roots = Vec::new();
    match installed_plugins(config) {
        InstalledPlugins::Document(doc) => {
            let listed = doc.get("plugins")?.as_object()?;
            for installs in listed.values() {
                for install in installs.as_array()? {
                    if let Some(path) = install.get("installPath").and_then(|p| p.as_str()) {
                        roots.push(PathBuf::from(path));
                    }
                }
            }
        }
        InstalledPlugins::Missing => {}
        InstalledPlugins::Unreadable => return None,
    }
    // cache/<marketplace>/<plugin>/<version>
    let mut layer = vec![config.join("plugins").join("cache")];
    for _ in 0..3 {
        let mut next = Vec::new();
        for dir in &layer {
            next.extend(subdirs(dir)?);
        }
        layer = next;
    }
    roots.extend(layer);
    Some(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("amont-agent-agents-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); // a scratch dir that is absent is the normal case
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(path, "---\nname: x\n---\n").expect("file");
    }

    #[test]
    fn nothing_installed_carries_nothing() {
        let dir = scratch("none");
        assert!(!carried_in(Need::PlanPanel, Some(&dir), &dir));
        assert!(!carried_in(Need::ImplementationReview, Some(&dir), &dir));
    }

    #[test]
    fn each_source_counts() {
        let dir = scratch("sources");
        let config = dir.join("config");
        let project = dir.join("repo");
        std::fs::create_dir_all(&project).unwrap();
        touch(&config.join("agents/plan-review-security.md"));
        assert!(carried_in(Need::PlanPanel, Some(&config), &project));
        assert!(!carried_in(
            Need::ImplementationReview,
            Some(&config),
            &project
        ));

        touch(&project.join(".claude/agents/implementation-review.md"));
        assert!(carried_in(
            Need::ImplementationReview,
            Some(&config),
            &project
        ));
    }

    #[test]
    fn a_plugins_agents_count() {
        let dir = scratch("plugin");
        let config = dir.join("config");
        let plugin = dir.join("elsewhere/grh");
        touch(&plugin.join("agents/implementation-review.md"));
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(config.join("plugins")).unwrap();
        std::fs::write(
            config.join("plugins/installed_plugins.json"),
            serde_json::json!({"version": 2, "plugins": {"grh@m": [{"installPath": plugin}]}})
                .to_string(),
        )
        .unwrap();
        assert!(carried_in(Need::ImplementationReview, Some(&config), &dir));
        assert!(!carried_in(Need::PlanPanel, Some(&config), &dir));
    }

    #[test]
    fn an_unreadable_plugin_manifest_keeps_the_hold() {
        let dir = scratch("broken");
        std::fs::create_dir_all(dir.join("plugins")).unwrap();
        std::fs::write(dir.join("plugins/installed_plugins.json"), "{nope").unwrap();
        assert!(carried_in(Need::PlanPanel, Some(&dir), &dir));
    }

    #[test]
    fn an_unknown_config_dir_keeps_the_hold() {
        let dir = scratch("noconfig");
        assert!(carried_in(Need::PlanPanel, None, &dir));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_agents_dir_keeps_the_hold() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("unreadable");
        let agents = dir.join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::set_permissions(&agents, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_dir(&agents).is_ok(); // root reads anything
        let held = carried_in(Need::PlanPanel, Some(&dir), &dir);
        std::fs::set_permissions(&agents, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(readable || held);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_plugin_cache_keeps_the_hold() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("unreadable-cache");
        let cache = dir.join("plugins").join("cache");
        std::fs::create_dir_all(cache.join("market")).unwrap();
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_dir(&cache).is_ok(); // root reads anything
        let held = carried_in(Need::PlanPanel, Some(&dir), &dir);
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(readable || held);
    }

    #[test]
    fn every_profile_agent_is_listed() {
        for profile in [
            "backend",
            "platform",
            "unix",
            "tui",
            "react",
            "architect",
            "ui-design",
            "lang:rust",
        ] {
            let (agent, _) = crate::code_review::agent_for(profile).unwrap();
            assert!(CODE_REVIEW_AGENTS.contains(&agent.as_str()), "{agent}");
        }
    }

    #[test]
    fn a_code_review_agent_is_found_by_its_file() {
        let dir = scratch("code-review");
        let config = dir.join("config");
        let project = dir.join("repo");
        std::fs::create_dir_all(&project).unwrap();
        let backend = Need::CodeReview("code-review-backend");
        assert!(!carried_in(backend, Some(&config), &project));
        touch(&config.join("agents/code-review-backend.md"));
        assert!(carried_in(backend, Some(&config), &project));
        assert!(!carried_in(
            Need::CodeReview("code-review-unix"),
            Some(&config),
            &project
        ));
    }

    #[test]
    fn only_the_named_agents_match() {
        assert!(Need::PlanPanel.matches("plan-review-ux.md"));
        assert!(!Need::PlanPanel.matches("plan-review-ux.txt"));
        assert!(!Need::PlanPanel.matches("implementation-review.md"));
        assert!(Need::ImplementationReview.matches("implementation-review.md"));
        assert!(!Need::ImplementationReview.matches("implementation-review-x.md"));
    }
}
