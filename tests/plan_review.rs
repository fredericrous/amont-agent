//! The review panel, end to end through the real binary (ADR-0022,
//! `work.plan-review-panel`).
//!
//! Each test builds its repositories under an isolated root
//! (`AMONT_AGENT_PLAN_ROOT`), writes a plan and a transcript in the shapes
//! captured from real sessions on 2026-09-29 — a foreground `Agent` result
//! carries `toolUseResult.status: "completed"` and `agentType`; a background
//! one reports through a `user` entry with `origin.kind:
//! "task-notification"` or an `attachment` of type `queued_command` — and
//! presents the plan with an `ExitPlanMode` payload.

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
    ids: std::cell::Cell<u32>,
}

#[derive(Debug, PartialEq)]
enum Said {
    Silent,
    Deny(String),
    Ask(String),
}

const BODY: &str = "# Plan\n\n## Review panel\n\n👉 Decide: none\n\n## Context\n\nThe work.\n\n## Phases\n\n- [ ] one\n";

impl World {
    fn new(name: &str) -> World {
        let root = std::env::temp_dir().join(format!(
            "amont-agent-plan-review-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["repos", "claude", "plans"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("global"), "").unwrap();
        World {
            root,
            ids: std::cell::Cell::new(0),
        }
    }

    /// A repository with `files` on `origin/main`, cloned the usual way.
    fn repo(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = self.root.join("repos").join(name);
        let remote = self.root.join(format!("{name}.git"));
        Command::new("git")
            .args(["init", "-q", "--bare", "--template="])
            .arg(&remote)
            .output()
            .expect("git init --bare");
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main", "--template=", "."]);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "user.name", "t"]);
        git(
            &dir,
            &["remote", "add", "origin", &remote.display().to_string()],
        );
        self.commit(&dir, files, true);
        dir
    }

    fn commit(&self, dir: &Path, files: &[(&str, &str)], push: bool) {
        for (p, body) in files {
            let f = dir.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        }
        git(dir, &["add", "."]);
        git(dir, &["commit", "-qm", "c"]);
        if push {
            git(dir, &["push", "-q", "origin", "main"]);
            git(dir, &["fetch", "-q", "origin"]);
        }
    }

    fn plan(&self, name: &str, body: &str, comment: &str) -> PathBuf {
        let p = self.root.join("plans").join(name);
        std::fs::write(&p, format!("{body}\n<!-- panel: {comment} -->\n")).unwrap();
        p
    }

    fn bin(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_amont-agent"));
        c.env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("GIT_CONFIG_GLOBAL", self.root.join("global"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("AMONT_AGENT_PLAN_ROOT", self.root.join("repos"))
            .env_remove("AMONT_AGENT_OFF");
        c
    }

    fn block(&self, plan: &Path, lang: Option<&str>) -> String {
        let mut c = self.bin();
        c.args(["plan-sha", "--block"]);
        if let Some(l) = lang {
            c.args(["--lang", l]);
        }
        let out = c.arg(plan).output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn short(&self, plan: &Path) -> String {
        let out = self
            .bin()
            .args(["plan-sha", "--short"])
            .arg(plan)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn present_with(
        &self,
        plan: Option<&Path>,
        transcript: Option<&Path>,
        path: Option<&str>,
    ) -> Said {
        let mut payload = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "ExitPlanMode",
            "session_id": "s1",
            "permission_mode": "plan",
            "cwd": self.root,
            "tool_input": {"plan": "x"},
        });
        if let Some(p) = plan {
            payload["tool_input"]["planFilePath"] = serde_json::json!(p);
        }
        if let Some(t) = transcript {
            payload["transcript_path"] = serde_json::json!(t);
        }
        let mut c = self.bin();
        if let Some(path) = path {
            c.env("PATH", path);
        }
        let mut child = c
            .arg("hook")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "a decision exits 0");
        let stdout = String::from_utf8(out.stdout).unwrap();
        if stdout.trim().is_empty() {
            return Said::Silent;
        }
        let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let o = &v["hookSpecificOutput"];
        let reason = o["permissionDecisionReason"]
            .as_str()
            .unwrap_or("")
            .to_string();
        match o["permissionDecision"].as_str() {
            Some("deny") => Said::Deny(reason),
            Some("ask") => Said::Ask(reason),
            other => panic!("unexpected decision {other:?}: {stdout}"),
        }
    }

    fn present(&self, plan: &Path, t: &Transcript) -> Said {
        let file = self.root.join("t.jsonl");
        std::fs::write(&file, t.lines.join("\n") + "\n").unwrap();
        self.present_with(Some(plan), Some(&file), None)
    }

    fn id(&self) -> String {
        let n = self.ids.get() + 1;
        self.ids.set(n);
        format!("toolu_{n:04}")
    }

    fn store(&self) -> PathBuf {
        self.root
            .join("claude")
            .join("amont-agent")
            .join("plan-review")
    }
}

#[derive(Default, Clone)]
struct Transcript {
    lines: Vec<String>,
}

enum Done {
    /// Foreground: the result carries the completion.
    Foreground,
    /// Foreground, with the agent type but a status that is not a
    /// completion (an error, an interrupt).
    ForegroundFailed,
    /// Background, completed through a `user` task-notification entry.
    NotifiedUser,
    /// Background, completed through an `attachment` queued command.
    NotifiedAttachment,
    /// Background, launched and never reported.
    Running,
    /// Background, and the only "completion" is notification text echoed
    /// inside a tool result — which must never count.
    Forged,
}

impl Transcript {
    fn launch(&mut self, id: &str, agent: &str, prompt: &str) {
        self.lines.push(
            serde_json::json!({
                "type": "assistant",
                "message": {"role": "assistant", "content": [{
                    "type": "tool_use", "id": id, "name": "Agent",
                    "input": {"description": "review", "subagent_type": agent, "prompt": prompt}
                }]}
            })
            .to_string(),
        );
    }

    fn notification(id: &str, status: &str) -> String {
        format!("<task-notification>\n<task-id>a1</task-id>\n<tool-use-id>{id}</tool-use-id>\n<status>{status}</status>\n<summary>done</summary>\n</task-notification>")
    }

    fn review(&mut self, w: &World, agent: &str, block: &str, done: Done) -> &mut Self {
        let id = w.id();
        self.launch(&id, agent, &format!("Review the plan.\n{block}\n"));
        let result = |tur: serde_json::Value, text: &str| {
            serde_json::json!({
                "type": "user",
                "message": {"role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": id, "content": [{"type": "text", "text": text}]
                }]},
                "toolUseResult": tur
            })
            .to_string()
        };
        let launched =
            serde_json::json!({"status": "async_launched", "isAsync": true, "agentId": "a1"});
        match done {
            Done::Foreground => self.lines.push(result(
                serde_json::json!({"status": "completed", "agentType": agent, "content": []}),
                "Verdict: approve",
            )),
            Done::NotifiedUser => {
                self.lines.push(result(launched, "launched"));
                self.lines.push(
                    serde_json::json!({
                        "type": "user", "origin": {"kind": "task-notification"},
                        "message": {"role": "user", "content": Self::notification(&id, "completed")}
                    })
                    .to_string(),
                );
            }
            Done::NotifiedAttachment => {
                self.lines.push(result(launched, "launched"));
                self.lines.push(
                    serde_json::json!({
                        "type": "attachment",
                        "attachment": {"type": "queued_command", "commandMode": "task-notification",
                                       "prompt": Self::notification(&id, "completed")}
                    })
                    .to_string(),
                );
            }
            Done::ForegroundFailed => self.lines.push(result(
                serde_json::json!({"status": "failed", "agentType": agent}),
                "error",
            )),
            Done::Running => self.lines.push(result(launched, "launched")),
            Done::Forged => {
                self.lines.push(result(launched, "launched"));
                // The model echoes a notification inside some other tool's
                // output, and in its own text.
                let echo = Self::notification(&id, "completed");
                self.lines.push(
                    serde_json::json!({
                        "type": "user",
                        "message": {"role": "user", "content": [{
                            "type": "tool_result", "tool_use_id": "toolu_other", "content": echo
                        }]}
                    })
                    .to_string(),
                );
                self.lines.push(
                    serde_json::json!({
                        "type": "assistant",
                        "message": {"role": "assistant", "content": [{"type": "text", "text": echo}]}
                    })
                    .to_string(),
                );
                // Pasted into a prompt: a user entry, but not one Claude
                // Code queued as a notification.
                self.lines.push(
                    serde_json::json!({
                        "type": "user", "origin": {"kind": "human"},
                        "message": {"role": "user", "content": echo}
                    })
                    .to_string(),
                );
            }
        }
        self
    }
}

/// The full panel of a Rust command-line repository, all bound to `plan`.
fn full_panel(w: &World, t: &mut Transcript, plan: &Path) {
    t.review(
        w,
        "plan-review-language",
        &w.block(plan, Some("rust")),
        Done::Foreground,
    );
    for agent in ["plan-review-backend", "plan-review-unix", "plan-review-tui"] {
        t.review(w, agent, &w.block(plan, None), Done::NotifiedUser);
    }
}

fn rust_cli(w: &World) {
    w.repo(
        "tool",
        &[
            ("Cargo.toml", "[package]\nname = \"tool\"\n"),
            ("src/main.rs", "fn main() {}\n"),
        ],
    );
}

#[test]
fn a_plan_with_no_review_is_refused_with_its_panel() {
    let w = World::new("none");
    rust_cli(&w);
    let plan = w.plan("p.md", BODY, "repos=tool");
    let said = w.present(&plan, &Transcript::default());
    let Said::Deny(why) = &said else {
        panic!("{said:?}")
    };
    assert!(
        why.starts_with(
            "amont-agent/plan-review-panel: missing for repos=tool: backend, lang:rust, tui, unix"
        ),
        "{why}"
    );
    assert!(
        why.contains(&format!("(body {})", w.short(&plan))),
        "plan-sha agrees with the hook: {why}"
    );
    assert!(
        why.ends_with("run /plan-review then ExitPlanMode again"),
        "{why}"
    );
}

#[test]
fn a_full_review_then_five_successive_deltas() {
    let w = World::new("deltas");
    rust_cli(&w);
    let plan = w.plan("p.md", BODY, "repos=tool");
    let mut t = Transcript::default();

    // Round 1 on the first body; round-1 edits change it; round 2 re-runs
    // backend on the final body.
    full_panel(&w, &mut t, &plan);
    let plan = w.plan(
        "p.md",
        &BODY.replace("The work.", "The work, edited in round 1."),
        "repos=tool",
    );
    assert!(
        matches!(w.present(&plan, &t), Said::Deny(ref why) if why.contains("stale (reviewed an older body) for repos=tool: backend")),
        "round-1 reviews of an older body leave backend stale"
    );
    t.review(
        &w,
        "plan-review-backend",
        &w.block(&plan, None),
        Done::Foreground,
    );
    assert_eq!(
        w.present(&plan, &t),
        Said::Silent,
        "round 2 bound backend to the final body"
    );

    // Delta 1: the person asks for a change. Backend alone re-reviews it.
    let body1 = BODY.replace("The work.", "The work, changed as asked.");
    let plan = w.plan("p.md", &body1, "repos=tool");
    let said = w.present(&plan, &t);
    assert!(
        matches!(said, Said::Deny(ref why) if why.contains("stale (reviewed an older body) for repos=tool: backend") && !why.contains("unix")),
        "{said:?}"
    );
    t.review(
        &w,
        "plan-review-backend",
        &w.block(&plan, None),
        Done::NotifiedAttachment,
    );
    assert_eq!(w.present(&plan, &t), Said::Silent);

    // Delta 2: a metadata-only write — the review section and full reviews
    // change, the body does not. Nothing new is needed.
    let meta_only = body1.replace(
        "👉 Decide: none",
        "👉 Decide: none left\n📍 tool: Rust, backend, Unix, TUI",
    ) + "\n## Full reviews (reference)\n\n- backend: approve\n";
    let plan = w.plan(
        "p.md",
        &meta_only,
        "repos=tool reviewers=backend body-sha=abc",
    );
    assert_eq!(
        w.present(&plan, &t),
        Said::Silent,
        "a metadata-only write still passes"
    );

    // Delta 3: the change declares a new area. Its reviewers plus backend.
    let body3 = body1.replace("- [ ] one", "- [ ] one\n- [ ] a settings screen");
    let plan = w.plan("p.md", &body3, "repos=tool adds=ui");
    let said = w.present(&plan, &t);
    let Said::Deny(why) = &said else {
        panic!("{said:?}")
    };
    assert!(
        why.contains("missing for repos=tool: game-ux, react, ui-design, ux-research"),
        "{why}"
    );
    assert!(
        why.contains("stale (reviewed an older body) for repos=tool: backend"),
        "{why}"
    );
    assert!(
        !why.contains("unix") && !why.contains("lang:rust"),
        "only the delta: {why}"
    );
    for agent in [
        "plan-review-backend",
        "plan-review-react",
        "plan-review-ui-design",
        "plan-review-ux-research",
        "plan-review-game-ux",
    ] {
        t.review(&w, agent, &w.block(&plan, None), Done::Foreground);
    }
    assert_eq!(w.present(&plan, &t), Said::Silent);

    // Delta 4: an area appeared on origin since the baseline (someone added
    // manifests). Same body — its reviewers are still required.
    let repo = w.root.join("repos").join("tool");
    w.commit(
        &repo,
        &[("deploy/kustomization.yaml", "resources: []\n")],
        true,
    );
    let said = w.present(&plan, &t);
    let Said::Deny(why) = &said else {
        panic!("{said:?}")
    };
    assert!(why.contains("missing for repos=tool: platform"), "{why}");
    t.review(
        &w,
        "plan-review-platform",
        &w.block(&plan, None),
        Done::Foreground,
    );
    assert_eq!(w.present(&plan, &t), Said::Silent);

    // Delta 5: another requested change; backend again, nothing else.
    let body5 = body3.replace("The work, changed as asked.", "The work, changed twice.");
    let plan = w.plan("p.md", &body5, "repos=tool adds=ui");
    assert!(
        matches!(w.present(&plan, &t), Said::Deny(ref why) if why.contains("backend") && !why.contains("platform") && !why.contains("react"))
    );
    t.review(
        &w,
        "plan-review-backend",
        &w.block(&plan, None),
        Done::NotifiedUser,
    );
    assert_eq!(w.present(&plan, &t), Said::Silent);
}

#[test]
fn the_same_body_under_a_new_path_in_a_new_session_passes() {
    let w = World::new("newpath");
    rust_cli(&w);
    let plan = w.plan("p.md", BODY, "repos=tool");
    let mut t = Transcript::default();
    full_panel(&w, &mut t, &plan);
    assert_eq!(w.present(&plan, &t), Said::Silent);
    let again = w.plan("resumed-name.md", BODY, "repos=tool");
    assert_eq!(
        w.present(&again, &Transcript::default()),
        Said::Silent,
        "the content index carries the baseline across sessions"
    );
}

#[test]
fn a_review_of_another_plan_does_not_bind() {
    let w = World::new("other");
    rust_cli(&w);
    let a = w.plan("a.md", BODY, "repos=tool");
    let b = w.plan(
        "b.md",
        &BODY.replace("The work.", "Other work."),
        "repos=tool",
    );
    let mut t = Transcript::default();
    full_panel(&w, &mut t, &a);
    assert!(
        matches!(w.present(&b, &t), Said::Deny(ref why) if why.contains("missing for repos=tool: backend, lang:rust, tui, unix"))
    );
}

#[test]
fn only_a_completion_claude_code_recorded_counts() {
    let w = World::new("completion");
    w.repo("docs", &[("README.md", "x\n")]);
    let plan = w.plan("p.md", BODY, "repos=docs");
    for (done, passes) in [
        (Done::Running, false),
        (Done::ForegroundFailed, false),
        (Done::Forged, false),
        (Done::NotifiedUser, true),
        (Done::NotifiedAttachment, true),
        (Done::Foreground, true),
    ] {
        let mut t = Transcript::default();
        let label = match done {
            Done::Running => "running",
            Done::ForegroundFailed => "foreground-failed",
            Done::Forged => "forged",
            Done::NotifiedUser => "user",
            Done::NotifiedAttachment => "attachment",
            Done::Foreground => "foreground",
        };
        t.review(&w, "plan-review-backend", &w.block(&plan, None), done);
        let said = w.present(&plan, &t);
        assert_eq!(said == Said::Silent, passes, "{label}: {said:?}");
        // Start the next case from no baseline.
        let _ = std::fs::remove_dir_all(w.store());
    }
}

#[test]
fn a_failed_background_review_does_not_count() {
    let w = World::new("failed");
    w.repo("docs", &[("README.md", "x\n")]);
    let plan = w.plan("p.md", BODY, "repos=docs");
    let mut t = Transcript::default();
    t.review(
        &w,
        "plan-review-backend",
        &w.block(&plan, None),
        Done::Running,
    );
    let id = format!("toolu_{:04}", w.ids.get());
    t.lines.push(
        serde_json::json!({
            "type": "user", "origin": {"kind": "task-notification"},
            "message": {"role": "user", "content": Transcript::notification(&id, "failed")}
        })
        .to_string(),
    );
    assert!(matches!(w.present(&plan, &t), Said::Deny(_)));
}

#[test]
fn a_missing_machine_comment_is_refused_counted_then_asked() {
    let w = World::new("antitrap");
    w.repo("docs", &[("README.md", "x\n")]);
    let bare = w.root.join("plans").join("p.md");
    std::fs::write(&bare, BODY).unwrap();
    let t = Transcript::default();
    for _ in 0..2 {
        assert!(
            matches!(w.present(&bare, &t), Said::Deny(ref why) if why.contains("machine comment"))
        );
    }
    let said = w.present(&bare, &t);
    assert!(
        matches!(said, Said::Ask(ref why) if why.contains("UNREVIEWED: refused 2×") && why.contains("Approve only to accept an unreviewed plan")),
        "{said:?}"
    );
    // Also once the plan is fixed but still unreviewed: it stays with the person.
    let plan = w.plan("p.md", BODY, "repos=docs");
    assert!(
        matches!(w.present(&plan, &t), Said::Ask(ref why) if why.contains("missing for repos=docs: backend"))
    );
    // A verified pass resets the count.
    let mut t = Transcript::default();
    t.review(
        &w,
        "plan-review-backend",
        &w.block(&plan, None),
        Done::Foreground,
    );
    assert_eq!(w.present(&plan, &t), Said::Silent);
    let changed = w.plan("p.md", &BODY.replace("The work.", "Changed."), "repos=docs");
    assert!(
        matches!(w.present(&changed, &t), Said::Deny(_)),
        "the count was reset by the pass"
    );
}

#[test]
fn invalid_repos_and_a_missing_section_are_the_models_to_fix() {
    let w = World::new("invalid");
    for comment in ["repos=../etc", "repos=-x", "adds=ui"] {
        let plan = w.plan(&format!("{}.md", comment.len()), BODY, comment);
        assert!(
            matches!(w.present(&plan, &Transcript::default()), Said::Deny(_)),
            "{comment}"
        );
    }
    w.repo("docs", &[("README.md", "x\n")]);
    let plan = w.plan(
        "nosection.md",
        &BODY.replace("## Review panel", "## Panel"),
        "repos=docs",
    );
    assert!(
        matches!(w.present(&plan, &Transcript::default()), Said::Deny(ref why) if why.contains("no `## Review panel` section"))
    );
}

#[test]
fn a_repository_not_created_yet_takes_its_areas_from_adds() {
    let w = World::new("newrepo");
    let plan = w.plan("p.md", BODY, "repos=fresh");
    assert!(
        matches!(w.present(&plan, &Transcript::default()), Said::Deny(ref why) if why.contains("adds=lang:<language>"))
    );
    let plan = w.plan("p.md", BODY, "repos=fresh adds=lang:rust,cli");
    assert!(
        matches!(w.present(&plan, &Transcript::default()), Said::Deny(ref why) if why.contains("missing for repos=fresh: backend, lang:rust, tui, unix"))
    );
}

#[test]
fn the_default_tree_falls_back_to_head_and_an_unborn_repository_is_empty() {
    let w = World::new("fallback");
    // No remote at all: HEAD is the tree.
    let local = w.root.join("repos").join("local");
    std::fs::create_dir_all(&local).unwrap();
    git(&local, &["init", "-q", "-b", "main", "--template=", "."]);
    git(&local, &["config", "user.email", "t@t"]);
    git(&local, &["config", "user.name", "t"]);
    w.commit(
        &local,
        &[("go.mod", "module x\n"), ("main.go", "package main\n")],
        false,
    );
    let plan = w.plan("p.md", BODY, "repos=local");
    assert!(
        matches!(w.present(&plan, &Transcript::default()), Said::Deny(ref why) if why.contains("missing for repos=local: backend, lang:go, tui, unix"))
    );
    // Unborn: only backend.
    let unborn = w.root.join("repos").join("unborn");
    std::fs::create_dir_all(&unborn).unwrap();
    git(&unborn, &["init", "-q", "--template=", "."]);
    let plan = w.plan("q.md", BODY, "repos=unborn");
    assert!(
        matches!(w.present(&plan, &Transcript::default()), Said::Deny(ref why) if why.contains("missing for repos=unborn: backend ("))
    );
}

#[test]
fn what_the_hook_cannot_check_goes_to_the_person() {
    let w = World::new("external");
    w.repo("docs", &[("README.md", "x\n")]);
    let plan = w.plan("p.md", BODY, "repos=docs");
    let asked = |s: Said, what: &str| {
        assert!(
            matches!(s, Said::Ask(ref why) if why.contains(what)),
            "{what}: {s:?}"
        )
    };
    asked(
        w.present_with(Some(&plan), None, None),
        "no transcript_path",
    );
    asked(
        w.present_with(Some(&plan), Some(&w.root.join("nope.jsonl")), None),
        "transcript cannot be read",
    );
    asked(
        w.present_with(None, Some(&w.root.join("nope.jsonl")), None),
        "no planFilePath",
    );
    let t = w.root.join("t.jsonl");
    std::fs::write(&t, "").unwrap();
    asked(
        w.present_with(Some(&plan), Some(&t), Some("/nonexistent")),
        "git could not be run",
    );
    assert!(
        !w.store().join("by-path").exists(),
        "an ask writes no baseline"
    );
    assert!(
        !w.store().join("refusals").exists(),
        "an ask is not a refusal"
    );
}

#[test]
fn a_skip_the_person_asked_for_is_theirs_to_confirm() {
    let w = World::new("skip");
    w.repo("docs", &[("README.md", "x\n")]);
    let body = BODY.replace(
        "👉 Decide: none",
        "⚠ unreviewed: the person asked to skip the panel\n👉 Decide: none",
    );
    let plan = w.plan("p.md", &body, "repos=docs");
    assert!(
        matches!(w.present(&plan, &Transcript::default()), Said::Ask(ref why) if why.contains("marked unreviewed"))
    );
    let mut t = Transcript::default();
    t.review(
        &w,
        "plan-review-backend",
        &w.block(&plan, None),
        Done::Foreground,
    );
    assert!(
        matches!(w.present(&plan, &t), Said::Ask(_)),
        "even reviewed, the marker asks"
    );
    assert!(
        !w.store().join("by-path").exists(),
        "an ask writes no baseline"
    );
}

#[test]
fn the_baseline_is_private() {
    let w = World::new("private");
    w.repo("docs", &[("README.md", "x\n")]);
    let plan = w.plan("p.md", BODY, "repos=docs");
    let mut t = Transcript::default();
    t.review(
        &w,
        "plan-review-backend",
        &w.block(&plan, None),
        Done::Foreground,
    );
    assert_eq!(w.present(&plan, &t), Said::Silent);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&w.store()), 0o700);
        for dir in ["by-path", "by-body"] {
            let d = w.store().join(dir);
            assert_eq!(mode(&d), 0o700);
            for f in std::fs::read_dir(&d).unwrap() {
                assert_eq!(mode(&f.unwrap().path()), 0o600);
            }
        }
    }
}

#[test]
fn plan_sha_reads_stdin_and_ignores_line_endings() {
    let w = World::new("sha");
    let plan = w.plan("p.md", BODY, "repos=docs");
    let crlf = w.root.join("plans").join("crlf.md");
    std::fs::write(
        &crlf,
        std::fs::read_to_string(&plan)
            .unwrap()
            .replace('\n', "\r\n"),
    )
    .unwrap();
    assert_eq!(w.short(&plan), w.short(&crlf));
    let mut child = w
        .bin()
        .args(["plan-sha", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(BODY.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let full = String::from_utf8(out.stdout).unwrap();
    assert_eq!(full.trim().len(), 64);
    assert!(
        full.starts_with(&w.short(&plan)),
        "the comment does not count"
    );
    let usage = w
        .bin()
        .args(["plan-sha", "--short", "--block", "x"])
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2));
    let missing = w
        .bin()
        .args(["plan-sha", "/nonexistent/plan.md"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
}

/// Run with `cargo test --release -- --ignored`: a 50 MB transcript, with
/// nothing in it about reviews, read under 300 ms.
#[test]
#[ignore]
fn a_large_transcript_is_read_quickly() {
    let w = World::new("large");
    w.repo("docs", &[("README.md", "x\n")]);
    let plan = w.plan("p.md", BODY, "repos=docs");
    let line = serde_json::json!({
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "id": "x", "name": "Bash", "input": {"command": "ls ".repeat(200)}}]}
    })
    .to_string();
    let mut t = Transcript::default();
    while t.lines.len() * line.len() < 50_000_000 {
        t.lines.push(line.clone());
    }
    let file = w.root.join("big.jsonl");
    std::fs::write(&file, t.lines.join("\n") + "\n").unwrap();
    let _ = w.present_with(Some(&plan), Some(&file), None); // warm the page cache
    let start = std::time::Instant::now();
    let _ = w.present_with(Some(&plan), Some(&file), None);
    let took = start.elapsed();
    eprintln!("50 MB transcript + git: {took:?}");
    assert!(
        took.as_millis() < 300 + 400,
        "took {took:?} (300 ms read + git)"
    );
}

#[test]
fn plan_panel_lists_what_the_hook_will_require() {
    let w = World::new("panel");
    rust_cli(&w);
    let panel = |plan: &Path| {
        let out = w.bin().arg("plan-panel").arg(plan).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let plan = w.plan("p.md", BODY, "repos=tool");
    let full = panel(&plan);
    assert!(
        full.starts_with(&format!(
            "repos=tool areas=cli,lang:rust body={} panel=full\n",
            w.short(&plan)
        )),
        "{full}"
    );
    assert!(full.ends_with("plan-review-backend\nplan-review-language --lang rust\nplan-review-tui\nplan-review-unix\n"), "{full}");

    // Launching exactly what it lists passes the hook.
    let mut t = Transcript::default();
    for line in full.lines().skip(1) {
        let mut parts = line.split_whitespace();
        let agent = parts.next().unwrap();
        let lang = parts.nth(1);
        t.review(&w, agent, &w.block(&plan, lang), Done::Foreground);
    }
    assert_eq!(w.present(&plan, &t), Said::Silent);
    assert!(panel(&plan).trim_end().ends_with("panel=current"));

    // A change the person asks for, adding an interface: the delta.
    let plan = w.plan(
        "p.md",
        &BODY.replace("The work.", "Changed."),
        "repos=tool adds=ui",
    );
    let delta = panel(&plan);
    assert!(delta.contains("panel=delta\n"), "{delta}");
    assert!(delta.ends_with("plan-review-backend\nplan-review-game-ux\nplan-review-react\nplan-review-ui-design\nplan-review-ux-research\n"), "{delta}");
    for line in delta.lines().skip(1) {
        t.review(&w, line, &w.block(&plan, None), Done::Foreground);
    }
    assert_eq!(
        w.present(&plan, &t),
        Said::Silent,
        "the delta it listed is the delta the hook wanted"
    );

    let bad = w
        .bin()
        .arg("plan-panel")
        .arg(w.plan("q.md", BODY, "adds=ui"))
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("names no repository"));
}

#[test]
fn a_formatted_copy_keeps_its_review() {
    let w = World::new("formatted");
    rust_cli(&w);
    let plan = w.plan("p.md", BODY, "repos=tool");
    let mut t = Transcript::default();
    full_panel(&w, &mut t, &plan);
    assert_eq!(w.present(&plan, &t), Said::Silent);
    // What a formatter does: blank lines, another list marker, emphasis.
    let formatted = BODY
        .replace("The work.", "The\n_work_.\n\n")
        .replace("- [ ] one", "* [ ] one");
    let plan = w.plan("p.md", &formatted, "repos=tool");
    assert_eq!(
        w.present(&plan, &t),
        Said::Silent,
        "formatting is not an edit"
    );
    let edited = w.plan("p.md", &formatted.replace("work", "works"), "repos=tool");
    assert!(
        matches!(w.present(&edited, &t), Said::Deny(ref why) if why.contains("stale")),
        "a word is"
    );
}

#[test]
fn an_edited_plan_in_a_new_session_is_a_delta_not_a_full_panel() {
    let w = World::new("titled");
    rust_cli(&w);
    let panel = |plan: &Path| {
        let out = w.bin().arg("plan-panel").arg(plan).output().unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let plan = w.plan("p.md", BODY, "repos=tool");
    let mut t = Transcript::default();
    full_panel(&w, &mut t, &plan);
    assert_eq!(w.present(&plan, &t), Said::Silent);

    // A new session: a new random file name, one line edited.
    let moved = w.plan(
        "resumed-name.md",
        &BODY.replace("The work.", "The work, edited."),
        "repos=tool",
    );
    let delta = panel(&moved);
    assert!(delta.contains("panel=delta\n"), "{delta}");
    assert!(
        delta.ends_with("panel=delta\nplan-review-backend\n"),
        "{delta}"
    );
    let mut fresh = Transcript::default();
    fresh.review(
        &w,
        "plan-review-backend",
        &w.block(&moved, None),
        Done::Foreground,
    );
    assert_eq!(w.present(&moved, &fresh), Said::Silent);

    // Another plan at another path: no history, the whole panel.
    let other = w.plan(
        "other-name.md",
        &BODY.replace("# Plan", "# Another plan"),
        "repos=tool",
    );
    assert!(panel(&other).contains("panel=full\n"));
}

#[test]
fn a_review_bound_before_2_25_still_passes_and_is_journalled() {
    let w = World::new("legacy");
    rust_cli(&w);
    let plan = w.plan("p.md", BODY, "repos=tool");
    let legacy = |plan: &Path| {
        let out = w
            .bin()
            .args(["plan-sha", "--legacy"])
            .arg(plan)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stderr).trim(),
            "amont-agent: --legacy is removed after the transition"
        );
        let s = String::from_utf8(out.stdout).unwrap();
        assert_eq!(s.lines().count(), 1, "one line on stdout");
        s.trim().to_string()
    };
    let old = legacy(&plan);
    assert_ne!(
        old,
        w.block(&plan, None)
            .split("sha=")
            .nth(1)
            .unwrap()
            .trim_end_matches(">>>")
    );
    // Reviews whose blocks a 2.24 binary printed: the byte sha.
    let mut t = Transcript::default();
    let at_old = |b: String| {
        let (head, _) = b.split_once("sha=").unwrap();
        let lang = b
            .split_once(" lang=")
            .map(|(_, l)| format!(" lang={}", l.trim_end_matches(">>>")));
        format!("{head}sha={old}{}>>>", lang.unwrap_or_default())
    };
    t.review(
        &w,
        "plan-review-language",
        &at_old(w.block(&plan, Some("rust"))),
        Done::Foreground,
    );
    for agent in ["plan-review-backend", "plan-review-unix", "plan-review-tui"] {
        t.review(&w, agent, &at_old(w.block(&plan, None)), Done::Foreground);
    }
    assert_eq!(w.present(&plan, &t), Said::Silent);
    let journal = std::fs::read_to_string(
        w.root
            .join("claude")
            .join("amont-agent")
            .join("journal.log"),
    )
    .unwrap();
    let line = journal
        .lines()
        .rfind(|l| l.contains("plan-review-panel"))
        .unwrap();
    assert!(line.contains("match=legacy"), "{line}");

    let both = w
        .bin()
        .args(["plan-sha", "--legacy", "--block"])
        .arg(&plan)
        .output()
        .unwrap();
    assert_eq!(both.status.code(), Some(2));
    assert!(both.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&both.stderr).contains("--legacy only prints a sha; drop --block")
    );
}
