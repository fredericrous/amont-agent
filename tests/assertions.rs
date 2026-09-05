//! The assertion tier, driven through the real binary.
//!
//! These build actual git repositories and actually push between them, because
//! the thing under test is whether a push LANDED — and a mocked remote would be
//! testing the mock. Everything is local paths on disk: no network, no
//! credentials, no remote host.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Reply {
    code: i32,
    stdout: String,
}

impl Reply {
    /// The text an assertion put into the model's context, if any.
    fn context(&self) -> String {
        serde_json::from_str::<serde_json::Value>(&self.stdout)
            .ok()
            .and_then(|v| {
                Some(
                    v.get("hookSpecificOutput")?
                        .get("additionalContext")?
                        .as_str()?
                        .to_string(),
                )
            })
            .unwrap_or_default()
    }
    fn event(&self) -> String {
        serde_json::from_str::<serde_json::Value>(&self.stdout)
            .ok()
            .and_then(|v| {
                Some(
                    v.get("hookSpecificOutput")?
                        .get("hookEventName")?
                        .as_str()?
                        .to_string(),
                )
            })
            .unwrap_or_default()
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "amont-assert-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository with one commit on `main` and a bare remote called `origin`.
fn repo_with_remote(name: &str) -> (PathBuf, PathBuf) {
    let root = scratch(name);
    let remote = root.join("remote.git");
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    Command::new("git")
        .args(["init", "-q", "--bare", "--template="])
        .arg(&remote)
        .output()
        .expect("git init --bare");
    git(&work, &["init", "-q", "-b", "main", "--template=", "."]);
    git(&work, &["config", "user.email", "t@t"]);
    git(&work, &["config", "user.name", "t"]);
    std::fs::write(work.join("a.txt"), "one\n").unwrap();
    git(&work, &["add", "a.txt"]);
    git(&work, &["commit", "-qm", "one"]);
    git(
        &work,
        &["remote", "add", "origin", &remote.display().to_string()],
    );
    (work, remote)
}

fn post_bash(command: &str, cwd: &Path) -> String {
    format!(
        r#"{{"hook_event_name":"PostToolUse","tool_name":"Bash","cwd":{},
             "session_id":"sess1234","tool_use_id":"t1","permission_mode":"default",
             "tool_input":{{"command":{}}},
             "tool_response":{{"stdout":"","stderr":"","interrupted":false}}}}"#,
        serde_json::Value::String(cwd.display().to_string()),
        serde_json::Value::String(command.to_string())
    )
}

fn send(payload: &str) -> Reply {
    let home = scratch("home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
        .arg("hook")
        .env("CLAUDE_CONFIG_DIR", &home)
        .env_remove("AMONT_AGENT_OFF")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .expect("write the payload");
    let out = child.wait_with_output().expect("the hook exits");
    Reply {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
    }
}

/// Speaking requires a stance above `observe`, which ships as the default. The
/// stance is read from global/system config only — a repository cannot raise
/// its own — so these tests set it through a scratch `GIT_CONFIG_GLOBAL`.
fn send_speaking(payload: &str) -> Reply {
    let home = scratch("speak-home");
    let cfg = home.join("gitconfig");
    // Written BY git, not by hand: `amont.agent.push-landed.stance` is a
    // three-part key whose middle part is a subsection, and the file syntax for
    // that is not what you would guess. Let git spell its own config.
    std::fs::write(&cfg, "").unwrap();
    let out = Command::new("git")
        .args([
            "config",
            "--file",
            &cfg.display().to_string(),
            "amont.agent.push-landed.stance",
            "advise",
        ])
        .output()
        .expect("git config runs");
    assert!(out.status.success(), "could not set the stance");
    let mut child = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
        .arg("hook")
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("AMONT_AGENT_OFF")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .expect("write the payload");
    let out = child.wait_with_output().expect("the hook exits");
    Reply {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
    }
}

/// The whole point: exit 0 from a push that did not land.
///
/// The push is never run. That is not a shortcut — it is the incident: the
/// model believed a push happened, and the remote had never heard of it.
#[test]
fn a_push_that_never_landed_is_reported() {
    let (work, _remote) = repo_with_remote("not-landed");
    let reply = send_speaking(&post_bash("git push origin main", &work));

    assert_eq!(reply.code, 0, "a decision always exits 0");
    assert_eq!(reply.event(), "PostToolUse", "answered on the right event");
    let text = reply.context();
    assert!(
        text.contains("push-landed") && text.contains("no refs/heads/main"),
        "expected the missing branch to be named, got: {text:?}"
    );
}

/// And the other half: a push that DID land must be completely silent. A guard
/// that speaks on success is a guard people stop reading.
#[test]
fn a_push_that_landed_says_nothing() {
    let (work, _remote) = repo_with_remote("landed");
    git(&work, &["push", "-q", "origin", "main"]);

    let reply = send_speaking(&post_bash("git push origin main", &work));
    assert_eq!(reply.stdout, "", "a held claim writes nothing at all");
    assert_eq!(reply.code, 0);
}

/// The remote moved on without us: local ahead of what the remote holds.
#[test]
fn a_stale_remote_is_reported_with_both_shas() {
    let (work, _remote) = repo_with_remote("stale");
    git(&work, &["push", "-q", "origin", "main"]);
    std::fs::write(work.join("a.txt"), "two\n").unwrap();
    git(&work, &["add", "a.txt"]);
    git(&work, &["commit", "-qm", "two"]);

    let reply = send_speaking(&post_bash("git push origin main", &work));
    let text = reply.context();
    let local = git(&work, &["rev-parse", "HEAD"]);
    assert!(
        text.contains(&local[..9]),
        "expected the local sha in the text, got: {text:?}"
    );
    assert!(
        text.contains("did not land"),
        "expected the verdict, got: {text:?}"
    );
}

/// Observing is the default, and observing is silent even when the claim is
/// broken. Promotion is a separate, deliberate act.
#[test]
fn the_default_stance_observes_without_speaking() {
    let (work, _remote) = repo_with_remote("observe");
    let reply = send(&post_bash("git push origin main", &work));
    assert_eq!(reply.stdout, "", "observe writes nothing");
}

/// A detached call has not finished doing what it claimed.
#[test]
fn a_backgrounded_push_is_not_judged() {
    let (work, _remote) = repo_with_remote("background");
    let payload = format!(
        r#"{{"hook_event_name":"PostToolUse","tool_name":"Bash","cwd":{},
             "session_id":"s","tool_use_id":"t","permission_mode":"default",
             "tool_input":{{"command":"git push origin main","run_in_background":true}},
             "tool_response":{{"stdout":"","backgroundTaskId":"bx1"}}}}"#,
        serde_json::Value::String(work.display().to_string())
    );
    assert_eq!(send_speaking(&payload).stdout, "");
}

/// A failed call is somebody else's event, and this crate has nothing to add to
/// a command that already said it failed.
#[test]
fn a_failed_call_is_never_judged() {
    let (work, _remote) = repo_with_remote("failed");
    let payload = format!(
        r#"{{"hook_event_name":"PostToolUseFailure","tool_name":"Bash","cwd":{},
             "session_id":"s","tool_use_id":"t","permission_mode":"default",
             "tool_input":{{"command":"git push origin main"}},
             "error":"Exit code 128"}}"#,
        serde_json::Value::String(work.display().to_string())
    );
    assert_eq!(send_speaking(&payload).stdout, "");
}

/// A command with no claim in it must not cost a single process.
#[test]
fn an_ordinary_command_is_silent() {
    let (work, _remote) = repo_with_remote("ordinary");
    assert_eq!(send_speaking(&post_bash("ls -la", &work)).stdout, "");
}
