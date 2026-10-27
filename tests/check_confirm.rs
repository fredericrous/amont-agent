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
