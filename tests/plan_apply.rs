//! `amont-agent plan-apply`, end to end through the real binary.
//!
//! The transcripts are built here in the shapes `tests/review_outcomes.rs`
//! uses; a reviewer's result carries a `Patches:` block in a four-backtick
//! fence, as the reviewer agents' output block specifies.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BODY: &str = "# Plan\n\n## Review panel\n\n👉 Decide: none\n\n## Context\n\nalpha beta\n\ndup\n\ndup\n\n## Behaviour\n\nthe gamma line\n\n```sh\necho hi\n```\n";

struct World {
    root: PathBuf,
    plan: PathBuf,
    lines: Vec<String>,
    n: u32,
}

fn patches(list: &[(u32, &str, &str)]) -> String {
    let json: Vec<_> = list
        .iter()
        .map(|(f, o, n)| serde_json::json!({"finding": f, "old_string": o, "new_string": n}))
        .collect();
    format!(
        "Verdict: approve-with-changes\nfine\n\nPatches:\n````json\n{}\n````\n",
        serde_json::Value::Array(json)
    )
}

impl World {
    fn new(name: &str) -> World {
        let root = std::env::temp_dir().join(format!(
            "amont-agent-plan-apply-{name}-{}",
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

    fn text(&self) -> String {
        std::fs::read_to_string(&self.plan).unwrap()
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

    fn launch(&mut self, agent: &str, prompt: &str) -> String {
        self.n += 1;
        let id = format!("toolu_{}", self.n);
        self.lines.push(
            serde_json::json!({"type": "assistant", "message": {"role": "assistant", "content": [{
                "type": "tool_use", "id": id, "name": "Agent",
                "input": {"subagent_type": agent, "prompt": prompt}
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

    /// A foreground review of the plan as it is now, answering `text`.
    fn review(&mut self, role: &str, agent_id: &str, text: &str) {
        let agent = format!("plan-review-{role}");
        let prompt = format!("Review.\n{}\n", self.block());
        let id = self.launch(&agent, &prompt);
        let tur = serde_json::json!({"status": "completed", "agentType": agent,
            "agentId": agent_id, "resolvedModel": "claude-opus-4-7",
            "totalTokens": 5000, "totalDurationMs": 12500});
        self.result(&id, tur, text);
    }

    /// A round resumed with `SendMessage`, answered by a notification.
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

    /// The fallback subagent, launched with `prompt`.
    fn fallback(&mut self, prompt: &str, agent_id: &str) {
        let id = self.launch("general-purpose", prompt);
        let tur = serde_json::json!({"status": "completed", "agentType": "general-purpose",
            "agentId": agent_id, "resolvedModel": "claude-sonnet-5-5",
            "totalTokens": 7000, "totalDurationMs": 9000});
        self.result(&id, tur, "ok");
    }

    fn transcript(&self) -> PathBuf {
        let t = self.root.join("t.jsonl");
        std::fs::write(&t, self.lines.join("\n") + "\n").unwrap();
        t
    }

    fn run(&self, args: &[&str], stdin: &str) -> Output {
        let t = self.transcript();
        self.run_with(&t, args, stdin)
    }

    fn run_with(&self, t: &Path, args: &[&str], stdin: &str) -> Output {
        let mut c = self
            .bin()
            .arg("plan-apply")
            .args(args)
            .arg("--transcript")
            .arg(t)
            .arg(&self.plan)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        c.wait_with_output().unwrap()
    }

    fn rows(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(
            self.root
                .join("claude/amont-agent/plan-review/outcomes.jsonl"),
        )
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
    }
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

const F1: &str = r#"[{"role":"backend","finding":1}]"#;
const F12: &str = r#"[{"role":"backend","finding":1},{"role":"backend","finding":2}]"#;

#[test]
fn a_unique_match_applies_and_writes_one_command_row() {
    let mut w = World::new("unique");
    w.review(
        "backend",
        "a1",
        &patches(&[(1, "alpha beta", "ALPHA BETA")]),
    );
    let o = w.run(&[], F1);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let said = out(&o);
    assert!(said.contains("applied backend#1 Context\n"), "{said}");
    assert!(said.contains("applied 1, unmatched 0, sha "), "{said}");
    assert!(!said.contains("<<<APPLY"), "{said}");
    assert!(w.text().contains("ALPHA BETA\n"));
    assert!(!w.text().contains("alpha beta"));
    let rows = w.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["schema"], "amont-agent-review-merge-v1");
    assert_eq!(rows[0]["mode"], "command");
    assert_eq!(rows[0]["applied"], 1);
    assert_eq!(rows[0]["accepted"], 1);
    assert_eq!(rows[0]["round"], 1);
    assert_eq!(rows[0]["body_sha_before"].as_str().unwrap().len(), 64);
    assert_eq!(rows[0]["body_sha_after"].as_str().unwrap().len(), 64);
    assert_ne!(rows[0]["body_sha_before"], rows[0]["body_sha_after"]);
}

#[test]
fn absent_and_multiple_matches_are_unmatched_and_the_file_is_untouched() {
    let mut w = World::new("none");
    w.review(
        "backend",
        "a1",
        &patches(&[(1, "no such text", "x"), (2, "dup", "DUP")]),
    );
    let before = w.text();
    let o = w.run(&[], F12);
    assert_eq!(o.status.code(), Some(3), "{}", err(&o));
    let said = out(&o);
    assert!(said.contains("unmatched backend#1: absent\n"), "{said}");
    assert!(
        said.contains("unmatched backend#2: 2 occurrences\n"),
        "{said}"
    );
    assert!(said.contains("applied 0, unmatched 2"), "{said}");
    assert_eq!(w.text(), before);
}

#[test]
fn whitespace_difference_is_labelled_but_never_applied() {
    let mut w = World::new("ws");
    w.review("backend", "a1", &patches(&[(1, "alpha  beta", "x")]));
    let o = w.run(&[], F1);
    assert_eq!(o.status.code(), Some(3));
    assert!(out(&o).contains("unmatched backend#1: absent\n"));
    let mut w = World::new("ws2");
    w.set_body(&BODY.replace("alpha beta", "alpha beta   "));
    w.review("backend", "a1", &patches(&[(1, "alpha beta\n", "x\n")]));
    let o = w.run(&[], F1);
    assert_eq!(o.status.code(), Some(3));
    assert!(
        out(&o).contains("unmatched backend#1: absent (whitespace)\n"),
        "{}",
        out(&o)
    );
}

#[test]
fn overlapping_patches_are_unmatched() {
    let mut w = World::new("overlap");
    w.review(
        "backend",
        "a1",
        &patches(&[(1, "alpha beta", "A"), (2, "beta\n\ndup", "B")]),
    );
    let o = w.run(&[], F12);
    assert_eq!(o.status.code(), Some(3), "{}", err(&o));
    let said = out(&o);
    assert!(said.contains("applied backend#1 Context\n"), "{said}");
    assert!(
        said.contains("unmatched backend#2: overlaps backend#1\n"),
        "{said}"
    );
    assert!(w.text().contains("A\n\ndup"));
}

#[test]
fn a_mixed_run_writes_the_matched_and_ends_stdout_with_the_apply_block() {
    let mut w = World::new("mixed");
    w.review(
        "backend",
        "a1",
        &patches(&[(1, "alpha beta", "ALPHA"), (2, "no such text", "x")]),
    );
    let o = w.run(&[], F12);
    assert_eq!(o.status.code(), Some(3), "{}", err(&o));
    assert!(w.text().contains("ALPHA"));
    let said = out(&o);
    let last = said.trim_end().lines().last().unwrap();
    let row = &w.rows()[0];
    let before = row["body_sha_before"].as_str().unwrap();
    assert_eq!(
        last,
        format!(
            "<<<APPLY path={} sha={}>>>",
            w.plan.display(),
            &before[..12]
        )
    );
    assert!(
        said.contains("\nnext: apply the unmatched edits, then run amont-agent plan-apply --check")
    );
    // The `next:` line is copied and the APPLY block is machine-read: both
    // carry the path unclipped. Every other line fits 80 columns.
    for l in said
        .lines()
        .filter(|l| !l.starts_with("<<<APPLY") && !l.starts_with("next:"))
    {
        assert!(l.chars().count() <= 80, "{l}");
    }
    assert!(said.contains(&format!("--check {}\n", w.plan.display())));
    assert_eq!(row["unmatched"], serde_json::json!(["backend#2: absent"]));
}

#[test]
fn a_patch_outside_the_body_is_applied_and_labelled() {
    let mut w = World::new("outside");
    w.review(
        "backend",
        "a1",
        &patches(&[(1, "Decide: none", "Decide: yes")]),
    );
    let o = w.run(&[], F1);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(
        out(&o).contains("applied backend#1 outside the body"),
        "{}",
        out(&o)
    );
    assert!(w.text().contains("Decide: yes"));
}

#[test]
fn check_passes_after_apply_and_fails_before() {
    let mut w = World::new("check");
    w.review("backend", "a1", &patches(&[(1, "alpha beta", "ALPHA")]));
    let o = w.run(&["--check"], F1);
    assert_eq!(o.status.code(), Some(3));
    assert!(
        out(&o).contains("unverified backend#1: new_string occurs 0 times"),
        "{}",
        out(&o)
    );
    assert_eq!(w.run(&[], F1).status.code(), Some(0));
    let o = w.run(&["--check"], F1);
    assert_eq!(o.status.code(), Some(0), "{}", out(&o));
    assert!(out(&o).contains("verified backend#1\n"));
    assert!(out(&o).contains("verified 1, unverified 0"));
}

#[test]
fn check_tolerates_a_new_string_that_contains_its_old_string() {
    let mut w = World::new("contains");
    w.review(
        "backend",
        "a1",
        &patches(&[(1, "the gamma line", "the gamma line, extended")]),
    );
    assert_eq!(w.run(&[], F1).status.code(), Some(0));
    let o = w.run(&["--check"], F1);
    assert_eq!(o.status.code(), Some(0), "{}", out(&o));
}

#[test]
fn check_fails_when_the_old_string_is_still_there() {
    let mut w = World::new("still");
    w.review("backend", "a1", &patches(&[(1, "dup", "alpha")]));
    // `alpha` occurs once already, and `dup` is present: unverified.
    let o = w.run(&["--check"], F1);
    assert_eq!(o.status.code(), Some(3));
    assert!(
        out(&o).contains("old_string is still present"),
        "{}",
        out(&o)
    );
}

#[test]
fn a_fence_inside_old_string_parses_and_applies() {
    let mut w = World::new("fence");
    w.review(
        "backend",
        "a1",
        &patches(&[(1, "```sh\necho hi\n```", "```sh\necho bye\n```")]),
    );
    let o = w.run(&[], F1);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(w.text().contains("echo bye"));
}

#[test]
fn a_dry_run_reports_and_writes_nothing() {
    let mut w = World::new("dry");
    w.review("backend", "a1", &patches(&[(1, "alpha beta", "ALPHA")]));
    let before = w.text();
    let o = w.run(&["--dry-run"], F1);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(out(&o).contains("applied backend#1"));
    assert_eq!(w.text(), before);
    assert!(w.rows().is_empty());
}

#[test]
fn a_missing_or_malformed_patches_block_is_unmatched_per_reviewer() {
    let mut w = World::new("missing");
    w.review("backend", "a1", "Verdict: approve\nno block");
    w.review("tui", "a2", "Patches:\n```json\n[{oops\n```\n");
    let o = w.run(
        &[],
        r#"[{"role":"backend","finding":1},{"role":"tui","finding":1}]"#,
    );
    assert_eq!(o.status.code(), Some(3), "{}", err(&o));
    assert!(
        out(&o).contains("unmatched backend#1: no patches block"),
        "{}",
        out(&o)
    );
    assert!(
        out(&o).contains("unmatched tui#1: patches block is not JSON"),
        "{}",
        out(&o)
    );
    assert!(
        err(&o).contains("backend result has no Patches block"),
        "{}",
        err(&o)
    );
}

#[test]
fn an_inline_patch_needs_no_transcript_result() {
    let mut w = World::new("inline");
    // The transcript holds no result of `backend`, and nothing needs it.
    w.review("tui", "a1", "nothing");
    let o = w.run(
        &[],
        r#"[{"role":"backend","finding":9,"patch":{"old_string":"alpha beta","new_string":"X"}}]"#,
    );
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(w.text().contains("\nX\n"));
    // A finding without an inline patch does need the role's result.
    let o = w.run(&[], r#"[{"role":"backend","finding":1}]"#);
    assert_eq!(o.status.code(), Some(1), "{}", err(&o));
}

#[test]
fn an_empty_old_string_is_a_usage_error() {
    let mut w = World::new("empty");
    w.review("backend", "a1", &patches(&[(1, "", "x")]));
    let before = w.text();
    let o = w.run(&[], F1);
    assert_eq!(o.status.code(), Some(2), "{}", err(&o));
    assert_eq!(w.text(), before);
    let o = w.run(
        &[],
        r#"[{"role":"backend","finding":1,"patch":{"old_string":"","new_string":"x"}}]"#,
    );
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn apply_then_edit_then_check_still_resolves_the_patches() {
    let mut w = World::new("edited");
    w.review("backend", "a1", &patches(&[(1, "alpha beta", "ALPHA")]));
    assert_eq!(w.run(&[], F1).status.code(), Some(0));
    let edited = w
        .text()
        .replace("the gamma line", "the gamma line, edited later");
    std::fs::write(&w.plan, edited).unwrap();
    let o = w.run(&["--check"], F1);
    assert_eq!(o.status.code(), Some(0), "{} {}", out(&o), err(&o));
}

#[test]
fn check_takes_a_before_prefix_and_refuses_an_unknown_one() {
    let mut w = World::new("before");
    w.review("backend", "a1", &patches(&[(1, "alpha beta", "ALPHA")]));
    assert_eq!(w.run(&[], F1).status.code(), Some(0));
    let sha = w.rows()[0]["body_sha_before"].as_str().unwrap().to_string();
    let o = w.run(&["--check", "--before", &sha[..12]], F1);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let o = w.run(&["--check", "--before", "000000000000"], F1);
    assert_eq!(o.status.code(), Some(1));
    let o = w.run(&["--before", &sha[..12]], F1);
    assert_eq!(o.status.code(), Some(2));
    let o = w.run(&["--check", "--before", "abc"], F1);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn round_two_patches_resolve_from_a_sendmessage_resume() {
    let mut w = World::new("resume");
    w.review("backend", "ag1", &patches(&[(1, "alpha beta", "ALPHA")]));
    w.set_body(&BODY.replace("the gamma line", "the delta line"));
    w.resume("ag1", &patches(&[(1, "the delta line", "THE DELTA")]));
    let o = w.run(&[], F1);
    assert_eq!(o.status.code(), Some(0), "{} {}", out(&o), err(&o));
    assert!(w.text().contains("THE DELTA"));
    assert_eq!(w.rows()[0]["round"], 2);
}

#[test]
fn record_fallback_writes_one_subagent_row_and_a_rerun_writes_nothing() {
    let mut w = World::new("fallback");
    w.review(
        "backend",
        "a1",
        &patches(&[
            (1, "alpha beta", "ALPHA"),
            (2, "gamma line!", "gamma line, extended"),
        ]),
    );
    let o = w.run(&[], F12);
    assert_eq!(o.status.code(), Some(3), "{}", err(&o));
    let said = out(&o);
    let apply_block = said.trim_end().lines().last().unwrap().to_string();
    // The subagent finishes the unmatched edit by hand.
    let edited = w
        .text()
        .replace("the gamma line", "the gamma line, extended");
    std::fs::write(&w.plan, edited).unwrap();
    w.fallback(&format!("{apply_block}\n\nApply the edits."), "gp-1");
    let o = w.run(&["--record-fallback"], F12);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(out(&o).contains("verified 2, unverified 0"), "{}", out(&o));
    let rows = w.rows();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let r = &rows[1];
    assert_eq!(r["mode"], "subagent");
    assert_eq!(r["agent_id"], "gp-1");
    assert_eq!(r["model"], "claude-sonnet-5-5");
    assert_eq!(r["tokens"], 7000);
    assert_eq!(r["verified"], true);
    assert_eq!(r["body_sha_before"], rows[0]["body_sha_before"]);
    let again = w.run(&["--record-fallback"], F12);
    assert_eq!(again.status.code(), Some(0));
    assert_eq!(w.rows().len(), 2);
    assert!(out(&again).is_empty());
}

#[test]
fn record_fallback_without_a_launch_exits_one() {
    let mut w = World::new("nolaunch");
    w.review("backend", "a1", &patches(&[(1, "alpha beta", "ALPHA")]));
    let o = w.run(&["--record-fallback"], F1);
    assert_eq!(o.status.code(), Some(1), "{}", err(&o));
    assert!(w.rows().is_empty());
}

#[test]
fn merge_rows_do_not_enter_the_review_dedupe_set() {
    let mut w = World::new("dedupe");
    w.review("backend", "a1", &patches(&[(1, "alpha beta", "ALPHA")]));
    let store = w.root.join("claude/amont-agent/plan-review");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(
        store.join("outcomes.jsonl"),
        "{\"schema\":\"amont-agent-review-merge-v1\",\"agent_id\":\"a1\",\"mode\":\"subagent\"}\n",
    )
    .unwrap();
    let t = w.transcript();
    let o = w
        .bin()
        .args(["plan-review", "record"])
        .arg(&w.plan)
        .arg("--transcript")
        .arg(&t)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(err(&o).contains("wrote 1 rows"), "{}", err(&o));
}

#[cfg(unix)]
#[test]
fn a_change_between_read_and_write_exits_one_and_writes_nothing() {
    let mut w = World::new("race");
    w.review("backend", "a1", &patches(&[(1, "alpha beta", "ALPHA")]));
    let t = w.transcript();
    let content = std::fs::read(&t).unwrap();
    // A FIFO as the transcript: the binary has read the plan by the time it
    // opens the transcript, and blocks there until we write.
    let fifo = w.root.join("t.fifo");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let mut c = w
        .bin()
        .arg("plan-apply")
        .arg("--transcript")
        .arg(&fifo)
        .arg(&w.plan)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(F1.as_bytes()).unwrap();
    let mut f = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
    let changed = format!("{}\nsomeone else was here\n", w.text());
    std::fs::write(&w.plan, &changed).unwrap();
    f.write_all(&content).unwrap();
    drop(f);
    let o = c.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(1), "{}", err(&o));
    assert!(
        err(&o).contains("plan changed under plan-apply; nothing written"),
        "{}",
        err(&o)
    );
    assert_eq!(w.text(), changed);
    assert!(w.rows().is_empty());
}

#[cfg(unix)]
#[test]
fn a_symlinked_plan_keeps_its_link() {
    let mut w = World::new("link");
    w.review("backend", "a1", &patches(&[(1, "alpha beta", "ALPHA")]));
    let link = w.root.join("plans/link.md");
    std::os::unix::fs::symlink(&w.plan, &link).unwrap();
    let t = w.transcript();
    let mut c = w
        .bin()
        .args(["plan-apply", "--transcript"])
        .arg(&t)
        .arg(&link)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(F1.as_bytes()).unwrap();
    let o = c.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(w.text().contains("ALPHA"));
}

#[test]
fn help_names_the_modes_the_exit_codes_and_the_apply_block() {
    let w = World::new("help");
    let o = w.bin().args(["plan-apply", "--help"]).output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    let said = out(&o);
    for word in [
        "--dry-run",
        "--check",
        "--record-fallback",
        "--before",
        "\"finding\"",
        "<<<APPLY",
        "0 every patch",
        "3 some patch",
        "1 file I/O",
        "2 usage",
    ] {
        assert!(said.contains(word), "help lacks {word}: {said}");
    }
}

#[test]
fn usage_errors_exit_two() {
    let w = World::new("usage");
    let o = w
        .bin()
        .args(["plan-apply", "--check", "--dry-run"])
        .arg(&w.plan)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    let o = w
        .bin()
        .args(["plan-apply"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
}

/// stdin on a terminal is a usage error, not a hang. `script` lends the
/// binary a pty: BSD `script -q <log> <cmd…>` on macOS, util-linux
/// `script -qc "<cmd>" <log>` elsewhere. Without `script` the test is a
/// no-op, since no other pty is available to it.
#[cfg(unix)]
#[test]
fn stdin_on_a_terminal_is_a_usage_error() {
    let w = World::new("tty");
    if Command::new("script").arg("--version").output().is_err()
        && Command::new("script").output().is_err()
    {
        return;
    }
    let bin = env!("CARGO_BIN_EXE_amont-agent");
    let log = w.root.join("script.log");
    let mut c = Command::new("script");
    if cfg!(target_os = "macos") {
        c.arg("-q")
            .arg(&log)
            .arg(bin)
            .arg("plan-apply")
            .arg(&w.plan);
    } else {
        c.arg("-qc")
            .arg(format!("'{bin}' plan-apply '{}'", w.plan.display()))
            .arg(&log);
    }
    let o = c
        .env("CLAUDE_CONFIG_DIR", w.root.join("claude"))
        .env("AMONT_AGENT_PLAN_ROOT", w.root.join("repos"))
        .env_remove("AMONT_AGENT_OFF")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    // BSD script returns the child's status; util-linux does with -e, so the
    // log is read instead of the status.
    let said = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        said.contains("come on stdin, not a terminal") && said.contains("usage:"),
        "status {:?}, log: {said}",
        o.status.code()
    );
    // The data lines never appear: the command refused before reading.
    assert!(!said.lines().any(|l| l.starts_with("applied ")), "{said}");
}
