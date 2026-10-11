//! Does fleet-lsp answer for this file, and does this agent have the LSP tool?
//!
//! Read by `code-read-not-lsp` and `symbol-grep-not-lsp`, and only after
//! their pure tests have fired: everything here touches the world. Each hook
//! call is a new process, so nothing is cached. Every failure — a file that
//! will not open, JSON that will not parse, a definition that is not there —
//! is silence: advice pointing at a tool that cannot answer costs the model
//! a turn and teaches it to ignore the next one.
//!
//! The checks run cheapest first: `PATH`, then the walk up to `.git`, then
//! the settings files, then the agent's definition.

use std::path::{Path, PathBuf};

/// What fleet-lsp's `plugin.json` maps to a server (fleet-lsp 0.5.2).
pub const EXTENSIONS: &[&str] = &[
    "rs", "go", "py", "pyi", "ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs",
];

/// Language names as `rg -t`, `--type` and the Grep tool's `type` spell them.
const LANGUAGES: &[&str] = &[
    "rust",
    "go",
    "py",
    "python",
    "ts",
    "typescript",
    "js",
    "javascript",
];

/// Built-in agents whose tools include LSP: every tool, or every tool but a
/// few that are not LSP. `claude` is this machine's catch-all agent.
const BUILTIN_WITH_LSP: &[&str] = &["Explore", "Plan", "general-purpose", "fork", "claude"];

/// Built-in agents with a short tool list that has no LSP.
const BUILTIN_WITHOUT_LSP: &[&str] = &["statusline-setup", "claude-code-guide"];

/// A path whose extension fleet-lsp serves. Pure.
pub fn is_code(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && EXTENSIONS.contains(&ext))
}

/// A glob (`*.rs`, `**/*.{ts,tsx}`) or a language name (`rust`) that selects
/// code fleet-lsp serves. Pure.
pub fn names_code(selector: &str) -> bool {
    if LANGUAGES.contains(&selector) {
        return true;
    }
    let Some((_, tail)) = selector.rsplit_once('.') else {
        return false;
    };
    let tail = tail.trim_start_matches('{').trim_end_matches('}');
    tail.split(',').any(|ext| EXTENSIONS.contains(&ext))
}

/// Who is making the call, as the hook payload says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Agent {
    /// No `agent_id`: the main conversation.
    Main,
    /// A subagent, with its `agent_type`: `Explore`, a custom agent's name,
    /// or a plugin agent's `plugin:name`.
    Named(String),
    /// A subagent whose payload named no type.
    Unnamed,
}

impl Agent {
    pub fn from_payload(agent_id: Option<&str>, agent_type: Option<&str>) -> Agent {
        fn present(s: Option<&str>) -> Option<&str> {
            s.filter(|s| !s.trim().is_empty())
        }
        match (present(agent_id), present(agent_type)) {
            (_, Some(t)) => Agent::Named(t.to_string()),
            (Some(_), None) => Agent::Unnamed,
            (None, None) => Agent::Main,
        }
    }
}

/// `Ok` when fleet-lsp would answer for `target` (a file or a directory):
/// the binary is on `PATH`, `target` is inside a git working tree, and the
/// plugin is enabled for that tree. `Err` names the first gate that failed.
pub fn serves(target: &Path) -> Result<(), &'static str> {
    if on_path("fleet-lsp").is_none() {
        return Err("fleet-lsp is not on PATH");
    }
    let Some(repo) = git_root(target) else {
        return Err("outside a git working tree");
    };
    if !plugin_enabled(&repo) {
        return Err("the fleet-lsp plugin is not enabled");
    }
    Ok(())
}

/// `Ok` when this agent can call the LSP tool. Main and the general built-in
/// agents can; any other agent is judged by its definition's `tools:` line,
/// and an agent whose definition cannot be found is assumed not to.
pub fn agent_has_tool(agent: &Agent, cwd: &Path) -> Result<(), &'static str> {
    let name = match agent {
        Agent::Main => return Ok(()),
        Agent::Unnamed => return Err("a subagent of unknown type"),
        Agent::Named(name) => name.as_str(),
    };
    if BUILTIN_WITH_LSP.contains(&name) {
        return Ok(());
    }
    if BUILTIN_WITHOUT_LSP.contains(&name) {
        return Err("this agent has no LSP tool");
    }
    let Some(text) = definition(name, cwd) else {
        return Err("the agent's definition was not found");
    };
    match tools_of(&text) {
        None => Ok(()),
        Some(tools) if tools.iter().any(|t| t == "LSP") => Ok(()),
        Some(_) => Err("this agent has no LSP tool"),
    }
}

/// The agent's definition file, read whole: project, then user, then the
/// installed plugin it is scoped to.
fn definition(name: &str, cwd: &Path) -> Option<String> {
    let read = |p: PathBuf| std::fs::read_to_string(p).ok();
    if let Some((plugin, agent)) = name.split_once(':') {
        let root = plugin_install_path(plugin)?;
        return read(root.join("agents").join(format!("{agent}.md")));
    }
    if name.contains('/') || name.contains("..") {
        return None;
    }
    let file = format!("{name}.md");
    let project = git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    read(project.join(".claude/agents").join(&file))
        .or_else(|| read(crate::settings::config_dir()?.join("agents").join(&file)))
}

/// The tools a definition's front matter grants: `None` when it sets no
/// `tools:` (every tool). Both spellings are read — `tools: Read, LSP` and a
/// YAML list under `tools:`.
pub fn tools_of(definition: &str) -> Option<Vec<String>> {
    let mut lines = definition.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let front: Vec<&str> = lines.take_while(|l| l.trim() != "---").collect();
    let at = front.iter().position(|l| l.starts_with("tools:"))?;
    let clean = |s: &str| s.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
    let inline = front[at]["tools:".len()..].trim();
    if !inline.is_empty() {
        let inline = inline.trim_start_matches('[').trim_end_matches(']');
        return Some(
            inline
                .split(',')
                .map(clean)
                .filter(|t| !t.is_empty())
                .collect(),
        );
    }
    Some(
        front[at + 1..]
            .iter()
            .map_while(|l| l.trim_start().strip_prefix("- "))
            .map(clean)
            .collect(),
    )
}

/// `enabledPlugins` merged user, then project, then local settings: the most
/// specific file that names a `fleet-lsp@…` plugin decides.
fn plugin_enabled(repo: &Path) -> bool {
    let files = [
        crate::settings::config_dir().map(|d| d.join("settings.json")),
        Some(repo.join(".claude/settings.json")),
        Some(repo.join(".claude/settings.local.json")),
    ];
    let said: Vec<Option<bool>> = files
        .into_iter()
        .map(|f| f.and_then(|f| std::fs::read_to_string(f).ok()))
        .map(|text| text.as_deref().and_then(fleet_lsp_enabled_in))
        .collect();
    most_specific(&said)
}

/// The last file, in user-project-local order, that says anything; off when
/// none does.
fn most_specific(said: &[Option<bool>]) -> bool {
    said.iter().rev().find_map(|s| *s).unwrap_or(false)
}

/// What one settings file says about the fleet-lsp plugin, if anything. A
/// file that does not parse says nothing.
fn fleet_lsp_enabled_in(text: &str) -> Option<bool> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    v.get("enabledPlugins")?
        .as_object()?
        .iter()
        .find(|(k, _)| k.starts_with("fleet-lsp@"))
        .and_then(|(_, on)| on.as_bool())
}

/// Where `~/.claude/plugins/installed_plugins.json` says a plugin is
/// installed: the last entry of the first key named `<plugin>@…`.
fn plugin_install_path(plugin: &str) -> Option<PathBuf> {
    let file = crate::settings::config_dir()?.join("plugins/installed_plugins.json");
    install_path_in(&std::fs::read_to_string(file).ok()?, plugin)
}

fn install_path_in(installed: &str, plugin: &str) -> Option<PathBuf> {
    let v: serde_json::Value = serde_json::from_str(installed).ok()?;
    let prefix = format!("{plugin}@");
    let (_, entries) = v
        .get("plugins")?
        .as_object()?
        .iter()
        .find(|(k, _)| k.starts_with(&prefix))?;
    let path = entries.as_array()?.last()?.get("installPath")?.as_str()?;
    Some(PathBuf::from(path))
}

/// The nearest directory at or above `target` holding `.git` (a directory,
/// or the file a linked worktree has).
pub fn git_root(target: &Path) -> Option<PathBuf> {
    let start = if target.is_dir() {
        target
    } else {
        target.parent()?
    };
    start
        .ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

/// `program` in a `PATH` directory, with the platform's executable suffix
/// (`fleet-lsp.exe` on Windows) or without.
pub fn on_path(program: &str) -> Option<PathBuf> {
    let suffixed = format!("{program}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|d| [d.join(program), d.join(&suffixed)])
        .find(|p| p.is_file())
}

/// Whether this session was already told about `key` (a file, or a grep
/// pattern), recording it when not.
pub enum Told {
    /// First time: recorded now.
    First,
    /// Told before in this session.
    Before,
    /// First time as far as can be known, and the record could not be kept:
    /// the next call may be told again. The reason is for the journal.
    Unrecorded(&'static str),
}

/// One line per key in `<journal>/sessions/<session>.lsp`, appended; the
/// session sweep removes it with the session's other files after a week.
/// Two hooks racing on the same key can both answer `First`. A record that
/// cannot be kept is `Unrecorded`: telling twice is the safe direction.
pub fn told(session: &str, key: &str) -> Told {
    use std::io::Write;
    if !crate::session_state::usable_id(session) || key.contains('\n') {
        return Told::Unrecorded("no usable session id or key");
    }
    let Some(dir) = crate::journal::dir().map(|d| d.join("sessions")) else {
        return Told::Unrecorded("no journal directory");
    };
    let file = dir.join(format!("{session}.lsp"));
    if let Ok(seen) = std::fs::read_to_string(&file) {
        if seen.lines().any(|l| l == key) {
            return Told::Before;
        }
    }
    if std::fs::create_dir_all(&dir).is_err() {
        return Told::Unrecorded("the sessions directory could not be created");
    }
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .and_then(|mut f| writeln!(f, "{key}"));
    match written {
        Ok(()) => Told::First,
        Err(_) => Told::Unrecorded("the session record could not be written"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_known_by_its_extension() {
        for p in ["a.rs", "src/x.go", "/a/b.pyi", "c.tsx", "d.mjs", "e.cts"] {
            assert!(is_code(p), "{p}");
        }
        for p in [
            "a.md",
            "Cargo.toml",
            "x.json",
            ".rs",
            "rs",
            "a.rs/b",
            "notes.RS",
        ] {
            assert!(!is_code(p), "{p}");
        }
    }

    #[test]
    fn selectors_name_code() {
        for s in ["*.rs", "**/*.{ts,tsx}", "rust", "python", "*.py"] {
            assert!(names_code(s), "{s}");
        }
        for s in ["*.md", "**/*macro*", "yaml", "*.{json,yaml}"] {
            assert!(!names_code(s), "{s}");
        }
    }

    #[test]
    fn the_payload_names_the_agent() {
        assert_eq!(Agent::from_payload(None, None), Agent::Main);
        assert_eq!(Agent::from_payload(Some("a1"), None), Agent::Unnamed);
        assert_eq!(
            Agent::from_payload(Some("a1"), Some("Explore")),
            Agent::Named("Explore".into())
        );
        assert_eq!(Agent::from_payload(Some(""), Some(" ")), Agent::Main);
    }

    #[test]
    fn tools_are_read_inline_and_as_a_list() {
        let inline = "---\nname: x\ntools: Read, Grep, LSP\n---\nbody";
        assert_eq!(tools_of(inline).unwrap(), ["Read", "Grep", "LSP"]);
        let bracket = "---\ntools: [Read, \"Bash\"]\n---\n";
        assert_eq!(tools_of(bracket).unwrap(), ["Read", "Bash"]);
        let list = "---\nname: x\ntools:\n  - Read\n  - 'LSP'\nmodel: haiku\n---\n";
        assert_eq!(tools_of(list).unwrap(), ["Read", "LSP"]);
        assert_eq!(tools_of("---\nname: x\n---\ntools: Read"), None);
        assert_eq!(tools_of("no front matter"), None);
    }

    #[test]
    fn the_most_specific_settings_file_decides() {
        assert!(!most_specific(&[None, None, None]), "off when nobody says");
        assert!(most_specific(&[Some(true), None, None]));
        assert!(
            !most_specific(&[Some(true), Some(false), None]),
            "project wins"
        );
        assert!(
            most_specific(&[Some(false), None, Some(true)]),
            "local wins"
        );
        let on = r#"{"enabledPlugins":{"fleet-lsp@fleet-lsp":true,"x@y":false}}"#;
        assert_eq!(fleet_lsp_enabled_in(on), Some(true));
        assert_eq!(
            fleet_lsp_enabled_in(r#"{"enabledPlugins":{"x@y":true}}"#),
            None
        );
        assert_eq!(
            fleet_lsp_enabled_in(r#"{"enabledPlugins":{"fleet-lsp@a":"yes"}}"#),
            None
        );
        assert_eq!(
            fleet_lsp_enabled_in(r#"{"enabledPlugins":{"fleet-"#),
            None,
            "truncated"
        );
        assert_eq!(fleet_lsp_enabled_in(""), None);
    }

    #[test]
    fn a_plugin_is_found_where_it_was_installed() {
        let installed = r#"{"version":2,"plugins":{
            "relais@relais-local":[{"installPath":"/old"},{"installPath":"/p/relais/0.12.0"}],
            "relaisx@other":[{"installPath":"/wrong"}]}}"#;
        assert_eq!(
            install_path_in(installed, "relais"),
            Some(PathBuf::from("/p/relais/0.12.0")),
            "the last entry"
        );
        assert_eq!(install_path_in(installed, "fleet-lsp"), None);
        assert_eq!(
            install_path_in(r#"{"plugins":{"relais@x":[]}}"#, "relais"),
            None
        );
        assert_eq!(
            install_path_in(r#"{"plugins":"#, "relais"),
            None,
            "malformed"
        );
    }

    #[test]
    fn builtin_agents_are_known_without_a_file() {
        let cwd = Path::new("/nonexistent");
        assert!(agent_has_tool(&Agent::Main, cwd).is_ok());
        assert!(agent_has_tool(&Agent::Named("Explore".into()), cwd).is_ok());
        assert!(agent_has_tool(&Agent::Named("statusline-setup".into()), cwd).is_err());
        assert!(agent_has_tool(&Agent::Unnamed, cwd).is_err());
        assert!(agent_has_tool(&Agent::Named("../x".into()), cwd).is_err());
    }
}
