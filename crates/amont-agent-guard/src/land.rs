//! `amont land` read as a push.
//!
//! A land runs `git push` as a child process, so the hook never sees that
//! push: without this, every guard keyed on `git push` (implementation
//! review, preview) would be walked around by typing `amont land` instead.
//! And a land's target is not on its command line: amont computes it at run
//! time from git config and HEAD. Re-implementing that here would drift, so
//! amont answers the question itself: `amont land --dry-run --json` (1.54.0)
//! says what a land would push, reading local git state only.
//!
//! Only one shape is read: `amont land [--json] [--force]`, or
//! `amont land --tag <name> [--json]` (amont 2.0, one existing tag), optionally
//! after one `cd <literal path> &&`, alone in the command, in the foreground.
//! Anything around it could change what the land does between this hook and
//! the push (a `git commit` before it moves HEAD, `git fetch --set-upstream`
//! moves its upstream), change what it runs against (`GIT_DIR=…`, `env`,
//! `sudo`), or separate the Bash result from the land's own result
//! (`amont land; true`, a pipe, `&`). Every such shape is unresolvable,
//! which a guard under deny holds, and the message names the one shape that
//! is read.
//!
//! What a land actually pushed is read afterwards from its receipt, not from
//! the tracking ref, which git itself moves from 2.56 on any accepted push to
//! a URL that matches a remote.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::shell::{Connector, Parsed, Simple};

/// What the dry-run may cost inside a hook whose own budget is ~19 s.
const BUDGET: Duration = Duration::from_secs(3);

/// The one shape, named in every refusal.
pub const SHAPE: &str =
    "only `amont land [--json] [--force]` or `amont land --tag <name> [--json]`, alone in the command or after one `cd <dir> &&`, in the foreground, is read";

/// Is this clause an `amont land`?
pub fn is_land(cmd: &Simple) -> bool {
    cmd.program() == Some("amont") && cmd.subcommand() == Some("land")
}

/// The tag an `amont land --tag <name>` (or `--tag=<name>`) pushes, if it
/// names one. A land without `--tag` pushes the branch.
pub fn tag_of(cmd: &Simple) -> Option<String> {
    let words = &cmd.words;
    words.iter().enumerate().find_map(|(i, w)| {
        if w.text == "--tag" {
            words.get(i + 1).map(|n| n.text.clone())
        } else {
            w.text.strip_prefix("--tag=").map(str::to_string)
        }
    })
}

/// Is `cmd`, an `amont land` clause of `parsed`, the one shape that is read?
/// `background` is the payload's `run_in_background`.
pub fn shape(parsed: &Parsed, cmd: &Simple, background: bool) -> Result<(), &'static str> {
    if background {
        return Err("a land run in the background: its result is not known when the call returns");
    }
    if !parsed.fully_read() {
        return Err("the command around the land could not be read");
    }
    let clauses = parsed.clauses();
    let Some(last) = clauses.last() else {
        return Err(SHAPE);
    };
    if last.at != cmd.at || cmd.next.is_some() || cmd.nested.is_some() {
        return Err(SHAPE);
    }
    match clauses {
        [_] => {}
        [cd, _] => {
            let literal_dir = cd.words.len() == 2
                && cd.words[0].text == "cd"
                && !cd.words[0].quoted
                && !cd.words[1].expanded
                && !cd.words[1].text.trim().is_empty()
                && cd.words[1].text != "-"
                && cd.redirects.is_empty()
                && !cd.heredoc
                && cd.nested.is_none();
            if !literal_dir || cd.next != Some(Connector::AndAnd) {
                return Err(SHAPE);
            }
        }
        _ => return Err(SHAPE),
    }
    // `amont` itself first: an assignment (`GIT_DIR=…`) or a wrapper (`env`,
    // `sudo`, `timeout`) would make the dry-run below answer for another
    // repository or identity than the land's.
    let words = &cmd.words;
    let head = words.len() >= 2
        && words[0].text == "amont"
        && words[1].text == "land"
        && words.iter().all(|w| !w.quoted && !w.expanded);
    if !head || !cmd.redirects.is_empty() || cmd.heredoc {
        return Err(SHAPE);
    }
    // Flags: `--json`, `--force`, and one `--tag <name>` / `--tag=<name>`
    // whose name is a plain word. A tag is never forced: amont refuses it.
    let rest = &words[2..];
    let mut tags = 0;
    let mut forced = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].text.as_str() {
            "--json" => {}
            "--force" => forced = true,
            "--tag" => {
                let Some(name) = rest.get(i + 1) else {
                    return Err(SHAPE);
                };
                if !plain_name(&name.text) {
                    return Err(SHAPE);
                }
                tags += 1;
                i += 1;
            }
            other => match other.strip_prefix("--tag=") {
                Some(name) if plain_name(name) => tags += 1,
                _ => return Err(SHAPE),
            },
        }
        i += 1;
    }
    if tags > 1 || (tags == 1 && forced) {
        return Err(SHAPE);
    }
    Ok(())
}

/// A tag name the land can be judged by: not empty, not a flag, and nothing
/// the shell would still expand (`$TAG`, a backtick): a guard never judges a
/// name it cannot resolve.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-') && !name.contains(['$', '`'])
}

/// What `amont land --dry-run --json` answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The land would push `pushed` to `dst` on `remote`.
    Would(Landing),
    /// amont refuses this land itself (main, a dirty tree, no branch…): it
    /// publishes nothing, so no guard needs to hold it.
    Refused(String),
    /// amont could not say, and why.
    Unknown(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    pub branch: String,
    pub remote: String,
    pub pushed: String,
    pub dst: String,
    pub receipt_dir: PathBuf,
}

/// The dry-run for a land that runs in `dir`, of `tag` when it names one.
/// Asked once per directory and tag per hook process: the implementation
/// review, the preview and the pre-push record all resolve the same land.
pub fn dry_run(dir: &Path, tag: Option<&str>) -> Answer {
    type Key = (PathBuf, Option<String>);
    static MEMO: OnceLock<Mutex<HashMap<Key, Answer>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    let key = (dir.to_path_buf(), tag.map(str::to_string));
    if let Some(a) = memo.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return a.clone();
    }
    let answer = ask(dir, tag);
    memo.lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, answer.clone());
    answer
}

fn ask(dir: &Path, tag: Option<&str>) -> Answer {
    if !dir.is_dir() {
        return Answer::Unknown("the land's directory does not exist");
    }
    let mut args = vec!["land", "--dry-run", "--json"];
    if let Some(name) = tag {
        args.push("--tag");
        args.push(name);
    }
    let Ok(mut child) = Command::new("amont")
        .args(&args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return Answer::Unknown("amont is not installed, so what a land would push cannot be read");
    };
    // Both pipes drained while the child runs, as `guidance::ask` does: a
    // child blocked on a full pipe would otherwise be killed at the budget.
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut buf);
            }
            buf
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    );
    let deadline = std::time::Instant::now() + BUDGET;
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Answer::Unknown("`amont land --dry-run` did not answer within 3 s");
            }
            Err(_) => return Answer::Unknown("`amont land --dry-run` could not be waited for"),
        }
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    read_answer(code, &stdout, &stderr)
}

/// Pure: the dry-run's exit code and output, as an [`Answer`].
pub fn read_answer(code: Option<i32>, stdout: &str, stderr: &str) -> Answer {
    if code == Some(2) {
        return Answer::Unknown(
            "this amont has no `land --dry-run` (1.54.0 or later), so what a land would push cannot be read",
        );
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout.trim()) else {
        return Answer::Unknown("`amont land --dry-run --json` printed no JSON");
    };
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    match (code, field("status").as_str()) {
        (Some(0), "would-land") => {
            let landing = Landing {
                branch: field("branch"),
                remote: field("remote"),
                pushed: field("pushed"),
                dst: field("dst"),
                receipt_dir: PathBuf::from(field("receipt_dir")),
            };
            // A branch land names `refs/heads/…`; a tag land (amont 2.0,
            // `--tag`) names `refs/tags/…` and no branch.
            let target =
                landing.dst.starts_with("refs/heads/") || landing.dst.starts_with("refs/tags/");
            if landing.pushed.is_empty() || !target {
                return Answer::Unknown("`amont land --dry-run` named no commit or branch");
            }
            Answer::Would(landing)
        }
        (Some(1), "refused") => {
            let why = stderr
                .lines()
                .find_map(|l| l.strip_prefix("amont: land: "))
                .unwrap_or("amont refuses this land")
                .to_string();
            Answer::Refused(why)
        }
        _ => Answer::Unknown("`amont land --dry-run` gave an answer this version does not know"),
    }
}

/// One landing receipt, as amont writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub started: i128,
    pub branch: String,
    pub pushed: String,
    pub status: String,
}

/// The receipts in `dir` for `branch` that started at or after `since`
/// (unix nanoseconds). Unreadable files are skipped.
pub fn receipts_since(dir: &Path, branch: &str, since: i128) -> Vec<Receipt> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(r) = read_receipt(&text) else {
            continue;
        };
        if r.branch == branch && r.started >= since {
            found.push(r);
        }
    }
    found
}

/// Pure: one receipt's JSON.
pub fn read_receipt(text: &str) -> Option<Receipt> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    Some(Receipt {
        started: v.get("started").and_then(|x| x.as_i64())? as i128,
        branch: s("branch")?,
        pushed: s("pushed")?,
        status: s("status")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn read(cmd: &str, background: bool) -> Result<(), &'static str> {
        let parsed = lex(cmd);
        let land = parsed
            .judgeable()
            .find(|c| is_land(c))
            .expect("a land clause");
        shape(&parsed, land, background)
    }

    #[test]
    fn the_supported_shapes_are_read() {
        assert_eq!(read("amont land", false), Ok(()));
        assert_eq!(read("amont land --json", false), Ok(()));
        assert_eq!(read("amont land --force --json", false), Ok(()));
        assert_eq!(read("cd /x/wt && amont land --json", false), Ok(()));
    }

    #[test]
    fn anything_around_the_land_is_not() {
        for cmd in [
            "git commit -m x && amont land",
            "git fetch --set-upstream origin x && amont land",
            "amont land; true",
            "amont land && false",
            "amont land || echo failed",
            "amont land | cat",
            "amont land > out",
            "amont land 2>&1",
            "GIT_DIR=/other/.git GIT_WORK_TREE=/other amont land",
            "env A=b amont land",
            "sudo amont land",
            "timeout 60 amont land",
            "nice amont land",
            "cd $(pwd) && amont land",
            "cd - && amont land",
            "cd a && cd b && amont land",
            "cd x; amont land",
            "amont land origin",
            "echo $(amont land)",
        ] {
            assert!(read(cmd, false).is_err(), "{cmd} must not be read");
        }
    }

    #[test]
    fn a_tag_land_is_read_and_names_its_tag() {
        for cmd in [
            "amont land --tag v1.2.3",
            "amont land --tag v1.2.3 --json",
            "amont land --json --tag=v1.2.3",
            "cd /x/wt && amont land --tag v1.2.3 --json",
        ] {
            assert_eq!(read(cmd, false), Ok(()), "{cmd}");
            let parsed = lex(cmd);
            let land = parsed.judgeable().find(|c| is_land(c)).unwrap();
            assert_eq!(tag_of(land).as_deref(), Some("v1.2.3"), "{cmd}");
        }
        for cmd in [
            "amont land --tag",
            "amont land --tag=",
            "amont land --tag --json",
            "amont land --tag v1 --tag v2",
            "amont land --tag v1 --force",
            "amont land --tag $TAG",
        ] {
            assert!(read(cmd, false).is_err(), "{cmd} must not be read");
        }
    }

    #[test]
    fn a_tag_answer_names_the_tag_ref() {
        let out = r#"{"branch":"","remote":"origin","push_url":"u","pushed":"abc123","dst":"refs/tags/v1.2.3","ref":"refs/tags/v1.2.3","retried":false,"status":"would-land","receipt_dir":"/r/.git/amont/land","lease":false}"#;
        match read_answer(Some(0), out, "") {
            Answer::Would(l) => assert_eq!(l.dst, "refs/tags/v1.2.3"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_land_in_the_background_is_not_read() {
        assert!(read("amont land", true).is_err());
    }

    #[test]
    fn a_would_land_answer_names_the_target() {
        let out = r#"{"branch":"feat","remote":"origin","push_url":"u","pushed":"abc123","dst":"refs/heads/feat","retried":false,"status":"would-land","receipt_dir":"/r/.git/amont/land","lease":false}"#;
        match read_answer(Some(0), out, "") {
            Answer::Would(l) => {
                assert_eq!(l.pushed, "abc123");
                assert_eq!(l.dst, "refs/heads/feat");
                assert_eq!(l.receipt_dir, PathBuf::from("/r/.git/amont/land"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_refusal_carries_amonts_reason() {
        let out = r#"{"branch":"main","remote":"origin","push_url":"","pushed":"","dst":"refs/heads/main","retried":false,"status":"refused","receipt_dir":"/r","lease":false}"#;
        let err = "amont: land: main and master change by pull request, never by a push\n";
        assert_eq!(
            read_answer(Some(1), out, err),
            Answer::Refused("main and master change by pull request, never by a push".into())
        );
    }

    #[test]
    fn an_old_amont_or_garbage_is_unknown() {
        assert!(matches!(
            read_answer(Some(2), "", "usage"),
            Answer::Unknown(_)
        ));
        assert!(matches!(
            read_answer(Some(0), "not json", ""),
            Answer::Unknown(_)
        ));
        assert!(matches!(
            read_answer(
                Some(0),
                r#"{"status":"would-land","pushed":"","dst":""}"#,
                ""
            ),
            Answer::Unknown(_)
        ));
    }

    #[test]
    fn a_receipt_is_read_and_filtered_by_branch_and_time() {
        let dir = std::env::temp_dir().join(format!("amont-agent-receipts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, started: i64, branch: &str| {
            std::fs::write(
                dir.join(name),
                format!(r#"{{"started":{started},"ended":{started},"branch":"{branch}","remote":"origin","push_url":"u","dst":"refs/heads/{branch}","pushed":"sha-{started}","retried":false,"status":"landed"}}"#),
            )
            .unwrap();
        };
        write("1-1.json", 100, "feat");
        write("2-1.json", 300, "feat");
        write("3-1.json", 400, "other");
        std::fs::write(dir.join("junk.json"), "not json").unwrap();
        let found = receipts_since(&dir, "feat", 200);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pushed, "sha-300");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
