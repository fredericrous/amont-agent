//! `amont-agent open <url|path>`: show a page to the person.
//!
//! The opener is a per-machine command template (`amont.agent.opener`, read
//! from the user's global git config), because the platform opener can hand a
//! page to a browser instance the person does not see. The template is split
//! into words first, by the guard's shell lexer, and `{}` is substituted
//! inside each word afterwards, so the page is always exactly one argv entry
//! and no shell runs.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use amont_agent_guard::doctor;
use amont_agent_guard::git;
use amont_agent_guard::shell::{self, Opaque, Parsed};
use amont_agent_process::guide;

const SETTING: &str = "amont.agent.opener";
const USAGE: &str = "usage: amont-agent open <url|path>";
const LAUNCH_WAIT: Duration = Duration::from_secs(2);
const POLL_EVERY: Duration = Duration::from_millis(20);

const HELP: &str = "\
usage: amont-agent open <url|path>

Open a page for the person, in a browser window they see. A path is made
absolute, must exist, and is opened as a file:// URL.

The opener is the command template in `git config --global amont.agent.opener`,
where `{}` stands for the page (the page is appended when there is no `{}`).
Unset, it is `open` (macOS), `xdg-open` (Linux) or `cmd /c start \"\"` (Windows).
Silent on success. Exit 0 launched, 1 the page could not be opened, 2 usage.
";

/// Where the opener template came from.
enum Source {
    Setting,
    PlatformDefault,
}

impl Source {
    fn label(&self) -> &'static str {
        match self {
            Source::Setting => "setting",
            Source::PlatformDefault => "platform default",
        }
    }
}

fn platform_default() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "cmd /c start \"\""
    } else {
        "xdg-open"
    }
}

fn template() -> (String, Source) {
    let set = git::output(&["config", "--global", "--get", SETTING])
        .filter(|o| o.code == 0 && !o.stdout.is_empty())
        .map(|o| o.stdout);
    match set {
        Some(t) => (t, Source::Setting),
        None => (platform_default().to_string(), Source::PlatformDefault),
    }
}

/// The template's words, split like a shell word list.
fn split(template: &str) -> Result<Vec<String>, String> {
    let Parsed::Clear(clauses) = shell::lex(template) else {
        return Err(format!("`{template}` is not a command that can be read"));
    };
    let [clause] = clauses.as_slice() else {
        return Err(format!("`{template}` is not one command"));
    };
    // `sh -c '…' sh {}` is a legitimate opener whose words the lexer reads
    // fine; it only cannot say what the inner script does, which is not
    // our question. Any other unreadable shape is refused.
    let unreadable = !matches!(clause.opaque, None | Some(Opaque::IndirectExecution(_)));
    if unreadable
        || !clause.redirects.is_empty()
        || clause.words.iter().any(|w| w.expanded)
        || clause.words.is_empty()
    {
        return Err(format!(
            "`{template}` is not a plain command with arguments"
        ));
    }
    Ok(clause.words.iter().map(|w| w.text.clone()).collect())
}

/// The argv for `page`: `{}` replaced inside each word, or the page appended.
fn argv(words: &[String], page: &str) -> Vec<String> {
    if words.iter().any(|w| w.contains("{}")) {
        words.iter().map(|w| w.replace("{}", page)).collect()
    } else {
        let mut all = words.to_vec();
        all.push(page.to_string());
        all
    }
}

/// A URL as given, or a path made absolute and turned into a `file://` URL.
fn page_for(target: &str) -> Result<String, String> {
    if target.contains("://") {
        return Ok(target.to_string());
    }
    let cwd = std::env::current_dir().map_err(|e| format!("no working directory: {e}"))?;
    let absolute: PathBuf = cwd.join(target);
    if !absolute.exists() {
        return Err(format!("{target} does not exist"));
    }
    Ok(guide::file_url(&absolute))
}

/// Open `page` (a URL) with the machine's opener. `Ok` when the opener
/// exited 0 or was still running after 2 s; it is never killed.
pub fn open_url(page: &str) -> Result<(), String> {
    let (template, _) = template();
    let words = split(&template).map_err(|why| format!("{why}; set {SETTING}"))?;
    let argv = argv(&words, page);
    let stderr_path = std::env::temp_dir().join(format!(
        "amont-agent-open-{}-{}.err",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let stderr_file = std::fs::File::create(&stderr_path)
        .map_err(|e| format!("could not capture the opener's output: {e}"))?;
    let result = launch(&argv, stderr_file, &stderr_path);
    // A leftover temp file is harmless; the opener may still hold it open.
    let _ = std::fs::remove_file(&stderr_path);
    result.map_err(|why| format!("could not open {page}: {why} (check {SETTING})"))
}

fn launch(argv: &[String], stderr_file: std::fs::File, stderr_path: &Path) -> Result<(), String> {
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("`{}` did not start: {e}", argv[0]))?;
    let deadline = Instant::now() + LAUNCH_WAIT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let said = std::fs::read_to_string(stderr_path).unwrap_or_default();
                let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
                return Err(if said.is_empty() {
                    format!("`{}` exited with {status}", argv[0])
                } else {
                    format!("`{}` exited with {status}: {said}", argv[0])
                });
            }
            Ok(None) if Instant::now() >= deadline => return Ok(()),
            Ok(None) => std::thread::sleep(POLL_EVERY),
            Err(e) => return Err(format!("`{}` could not be waited on: {e}", argv[0])),
        }
    }
}

pub fn run(args: &[OsString]) -> ExitCode {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let [target] = args.as_slice() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let outcome = page_for(target).and_then(|page| open_url(&page));
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("amont-agent: {why}");
            ExitCode::from(1)
        }
    }
}

/// Is `program` runnable: a path that exists, or a name found on PATH.
fn program_found(program: &str) -> bool {
    if program.contains('/') || program.contains('\\') {
        return Path::new(program).exists();
    }
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|dir| {
                dir.join(program).is_file() || dir.join(format!("{program}.exe")).is_file()
            })
        })
        .unwrap_or(false)
}

/// The opener in use and where it came from.
pub fn opener_finding() -> doctor::Finding {
    let (template, source) = template();
    let remedy = "git config --global amont.agent.opener '<browser command> {}'";
    let words = match split(&template) {
        Ok(words) => words,
        Err(why) => {
            return doctor::Finding::warn(
                format!("opener: {template} ({})", source.label()),
                format!("{why}; {remedy}"),
            )
        }
    };
    if program_found(&words[0]) {
        doctor::Finding::good(format!("opener: {template} ({})", source.label()))
    } else {
        doctor::Finding::warn(
            format!("opener: {} not found ({})", words[0], source.label()),
            format!("`amont-agent open` cannot show a page; {remedy}"),
        )
    }
}
