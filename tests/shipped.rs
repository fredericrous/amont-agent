//! `amont-agent shipped` through the real binary, on scratch repositories
//! with a local bare origin. No network and no `gh`: the origin is not
//! github.com, so the head of a squash is looked up in git only.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_amont-agent");
const PROV: &str = "refs/notes/amont-provenance";
const ATTEST: &str = "refs/notes/amont-attest";

fn run(dir: &Path, prog: &str, args: &[&str]) -> String {
    let out = Command::new(prog)
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("runs");
    assert!(
        out.status.success(),
        "{prog} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git(dir: &Path, args: &[&str]) -> String {
    run(dir, "git", args)
}

struct World {
    top: PathBuf,
    repo: PathBuf,
}

fn plan_text(n: u32) -> String {
    format!("---\nstatus: active\nrepos: [repo]\n---\n# Plan\n\nbody {n}\n")
}

impl World {
    fn new(tag: &str) -> World {
        let top = std::env::temp_dir().join(format!("amont-shipped-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        let repo = top.join("root").join("repo");
        let origin = top.join("origin.git");
        std::fs::create_dir_all(&repo).unwrap();
        git(
            &top,
            &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
        );
        git(&repo, &["init", "-b", "main"]);
        git(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        let w = World { top, repo };
        w.keygen("key");
        w.keygen("other");
        let pubkey = std::fs::read_to_string(w.top.join("key.pub")).unwrap();
        let signers = format!(
            "signer@example.com namespaces=\"amont-provenance\" {}\n",
            pubkey.trim()
        );
        std::fs::create_dir_all(w.repo.join(".github")).unwrap();
        std::fs::write(w.repo.join(".github/allowed_signers"), signers).unwrap();
        w.write_plan(1);
        git(&w.repo, &["add", "-A"]);
        git(&w.repo, &["commit", "-m", "init"]);
        git(&w.repo, &["push", "origin", "main"]);
        w
    }

    fn keygen(&self, name: &str) {
        std::fs::create_dir_all(&self.top).unwrap();
        run(
            &self.top,
            "ssh-keygen",
            &["-q", "-t", "ed25519", "-N", "", "-f", name],
        );
    }

    fn write_plan(&self, n: u32) {
        std::fs::create_dir_all(self.repo.join("docs/plans")).unwrap();
        std::fs::write(self.repo.join("docs/plans/p.md"), plan_text(n)).unwrap();
    }

    fn plan_file(&self) -> PathBuf {
        let p = self.top.join("p.md");
        std::fs::write(&p, plan_text(1)).unwrap();
        p
    }

    fn body_sha(&self, n: u32) -> String {
        let p = self.top.join("sha.md");
        std::fs::write(&p, plan_text(n)).unwrap();
        run(&self.top, BIN, &["plan-sha", p.to_str().unwrap()])
    }

    /// A feature branch from main's tip with a code change and plan body 2,
    /// pushed. Returns its head.
    fn feature(&self) -> String {
        git(&self.repo, &["checkout", "-q", "-b", "feature", "main"]);
        std::fs::write(self.repo.join("a.txt"), "a\n").unwrap();
        self.write_plan(2);
        git(&self.repo, &["add", "-A"]);
        git(&self.repo, &["commit", "-m", "feature"]);
        git(&self.repo, &["push", "origin", "feature"]);
        git(&self.repo, &["checkout", "-q", "main"]);
        git(&self.repo, &["rev-parse", "feature"])
    }

    fn squash(&self, subject: &str) -> String {
        git(&self.repo, &["merge", "--squash", "feature"]);
        git(&self.repo, &["commit", "-m", subject]);
        git(&self.repo, &["push", "origin", "main"]);
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    fn record(&self, commit: &str, plan_sha: &str, patch: Option<&str>) -> String {
        let tree = git(&self.repo, &["rev-parse", &format!("{commit}^{{tree}}")]);
        let mut s = format!("amont-provenance-v1\ncommit {commit}\ntree {tree}\ncanon c0ffee\n");
        if let Some(p) = patch {
            s.push_str(&format!("patch {p}\n"));
        }
        s.push_str(&format!(
            "plan docs/plans/p.md {plan_sha}\nreview reviewed c0ffee\npreview not-required\nproducer amont-agent 0\n"
        ));
        s
    }

    fn sign(&self, payload: &str, key: &str) -> String {
        let f = self.top.join("payload");
        let _ = std::fs::remove_file(self.top.join("payload.sig"));
        std::fs::write(&f, payload).unwrap();
        run(
            &self.top,
            "ssh-keygen",
            &["-Y", "sign", "-n", "amont-provenance", "-f", key, "payload"],
        );
        let sig = std::fs::read_to_string(self.top.join("payload.sig")).unwrap();
        format!("{payload}\n{sig}")
    }

    fn note(&self, note_ref: &str, object: &str, text: &str) {
        let f = self.top.join("note");
        std::fs::write(&f, text).unwrap();
        git(
            &self.repo,
            &[
                "notes",
                "--ref",
                note_ref,
                "add",
                "-f",
                "-F",
                f.to_str().unwrap(),
                object,
            ],
        );
        git(&self.repo, &["push", "-f", "origin", note_ref]);
    }

    /// Stdout, stderr and exit code. The newest merge is the first line; the
    /// `init` commit that created the plan is the last.
    fn shipped(&self, extra: &[&str]) -> (String, String, i32) {
        let plan = self.plan_file();
        let out = Command::new(BIN)
            .current_dir(&self.top)
            .arg("shipped")
            .arg(&plan)
            .args(extra)
            .env("AMONT_AGENT_PLAN_ROOT", self.top.join("root"))
            .env("CLAUDE_CONFIG_DIR", self.top.join("claude"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("AMONT_AGENT_GH", "/nonexistent/gh")
            .stdin(Stdio::null())
            .output()
            .expect("runs");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.top);
    }
}

#[test]
fn squash_with_pr_number_and_tag_is_verified_by_tree() {
    let w = World::new("squash");
    let head = w.feature();
    let merge = w.squash("feat: a (#7)");
    git(&w.repo, &["tag", "v1.10.0"]);
    git(&w.repo, &["tag", "v1.9.0"]);
    let tree = git(&w.repo, &["rev-parse", &format!("{merge}^{{tree}}")]);
    let sha = w.body_sha(2);
    let signed = w.sign(&w.record(&head, &sha, None), "key");
    w.note(PROV, &tree, &signed);
    w.note(ATTEST, &tree, "gates cargo-test clippy\n");
    let (out, err, code) = w.shipped(&[]);
    let out = out.lines().next().unwrap_or("").to_string();
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("repo  #7  "), "{out}");
    assert!(out.contains("v1.9.0"), "first release by version: {out}");
    assert!(
        out.contains("review reviewed · preview not-required"),
        "{out}"
    );
    assert!(out.contains(&format!("plan {}", &sha[..12])), "{out}");
    assert!(out.contains("gates cargo-test clippy"), "{out}");
    assert!(out.trim_end().ends_with("verified"), "{out}");
}

#[test]
fn merge_commit_uses_its_second_parent_as_head() {
    let w = World::new("merge");
    let head = w.feature();
    git(
        &w.repo,
        &["merge", "--no-ff", "feature", "-m", "Merge (#9)"],
    );
    git(&w.repo, &["push", "origin", "main"]);
    let sha = w.body_sha(2);
    let signed = w.sign(&w.record(&head, &sha, None), "key");
    w.note(PROV, &head, &signed);
    let (out, err, code) = w.shipped(&["--json"]);
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("stdout is JSON");
    let row = &v[0];
    assert_eq!(row["head"], head.as_str());
    assert_eq!(row["pr"], 9);
    assert_eq!(row["release"], serde_json::Value::Null);
    assert_eq!(row["provenance"]["status"], "verified");
    assert_eq!(row["provenance"]["plan_body_sha"], sha.as_str());
    assert_eq!(row["provenance"]["flags"], serde_json::json!([]));
}

#[test]
fn squash_onto_a_moved_main_is_found_by_patch() {
    let w = World::new("patch");
    let head = w.feature();
    // main moves on after the branch point.
    std::fs::write(w.repo.join("b.txt"), "b\n").unwrap();
    git(&w.repo, &["add", "b.txt"]);
    git(&w.repo, &["commit", "-m", "other"]);
    let merge = w.squash("feat: a (#11)");
    let base = git(&w.repo, &["merge-base", "main", "feature"]);
    let diff = Command::new("git")
        .current_dir(&w.repo)
        .args(["diff", "--binary", &base, &head, "--", ".", ":!docs/plans"])
        .output()
        .unwrap()
        .stdout;
    let mut child = Command::new("git")
        .current_dir(&w.repo)
        .args(["patch-id", "--verbatim"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(&diff).unwrap();
    let out = child.wait_with_output().unwrap();
    let pid = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    assert_ne!(
        git(&w.repo, &["rev-parse", &format!("{merge}^{{tree}}")]),
        git(&w.repo, &["rev-parse", &format!("{head}^{{tree}}")])
    );
    let sha = w.body_sha(2);
    let signed = w.sign(&w.record(&head, &sha, Some(&pid)), "key");
    w.note(PROV, &head, &signed);
    let (out, err, code) = w.shipped(&[]);
    let out = out.lines().next().unwrap_or("").to_string();
    assert_eq!(code, 0, "{err}");
    assert!(
        out.trim_end().ends_with("verified"),
        "patch fallback: {out}"
    );
}

#[test]
fn a_key_missing_from_allowed_signers_is_not_allowed() {
    let w = World::new("stranger");
    let head = w.feature();
    let merge = w.squash("feat: a (#3)");
    let tree = git(&w.repo, &["rev-parse", &format!("{merge}^{{tree}}")]);
    let signed = w.sign(&w.record(&head, &w.body_sha(2), None), "other");
    w.note(PROV, &tree, &signed);
    let (out, _, code) = w.shipped(&[]);
    let out = out.lines().next().unwrap_or("").to_string();
    assert_eq!(code, 0);
    assert!(
        out.trim_end().ends_with("signed, signer not allowed"),
        "{out}"
    );
}

#[test]
fn no_record_is_no_provenance() {
    let w = World::new("none");
    w.feature();
    w.squash("feat: a (#4)");
    let (out, _, code) = w.shipped(&[]);
    let out = out.lines().next().unwrap_or("").to_string();
    assert_eq!(code, 0);
    assert!(out.contains("not released"), "{out}");
    assert!(out.trim_end().ends_with("no provenance"), "{out}");
}

#[test]
fn a_plan_line_that_differs_from_its_tree_is_flagged() {
    let w = World::new("mismatch");
    let head = w.feature();
    let merge = w.squash("feat: a (#5)");
    let tree = git(&w.repo, &["rev-parse", &format!("{merge}^{{tree}}")]);
    let wrong = "a".repeat(64);
    let signed = w.sign(&w.record(&head, &wrong, None), "key");
    w.note(PROV, &tree, &signed);
    let (out, _, _) = w.shipped(&[]);
    let out = out.lines().next().unwrap_or("").to_string();
    assert!(out.contains("plan line does not match tree"), "{out}");
    assert!(out.contains("reviewed an earlier body"), "{out}");
}

#[test]
fn a_record_whose_commit_is_unreachable_is_ignored() {
    let w = World::new("unreachable");
    w.feature();
    let merge = w.squash("feat: a (#6)");
    let tree = git(&w.repo, &["rev-parse", &format!("{merge}^{{tree}}")]);
    let lonely = git(&w.repo, &["commit-tree", &tree, "-m", "never pushed"]);
    let signed = w.sign(&w.record(&lonely, &w.body_sha(2), None), "key");
    w.note(PROV, &tree, &signed);
    let (out, _, _) = w.shipped(&[]);
    let out = out.lines().next().unwrap_or("").to_string();
    assert!(out.trim_end().ends_with("no provenance"), "{out}");
}

#[test]
fn usage_errors_and_help() {
    let w = World::new("usage");
    let (_, _, code) = w.shipped(&["--nope"]);
    assert_eq!(code, 2);
    let help = Command::new(BIN)
        .args(["shipped", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("usage: amont-agent shipped"));
    let main_help = Command::new(BIN).arg("--help").output().unwrap();
    assert!(String::from_utf8_lossy(&main_help.stdout).contains("shipped <plan.md>"));
    let missing = Command::new(BIN)
        .args(["shipped", "/nonexistent/plan.md"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
}
