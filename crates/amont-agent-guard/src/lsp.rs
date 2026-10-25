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

/// Whether `key` is already on the session's record, without adding it.
pub fn was_told(session: &str, key: &str) -> bool {
    if !crate::session_state::usable_id(session) || key.contains('\n') {
        return false;
    }
    let Some(dir) = crate::journal::dir().map(|d| d.join("sessions")) else {
        return false;
    };
    std::fs::read_to_string(dir.join(format!("{session}.lsp")))
        .is_ok_and(|seen| seen.lines().any(|l| l == key))
}

/// The one normal form a path is stored and looked up in: `.` and `..`
/// removed lexically, then `canonicalize` when the file exists (macOS `/tmp`
/// is `/private/tmp`). A file that does not exist keeps its lexical form.
pub fn normal(path: &Path) -> PathBuf {
    let lexical = lexical(path);
    std::fs::canonicalize(&lexical).unwrap_or(lexical)
}

/// `.` and `..` removed without touching the file system.
fn lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut lexical = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !lexical.pop() {
                    lexical.push("..");
                }
            }
            other => lexical.push(other.as_os_str()),
        }
    }
    lexical
}

/// A heading line of an LSP answer: `<path>:` alone, or `<path>:<line>[:<col>]`
/// at the start of a line. Indented lines are the entries under a heading.
fn heading_path(line: &str) -> Option<&str> {
    if line.is_empty() || line.starts_with(char::is_whitespace) {
        return None;
    }
    let mut rest = line.strip_suffix(':').unwrap_or(line);
    for _ in 0..2 {
        match rest.rsplit_once(':') {
            Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => {
                rest = head
            }
            _ => break,
        }
    }
    (rest.len() < line.len() && is_code(rest)).then_some(rest)
}

/// The files an LSP call was about, in normal form and without repeats: its
/// `filePath` (except for `workspaceSymbol`, where it only picks the
/// repository), and every heading of the answer that names a code file under
/// the cwd's repository. Touches the file system only to normalise paths.
pub fn asked_files(call: &crate::payload::LspCall) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    if call.operation != "workspaceSymbol" && !call.file_path.as_os_str().is_empty() {
        files.push(normal(&call.file_path));
    }
    let repo = normal(&git_root(&call.cwd).unwrap_or_else(|| call.cwd.clone()));
    let base = normal(&call.cwd);
    // A heading repeats across a long answer; `normal` costs a syscall, so
    // each spelling is resolved once.
    let mut spelled = std::collections::HashSet::new();
    for line in call.response.as_deref().unwrap_or_default().lines() {
        let Some(named) = heading_path(line) else {
            continue;
        };
        if !spelled.insert(named) {
            continue;
        }
        let named = named.strip_prefix("file://").unwrap_or(named);
        let named = Path::new(named);
        // A relative heading joins the cwd's normal form lexically: one
        // `canonicalize` per answer instead of one per file. Only a symlink
        // inside the repository then spells a file differently, and a missed
        // match only means a later Read is advised.
        // holds-until: the PostToolUse LSP budget (15 ms p95 on a 64 KiB
        // answer) has room for one `canonicalize` per distinct heading, or a
        // `lsp-first` miss is reported for a file reached through a symlink.
        // On Windows the canonical cwd is a `\\?\` path, where neither `.`
        // nor `/` is special: a lexical join would never match, so each
        // heading is canonicalized there.
        let abs = if named.is_absolute() || cfg!(windows) {
            normal(&call.cwd.join(named))
        } else {
            lexical(&base.join(named))
        };
        if abs.starts_with(&repo) && !files.contains(&abs) {
            files.push(abs);
        }
    }
    files
}

/// `main`, or the payload's `agent_id`. The one parser for every per-agent
/// record: absent is `main`, a valid id is itself, and a present id that is
/// blank, has whitespace, or could leave `sessions/` (`/`, `..`) is `None` —
/// no record is kept under it and nothing is said from it.
pub fn agent_key(agent_id: Option<&str>) -> Option<String> {
    match agent_id {
        None => Some("main".to_string()),
        Some(id)
            if !id.is_empty()
                && !id.contains(char::is_whitespace)
                && crate::session_state::usable_id(id) =>
        {
            Some(id.to_string())
        }
        Some(_) => None,
    }
}

fn record_file(session: &str) -> Option<PathBuf> {
    if !crate::session_state::usable_id(session) {
        return None;
    }
    Some(
        crate::journal::dir()?
            .join("sessions")
            .join(format!("{session}.lsp")),
    )
}

/// Append `asked <agent> <path>` for each file to the session's record (the
/// file `told` uses). A path with a newline is skipped and, like a record
/// that cannot be written, makes the result `Err`: the reason is for the
/// journal, and the lookup then errs toward advising.
pub fn record_asked(
    session: &str,
    agent_id: Option<&str>,
    files: &[PathBuf],
) -> Result<(), &'static str> {
    use std::io::Write;
    let Some(agent) = agent_key(agent_id) else {
        return Err("no usable agent id");
    };
    let mut skipped = false;
    let lines: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let text = f.to_string_lossy();
            if text.contains('\n') {
                skipped = true;
                return None;
            }
            Some(format!("asked {agent} {text}"))
        })
        .collect();
    if lines.is_empty() {
        return if skipped {
            Err("a path with a newline")
        } else {
            Ok(())
        };
    }
    let Some(file) = record_file(session) else {
        return Err("no usable session id or journal directory");
    };
    if let Some(dir) = file.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return Err("the sessions directory could not be created");
        }
    }
    let have = std::fs::read_to_string(&file).unwrap_or_default();
    let mut out = String::new();
    for line in &lines {
        if !have.lines().any(|l| l == line) && !out.lines().any(|l| l == line) {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !out.is_empty() {
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .and_then(|mut f| f.write_all(out.as_bytes()));
        if written.is_err() {
            return Err("the session record could not be written");
        }
    }
    if skipped {
        return Err("a path with a newline");
    }
    Ok(())
}

/// Whether this agent, in this session, asked LSP about `path` (looked up in
/// the same normal form it was recorded in).
pub fn asked(session: &str, agent_id: Option<&str>, path: &Path) -> bool {
    let Some(agent) = agent_key(agent_id) else {
        return false;
    };
    let Some(file) = record_file(session) else {
        return false;
    };
    let Ok(seen) = std::fs::read_to_string(file) else {
        return false;
    };
    let want = format!("asked {agent} {}", normal(path).to_string_lossy());
    seen.lines().any(|l| l == want)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::LspCall;

    // Used only by the Unix-only fixture test.
    #[cfg(unix)]
    fn fixture(name: &str) -> LspCall {
        let raw = std::fs::read_to_string(format!(
            "{}/../../tests/fixtures/lsp-posttooluse-{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture");
        match crate::payload::parse(&raw) {
            crate::payload::Event::PostLsp(c) => *c,
            _ => panic!("not an LSP call"),
        }
    }

    fn call(op: &str, file: &str, response: &str) -> LspCall {
        LspCall {
            operation: op.into(),
            file_path: PathBuf::from("/nonexistent/repo").join(file),
            response: Some(response.into()),
            cwd: PathBuf::from("/nonexistent/repo"),
            session: "s".into(),
            permission_mode: "default".into(),
            agent_id: None,
        }
    }

    // Unix path literals and symlinks: on Windows `/home/...` is not
    // absolute and `canonicalize` adds a `\\?\` prefix.
    #[cfg(unix)]
    #[test]
    fn the_fixtures_are_parsed_as_real_payloads() {
        let ds = fixture("documentsymbol");
        assert_eq!(ds.operation, "documentSymbol");
        assert!(ds
            .response
            .as_deref()
            .unwrap()
            .starts_with("Document symbols:"));
        let files = asked_files(&ds);
        assert_eq!(
            files,
            [PathBuf::from("/home/user/Developer/amont-agent/src/lsp.rs")]
        );
        let fr = fixture("findreferences");
        let names: Vec<String> = asked_files(&fr)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let root = "/home/user/Developer/amont-agent";
        assert_eq!(
            names,
            [
                format!("{root}/src/journal.rs"),
                format!("{root}/src/main.rs"),
                format!("{root}/src/doctor.rs")
            ]
        );
        let err = fixture("error");
        assert!(err
            .response
            .as_deref()
            .unwrap()
            .starts_with("Error performing"));
        assert_eq!(asked_files(&err).len(), 1, "filePath alone");
    }

    #[test]
    fn a_payload_without_a_file_path_still_counts_its_headings() {
        let raw = r#"{"hook_event_name":"PostToolUse","tool_name":"LSP","session_id":"s","cwd":"/nonexistent/repo",
            "tool_input":{"operation":"workspaceSymbol","query":"Foo"},
            "tool_response":{"result":"src/a.rs:\n  Foo (Struct) - Line 3"}}"#;
        let crate::payload::Event::PostLsp(c) = crate::payload::parse(raw) else {
            panic!("a workspaceSymbol call without filePath is still ours");
        };
        assert_eq!(
            asked_files(&c),
            [PathBuf::from("/nonexistent/repo/src/a.rs")]
        );
        let mut hover = *c;
        hover.operation = "hover".into();
        assert!(asked_files(&hover)
            .iter()
            .all(|p| !p.as_os_str().is_empty()));
    }

    #[test]
    fn workspace_symbol_ignores_its_file_path_and_reads_headings() {
        let c = call(
            "workspaceSymbol",
            "src/x.rs",
            "Found 2 symbols:\n\nrelais/crates/a/src/mod.rs:\n  Foo (Struct) - Line 3\n./src/y.rs:12:4\n../outside/z.rs:\n/etc/passwd.rs:\nNOTES.md:",
        );
        let got = asked_files(&c);
        assert_eq!(
            got,
            [
                PathBuf::from("/nonexistent/repo/relais/crates/a/src/mod.rs"),
                PathBuf::from("/nonexistent/repo/src/y.rs"),
            ]
        );
    }

    #[test]
    fn hover_counts_its_file_path() {
        let c = call("hover", "src/x.rs", "```rust\nfn x()\n```");
        assert_eq!(
            asked_files(&c),
            [PathBuf::from("/nonexistent/repo/src/x.rs")]
        );
    }

    #[test]
    fn one_normal_form_for_every_spelling() {
        let dir = std::env::temp_dir().join(format!("amont-agent-lsp-nf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real/src")).expect("dirs");
        std::fs::write(dir.join("real/src/x.rs"), "x").expect("file");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).expect("symlink");
        let direct = normal(&dir.join("real/src/x.rs"));
        assert_eq!(normal(&dir.join("real/./src/../src/x.rs")), direct);
        #[cfg(unix)]
        assert_eq!(normal(&dir.join("link/src/x.rs")), direct);
        let mut c = call("findReferences", "src/x.rs", "./src/x.rs:\n  Line 1:1");
        // Through the symlink where there is one; Windows has none here.
        let via = if cfg!(unix) { "link" } else { "real" };
        c.cwd = dir.join(via);
        c.file_path = dir.join(via).join("src/x.rs");
        assert_eq!(asked_files(&c), [direct]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_with_a_newline_is_unrecorded() {
        let bad = PathBuf::from("/a/b\nc.rs");
        assert!(record_asked("s-1", None, &[bad]).is_err());
        assert!(record_asked("s-1", Some("a 1"), &[PathBuf::from("/a.rs")]).is_err());
    }

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
