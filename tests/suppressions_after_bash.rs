//! The after-Bash suppression check, driven through the built binary with
//! PostToolUse and PostToolUseFailure payloads in scratch repositories.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct World {
    home: PathBuf,
    repo: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

impl World {
    fn new(name: &str) -> World {
        let root =
            std::env::temp_dir().join(format!("amont-agent-sab-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root); // a leftover from an earlier run may or may not exist
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join("src")).expect("scratch repo");
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "t@t.test"]);
        git(&repo, &["config", "user.name", "t"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("src/main.rs"), "mod exit;\nfn main() {}\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init", "--no-verify"]);
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        World { home, repo }
    }

    /// A stance lives in the person's global config: a repository's own
    /// config cannot set one (`tests/stance_scope.rs`).
    fn stance(&self, stance: &str) {
        std::fs::write(
            self.home.join("no-global-gitconfig"),
            format!("[amont \"agent.lint-suppression-added\"]\n\tstance = {stance}\n"),
        )
        .expect("global gitconfig");
    }

    fn payload(&self, event: &str, session: &str, command: &str) -> String {
        let mut body = serde_json::json!({
            "hook_event_name": event,
            "tool_name": "Bash",
            "cwd": self.repo.display().to_string(),
            "session_id": session,
            "tool_use_id": "t1",
            "permission_mode": "default",
            "tool_input": {"command": command},
        });
        if event == "PostToolUseFailure" {
            body["error"] = "Exit code 1".into();
        } else {
            body["tool_response"] =
                serde_json::json!({"stdout": "", "stderr": "", "interrupted": false});
        }
        body.to_string()
    }

    fn send(&self, payload: &str, path: Option<&Path>) -> String {
        let search = std::env::var("PATH").unwrap_or_default();
        let search = match path {
            Some(dir) => format!("{}:{search}", dir.display()),
            None => search,
        };
        let mut child = Command::new(env!("CARGO_BIN_EXE_greenpiste"))
            .arg("hook")
            .env("CLAUDE_CONFIG_DIR", &self.home)
            .env("PATH", search)
            .env("GIT_CONFIG_GLOBAL", self.home.join("no-global-gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("AMONT_AGENT_OFF")
            .env_remove("GREENHARNESS_HOST_SLOT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the binary runs");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(payload.as_bytes())
            .expect("write the payload");
        let out = child.wait_with_output().expect("the hook exits");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn journal(&self) -> String {
        let mut text = String::new();
        let mut stack = vec![self.home.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(body) = std::fs::read_to_string(&path) {
                    text.push_str(&body);
                }
            }
        }
        text
    }
}

const SED: &str = "sed -i 's/^mod exit;$/#[allow(dead_code)]\\nmod exit;/' src/main.rs";

fn apply_the_sed(world: &World) {
    std::fs::write(
        world.repo.join("src/main.rs"),
        "#[allow(dead_code)]\nmod exit;\nfn main() {}\n",
    )
    .unwrap();
}

#[test]
fn the_grh_sed_shape_is_asserted_with_path_and_line() {
    let world = World::new("sed");
    apply_the_sed(&world);
    let out = world.send(&world.payload("PostToolUse", "s-sed", SED), None);
    assert!(out.contains("src/main.rs:1:"), "{out}");
    assert!(out.contains("lint suppression"), "{out}");
    assert!(out.contains("the working tree of repo now holds"), "{out}");
}

#[test]
fn a_suppression_staged_by_the_same_command_is_still_asserted() {
    // `sed -i … && git add`: the index already holds it, HEAD does not.
    let world = World::new("staged");
    apply_the_sed(&world);
    git(&world.repo, &["add", "src/main.rs"]);
    let command = format!("{SED} && git add src/main.rs");
    let out = world.send(&world.payload("PostToolUse", "s-staged", &command), None);
    assert!(out.contains("src/main.rs:1:"), "{out}");
}

#[test]
fn a_heredoc_adding_an_allow_is_asserted() {
    let world = World::new("heredoc");
    std::fs::write(world.repo.join("x.rs"), "#[allow(dead_code)]\nfn f() {}\n").unwrap();
    let command = "cat > x.rs <<'EOF'\n#[allow(dead_code)]\nfn f() {}\nEOF";
    let out = world.send(&world.payload("PostToolUse", "s-heredoc", command), None);
    assert!(out.contains("x.rs:1:"), "{out}");
}

#[test]
fn the_same_hit_is_not_told_twice_in_a_session() {
    let world = World::new("twice");
    apply_the_sed(&world);
    let payload = world.payload("PostToolUse", "s-twice", SED);
    assert!(world.send(&payload, None).contains("src/main.rs:1:"));
    let second = world.send(&payload, None);
    assert!(!second.contains("src/main.rs"), "{second}");
}

#[test]
fn a_failing_command_is_answered_with_the_failure_event() {
    let world = World::new("failure");
    apply_the_sed(&world);
    let out = world.send(
        &world.payload(
            "PostToolUseFailure",
            "s-fail",
            &format!("{SED} && cargo clippy"),
        ),
        None,
    );
    assert!(out.contains("src/main.rs:1:"), "{out}");
    assert!(out.contains("PostToolUseFailure"), "{out}");
}

#[test]
fn observe_is_silent_and_journalled_as_bash() {
    let world = World::new("observe");
    world.stance("observe");
    apply_the_sed(&world);
    let out = world.send(&world.payload("PostToolUse", "s-observe", SED), None);
    assert!(!out.contains("src/main.rs"), "{out}");
    assert!(world.journal().contains(" bash"), "{}", world.journal());
}

#[cfg(unix)]
fn real_git() -> String {
    let real = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .expect("sh runs");
    String::from_utf8_lossy(&real.stdout).trim().to_string()
}

/// A `git` that logs its arguments to `log` and runs the real one.
#[cfg(unix)]
fn fake_git(dir: &Path, log: &Path) {
    common::write_fake(
        &dir.join("git"),
        &format!(
            "#!/bin/sh\necho \"$*\" >> {}\nexec {} \"$@\"\n",
            log.display(),
            real_git()
        ),
    );
}

/// [`fake_git`] that sleeps six seconds before any `cat-file`.
#[cfg(unix)]
fn fake_git_sleeping_in_cat_file(dir: &Path, log: &Path) {
    common::write_fake(
        &dir.join("git"),
        &format!(
            "#!/bin/sh\necho \"$*\" >> {}\ncase \"$*\" in *cat-file*) sleep 6;; esac\nexec {} \"$@\"\n",
            log.display(),
            real_git()
        ),
    );
}

#[cfg(unix)]
#[test]
fn a_command_naming_no_keyword_spawns_no_git() {
    let world = World::new("nokeyword");
    apply_the_sed(&world);
    let bin = world.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = world.home.join("git-calls.log");
    fake_git(&bin, &log);
    let out = world.send(
        &world.payload("PostToolUse", "s-nokw", "echo hello && ls"),
        Some(&bin),
    );
    assert!(!out.contains("src/main.rs"), "{out}");
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(calls, "", "git was called: {calls}");
}

#[cfg(unix)]
#[test]
fn a_git_that_sleeps_in_cat_file_fails_open_within_the_deadline() {
    let world = World::new("slow");
    apply_the_sed(&world);
    let bin = world.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = world.home.join("git-calls.log");
    fake_git_sleeping_in_cat_file(&bin, &log);
    let started = Instant::now();
    let out = world.send(&world.payload("PostToolUse", "s-slow", SED), Some(&bin));
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert!(!out.contains("src/main.rs"), "{out}");
    assert!(world.journal().contains("bash-slow"), "{}", world.journal());
}

#[test]
fn a_git_mv_of_a_file_with_markers_is_silent() {
    let world = World::new("mv");
    std::fs::write(
        world.repo.join("old.rs"),
        "#[allow(dead_code)]\nfn f() {}\n",
    )
    .unwrap();
    git(&world.repo, &["add", "old.rs"]);
    git(
        &world.repo,
        &["commit", "-q", "-m", "marked", "--no-verify"],
    );
    git(&world.repo, &["mv", "old.rs", "new.rs"]);
    let out = world.send(
        &world.payload("PostToolUse", "s-mv", "git mv old.rs new.rs # #[allow"),
        None,
    );
    assert!(!out.contains("new.rs"), "{out}");
    assert_eq!(out, "");
}

#[test]
fn an_extensionless_shebang_script_written_by_heredoc_is_asserted() {
    let world = World::new("shebang");
    std::fs::create_dir_all(world.repo.join("bin")).unwrap();
    std::fs::write(
        world.repo.join("bin/x"),
        "#!/bin/sh\n# shellcheck disable=SC2086\necho $1\n",
    )
    .unwrap();
    let command = "cat > bin/x <<EOF\n#!/bin/sh\n# shellcheck disable=SC2086\necho $1\nEOF";
    let out = world.send(&world.payload("PostToolUse", "s-shebang", command), None);
    assert!(out.contains("bin/x:2:"), "{out}");
}
