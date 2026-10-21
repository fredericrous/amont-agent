//! `plan-review stats` over a fixture ledger and `implementation-review
//! record`, through the real binary.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn root(name: &str) -> PathBuf {
    let r = std::env::temp_dir().join(format!("amont-agent-stats-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&r);
    std::fs::create_dir_all(r.join("claude/amont-agent/plan-review")).unwrap();
    std::fs::create_dir_all(r.join("claude/amont-agent/implementation-review-outcomes")).unwrap();
    r
}

fn bin(root: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_amont-agent"));
    c.env("CLAUDE_CONFIG_DIR", root.join("claude"))
        .env_remove("AMONT_AGENT_OFF");
    c
}

fn review(role: &str, model: &str, findings: &str) -> String {
    format!(
        "{{\"schema\":\"amont-agent-review-outcome-v1\",\"role\":\"{role}\",\"model\":\"{model}\",\"findings\":{findings}}}\n"
    )
}

#[test]
fn stats_gives_a_row_per_role_and_model_and_marks_one_retire_candidate() {
    let r = root("stats");
    let mut plan = String::new();
    // 20 reviews, nothing serious accepted: a retire candidate.
    for _ in 0..20 {
        plan += &review(
            "tui",
            "m1",
            r#"[{"severity":"low","disposition":"accepted"}]"#,
        );
    }
    // Two reviews and a serious accepted finding: kept.
    plan += &review(
        "security",
        "m1",
        r#"[{"severity":"high","disposition":"accepted"},{"severity":"medium","disposition":"rejected"}]"#,
    );
    plan += &review("security", "m1", "[]");
    plan += &review("security", "m2", "[]");
    // A merge row is not a review.
    plan += "{\"schema\":\"amont-agent-review-merge-v1\",\"role\":\"tui\",\"model\":\"m1\"}\n";
    std::fs::write(
        r.join("claude/amont-agent/plan-review/outcomes.jsonl"),
        plan,
    )
    .unwrap();
    std::fs::write(
        r.join("claude/amont-agent/implementation-review-outcomes/outcomes.jsonl"),
        "{\"schema\":\"amont-agent-impl-review-outcome-v1\",\"model\":\"m1\",\"findings\":[{\"severity\":\"blocking\",\"disposition\":\"fixed\"}]}\n",
    )
    .unwrap();

    let out = bin(&r).args(["plan-review", "stats"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{text}");
    assert_eq!(
        lines[0],
        "implementation-review\tm1\treviews=1\tfound=1/0/0/0\taccepted=1/0/0/0\tserious/review=1.00"
    );
    assert_eq!(
        lines[1],
        "security\tm1\treviews=2\tfound=0/1/1/0\taccepted=0/1/0/0\tserious/review=0.50"
    );
    assert_eq!(
        lines[2],
        "security\tm2\treviews=1\tfound=0/0/0/0\taccepted=0/0/0/0\tserious/review=0.00"
    );
    assert_eq!(
        lines[3],
        "tui\tm1\treviews=20\tfound=0/0/0/20\taccepted=0/0/0/20\tserious/review=0.00\tretire-candidate"
    );
    assert_eq!(text.matches("retire-candidate").count(), 1);

    let out = bin(&r)
        .args(["plan-review", "stats", "--json", "--model", "m2"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["role"], "security");
    assert_eq!(v[0]["retire_candidate"], false);
}

fn git(repo: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(repo)
        .args([
            "-c",
            "user.email=a@b",
            "-c",
            "user.name=t",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn record(r: &Path, repo: &Path, args: &[&str], stdin: &str) -> std::process::Output {
    let mut c = bin(r)
        .current_dir(repo)
        .args(["implementation-review", "record"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    c.wait_with_output().unwrap()
}

#[test]
fn record_appends_one_v1_row_and_refuses_a_bad_disposition() {
    let r = root("record");
    let repo = r.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-q", "-m", "a"]);
    let plan = std::fs::canonicalize(&repo).unwrap().join("p.md");
    std::fs::write(&plan, "# p\n").unwrap();
    let plan = plan.to_str().unwrap();
    let ledger = r.join("claude/amont-agent/implementation-review-outcomes/outcomes.jsonl");

    let bad = record(
        &r,
        &repo,
        &[plan, "--verdict", "approve"],
        r#"[{"severity":"high","disposition":"maybe"}]"#,
    );
    assert_eq!(bad.status.code(), Some(2));
    assert!(!ledger.exists());

    let ok = record(
        &r,
        &repo,
        &[plan, "--verdict", "approve-with-changes"],
        r#"[{"severity":"high","disposition":"fixed","section":"Verification"}]"#,
    );
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let text = std::fs::read_to_string(&ledger).unwrap();
    assert_eq!(text.lines().count(), 1);
    let row: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(row["schema"], "amont-agent-impl-review-outcome-v1");
    assert_eq!(row["round"], 1);
    assert_eq!(row["verdict"], "approve-with-changes");
    assert_eq!(row["model"], "");
    assert_eq!(row["plan_path"], plan);
    assert_eq!(row["tree"].as_str().unwrap().len(), 64);
    assert_eq!(row["findings"][0]["section"], "Verification");

    let again = record(&r, &repo, &[plan, "--verdict", "rework"], "");
    assert!(again.status.success());
    let rows = std::fs::read_to_string(&ledger).unwrap();
    assert!(rows.lines().nth(1).unwrap().contains("\"round\":2"));
}

/// A repo with one commit and a plan file; returns (repo, plan, tree).
fn fixture(r: &Path) -> (PathBuf, String, String) {
    let repo = r.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git(&repo, &["add", "a.txt"]);
    git(&repo, &["commit", "-q", "-m", "a"]);
    let plan = std::fs::canonicalize(&repo).unwrap().join("p.md");
    std::fs::write(&plan, "# p\n").unwrap();
    let out = bin(r).current_dir(&repo).arg("tree-sha").output().unwrap();
    let tree = String::from_utf8(out.stdout).unwrap().trim().to_string();
    (repo, plan.to_str().unwrap().to_string(), tree)
}

fn ledger_rows(r: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(
        r.join("claude/amont-agent/implementation-review-outcomes/outcomes.jsonl"),
    )
    .unwrap()
    .lines()
    .map(|l| serde_json::from_str(l).unwrap())
    .collect()
}

#[test]
fn the_ledger_lives_outside_the_hook_only_tree() {
    let r = root("path");
    let (repo, plan, _) = fixture(&r);
    let ok = record(&r, &repo, &[&plan, "--verdict", "approve"], "");
    assert!(ok.status.success());
    assert_eq!(ledger_rows(&r).len(), 1);
    assert!(!r
        .join("claude/amont-agent/implementation-review/outcomes.jsonl")
        .exists());
}

#[test]
fn only_the_validated_keys_of_a_finding_are_kept() {
    let r = root("keys");
    let (repo, plan, _) = fixture(&r);
    let ok = record(
        &r,
        &repo,
        &[&plan, "--verdict", "approve"],
        r#"[{"severity":"low","disposition":"fixed","extra":"x","section":"S"}]"#,
    );
    assert!(ok.status.success());
    assert_eq!(
        ledger_rows(&r)[0]["findings"][0],
        serde_json::json!({"severity":"low","disposition":"fixed","section":"S"})
    );
}

#[test]
fn the_model_is_the_review_agents_resolved_model_not_the_sessions() {
    let r = root("model");
    let (repo, plan, _) = fixture(&r);
    let cwd = std::fs::canonicalize(&repo).unwrap();
    // Claude Code's own layout, observed on 2026-10-22 under ~/.claude/projects:
    // `/Users/fredericrous/.local/state/relais` is stored as
    // `-Users-fredericrous--local-state-relais` — every byte that is not
    // ASCII alphanumeric becomes `-`. The expected name below is checked
    // against that observed pair first, so it does not lean on the code under
    // test.
    let project_name = |p: &str| -> String {
        p.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect()
    };
    assert_eq!(
        project_name("/Users/fredericrous/.local/state/relais"),
        "-Users-fredericrous--local-state-relais"
    );
    let name = project_name(&cwd.to_string_lossy());
    let dir = r.join("claude/projects").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let lines = [
        serde_json::json!({"type":"assistant","message":{"model":"model-Y","content":[
            {"type":"tool_use","id":"t1","name":"Agent","input":{"subagent_type":"implementation-review","prompt":"p"}}]}}),
        serde_json::json!({"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"Verdict: approve"}]}]},
            "toolUseResult":{"status":"completed","agentType":"implementation-review","agentId":"a1","resolvedModel":"model-X"}}),
        serde_json::json!({"type":"assistant","message":{"model":"model-Y","content":[{"type":"text","text":"done"}]}}),
    ];
    let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    std::fs::write(dir.join("s.jsonl"), text.join("\n") + "\n").unwrap();
    let ok = record(&r, &repo, &[&plan, "--verdict", "approve"], "");
    assert!(ok.status.success());
    assert_eq!(ledger_rows(&r)[0]["model"], "model-X");
}

fn remember_pass(r: &Path, repo: &Path, tree: &str, verdict: &str) {
    let name = std::fs::canonicalize(repo).unwrap();
    let name = name.file_name().unwrap().to_str().unwrap();
    let dir = r
        .join("claude/amont-agent/implementation-review/by-tree")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{tree}.json")),
        format!("{{\"verdict\":\"{verdict}\"}}"),
    )
    .unwrap();
}

#[test]
fn a_remembered_pass_that_agrees_warns_of_nothing() {
    let r = root("agree");
    let (repo, plan, tree) = fixture(&r);
    remember_pass(&r, &repo, &tree, "approve");
    let o = record(&r, &repo, &[&plan, "--verdict", "approve"], "");
    assert!(o.status.success());
    assert!(!String::from_utf8_lossy(&o.stderr).contains("remembered"));
}

#[test]
fn a_remembered_pass_that_disagrees_warns_on_stderr() {
    let r = root("disagree");
    let (repo, plan, tree) = fixture(&r);
    remember_pass(&r, &repo, &tree, "approve");
    let o = record(&r, &repo, &[&plan, "--verdict", "rework"], "");
    assert!(o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("remembered") && err.contains("approve"),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn a_failed_stdin_read_exits_1() {
    let r = root("stdin");
    let (repo, plan, _) = fixture(&r);
    let o = bin(&r)
        .current_dir(&repo)
        .args([
            "implementation-review",
            "record",
            &plan,
            "--verdict",
            "approve",
        ])
        .stdin(std::fs::File::open(&repo).unwrap())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("stdin cannot be read"));
}
