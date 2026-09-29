//! `amont-agent tree-sha`, through the real binary (ADR-0022,
//! `work.implementation-review`): the canonical tree id an implementation
//! review binds to.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct World {
    root: PathBuf,
}

impl World {
    fn new(name: &str) -> World {
        let root = std::env::temp_dir().join(format!(
            "amont-agent-tree-sha-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        World { root }
    }

    fn repo(&self, name: &str, object_format: Option<&str>) -> PathBuf {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut args = vec!["init", "-q", "-b", "main", "--template="];
        let fmt;
        if let Some(f) = object_format {
            fmt = format!("--object-format={f}");
            args.push(&fmt);
        }
        args.push(".");
        git(&dir, &args);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "user.name", "t"]);
        self.commit(&dir, &[("src/lib.rs", "fn a() {}\n")]);
        dir
    }

    fn commit(&self, dir: &Path, files: &[(&str, &str)]) -> String {
        for (p, body) in files {
            let f = dir.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        }
        git(dir, &["add", "."]);
        git(dir, &["commit", "-qm", "c"]);
        git(dir, &["rev-parse", "HEAD"])
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> (i32, String, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .arg("tree-sha")
            .args(args)
            .current_dir(cwd)
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        )
    }

    fn sha(&self, dir: &Path) -> String {
        let (code, out, err) = self.run(dir, &[]);
        assert_eq!(code, 0, "{err}");
        out
    }
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[test]
fn the_id_is_64_hex_idempotent_and_ignores_the_plans_directory() {
    let w = World::new("plans");
    let repo = w.repo("app", None);
    let before = w.sha(&repo);
    assert!(is_sha256(&before), "{before}");
    assert_eq!(before, w.sha(&repo), "idempotent");

    // Recording the review, or landing the plan, changes nothing.
    w.commit(
        &repo,
        &[(
            "docs/plans/2026-09-29-x.md",
            "# Plan\n\n## Implementation review\n\nVerdict: approve\n",
        )],
    );
    assert_eq!(before, w.sha(&repo), "docs/plans/ is not part of the id");

    // A code change is a different tree.
    w.commit(&repo, &[("src/lib.rs", "fn a() { let _ = 1; }\n")]);
    let after = w.sha(&repo);
    assert!(is_sha256(&after));
    assert_ne!(before, after);

    // A rev is honoured, and the id of the earlier commit is the same as it
    // was when it was HEAD.
    let (code, out, _) = w.run(&repo, &["HEAD~2"]);
    assert_eq!(code, 0);
    assert_eq!(out, before);
}

#[test]
fn a_sha256_repository_gets_the_same_width_and_a_worktree_the_parents_name() {
    let w = World::new("formats");
    let repo = w.repo("app", Some("sha256"));
    let sha = w.sha(&repo);
    assert!(is_sha256(&sha), "{sha}");

    // A tree oid is 64 hex here too, so the id must not be mistaken for one.
    let tree = git(&repo, &["rev-parse", "HEAD^{tree}"]);
    assert_ne!(
        tree, sha,
        "the id is a hash of the listing, not the tree oid"
    );

    let (code, block, err) = w.run(&repo, &["--block"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(block, format!("<<<TREE repo=app sha={sha}>>>"));

    // From a worktree named after the task, the block still names the repo.
    let wt = w.root.join("app-wt-task");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            &wt.display().to_string(),
            "-b",
            "feat/x",
        ],
    );
    let (code, block, err) = w.run(&wt, &["--block"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(block, format!("<<<TREE repo=app sha={sha}>>>"));
    let (code, from_c, _) = w.run(&w.root, &["-C", &wt.display().to_string(), "--block"]);
    assert_eq!(code, 0);
    assert_eq!(from_c, block, "-C answers for that repository");
}

#[test]
fn the_exit_codes_tell_the_failures_apart() {
    let w = World::new("exit");
    let repo = w.repo("app", None);

    let (code, out, err) = w.run(&w.root, &[]);
    assert_eq!(code, 1, "outside a repository");
    assert!(out.is_empty());
    assert!(err.contains("not inside a git repository"), "{err}");

    let (code, _, err) = w.run(&repo, &["no-such-rev"]);
    assert_eq!(code, 1);
    assert!(err.contains("does not name a tree"), "{err}");

    let (code, _, err) = w.run(&repo, &["--bogus"]);
    assert_eq!(code, 2);
    assert!(err.contains("unknown flag"), "{err}");

    let (code, _, err) = w.run(&repo, &["HEAD", "HEAD~1"]);
    assert_eq!(code, 2);
    assert!(err.contains("one rev"), "{err}");

    let (code, out, _) = w.run(&repo, &["--help"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("usage: amont-agent tree-sha"));

    // `--` ends the flags: a rev that starts with a dash is still a rev.
    let (code, _, err) = w.run(&repo, &["--", "-bad"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("does not name a tree"), "{err}");
}
