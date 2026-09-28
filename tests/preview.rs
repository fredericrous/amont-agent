//! Preview approval, end to end through the real binary (ADR-0023).
//!
//! Each test builds a repository with a user interface (a `dev` script) and a
//! bare remote, then drives the hook with the payload shapes captured from a
//! real session on 2026-09-28 (`AskUserQuestion` answers arrive in
//! `tool_response.answers`, keyed by question text; `prompt_id` and
//! `tool_use_id` ride on every event).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct World {
    root: PathBuf,
    work: PathBuf,
}

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

impl World {
    /// A UI repository whose `main` is on the remote, on a fresh branch.
    fn new(name: &str) -> World {
        let root = std::env::temp_dir().join(format!(
            "amont-agent-preview-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let work = root.join("app");
        let remote = root.join("remote.git");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(root.join("claude")).unwrap();
        Command::new("git")
            .args(["init", "-q", "--bare", "--template="])
            .arg(&remote)
            .output()
            .expect("git init --bare");
        git(&work, &["init", "-q", "-b", "main", "--template=", "."]);
        git(&work, &["config", "user.email", "t@t"]);
        git(&work, &["config", "user.name", "t"]);
        std::fs::write(work.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        std::fs::write(work.join("README.md"), "x\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-qm", "init"]);
        git(
            &work,
            &["remote", "add", "origin", &remote.display().to_string()],
        );
        git(&work, &["push", "-q", "origin", "main"]);
        git(&work, &["checkout", "-q", "-b", "feat/x"]);
        World { root, work }
    }

    fn commit(&self, path: &str, body: &str) -> String {
        let p = self.work.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        git(&self.work, &["add", "."]);
        git(&self.work, &["commit", "-qm", path]);
        git(&self.work, &["rev-parse", "HEAD"])
    }

    fn set_global(&self, key: &str, value: &str) {
        let global = self.root.join("global");
        Command::new("git")
            .args(["config", "--file"])
            .arg(&global)
            .args([key, value])
            .output()
            .expect("git config");
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .args(args)
            .current_dir(&self.work)
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("GIT_CONFIG_GLOBAL", self.root.join("global"))
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env_remove("AMONT_AGENT_OFF")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the binary runs");
        let mut input = child.stdin.take().unwrap();
        if let Some(s) = stdin {
            input.write_all(s.as_bytes()).unwrap();
        }
        drop(input);
        let out = child.wait_with_output().unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn hook(&self, payload: serde_json::Value) -> String {
        self.run(&["hook"], Some(&payload.to_string())).1
    }

    fn pre_bash(&self, session: &str, tool_use_id: &str, command: &str) -> String {
        self.hook(serde_json::json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "cwd": self.work, "session_id": session, "prompt_id": "p1",
            "tool_use_id": tool_use_id, "permission_mode": "default",
            "tool_input": {"command": command}
        }))
    }

    fn post_bash(
        &self,
        session: &str,
        prompt: &str,
        tool_use_id: &str,
        command: &str,
        stdout: &str,
    ) -> String {
        self.hook(serde_json::json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "cwd": self.work, "session_id": session, "prompt_id": prompt,
            "tool_use_id": tool_use_id, "permission_mode": "default",
            "tool_input": {"command": command},
            "tool_response": {"stdout": stdout, "stderr": "", "interrupted": false}
        }))
    }

    /// Validate a preview through the CLI and bind it through the hook, the
    /// way a session does. Returns the registration id.
    fn register(&self, session: &str, prompt: &str) -> String {
        let record = self.root.join("attest.md");
        std::fs::write(&record, "drove /settings; 0 console errors\n").unwrap();
        let command = format!(
            "amont-agent preview register --url http://localhost:5173/ --attestation {}",
            record.display()
        );
        let (code, out, err) = self.run(
            &[
                "preview",
                "register",
                "--url",
                "http://localhost:5173/",
                "--attestation",
                &record.display().to_string(),
            ],
            None,
        );
        assert_eq!(code, 0, "register refused: {err}");
        self.post_bash(session, prompt, "reg", &command, &out);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        v["id"].as_str().unwrap().to_string()
    }

    fn label(&self) -> String {
        let sha = git(&self.work, &["rev-parse", "HEAD"]);
        format!("app@{}", &sha[..7])
    }

    fn ask(
        &self,
        stage: &str,
        (session, prompt): (&str, &str),
        tool_use_id: &str,
        question: &str,
        answer: Option<&str>,
        prefill: bool,
    ) -> String {
        let mut input = serde_json::json!({"questions": [{"question": question, "options": [
            {"label": "Approve"}, {"label": "Request changes"}, {"label": "Hold"}]}]});
        if prefill {
            input["answers"] = serde_json::json!({question: "Approve"});
        }
        let mut payload = serde_json::json!({
            "hook_event_name": stage, "tool_name": "AskUserQuestion",
            "cwd": self.work, "session_id": session, "prompt_id": prompt,
            "tool_use_id": tool_use_id, "tool_input": input, "duration_ms": 4200
        });
        if stage == "PostToolUse" {
            let answers = match answer {
                Some(a) => serde_json::json!({question: a}),
                None => serde_json::json!({}),
            };
            payload["tool_response"] =
                serde_json::json!({"questions": [], "answers": answers, "annotations": {}});
        }
        self.hook(payload)
    }

    /// The full marked-question exchange for one registration.
    fn answer(&self, session: &str, prompt: &str, id: &str, answer: Option<&str>) {
        let q = format!("[preview {id}] Ship {}?", self.label());
        self.ask("PreToolUse", (session, prompt), "q1", &q, None, false);
        self.ask("PostToolUse", (session, prompt), "q1", &q, answer, false);
    }

    fn prompt(&self, session: &str, prompt_id: &str, text: &str) {
        self.hook(serde_json::json!({
            "hook_event_name": "UserPromptSubmit", "prompt": text,
            "cwd": self.work, "session_id": session, "prompt_id": prompt_id
        }));
    }

    fn push_advised(&self, session: &str) -> bool {
        self.pre_bash(session, "push1", "git push -u origin feat/x")
            .contains("amont-agent/push-preview")
    }

    fn journal(&self) -> String {
        std::fs::read_to_string(self.root.join("claude/amont-agent/journal.log"))
            .unwrap_or_default()
    }
}

#[test]
fn an_unapproved_ui_push_is_advised_and_a_docs_only_one_is_not() {
    let w = World::new("advise");
    w.commit("docs/plan.md", "x\n");
    assert!(
        !w.push_advised("s"),
        "docs-only pushes carry nothing to preview"
    );
    w.commit("app/routes/home.tsx", "export default 1\n");
    assert!(w.push_advised("s"));
}

#[test]
fn a_marked_approve_in_the_same_turn_releases_exactly_that_commit() {
    let w = World::new("approve");
    w.commit("app/routes/home.tsx", "1\n");
    let id = w.register("s", "p1");
    w.answer("s", "p1", &id, Some("Approve"));
    assert!(!w.push_advised("s"), "the approved commit pushes silently");
    assert!(w.journal().contains("approved"));

    w.commit("app/routes/home.tsx", "2\n");
    assert!(w.push_advised("s"), "a new commit needs a new preview");
}

#[test]
fn request_changes_and_hold_drop_the_preview() {
    for answer in ["Request changes", "Hold"] {
        let w = World::new(&answer.replace(' ', "-"));
        w.commit("app/a.tsx", "1\n");
        let id = w.register("s", "p1");
        w.answer("s", "p1", &id, Some(answer));
        w.prompt("s", "p2", "approve");
        assert!(
            w.push_advised("s"),
            "{answer} then a typed approve does not approve"
        );
    }
}

#[test]
fn an_unmarked_question_answered_approve_approves_nothing() {
    let w = World::new("unmarked");
    w.commit("app/a.tsx", "1\n");
    let _id = w.register("s", "p1");
    let q = "Ship the release?";
    w.ask("PreToolUse", ("s", "p1"), "q9", q, None, false);
    w.ask("PostToolUse", ("s", "p1"), "q9", q, Some("Approve"), false);
    assert!(w.push_advised("s"));
}

#[test]
fn a_prefilled_marked_question_never_approves_and_is_refused_under_deny() {
    let w = World::new("prefill");
    w.commit("app/a.tsx", "1\n");
    let id = w.register("s", "p1");
    let q = format!("[preview {id}] Ship {}?", w.label());
    w.ask("PreToolUse", ("s", "p1"), "q1", &q, None, true);
    w.ask("PostToolUse", ("s", "p1"), "q1", &q, Some("Approve"), false);
    assert!(
        w.push_advised("s"),
        "a pre-answered question is not the person"
    );

    w.set_global("amont.agent.push-preview.stance", "deny");
    let out = w.ask("PreToolUse", ("s", "p1"), "q2", &q, None, true);
    assert!(out.contains("\"deny\""), "refused before it runs: {out}");
}

#[test]
fn a_timed_out_question_stays_pending_for_a_typed_approval() {
    let w = World::new("timeout");
    w.commit("app/a.tsx", "1\n");
    let id = w.register("s", "p1");
    w.answer("s", "p1", &id, None);
    assert!(w.journal().contains("unanswered"));
    w.prompt("s", "p2", "approve");
    assert!(!w.push_advised("s"));
}

#[test]
fn a_typed_yes_approves_nothing_and_drops_the_preview() {
    let w = World::new("yes");
    w.commit("app/a.tsx", "1\n");
    let id = w.register("s", "p1");
    w.answer("s", "p1", &id, None);
    w.prompt("s", "p2", "yes");
    assert!(w.push_advised("s"));
    w.prompt("s", "p3", "approve");
    assert!(
        w.push_advised("s"),
        "the preview lapsed at the first other prompt"
    );
}

#[test]
fn a_typed_approve_without_a_marked_question_approves_nothing() {
    let w = World::new("never-asked");
    w.commit("app/a.tsx", "1\n");
    let _id = w.register("s", "p1");
    w.prompt("s", "p2", "approve");
    assert!(w.push_advised("s"));
}

#[test]
fn another_sessions_answer_does_not_approve() {
    let w = World::new("isolation");
    w.commit("app/a.tsx", "1\n");
    let id = w.register("A", "p1");
    w.answer("B", "p1", &id, Some("Approve"));
    w.prompt("B", "p2", "approve");
    assert!(w.push_advised("A"));
}

#[test]
fn registration_refuses_a_dirty_tree_and_a_record_inside_the_worktree() {
    let w = World::new("refuse");
    w.commit("app/a.tsx", "1\n");
    std::fs::write(w.work.join("app/a.tsx"), "dirty\n").unwrap();
    let outside = w.root.join("a.md");
    std::fs::write(&outside, "x\n").unwrap();
    let (code, _, err) = w.run(
        &[
            "preview",
            "register",
            "--url",
            "http://localhost:1/",
            "--attestation",
            &outside.display().to_string(),
        ],
        None,
    );
    assert_eq!(code, 1);
    assert!(err.contains("not clean"), "{err}");

    git(&w.work, &["checkout", "--", "app/a.tsx"]);
    let inside = w.work.join("attest.md");
    std::fs::write(&inside, "x\n").unwrap();
    git(&w.work, &["add", "attest.md"]);
    git(&w.work, &["commit", "-qm", "attest"]);
    let (code, _, err) = w.run(
        &[
            "preview",
            "register",
            "--url",
            "http://localhost:1/",
            "--attestation",
            &inside.display().to_string(),
        ],
        None,
    );
    assert_eq!(code, 1);
    assert!(err.contains("inside the worktree"), "{err}");
}

#[test]
fn a_chained_register_is_not_bound() {
    let w = World::new("chained");
    w.commit("app/a.tsx", "1\n");
    let record = w.root.join("attest.md");
    std::fs::write(&record, "x\n").unwrap();
    let (_, out, _) = w.run(
        &[
            "preview",
            "register",
            "--url",
            "http://localhost:1/",
            "--attestation",
            &record.display().to_string(),
        ],
        None,
    );
    let id = serde_json::from_str::<serde_json::Value>(&out).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let command = format!(
        "npm test && amont-agent preview register --url http://localhost:1/ --attestation {}",
        record.display()
    );
    w.post_bash("s", "p1", "reg", &command, &out);
    w.answer("s", "p1", &id, Some("Approve"));
    assert!(w.push_advised("s"));
    assert!(w.journal().contains("not a standalone command"));
}

#[test]
fn under_deny_an_unreadable_push_to_a_ui_repository_is_held() {
    let w = World::new("deny");
    w.set_global("amont.agent.push-preview.stance", "deny");
    w.commit("app/a.tsx", "1\n");
    let out = w.pre_bash("s", "t", "git push --all origin");
    assert!(out.contains("\"deny\""), "{out}");
    let out = w.pre_bash("s", "t", "git push -u origin feat/x");
    assert!(out.contains("\"deny\""), "{out}");
}

#[test]
fn a_repository_can_opt_out() {
    let w = World::new("opt-out");
    git(&w.work, &["config", "amont.agent.push-preview.ui", "false"]);
    w.commit("app/a.tsx", "1\n");
    assert!(!w.push_advised("s"));
}

#[test]
fn a_real_push_is_recorded_as_published_with_its_approval() {
    let w = World::new("published");
    w.commit("app/a.tsx", "1\n");
    let id = w.register("s", "p1");
    w.answer("s", "p1", &id, Some("Approve"));
    let command = "git push -q -u origin feat/x";
    w.pre_bash("s", "push9", command);
    git(&w.work, &["push", "-q", "-u", "origin", "feat/x"]);
    w.post_bash("s", "p1", "push9", command, "");
    assert!(
        w.journal().contains("published-approved"),
        "{}",
        w.journal()
    );

    // Pushing again publishes nothing new.
    w.pre_bash("s", "push10", command);
    git(&w.work, &["push", "-q", "-u", "origin", "feat/x"]);
    w.post_bash("s", "p1", "push10", command, "");
    assert!(w.journal().contains("already-present"), "{}", w.journal());
}
