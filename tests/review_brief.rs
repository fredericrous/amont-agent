//! `amont-agent rules-sha` and `review-brief`, through the real binary
//! (decisions ADR-0043, ADR-0044).

use std::path::PathBuf;
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "amont-agent-review-brief-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => panic!("cannot clear {}: {e}", dir.display()),
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs the binary with the fake `aval` in `bin` first on PATH, so no test
/// reaches the machine's own `aval`.
fn run(args: &[&str], bin: &std::path::Path) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_amont-agent"));
    cmd.args(args);
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![bin.to_path_buf()];
    dirs.extend(std::env::split_paths(&inherited));
    cmd.env("PATH", std::env::join_paths(dirs).unwrap());
    cmd.output().expect("amont-agent runs")
}

/// The fake is a `#!/bin/sh` script: Windows resolves `Command::new("aval")`
/// to `aval.exe` only, so a `.cmd` fake would never be found. Every test that
/// needs it is therefore gated `cfg(unix)`, and none runs the real `aval`.
#[cfg(unix)]
fn fake_aval(dir: &std::path::Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("aval");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn a_failing_aval_exits_3_with_nothing_on_stdout() {
    let bin = scratch("failing");
    fake_aval(&bin, "#!/bin/sh\nexit 1\n");
    for sub in ["rules-sha", "review-brief"] {
        let out = run(&[sub, "-C", "."], &bin);
        assert_eq!(out.status.code(), Some(3), "{sub}");
        assert!(out.stdout.is_empty(), "{sub}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(err.trim().lines().count(), 1, "{err}");
        assert!(err.contains("aval --json rules"), "{err}");
    }
}

#[cfg(unix)]
#[test]
fn aval_rules_that_are_not_json_exit_3_with_nothing_on_stdout() {
    let bin = scratch("not-json");
    fake_aval(&bin, "#!/bin/sh\nprintf 'not json'\n");
    let out = run(&["review-brief", "-C", "."], &bin);
    assert_eq!(out.status.code(), Some(3));
    assert!(out.stdout.is_empty());
}

#[cfg(unix)]
#[test]
fn rules_sha_prints_the_sha256_of_what_aval_printed() {
    let bin = scratch("sha");
    fake_aval(&bin, "#!/bin/sh\nprintf abc\n");
    let out = run(&["rules-sha"], &bin);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n"
    );
}

#[cfg(unix)]
#[test]
fn both_are_listed_and_answer_help() {
    let bin = scratch("help");
    fake_aval(&bin, "#!/bin/sh\nexit 1\n");
    let help = run(&["--help"], &bin);
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("rules-sha"), "{text}");
    assert!(text.contains("review-brief"), "{text}");
    for sub in ["rules-sha", "review-brief"] {
        assert_eq!(run(&[sub, "--help"], &bin).status.code(), Some(0), "{sub}");
    }
}

#[cfg(unix)]
#[test]
fn a_usage_error_exits_2() {
    let bin = scratch("usage");
    fake_aval(&bin, "#!/bin/sh\nexit 1\n");
    for sub in ["rules-sha", "review-brief"] {
        assert_eq!(run(&[sub, "--bogus"], &bin).status.code(), Some(2), "{sub}");
        assert_eq!(run(&[sub, "-C"], &bin).status.code(), Some(2), "{sub}");
        // A word whose second byte is inside a character is a usage error, not a panic.
        assert_eq!(run(&[sub, "aé"], &bin).status.code(), Some(2), "{sub}");
    }
    assert_eq!(
        run(&["rules-sha", "--plan", "x"], &bin).status.code(),
        Some(2)
    );
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

#[cfg(unix)]
#[test]
fn a_failed_git_diff_is_a_repository_error_with_nothing_on_stdout() {
    let bin = scratch("diff-fails");
    fake_aval(&bin, "#!/bin/sh\nprintf '{\"rules\":[]}'\n");
    let repo = scratch("diff-fails-repo");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "base"]);
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    git(&repo, &["commit", "-qam", "change"]);
    // Remove the changed file's new blob: commits and trees still read, a diff cannot.
    let blob = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "HEAD:a.txt"])
        .output()
        .unwrap();
    let blob = String::from_utf8(blob.stdout).unwrap();
    let blob = blob.trim();
    let object = repo.join(".git/objects").join(&blob[..2]).join(&blob[2..]);
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&object, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    std::fs::remove_file(&object).unwrap();

    let out = run(
        &[
            "review-brief",
            "-C",
            repo.to_str().unwrap(),
            "--base",
            "HEAD~1",
        ],
        &bin,
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("git diff"),
        "{out:?}"
    );
}

/// A scratch repository with a base commit and one change on top, for the
/// brief's success path and the flag shapes.
#[cfg(unix)]
fn two_commit_repo(name: &str) -> PathBuf {
    let repo = scratch(name);
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "base"]);
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    git(&repo, &["commit", "-qam", "change"]);
    repo
}

/// An `aval` that answers each command the brief runs with fixed text.
#[cfg(unix)]
const BRIEF_AVAL: &str = r#"#!/bin/sh
case "$*" in
  "--json rules --level constraint --governs code --full")
    printf '{"rules":[{"id":"names.reveal-intent","level":"constraint","statement":"A name says what it is.","body":"No placeholder names.","active":true}]}' ;;
  "rules --level constraint") printf 'constraint names.reveal-intent\n' ;;
  "--json relevant"*) printf '{"keys":[]}' ;;
  *) exit 1 ;;
esac
"#;

#[cfg(unix)]
#[test]
fn the_brief_binds_the_same_tree_and_rules_as_tree_sha_and_rules_sha() {
    let bin = scratch("brief-ok");
    fake_aval(&bin, BRIEF_AVAL);
    let repo = two_commit_repo("brief-ok-repo");
    let dir = repo.to_str().unwrap();
    let brief = run(&["review-brief", "-C", dir, "--base", "HEAD~1"], &bin);
    assert_eq!(brief.status.code(), Some(0), "{brief:?}");
    let brief = String::from_utf8(brief.stdout).unwrap();
    let rules_sha = run(&["rules-sha", "-C", dir], &bin);
    assert_eq!(rules_sha.status.code(), Some(0), "{rules_sha:?}");
    let rules_sha = String::from_utf8(rules_sha.stdout).unwrap();
    assert_eq!(rules_sha.trim().len(), 64, "{rules_sha}");
    let tree = run(&["tree-sha", "--block", "-C", dir], &bin);
    assert_eq!(tree.status.code(), Some(0), "{tree:?}");
    let tree = String::from_utf8(tree.stdout).unwrap();
    assert!(tree.trim().starts_with("<<<TREE "), "{tree}");
    assert!(
        brief.contains(&format!("<<<RULES sha={}>>>", rules_sha.trim())),
        "{brief}"
    );
    assert!(brief.contains(tree.trim()), "{brief}");
    assert!(brief.contains("No placeholder names."), "{brief}");
}

#[cfg(unix)]
#[test]
fn flags_take_the_posix_shapes() {
    let bin = scratch("posix");
    fake_aval(&bin, BRIEF_AVAL);
    let repo = two_commit_repo("posix-repo");
    let attached = format!("-C{}", repo.display());
    for args in [
        vec!["review-brief", attached.as_str(), "--base=HEAD~1"],
        vec!["review-brief", attached.as_str(), "--base=HEAD~1", "--"],
        vec!["rules-sha", attached.as_str()],
    ] {
        let out = run(&args, &bin);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {out:?}");
    }
    let operand = run(&["rules-sha", attached.as_str(), "--", "extra"], &bin);
    assert_eq!(operand.status.code(), Some(2), "{operand:?}");
}
