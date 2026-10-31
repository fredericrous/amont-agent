//! `amont-agent board` through the real binary, on scratch repositories and a
//! scratch CLAUDE_CONFIG_DIR (never the real ~/.claude).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BIN: &str = env!("CARGO_BIN_EXE_amont-agent");
const SESSION_ONE: &str = "11111111-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const SESSION_TWO: &str = "22222222-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const PLAN: &str = "---\nstatus: active\nrepos: [a, b]\n---\n# Board plan\n\n- [ ] Phase 1\n";

fn epoch_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn git_at(dir: &Path, args: &[&str], seconds_ago: u64) -> String {
    let date = format!("{} +0000", epoch_now() - seconds_ago);
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git(dir: &Path, args: &[&str]) -> String {
    git_at(dir, args, 0)
}

fn commit_file(dir: &Path, file: &str, text: &str, seconds_ago: u64) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    git(dir, &["add", file]);
    git_at(
        dir,
        &["commit", "-q", "-m", &format!("add {file}")],
        seconds_ago,
    );
}

struct World {
    top: PathBuf,
    root: PathBuf,
    config: PathBuf,
}

impl World {
    fn new(tag: &str) -> World {
        let top = std::env::temp_dir().join(format!("amont-board-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top); // a missing directory is the normal first run
        std::fs::create_dir_all(top.join("root")).unwrap();
        std::fs::create_dir_all(top.join("config")).unwrap();
        let top = std::fs::canonicalize(&top).unwrap();
        World {
            root: top.join("root"),
            config: top.join("config"),
            top,
        }
    }

    fn repo(&self, name: &str) -> PathBuf {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        commit_file(&dir, "README.md", "x\n", 10 * 86_400);
        dir
    }

    fn activity(&self, session: &str, repo: &Path, worktree: &Path, branch: &str, seen_ago: u64) {
        let sessions = self.config.join("amont-agent/sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let file = sessions.join(format!("{session}.activity"));
        let identity = repo.join(".git");
        std::fs::write(
            &file,
            format!(
                "{} {} {} {branch}\n",
                epoch_now() - seen_ago,
                identity.display(),
                worktree.display()
            ),
        )
        .unwrap();
        let handle = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
        handle
            .set_modified(SystemTime::now() - Duration::from_secs(seen_ago))
            .unwrap();
    }

    fn board(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(BIN)
            .current_dir(cwd)
            .arg("board")
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("CLAUDE_CONFIG_DIR", &self.config)
            .env("AMONT_AGENT_PLAN_ROOT", &self.root)
            .output()
            .expect("runs")
    }

    /// Runs `board` with a `git` shim first on PATH; returns the output and
    /// the log of every git invocation's arguments.
    fn board_with_git_log(&self, args: &[&str]) -> (Output, String) {
        let real_git = Command::new("which").arg("git").output().unwrap().stdout;
        let real_git = String::from_utf8(real_git).unwrap().trim().to_string();
        let shim_dir = self.top.join("shim");
        std::fs::create_dir_all(&shim_dir).unwrap();
        let log = self.top.join("git-calls.log");
        let shim = shim_dir.join("git");
        std::fs::write(
            &shim,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\nexec '{real_git}' \"$@\"\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!(
            "{}:{}",
            shim_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = Command::new(BIN)
            .current_dir(&self.top)
            .arg("board")
            .args(args)
            .env("PATH", path)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("CLAUDE_CONFIG_DIR", &self.config)
            .env("AMONT_AGENT_PLAN_ROOT", &self.root)
            .output()
            .expect("runs");
        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        (out, calls)
    }

    fn stdout(&self, args: &[&str]) -> String {
        let out = self.board(&self.top, args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

struct Scene {
    world: World,
    a: PathBuf,
    plan_tree: PathBuf,
    other_tree: PathBuf,
    merge_sha: String,
}

fn scene(tag: &str) -> Scene {
    let world = World::new(tag);
    let a = world.repo("a");
    let plan_tree = world.top.join("a-plan");
    let other_tree = world.top.join("a-other");
    let add_tree = |branch: &str, tree: &Path| {
        git(
            &a,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                tree.to_str().unwrap(),
            ],
        );
    };
    add_tree("feat/plan", &plan_tree);
    commit_file(&plan_tree, "docs/plans/2026-10-31-board.md", PLAN, 60);
    add_tree("feat/other", &other_tree);
    commit_file(&other_tree, "other.txt", "o\n", 60);
    git(&a, &["branch", "prog/x/t02", "feat/other"]);
    git(&a, &["checkout", "-q", "-b", "old/stale"]);
    commit_file(&a, "stale.txt", "s\n", 9 * 86_400);
    git(&a, &["checkout", "-q", "main"]);
    git(&a, &["checkout", "-q", "-b", "feat/merged"]);
    commit_file(&a, "merged.txt", "m\n", 4 * 3600);
    git(&a, &["checkout", "-q", "main"]);
    git_at(
        &a,
        &[
            "merge",
            "--no-ff",
            "-q",
            "-m",
            "land feature",
            "feat/merged",
        ],
        3 * 3600 + 630,
    );
    let merge_sha = git(&a, &["rev-parse", "--short", "HEAD"]);
    world.activity(SESSION_ONE, &a, &plan_tree, "feat/plan", 630);
    world.activity(SESSION_TWO, &a, &other_tree, "feat/other", 2 * 3600 + 330);
    world.repo("b");
    Scene {
        world,
        a,
        plan_tree,
        other_tree,
        merge_sha,
    }
}

#[test]
fn help_exits_zero_and_a_bad_flag_exits_two() {
    let world = World::new("help");
    let both = world.board(&world.top, &["--bogus", "--help"]);
    assert_eq!(both.status.code(), Some(0));
    let help = world.board(&world.top, &["--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&help.stdout).contains("amont-agent board"));
    assert_eq!(world.board(&world.top, &["--bogus"]).status.code(), Some(2));
    assert_eq!(
        world.board(&world.top, &["--since", "soon"]).status.code(),
        Some(2)
    );
}

#[test]
fn the_plain_board_of_a_repository_is_written_word_for_word() {
    let scene = scene("plain");
    let got = scene.world.stdout(&["--repo", "a"]);
    let want = format!(
        "a ({a})
  in flight:
    {other_tree} [feat/other]
      session {two} (seen 2h 5m ago)
    {plan_tree} [feat/plan]
      plan: Board plan
      session {one} (seen 10m ago)
    (no worktree) [prog/x/t02]
      programme x ticket t02
  just landed (last 2d):
    {sha} land feature (3h 10m ago)
",
        a = scene.a.display(),
        plan_tree = scene.plan_tree.display(),
        other_tree = scene.other_tree.display(),
        one = SESSION_ONE,
        two = SESSION_TWO,
        sha = scene.merge_sha,
    );
    assert_eq!(got, want);
    assert!(
        !got.contains("old/stale"),
        "a stale unmerged branch is left out"
    );
    assert!(
        !got.contains("feat/merged"),
        "a merged branch is not in flight"
    );
}

#[test]
fn a_plan_naming_two_repositories_is_listed_under_each() {
    let scene = scene("both");
    let got = scene.world.stdout(&["--repo", "b"]);
    assert!(got.contains("[a:feat/plan]"), "{got}");
    assert!(got.contains("plan: Board plan"), "{got}");
    assert!(
        !got.contains(SESSION_ONE),
        "sessions of a are not b's: {got}"
    );
}

#[test]
fn the_json_carries_the_same_facts() {
    let scene = scene("json");
    let parsed: serde_json::Value =
        serde_json::from_str(&scene.world.stdout(&["--repo", "a", "--json"])).expect("json");
    let rows = parsed.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["repo"], "a");
    assert_eq!(rows[0]["path"], scene.a.display().to_string());
    let trees = rows[0]["worktrees"].as_array().unwrap();
    let branches: Vec<&str> = trees
        .iter()
        .map(|t| t["branch"].as_str().unwrap())
        .collect();
    assert_eq!(branches, ["feat/other", "feat/plan", "prog/x/t02"]);
    assert_eq!(trees[1]["path"], scene.plan_tree.display().to_string());
    assert_eq!(trees[1]["plans"], serde_json::json!(["Board plan"]));
    assert_eq!(trees[1]["sessions"][0]["id"], SESSION_ONE);
    let seen = trees[1]["sessions"][0]["last_seen_secs"].as_u64().unwrap();
    assert!((630..690).contains(&seen), "{seen}");
    assert_eq!(trees[0]["plans"], serde_json::json!([]));
    assert_eq!(trees[0]["sessions"][0]["id"], SESSION_TWO);
    assert_eq!(trees[2]["path"], serde_json::Value::Null);
    assert_eq!(trees[2]["programme_ticket"], "x/t02");
    let landed = rows[0]["landed"].as_array().unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(landed[0]["sha"], scene.merge_sha);
    assert_eq!(landed[0]["subject"], "land feature");
}

#[test]
fn an_unnamed_repository_with_nothing_to_show_prints_nothing() {
    let world = World::new("quiet");
    world.repo("q");
    assert_eq!(world.stdout(&[]), "");
    let named = world.stdout(&["--repo", "q"]);
    assert!(named.contains("nothing in flight"), "{named}");
}

/// 60 unmerged branches whose tip is `seconds_ago` old, the first ten of
/// which carry an active plan.
fn stale_branches(repo: &Path, seconds_ago: u64) {
    git(repo, &["checkout", "-q", "-b", "stale-base"]);
    commit_file(repo, "stale.txt", "s\n", seconds_ago);
    git(repo, &["checkout", "-q", "-b", "stale-plan"]);
    commit_file(repo, "docs/plans/2026-01-01-old.md", PLAN, seconds_ago);
    git(repo, &["checkout", "-q", "main"]);
    for n in 0..60 {
        let from = if n < 10 { "stale-plan" } else { "stale-base" };
        git(repo, &["branch", &format!("stale/{n}"), from]);
    }
    git(repo, &["branch", "-q", "-D", "stale-base", "stale-plan"]);
}

#[test]
fn stale_branches_cost_no_git_call_beyond_the_listing() {
    let world = World::new("stale");
    let a = world.repo("a");
    let b = world.repo("b");
    stale_branches(&a, 9 * 86_400);
    stale_branches(&b, 9 * 86_400);
    git(&a, &["checkout", "-q", "-b", "feat/live"]);
    commit_file(&a, "docs/plans/2026-10-31-live.md", PLAN, 60);
    git(&a, &["checkout", "-q", "main"]);

    let (out, log) = world.board_with_git_log(&["--repo", "a"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("[feat/live]"), "{text}");
    assert!(text.contains("plan: Board plan"), "{text}");
    assert!(!text.contains("stale/"), "{text}");
    assert!(!text.contains("b:"), "{text}");
    let calls = log.lines().count();
    assert!(calls < 30, "{calls} git calls");
}

#[test]
fn siblings_cost_one_call_and_linked_worktrees_are_not_scanned() {
    let world = World::new("siblings");
    let a = world.repo("a");
    for name in ["a-wt-a", "a-wt-b"] {
        let path = world.root.join(name).display().to_string();
        git(&a, &["worktree", "add", "-q", "-b", name, &path]);
    }
    for name in ["s1", "s2", "s3"] {
        let sibling = world.repo(name);
        stale_branches(&sibling, 9 * 86_400);
    }
    let (out, log) = world.board_with_git_log(&["--repo", "a"]);
    assert!(out.status.success());
    for worktree in ["a-wt-a", "a-wt-b"] {
        let inside = world.root.join(worktree).display().to_string();
        assert!(
            !log.lines().any(|l| l.contains(&inside)),
            "scanned {worktree}:\n{log}"
        );
    }
    for sibling in ["s1", "s2", "s3"] {
        let inside = world.root.join(sibling).display().to_string();
        let calls = log.lines().filter(|l| l.contains(&inside)).count();
        assert!(calls <= 1, "{calls} git calls for {sibling}:\n{log}");
    }
}
