//! `amont-agent open <url|path>`: show a page to the person.
//!
//! The opener is a per-machine command template (`amont.agent.opener`, read
//! from the user's global git config), because the platform opener can hand a
//! page to a browser instance the person does not see. The template is split
//! into words first, by the guard's shell lexer, and `{}` is substituted
//! inside each word afterwards, so the page is always exactly one argv entry
//! and no shell runs.

use greenpiste_guard::names::BIN;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use greenpiste_guard::doctor;
use greenpiste_guard::git;
use greenpiste_guard::shell::{self, Opaque, Parsed};
use greenpiste_process::guide;

const SETTING: &str = "amont.agent.opener";
const USAGE: &str = concat!("usage: ", greenpiste_guard::bin!(), " open <url|path>");
const LAUNCH_WAIT: Duration = Duration::from_secs(2);
const CAPTURE_ATTEMPTS: u32 = 16;
const POLL_EVERY: Duration = Duration::from_millis(20);

const HELP: &str = concat!("\
usage: ", greenpiste_guard::bin!(), " open <url|path>

Open a page for the person, in a browser window they see. A path is made
absolute, must exist, and is opened as a file:// URL.

The opener is the command template in `git config --global amont.agent.opener`,
where `{}` stands for the page (the page is appended when there is no `{}`).
Unset, it is `open` (macOS), `xdg-open` (Linux) or `rundll32 url.dll,FileProtocolHandler {}` (Windows).
Silent on success. Exit 0 launched, 1 the page could not be opened, 2 usage.
");

/// Why a page could not be opened.
#[derive(Debug)]
pub enum OpenError {
    /// The opener template cannot be read or used.
    Template(String),
    NoCwd(std::io::Error),
    Missing(std::ffi::OsString),
    Capture(std::io::Error),
    NotStarted {
        page: String,
        program: String,
        cause: std::io::Error,
    },
    Exited {
        page: String,
        program: String,
        status: std::process::ExitStatus,
        said: String,
    },
    Wait {
        page: String,
        program: String,
        cause: std::io::Error,
    },
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Template(why) => write!(f, "{why}"),
            OpenError::NoCwd(cause) => write!(f, "no working directory: {cause}"),
            OpenError::Missing(target) => write!(f, "{} does not exist", target.to_string_lossy()),
            OpenError::Capture(cause) => {
                write!(f, "could not capture the opener's output: {cause}")
            }
            OpenError::NotStarted {
                page,
                program,
                cause,
            } => write!(
                f,
                "could not open {page}: `{program}` did not start: {cause} (check {SETTING})"
            ),
            OpenError::Exited {
                page,
                program,
                status,
                said,
            } if said.is_empty() => write!(
                f,
                "could not open {page}: `{program}` exited with {status} (check {SETTING})"
            ),
            OpenError::Exited {
                page,
                program,
                status,
                said,
            } => write!(
                f,
                "could not open {page}: `{program}` exited with {status}: {said} (check {SETTING})"
            ),
            OpenError::Wait {
                page,
                program,
                cause,
            } => write!(
                f,
                "could not open {page}: `{program}` could not be waited on: {cause} (check {SETTING})"
            ),
        }
    }
}

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
        "rundll32 url.dll,FileProtocolHandler {}"
    } else {
        "xdg-open"
    }
}

/// The opener template. Only git's exit 1 ("key not set") means unset; any
/// other exit, or a git that cannot run, is an error.
fn template() -> Result<(String, Source), OpenError> {
    let Some(answer) = git::output(&["config", "--global", "--get", SETTING]) else {
        return Err(OpenError::Template(format!(
            "git could not be run to read {SETTING}; set {SETTING} once git works"
        )));
    };
    match answer.code {
        0 if !answer.stdout.is_empty() => Ok((answer.stdout, Source::Setting)),
        0 | 1 => Ok((platform_default().to_string(), Source::PlatformDefault)),
        code => Err(OpenError::Template(format!(
            "git config exited {code} reading {SETTING}: {}; fix {SETTING}",
            answer.stderr
        ))),
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
fn page_for(target: &OsStr) -> Result<String, OpenError> {
    if let Some(url) = target.to_str().filter(|t| t.contains("://")) {
        return Ok(url.to_string());
    }
    let cwd = std::env::current_dir().map_err(OpenError::NoCwd)?;
    let absolute: PathBuf = cwd.join(target);
    if !absolute.exists() {
        return Err(OpenError::Missing(target.to_os_string()));
    }
    Ok(guide::file_url(&absolute))
}

/// Create the opener's stderr capture: a new file, owner-only, never a name
/// somebody else already made.
fn capture_file() -> Result<(std::fs::File, PathBuf), OpenError> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0); // a clock before 1970 only makes the name less unique; create_new decides
    let mut last = None;
    for attempt in 0..CAPTURE_ATTEMPTS {
        let path = std::env::temp_dir().join(format!(
            "amont-agent-open-{}-{stamp}-{attempt}.err",
            std::process::id()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => last = Some(cause),
            Err(cause) => return Err(OpenError::Capture(cause)),
        }
    }
    Err(OpenError::Capture(last.unwrap_or_else(|| {
        std::io::Error::other("no free capture file name")
    })))
}

/// Open `page` (a URL) with the machine's opener. `Ok` when the opener
/// exited 0 or was still running after 2 s; it is never killed.
pub fn open_url(page: &str) -> Result<(), OpenError> {
    let (template, _) = template()?;
    let words =
        split(&template).map_err(|why| OpenError::Template(format!("{why}; set {SETTING}")))?;
    let argv = argv(&words, page);
    let (stderr_file, stderr_path) = capture_file()?;
    let launched = launch(&argv, page, stderr_file, &stderr_path);
    // holds-until: an opener that keeps writing stderr after it was left running; the unlinked file then belongs to the browser and grows with it, until the capture is size-capped
    let _ = std::fs::remove_file(&stderr_path); // a leftover temp file is harmless, and the opener may still hold it open
    launched
}

fn launch(
    argv: &[String],
    page: &str,
    stderr_file: std::fs::File,
    stderr_path: &Path,
) -> Result<(), OpenError> {
    let program = argv[0].clone();
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
    let mut child = command.spawn().map_err(|cause| OpenError::NotStarted {
        page: page.to_string(),
        program: program.clone(),
        cause,
    })?;
    let deadline = Instant::now() + LAUNCH_WAIT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let said = std::fs::read_to_string(stderr_path).unwrap_or_default(); // the capture is only a hint for the message
                let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
                return Err(OpenError::Exited {
                    page: page.to_string(),
                    program,
                    status,
                    said,
                });
            }
            Ok(None) if Instant::now() >= deadline => return Ok(()),
            Ok(None) => std::thread::sleep(POLL_EVERY),
            Err(cause) => {
                return Err(OpenError::Wait {
                    page: page.to_string(),
                    program,
                    cause,
                })
            }
        }
    }
}

fn unknown_option(word: &OsStr) -> ExitCode {
    eprintln!(
        concat!(greenpiste_guard::bin!(), ": unknown option {}"),
        word.to_string_lossy()
    );
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

pub fn run(args: &[OsString]) -> ExitCode {
    let after_separator = args.first().is_some_and(|a| a == "--");
    let operands = if after_separator { &args[1..] } else { args };
    if !after_separator {
        if args.iter().any(|a| a == "--help" || a == "-h") {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        if let Some(word) = args.iter().find(|a| a.to_string_lossy().starts_with('-')) {
            return unknown_option(word);
        }
    }
    let [target] = operands else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match page_for(target).and_then(|page| open_url(&page)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{BIN}: {why}");
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
    let (template, source) = match template() {
        Ok(found) => found,
        Err(why) => {
            return doctor::Finding::warn(
                "opener: unreadable".to_string(),
                format!("{why}; {SETTING} names the opener"),
            )
        }
    };
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
            format!("`{BIN} open` cannot show a page; {remedy}"),
        )
    }
}
