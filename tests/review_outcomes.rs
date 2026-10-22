//! `amont-agent plan-review record`, end to end through the real binary.
//!
//! The transcripts are built here in the shapes measured on real sessions
//! (2026-10-11): a foreground `Agent` result carries `agentId`,
//! `resolvedModel`, `totalTokens` and `totalDurationMs`; a background launch
//! carries `status: async_launched`, `agentId` and `resolvedModel`, and its
//! task notification carries `<task-id>` and a `<usage>` block.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BODY: &str = "# Plan\n\n## Review panel\n\n👉 Decide: none\n\n## Context\n\nThe work.\n";

struct World {
    root: PathBuf,
    plan: PathBuf,
    lines: Vec<String>,
    n: u32,
}

impl World {
    fn new(name: &str) -> World {
        let root = std::env::temp_dir().join(format!(
            "amont-agent-outcomes-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["claude", "plans", "repos"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let plan = std::fs::canonicalize(root.join("plans"))
            .unwrap()
            .join("p.md");
        let mut w = World {
            root,
            plan,
            lines: Vec::new(),
            n: 0,
        };
        w.set_body(BODY);
        w
    }

    fn set_body(&mut self, body: &str) {
        std::fs::write(&self.plan, format!("{body}\n<!-- panel: repos=docs -->\n")).unwrap();
    }

    fn bin(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_amont-agent"));
        c.env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("AMONT_AGENT_PLAN_ROOT", self.root.join("repos"))
            .env_remove("AMONT_AGENT_OFF");
        c
    }

    fn block(&self) -> String {
        let out = self
            .bin()
            .args(["plan-sha", "--block"])
            .arg(&self.plan)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn launch(&mut self, agent: &str) -> String {
        self.n += 1;
        let id = format!("toolu_{}", self.n);
        self.lines.push(
            serde_json::json!({"type": "assistant", "message": {"role": "assistant", "content": [{
                "type": "tool_use", "id": id, "name": "Agent",
                "input": {"subagent_type": agent, "prompt": format!("Review.\n{}\n", self.block())}
            }]}})
            .to_string(),
        );
        id
    }

    fn result(&mut self, id: &str, tur: serde_json::Value, text: &str) {
        self.lines.push(
            serde_json::json!({"type": "user", "message": {"role": "user", "content": [{
                "type": "tool_result", "tool_use_id": id, "content": [{"type": "text", "text": text}]
            }]}, "toolUseResult": tur})
            .to_string(),
        );
    }

    /// A foreground review; `agent_id` None omits it from the result.
    fn foreground(&mut self, role: &str, agent_id: Option<&str>) {
        let agent = format!("plan-review-{role}");
        let id = self.launch(&agent);
        let mut tur = serde_json::json!({"status": "completed", "agentType": agent,
            "resolvedModel": "claude-opus-4-7", "totalTokens": 5000, "totalDurationMs": 12500});
        if let Some(a) = agent_id {
            tur["agentId"] = a.into();
        }
        self.result(&id, tur, "Verdict: Approve-With-Changes\nfine");
    }

    /// A foreground review of the plan as it is now, answering `text`.
    fn reviewed(&mut self, role: &str, agent_id: &str, text: &str) {
        let agent = format!("plan-review-{role}");
        let id = self.launch(&agent);
        let tur = serde_json::json!({"status": "completed", "agentType": agent,
            "agentId": agent_id, "resolvedModel": "claude-opus-4-7",
            "totalTokens": 5000, "totalDurationMs": 12500});
        self.result(&id, tur, text);
    }

    /// A round resumed with `SendMessage` on the plan as it is now.
    fn resume(&mut self, agent_id: &str, text: &str) {
        let id = "toolu_send".to_string();
        let message = format!("Round 2.\n{}\n", self.block());
        self.lines.push(
            serde_json::json!({"type": "assistant", "message": {"role": "assistant", "content": [{
                "type": "tool_use", "id": id, "name": "SendMessage",
                "input": {"to": agent_id, "message": message}
            }]}})
            .to_string(),
        );
        self.result(
            &id,
            serde_json::json!({"resumedAgentId": agent_id}),
            "resumed",
        );
        let note = format!(
            "<task-notification>\n<task-id>{agent_id}</task-id>\n<tool-use-id>{id}</tool-use-id>\n<status>completed</status>\n<result>{text}</result>\n<usage><subagent_tokens>900</subagent_tokens><tool_uses>1</tool_uses><duration_ms>2000</duration_ms></usage>\n</task-notification>"
        );
        self.lines.push(
            serde_json::json!({"type": "user", "origin": {"kind": "task-notification"},
                "message": {"role": "user", "content": note}})
            .to_string(),
        );
    }

    fn background(&mut self, role: &str, agent_id: &str) {
        let id = self.launch(&format!("plan-review-{role}"));
        self.result(
            &id,
            serde_json::json!({"status": "async_launched", "isAsync": true,
                "agentId": agent_id, "resolvedModel": "claude-sonnet-5-5"}),
            "launched",
        );
        let note = format!(
            "<task-notification>\n<task-id>{agent_id}</task-id>\n<tool-use-id>{id}</tool-use-id>\n<status>completed</status>\n<result>Verdict: rework. Not approve.</result>\n<usage><subagent_tokens>36170</subagent_tokens><tool_uses>13</tool_uses><duration_ms>211987</duration_ms></usage>\n</task-notification>"
        );
        self.lines.push(
            serde_json::json!({"type": "user", "origin": {"kind": "task-notification"},
                "message": {"role": "user", "content": note}})
            .to_string(),
        );
    }

    fn transcript(&self) -> PathBuf {
        let t = self.root.join("t.jsonl");
        std::fs::write(&t, self.lines.join("\n") + "\n").unwrap();
        t
    }

    fn record(&self, stdin: &str) -> std::process::Output {
        let t = self.transcript();
        self.record_with(&t, stdin)
    }

    fn record_with(&self, t: &Path, stdin: &str) -> std::process::Output {
        let mut c = self
            .bin()
            .args(["plan-review", "record"])
            .arg(&self.plan)
            .arg("--transcript")
            .arg(t)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        c.wait_with_output().unwrap()
    }

    fn store(&self) -> PathBuf {
        self.root
            .join("claude")
            .join("amont-agent")
            .join("plan-review")
    }

    fn rows(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.store().join("outcomes.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

fn out(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn err(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn foreground_and_background_launches_both_yield_rows() {
    let mut w = World::new("both");
    w.foreground("backend", Some("agent-fg"));
    w.background("architect", "agent-bg");
    let o = w.record("");
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(
        err(&o).contains("wrote 2 rows (round 1, plan "),
        "{}",
        err(&o)
    );
    let rows = w.rows();
    let by = |r: &str| rows.iter().find(|x| x["role"] == r).unwrap().clone();
    let fg = by("backend");
    assert_eq!(fg["agent_id"], "agent-fg");
    assert_eq!(fg["model"], "claude-opus-4-7");
    assert_eq!(fg["tokens"], 5000);
    assert_eq!(fg["seconds"].as_f64(), Some(12.5));
    assert_eq!(fg["verdict"], "approve-with-changes");
    let bg = by("architect");
    assert_eq!(bg["agent_id"], "agent-bg");
    assert_eq!(bg["model"], "claude-sonnet-5-5");
    assert_eq!(bg["tokens"], 36170);
    assert_eq!(bg["seconds"].as_f64(), Some(211.987));
    assert_eq!(bg["verdict"], "rework");
    assert_eq!(bg["remit"], serde_json::json!(["large"]));
    assert_eq!(fg["remit"], serde_json::json!(["*"]));
}

#[test]
fn a_row_has_every_field_and_the_files_are_private() {
    let mut w = World::new("shape");
    w.foreground("backend", Some("a1"));
    let o = w.record(
        r#"[{"role":"backend","severity":"high","disposition":"accepted","section":"Context"}]"#,
    );
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let rows = w.rows();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    for k in [
        "schema",
        "recorded_at",
        "role",
        "agent_id",
        "model",
        "tokens",
        "seconds",
        "round",
        "body_sha",
        "first_round_sha",
        "plan_path",
        "plan",
        "verdict",
        "remit",
        "findings",
    ] {
        assert!(r.get(k).is_some(), "missing {k}: {r}");
    }
    assert_eq!(r["schema"], "amont-agent-review-outcome-v1");
    assert_eq!(r["round"], 1);
    assert_eq!(r["body_sha"], r["first_round_sha"]);
    assert_eq!(r["plan_path"], w.plan.to_string_lossy().as_ref());
    for k in ["areas", "repos", "body_lines"] {
        assert!(r["plan"].get(k).is_some(), "plan.{k}");
    }
    assert_eq!(r["plan"]["repos"], serde_json::json!(["docs"]));
    assert_eq!(r["findings"][0]["section"], "Context");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&w.store()), 0o700);
        assert_eq!(mode(&w.store().join("bodies")), 0o700);
        assert_eq!(mode(&w.store().join("outcomes.jsonl")), 0o600);
    }
}

#[test]
fn a_second_run_writes_nothing() {
    let mut w = World::new("again");
    w.foreground("backend", Some("a1"));
    assert_eq!(w.record("[]").status.code(), Some(0));
    let o = w.record("[]");
    assert_eq!(o.status.code(), Some(0));
    assert!(
        err(&o).contains("already recorded round 1 of "),
        "{}",
        err(&o)
    );
    assert!(err(&o).contains("; nothing written\n"));
    assert_eq!(w.rows().len(), 1);
}

// The lock is an flock; Windows has none yet (see `lock` in
// src/review_outcomes.rs).
#[cfg(unix)]
#[test]
fn concurrent_records_write_one_row_per_reviewer() {
    let mut w = World::new("race");
    for (i, r) in ["backend", "architect", "po"].iter().enumerate() {
        w.foreground(r, Some(&format!("a{i}")));
    }
    let t = w.transcript();
    let kids: Vec<_> = (0..6)
        .map(|_| {
            w.bin()
                .args(["plan-review", "record"])
                .arg(&w.plan)
                .arg("--transcript")
                .arg(&t)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for k in kids {
        let o = k.wait_with_output().unwrap();
        assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    }
    assert_eq!(w.rows().len(), 3);
}

#[test]
fn the_body_is_saved_once() {
    let mut w = World::new("body");
    w.foreground("backend", Some("a1"));
    assert_eq!(w.record("[]").status.code(), Some(0));
    let sha = w.rows()[0]["body_sha"].as_str().unwrap().to_string();
    let saved = w.store().join("bodies").join(format!("{sha}.md"));
    assert_eq!(
        std::fs::read_to_string(&saved).unwrap(),
        std::fs::read_to_string(&w.plan).unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    std::fs::write(&saved, "marker").unwrap();
    w.foreground("architect", Some("a2"));
    assert_eq!(w.record("[]").status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), "marker");
}

#[test]
fn a_changed_plan_body_is_not_saved() {
    let mut w = World::new("changed");
    w.foreground("backend", Some("a1"));
    let t = w.transcript();
    w.set_body(&format!("{BODY}\nAnd more words.\n"));
    let o = w.record_with(&t, "[]");
    assert_eq!(o.status.code(), Some(0));
    assert!(err(&o).contains("body of round 1 not saved: the plan changed since it was reviewed"));
    assert_eq!(
        std::fs::read_dir(w.store().join("bodies")).unwrap().count(),
        0
    );
}

#[test]
fn a_launch_without_an_agent_id_is_reported_after_the_rest_are_written() {
    let mut w = World::new("noid");
    w.foreground("backend", Some("a1"));
    w.foreground("architect", None);
    let o = w.record("[]");
    assert_eq!(o.status.code(), Some(1));
    assert!(
        err(&o).contains("amont-agent: no agent id for architect in "),
        "{}",
        err(&o)
    );
    let rows = w.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["role"], "backend");
}

#[test]
fn bad_stdin_is_a_usage_error() {
    let mut w = World::new("stdin");
    w.foreground("backend", Some("a1"));
    for bad in [
        "{",
        r#"{"role":"x"}"#,
        r#"[{"role":"backend","severity":"urgent","disposition":"accepted"}]"#,
        r#"[{"role":"backend","severity":"low","disposition":"maybe"}]"#,
    ] {
        let o = w.record(bad);
        assert_eq!(o.status.code(), Some(2), "{bad}: {}", err(&o));
        assert!(!err(&o).is_empty());
    }
    assert!(w.rows().is_empty());
}

#[test]
fn rounds_follow_the_order_bodies_first_appear() {
    let mut w = World::new("rounds");
    w.foreground("backend", Some("a1"));
    w.foreground("architect", Some("a2"));
    w.set_body(&format!("{BODY}\nRevised.\n"));
    w.foreground("backend", Some("a3"));
    let f = r#"[
        {"role":"backend","severity":"low","disposition":"rejected"},
        {"role":"architect","severity":"high","disposition":"decision"}
    ]"#;
    let o = w.record(f);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(
        err(&o).contains("wrote 3 rows (round 2, plan "),
        "{}",
        err(&o)
    );
    let rows = w.rows();
    let row = |id: &str| rows.iter().find(|r| r["agent_id"] == id).unwrap().clone();
    assert_eq!(row("a1")["round"], 1);
    assert_eq!(row("a2")["round"], 1);
    assert_eq!(row("a3")["round"], 2);
    assert_eq!(row("a1")["first_round_sha"], row("a3")["first_round_sha"]);
    assert_ne!(row("a1")["body_sha"], row("a3")["body_sha"]);
    assert_eq!(row("a1")["findings"], serde_json::json!([]));
    assert_eq!(row("a2")["findings"], serde_json::json!([]));
    assert_eq!(row("a3")["findings"].as_array().unwrap().len(), 1);
    assert_eq!(row("a3")["findings"][0]["disposition"], "rejected");
}

#[test]
fn help_and_usage() {
    let w = World::new("help");
    let o = w
        .bin()
        .args(["plan-review", "record", "--help"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert!(out(&o).contains("usage: amont-agent plan-review record"));
    let o = w.bin().args(["plan-review", "--help"]).output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    let o = w
        .bin()
        .args(["plan-review", "record"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn discovery_finds_the_transcript_that_reviewed_the_plan() {
    let mut w = World::new("discover");
    w.foreground("backend", Some("a1"));
    let proj = w.root.join("claude").join("projects").join("p");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("s.jsonl"), w.lines.join("\n") + "\n").unwrap();
    let mut c = w
        .bin()
        .args(["plan-review", "record"])
        .arg(&w.plan)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(c.status.code(), Some(0), "{}", err(&c));
    assert_eq!(w.rows().len(), 1);
    std::fs::remove_dir_all(w.root.join("claude").join("projects")).unwrap();
    c = w
        .bin()
        .args(["plan-review", "record"])
        .arg(&w.plan)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(c.status.code(), Some(1));
    assert!(err(&c).contains(&*w.plan.to_string_lossy()));
}

#[test]
fn discovery_prefers_the_session_that_reviewed_the_current_body() {
    // An older session that reviewed an earlier body is still being written
    // to, so its mtime is the newest; the review of the current body wins.
    let mut old = World::new("discover-body");
    old.foreground("backend", Some("stale"));
    let old_lines = old.lines.clone();
    old.lines.clear();
    old.set_body(&format!("{BODY}\nRevised.\n"));
    old.foreground("backend", Some("current"));
    let proj = old.root.join("claude").join("projects").join("p");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("new.jsonl"), old.lines.join("\n") + "\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(proj.join("long.jsonl"), old_lines.join("\n") + "\n").unwrap();
    let o = old
        .bin()
        .args(["plan-review", "record"])
        .arg(&old.plan)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let ids: Vec<_> = old.rows().iter().map(|r| r["agent_id"].clone()).collect();
    assert_eq!(ids, vec![serde_json::json!("current")]);
}

#[test]
fn findings_go_to_the_highest_round_this_call_writes() {
    // Round 2's reviewer is already on record; this call writes a round-1
    // reviewer alone, and the merge's findings belong to it.
    let mut w = World::new("attach");
    w.foreground("architect", Some("a1"));
    w.set_body(&format!("{BODY}\nRevised.\n"));
    w.foreground("backend", Some("a2"));
    let first = w.lines.clone();
    w.lines = first[2..].to_vec();
    assert_eq!(w.record("[]").status.code(), Some(0));
    w.lines = first;
    let o = w.record(r#"[{"role":"architect","severity":"high","disposition":"accepted"}]"#);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let rows = w.rows();
    let a1 = rows.iter().find(|r| r["agent_id"] == "a1").unwrap();
    assert_eq!(a1["findings"].as_array().unwrap().len(), 1);
}

#[test]
fn a_row_describes_the_body_its_reviewer_read() {
    let mut w = World::new("features");
    w.foreground("backend", Some("a1"));
    w.foreground("architect", Some("a2"));
    let all = w.lines.clone();
    w.lines = all[..2].to_vec();
    assert_eq!(w.record("[]").status.code(), Some(0));
    w.lines = all;
    w.set_body(&format!("{BODY}\nOne.\nTwo.\nThree.\n"));
    assert_eq!(w.record("[]").status.code(), Some(0));
    let rows = w.rows();
    let lines =
        |id: &str| rows.iter().find(|r| r["agent_id"] == id).unwrap()["plan"]["body_lines"].clone();
    assert_eq!(lines("a1"), lines("a2"));
}

#[test]
fn bodies_older_than_90_days_are_pruned() {
    let mut w = World::new("prune");
    w.foreground("backend", Some("a1"));
    let bodies = w.store().join("bodies");
    std::fs::create_dir_all(&bodies).unwrap();
    let old = bodies.join("old.md");
    std::fs::write(&old, "old").unwrap();
    let f = std::fs::File::options().write(true).open(&old).unwrap();
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(91 * 86_400))
        .unwrap();
    drop(f);
    assert_eq!(w.record("[]").status.code(), Some(0));
    assert!(!old.exists(), "a 91-day-old body is pruned");
    let kept: Vec<_> = std::fs::read_dir(&bodies).unwrap().collect();
    assert_eq!(kept.len(), 1, "the body this call saved stays");
}

#[test]
fn round_one_is_the_first_body_on_record_across_sessions() {
    let mut w = World::new("first-round");
    w.foreground("backend", Some("a1"));
    assert_eq!(w.record("[]").status.code(), Some(0));
    let first = w.rows()[0]["body_sha"].clone();
    // A new session reviews the revised body; its transcript has no round 1.
    w.lines.clear();
    w.set_body(&format!("{BODY}\nRevised.\n"));
    w.foreground("backend", Some("a2"));
    assert_eq!(w.record("[]").status.code(), Some(0));
    let rows = w.rows();
    let a2 = rows.iter().find(|r| r["agent_id"] == "a2").unwrap();
    assert_eq!(a2["first_round_sha"], first);
}

#[test]
fn an_unsaved_body_records_no_features() {
    let mut w = World::new("unsaved");
    w.foreground("backend", Some("a1"));
    w.set_body(&format!("{BODY}\nRevised.\n"));
    assert_eq!(w.record("[]").status.code(), Some(0));
    assert_eq!(w.rows()[0]["plan"], serde_json::Value::Null);
}

#[test]
fn transcript_takes_the_equals_form_and_double_dash_ends_options() {
    let mut w = World::new("posix");
    w.foreground("backend", Some("a1"));
    let t = w.transcript();
    let o = w
        .bin()
        .args(["plan-review", "record"])
        .arg(format!("--transcript={}", t.display()))
        .arg("--")
        .arg(&w.plan)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert_eq!(out(&o), "", "the status line is talk, on stderr");
    assert_eq!(w.rows().len(), 1);
}

#[test]
fn every_id_missing_says_no_rows_not_already_recorded() {
    let mut w = World::new("all-missing");
    w.foreground("backend", None);
    let o = w.record("[]");
    assert_eq!(o.status.code(), Some(1));
    assert!(
        err(&o).contains("no rows written for round 1"),
        "{}",
        err(&o)
    );
    assert!(!err(&o).contains("already recorded"));
}

#[test]
fn the_last_transcript_flag_wins() {
    let mut w = World::new("last-wins");
    w.foreground("backend", Some("a1"));
    let t = w.transcript();
    let o = w
        .bin()
        .args(["plan-review", "record"])
        .arg("--transcript=/nonexistent/first.jsonl")
        .arg("--transcript")
        .arg(&t)
        .arg(&w.plan)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert_eq!(w.rows().len(), 1);
}

const TWO: &str = "Verdict: approve-with-changes\n\n**Findings:**\n1. [high] A. Edit: x.\n2. [low] B.\n3. bad line\n\n**Patches:**\n";

#[test]
fn stdin_findings_win_for_their_role_and_the_rest_are_parsed() {
    let mut w = World::new("parsed");
    w.reviewed("backend", "ag-b", TWO);
    w.reviewed("architect", "ag-a", TWO);
    let o = w.record(r#"[{"role":"backend","severity":"blocking","disposition":"rejected"}]"#);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let rows = w.rows();
    let by = |r: &str| rows.iter().find(|x| x["role"] == r).unwrap().clone();
    let b = by("backend");
    assert_eq!(b["findings"].as_array().unwrap().len(), 1);
    assert_eq!(b["findings"][0]["disposition"], "rejected");
    assert!(b.get("findings_unparsed").is_none());
    let a = by("architect");
    assert_eq!(
        a["findings"],
        serde_json::json!([{"n": 1, "severity": "high"}, {"n": 2, "severity": "low"}])
    );
    assert_eq!(a["findings_unparsed"], 1);
}

#[test]
fn a_resumed_round_on_a_changed_body_gets_its_own_row() {
    let mut w = World::new("resume-changed");
    w.reviewed("backend", "ag-b", TWO);
    w.set_body(&format!("{BODY}\nMore.\n"));
    w.resume("ag-b", TWO);
    let o = w.record("");
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let rows = w.rows();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0]["round"], 1);
    assert_eq!(rows[1]["round"], 2);
    assert_ne!(rows[0]["body_sha"], rows[1]["body_sha"]);
}

#[test]
fn a_resumed_round_on_an_unchanged_body_writes_nothing_more() {
    let mut w = World::new("resume-same");
    w.reviewed("backend", "ag-b", TWO);
    w.resume("ag-b", TWO);
    let o = w.record("");
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert_eq!(w.rows().len(), 1);
}

#[test]
fn stdin_for_a_recorded_reviewer_appends_disposition_rows_only() {
    let mut w = World::new("disposition");
    w.reviewed("backend", "ag-b", TWO);
    assert_eq!(w.record("").status.code(), Some(0));
    let o = w.record(
        r#"[{"role":"backend","n":1,"severity":"high","disposition":"accepted"},{"role":"backend","n":2,"severity":"low","disposition":"rejected"}]"#,
    );
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let rows = w.rows();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[0]["schema"], "amont-agent-review-outcome-v1");
    for (r, n, d) in [(&rows[1], 1, "accepted"), (&rows[2], 2, "rejected")] {
        assert_eq!(r["schema"], "amont-agent-review-disposition-v1");
        assert_eq!(r["agent_id"], "ag-b");
        assert_eq!(r["body_sha"], rows[0]["body_sha"]);
        assert_eq!(r["role"], "backend");
        assert_eq!(r["n"], n);
        assert_eq!(r["disposition"], d);
    }
}
