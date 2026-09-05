//! The generated guidance block, checked at the moment it is about to be
//! believed.
//!
//! amont's `pre-commit-agents-md` says at COMMIT time that `AGENTS.md` is
//! behind the binary that generates it. A session starts earlier than that,
//! and an agent reads the file at the start and follows it for the whole
//! session — "give commits a ten-minute timeout" from a block two releases
//! old is wrong before any commit happens. So the same question is asked
//! here, once, when the session opens.
//!
//! # Why this shells out
//!
//! This is the one thing in this crate that is about amont specifically.
//! Rather than link amont's block generator — which would put a git-hook
//! manager in this binary's dependency tree for one advisory notice — it
//! asks the installed `amont` the question amont already answers:
//!
//!     amont agents-md --check
//!
//! Somebody who does not use amont has no `amont` on `PATH`, and this
//! module then says nothing at all, which is the correct answer for them.
//!
//! # Why it reads stderr rather than the exit code
//!
//! `amont agents-md --check` exits **1** for drift AND for a file it could
//! not read or whose marker span is malformed — the `Err` arms return the
//! same code as the `Drifted` arm. Treating exit 1 as drift would tell
//! somebody their block is stale when the file merely could not be opened.
//! So the decision is made on the two sentences amont prints, and anything
//! else is silence. A session-opening notice that cries wolf is worse than
//! one that occasionally says nothing: the whole module is opt-in and
//! advisory, and it gets exactly one line of the reader's attention.
//!
//! Cheap first: two file reads and a marker match decide whether to spawn
//! anything at all, so the overwhelmingly common case — a repository with no
//! generated block — costs no process.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::gitconfig as config;

/// `amont.agent.agentsMdNotice` — whether a session opening may mention a
/// stale guidance block. Boolean rather than a stance: there is nothing to
/// deny, and nothing to measure a rate of.
pub const KEY_NOTICE: &str = "amont.agent.agentsMdNotice";

/// The opening marker amont writes. A literal, not an import: matching it is
/// the cheap gate in front of the spawn, and one string is not a dependency.
const START: &str = "<!-- amont:start -->";

/// The sentences amont prints on stderr when a generated file has drifted.
/// Matched as substrings so the filename in front of them is free to change.
const DRIFT_BLOCK: &str = "drifted from the generated block";
const DRIFT_POINTER: &str = "signpost drifted from the generated one";

/// How long amont gets to answer before we stop caring. The same bound
/// `stale.rs` puts on its fetch, and for the same reason: this runs in front
/// of somebody who is waiting to start work.
const BUDGET: Duration = Duration::from_secs(5);

pub fn notice(cwd: &Path) -> Option<String> {
    if crate::stance::switched_off() || !config::boolean_or(crate::stance::KEY_ENABLED, true) {
        return None;
    }
    let root = crate::git::stdout_in(cwd, &["rev-parse", "--show-toplevel"])?;
    let root = Path::new(&root);

    // The cheap gate: no generated block anywhere, no process.
    let has_markers = |p: &Path| std::fs::read_to_string(p).is_ok_and(|s| s.contains(START));
    if !has_markers(&root.join("AGENTS.md")) && !has_markers(&root.join("CLAUDE.md")) {
        return None;
    }
    if !config::boolean_or(KEY_NOTICE, true) {
        return None;
    }

    let stale = drifted(root)?;
    Some(format!(
        "amont-agent/agents-md: {} in this repository {} behind the block amont \
         generates — the hook list, budgets and conventions it states may be last \
         release's. `amont agents-md` regenerates it (commit the result); until \
         then, `amont list --json` is the current answer.",
        stale.join(" and "),
        if stale.len() == 1 { "is" } else { "are" },
    ))
}

/// Ask amont, and return the names of the files it reported as drifted.
///
/// `None` for every other outcome: amont absent, amont failing to run, the
/// budget expiring, an exit code with no drift line behind it.
fn drifted(root: &Path) -> Option<Vec<String>> {
    let stderr = ask(root)?;
    let mut stale: Vec<String> = Vec::new();
    for line in stderr.lines() {
        let marker = if line.contains(DRIFT_POINTER) {
            DRIFT_POINTER
        } else if line.contains(DRIFT_BLOCK) {
            DRIFT_BLOCK
        } else {
            continue;
        };
        // amont prints "<path>: <sentence>". Take its own filename rather
        // than re-deriving one, so the two stay in step by construction.
        let named = line[..line.find(marker)?]
            .trim_end()
            .trim_end_matches(':')
            .trim();
        let name = Path::new(named)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| named.to_string());
        if !name.is_empty() && !stale.contains(&name) {
            stale.push(crate::ui::sanitize(&name));
        }
    }
    (!stale.is_empty()).then_some(stale)
}

/// Run `amont agents-md --check` in `root` and hand back its stderr.
///
/// Spawned rather than `output()`-ed so the budget can be enforced: a hung
/// amont must not hold a session open. The child is killed on expiry and the
/// answer discarded.
///
/// # The pipe is drained WHILE the child runs
///
/// An earlier shape polled `try_wait` to completion and only then read the
/// pipe. That is the classic deadlock and it is silent: a child whose stderr
/// exceeds the pipe buffer — 64 KB on Linux, less on macOS — blocks in
/// `write`, never exits, and the budget above kills it. The notice would then
/// go missing precisely when amont had the most to say. Today amont prints
/// two sentences, so it never bit; a reader thread costs one `spawn` on a
/// path that already spawns a process, and removes the shape rather than
/// betting on the child staying quiet.
///
/// Killing the child closes its end of the pipe, so the reader always
/// finishes and the join cannot hang either.
fn ask(root: &Path) -> Option<String> {
    let mut child = Command::new("amont")
        .arg("agents-md")
        .arg("--check")
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let mut pipe = child.stderr.take()?;
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = String::new();
        let _ = pipe.read_to_string(&mut buf);
        buf
    });

    let deadline = std::time::Instant::now() + BUDGET;
    let finished = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
            Err(_) => break false,
        }
    };
    let buf = reader.join().ok()?;
    finished.then_some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(stderr: &str) -> Option<Vec<String>> {
        // The parsing half of `drifted`, without the spawn.
        let mut stale: Vec<String> = Vec::new();
        for line in stderr.lines() {
            let marker = if line.contains(DRIFT_POINTER) {
                DRIFT_POINTER
            } else if line.contains(DRIFT_BLOCK) {
                DRIFT_BLOCK
            } else {
                continue;
            };
            let named = line[..line.find(marker).unwrap()]
                .trim_end()
                .trim_end_matches(':')
                .trim();
            let name = Path::new(named)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| named.to_string());
            if !name.is_empty() && !stale.contains(&name) {
                stale.push(name);
            }
        }
        (!stale.is_empty()).then_some(stale)
    }

    #[test]
    fn a_drifted_block_is_named_by_its_filename() {
        let out = "/repo/AGENTS.md: drifted from the generated block — run `amont agents-md`";
        assert_eq!(names(out), Some(vec!["AGENTS.md".to_string()]));
    }

    #[test]
    fn both_files_are_reported_in_the_order_amont_printed_them() {
        let out = "/r/CLAUDE.md: signpost drifted from the generated one — run `amont agents-md`\n\
                   /r/AGENTS.md: drifted from the generated block — run `amont agents-md`";
        assert_eq!(
            names(out),
            Some(vec!["CLAUDE.md".to_string(), "AGENTS.md".to_string()])
        );
    }

    /// The whole reason this reads stderr instead of the exit code: amont
    /// exits 1 for these too, and none of them means the block is stale.
    #[test]
    fn an_error_that_is_not_drift_says_nothing() {
        assert_eq!(
            names("/repo/AGENTS.md: Permission denied (os error 13)"),
            None
        );
        assert_eq!(names("fatal: not a git repository"), None);
        assert_eq!(
            names("/repo/AGENTS.md: no closing <!-- amont:end --> marker"),
            None
        );
        assert_eq!(names(""), None);
    }

    /// Up to date prints on stdout, not stderr, so stderr is empty.
    #[test]
    fn up_to_date_says_nothing() {
        assert_eq!(names("/repo/AGENTS.md: up to date"), None);
    }
}
