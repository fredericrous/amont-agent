//! `amont-agent check` decides as the hook does: a rule with a `confirm` step
//! is printed only when it confirms in the directory given by `--cwd`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

fn check(dir: &Path, extra: &[&str], command: &str) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
        .arg("check")
        .arg("--cwd")
        .arg(dir)
        .args(extra)
        .arg(command)
        .output()
        .expect("the binary runs");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn plain_repository(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("amont-check-confirm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup of a temp directory; a leftover is harmless
    std::fs::create_dir_all(&dir).expect("temp dir");
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "t@example.invalid"]);
    git(&dir, &["config", "user.name", "t"]);
    git(
        &dir,
        &["commit", "-q", "--allow-empty", "-m", "chore: init"],
    );
    git(&dir, &["switch", "-q", "-c", "feat/x"]);
    dir
}

#[test]
fn an_unconfirmed_rule_is_not_printed_as_firing() {
    let dir = plain_repository("push");
    let (ok, stdout) = check(&dir, &[], "git push -u origin feat/x");
    assert!(ok);
    assert!(
        !stdout.lines().any(|l| l.starts_with("push-preview [")),
        "{stdout}"
    );
    let (ok, explained) = check(&dir, &["--explain"], "git push -u origin feat/x");
    assert!(ok);
    assert!(
        explained.contains("push-preview not confirmed:"),
        "{explained}"
    );
    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup of a temp directory; a leftover is harmless
}

#[test]
fn a_rule_without_confirm_is_printed_unchanged() {
    let dir = plain_repository("pipe");
    let (ok, stdout) = check(&dir, &[], "git push origin feat/x | tail -5");
    assert!(ok);
    assert!(
        stdout.lines().any(|l| l.starts_with("pipe-to-tail [")),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup of a temp directory; a leftover is harmless
}

fn assert_prints_remedy(dir: &Path, rule: &str, command: &str) {
    let (ok, stdout) = check(dir, &[], command);
    assert!(ok, "{stdout}");
    let line_at = stdout
        .lines()
        .position(|l| l.starts_with(&format!("{rule} [")))
        .unwrap_or_else(|| panic!("{rule} did not confirm for {command:?}:\n{stdout}"));
    assert!(
        stdout.lines().skip(line_at).any(|l| l.contains("→ ")),
        "no remedy line for {command:?}:\n{stdout}"
    );
}

#[test]
fn a_confirmed_bare_stash_pop_prints_its_remedy() {
    let dir = plain_repository("stash");
    let second = dir.with_file_name(format!(
        "amont-check-confirm-stash-wt-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&second); // best-effort cleanup of a temp directory; a leftover is harmless
    git(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            second.to_str().expect("utf-8"),
        ],
    );
    assert_prints_remedy(&dir, "bare-stash-pop", "git stash pop");
    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup of a temp directory; a leftover is harmless
    let _ = std::fs::remove_dir_all(&second); // best-effort cleanup of a temp directory; a leftover is harmless
}

#[test]
fn a_confirmed_stale_base_prints_its_remedy() {
    let origin = plain_repository("origin");
    let clone = origin.with_file_name(format!("amont-check-confirm-clone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&clone); // best-effort cleanup of a temp directory; a leftover is harmless
    let status = Command::new("git")
        .args(["clone", "-q"])
        .arg(&origin)
        .arg(&clone)
        .status()
        .expect("git runs");
    assert!(status.success());
    git(
        &origin,
        &["commit", "-q", "--allow-empty", "-m", "chore: newer"],
    );
    git(&clone, &["config", "user.email", "t@example.invalid"]);
    git(&clone, &["config", "user.name", "t"]);
    git(&clone, &["config", "amont.agent.fetch", "true"]);
    assert_prints_remedy(&clone, "stale-base", "git switch -c feat/z");
    let _ = std::fs::remove_dir_all(&origin); // best-effort cleanup of a temp directory; a leftover is harmless
    let _ = std::fs::remove_dir_all(&clone); // best-effort cleanup of a temp directory; a leftover is harmless
}
