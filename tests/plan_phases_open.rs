//! `plan-phases-open`, end to end through the real binary: a `Stop` payload
//! against a temp repository with a bare `origin`, whose default branch is
//! `main`, and a branch carrying a plan (ADR-0022).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

const SESSION: &str = "sess-1";

/// Phases one and two, the second open.
fn plan_text(branch: &str, status: &str, phases: &str) -> String {
    format!(
        "---\nstatus: {status}\nbranch: {branch}\n---\n# Plan\n\n## Phases\n{phases}\n\n## Verification\n- [ ] drive it\n"
    )
}

const OPEN_2: &str = "- [x] Phase 1 — plumbing\n- [ ] Phase 2 — the rule";

struct World {
    root: PathBuf,
    repo: PathBuf,
    claude: PathBuf,
    global: PathBuf,
}

struct Reply {
    stdout: String,
}

impl Reply {
    fn silent(&self) -> bool {
        self.stdout.is_empty()
    }
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("{e}: {:?}", self.stdout))
    }
    fn blocks(&self) -> bool {
        !self.silent() && self.json()["decision"] == "block"
    }
    fn note(&self) -> Option<String> {
        self.json()["systemMessage"].as_str().map(str::to_string)
    }
}

impl World {
    /// `main` on a bare `origin`, and a checkout on `feat/x`.
    fn new(name: &str) -> World {
        let root = std::env::temp_dir().join(format!(
            "amont-agent-phases-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let remote = root.join("origin.git");
        let claude = root.join("claude");
        for d in [&repo, &claude] {
            std::fs::create_dir_all(d).unwrap();
        }
        let global = root.join("global");
        std::fs::write(&global, "").unwrap();
        Command::new("git")
            .args(["init", "-q", "--bare", "--template=", "-b", "main"])
            .arg(&remote)
            .output()
            .expect("git init --bare");
        git(&repo, &["init", "-q", "-b", "main", "--template=", "."]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["config", "user.name", "t"]);
        git(
            &repo,
            &["remote", "add", "origin", &remote.display().to_string()],
        );
        std::fs::create_dir_all(repo.join("docs/plans")).unwrap();
        std::fs::write(repo.join("docs/plans/README.md"), "# Plans\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "init"]);
        git(&repo, &["push", "-q", "origin", "main"]);
        git(&repo, &["fetch", "-q", "origin"]);
        git(&repo, &["checkout", "-q", "-b", "feat/x"]);
        World {
            root,
            repo,
            claude,
            global,
        }
    }

    fn plan(&self, name: &str, text: &str) {
        std::fs::write(self.repo.join("docs/plans").join(name), text).unwrap();
    }

    fn commit(&self) {
        git(&self.repo, &["add", "."]);
        git(&self.repo, &["commit", "-qm", "plan"]);
    }

    fn run(&self, payload: &serde_json::Value, claude: &Path) -> Reply {
        let mut child = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .arg("hook")
            .env("CLAUDE_CONFIG_DIR", claude)
            .env("GIT_CONFIG_GLOBAL", &self.global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("AMONT_AGENT_OFF")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the binary runs");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0));
        Reply {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        }
    }

    fn payload(&self, session: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut p = serde_json::json!({
            "hook_event_name": "Stop",
            "session_id": session,
            "cwd": self.repo,
        });
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        p
    }

    fn stop_with(&self, session: &str, extra: serde_json::Value) -> Reply {
        self.run(&self.payload(session, extra), &self.claude)
    }

    fn stop(&self) -> Reply {
        self.stop_with(SESSION, serde_json::json!({}))
    }

    fn prompt(&self, session: &str) -> Reply {
        self.run(
            &serde_json::json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": session,
                "cwd": self.repo,
                "prompt": "continue",
            }),
            &self.claude,
        )
    }

    fn state_dir(&self) -> PathBuf {
        self.claude.join("amont-agent/plan-phases-open")
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A world on `feat/x` with a committed plan whose Phase 2 is open.
fn with_open_plan(name: &str) -> World {
    let w = World::new(name);
    w.plan("2026-10-07-x.md", &plan_text("feat/x", "active", OPEN_2));
    w.commit();
    w
}

#[test]
fn an_open_phase_blocks_and_is_named() {
    let w = with_open_plan("block");
    let r = w.stop();
    assert!(r.blocks(), "{}", r.stdout);
    let v = r.json();
    let reason = v["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("amont-agent/plan-phases-open:"),
        "{reason}"
    );
    assert!(reason.contains("2026-10-07-x.md"), "{reason}");
    assert!(reason.contains("\"Phase 2\""), "{reason}");
    assert!(reason.contains("WAITING:"), "{reason}");
    assert!(v.get("hookSpecificOutput").is_none());
}

#[test]
fn a_ticked_plan_is_silent() {
    let w = with_open_plan("tick");
    w.plan(
        "2026-10-07-x.md",
        &plan_text("feat/x", "active", "- [x] Phase 1\n- [x] Phase 2"),
    );
    assert!(w.stop().silent());
}

#[test]
fn a_human_decision_phase_is_silent() {
    let w = with_open_plan("human");
    w.plan(
        "2026-10-07-x.md",
        &plan_text(
            "feat/x",
            "active",
            "- [x] Phase 1\n- [ ] 🧑 decision: which",
        ),
    );
    assert!(w.stop().silent());
}

#[test]
fn plan_mode_is_silent() {
    let w = with_open_plan("planmode");
    let r = w.stop_with(SESSION, serde_json::json!({"permission_mode": "plan"}));
    assert!(r.silent(), "{}", r.stdout);
}

#[test]
fn background_tasks_are_silent() {
    let w = with_open_plan("background");
    let r = w.stop_with(
        SESSION,
        serde_json::json!({"background_tasks": [{"id": "b1"}]}),
    );
    assert!(r.silent(), "{}", r.stdout);
    // An empty list is nothing running.
    let r = w.stop_with(SESSION, serde_json::json!({"background_tasks": []}));
    assert!(r.blocks(), "{}", r.stdout);
}

#[test]
fn a_waiting_line_with_leading_spaces_is_silent() {
    let w = with_open_plan("waiting");
    let r = w.stop_with(
        SESSION,
        serde_json::json!({"last_assistant_message": "Preview is up.\n    WAITING: preview at :5173\n"}),
    );
    assert!(r.silent(), "{}", r.stdout);
    let r = w.stop_with(
        SESSION,
        serde_json::json!({"last_assistant_message": "I am WAITING: for nothing"}),
    );
    assert!(r.blocks(), "{}", r.stdout);
}

#[test]
fn a_pointer_file_has_no_phases_here() {
    let w = World::new("pointer");
    w.plan(
        "2026-10-07-p.md",
        "---\ncanonical: decisions:docs/plans/p.md\nphases: [2]\nstatus: active\nbranch: feat/x\n---\n## Phases\n- [ ] Phase 2 — remote\n",
    );
    w.commit();
    assert!(w.stop().silent());
}

#[test]
fn the_default_branch_is_silent() {
    let w = with_open_plan("main");
    git(&w.repo, &["checkout", "-q", "main"]);
    // The plan is carried over as an untracked file.
    w.plan("2026-10-07-y.md", &plan_text("main", "active", OPEN_2));
    assert!(w.stop().silent());
}

/// A clone sets `origin/HEAD`, which a fetch does not: one `rev-parse` then
/// names the branch and the base, and the general path is never taken.
#[test]
fn with_origin_head_set_a_branch_still_blocks() {
    let w = with_open_plan("head-set");
    git(&w.repo, &["remote", "set-head", "origin", "main"]);
    assert!(w.stop().blocks());
}

#[test]
fn with_origin_head_set_the_default_branch_is_silent() {
    let w = with_open_plan("head-set-main");
    git(&w.repo, &["remote", "set-head", "origin", "main"]);
    git(&w.repo, &["checkout", "-q", "main"]);
    w.plan("2026-10-07-y.md", &plan_text("main", "active", OPEN_2));
    assert!(w.stop().silent());
}

#[test]
fn an_untracked_plan_blocks() {
    let w = World::new("untracked");
    w.plan("2026-10-07-x.md", &plan_text("feat/x", "active", OPEN_2));
    let r = w.stop();
    assert!(r.blocks(), "{}", r.stdout);
}

#[test]
fn two_active_plans_choose_by_branch() {
    let w = World::new("two");
    w.plan(
        "2026-10-01-a.md",
        &plan_text("feat/other", "active", "- [ ] Alpha — first"),
    );
    w.plan(
        "2026-10-02-b.md",
        &plan_text("feat/x", "active", "- [ ] Beta — second"),
    );
    let r = w.stop();
    let reason = r.json()["reason"].as_str().unwrap().to_string();
    assert!(
        reason.contains("2026-10-02-b.md") && reason.contains("\"Beta\""),
        "{reason}"
    );

    // No `branch:` match: the oldest file name.
    w.plan(
        "2026-10-02-b.md",
        &plan_text("feat/else", "active", "- [ ] Beta — second"),
    );
    let reason = w.stop_with("sess-2", serde_json::json!({})).json()["reason"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(reason.contains("2026-10-01-a.md"), "{reason}");
}

#[test]
fn an_unusable_session_id_touches_nothing() {
    let w = with_open_plan("badid");
    let agent_dir = w.claude.join("amont-agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    // `../x` from the counter directory lands here.
    let sentinel = agent_dir.join("x");
    std::fs::write(&sentinel, "keep").unwrap();
    for id in ["", "../x"] {
        assert!(
            w.stop_with(id, serde_json::json!({})).silent(),
            "stop {id:?}"
        );
        assert!(w.prompt(id).silent(), "prompt {id:?}");
        assert_eq!(
            std::fs::read_to_string(&sentinel).unwrap(),
            "keep",
            "{id:?}"
        );
        let names: Vec<_> = std::fs::read_dir(&agent_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["x"], "nothing else written for {id:?}");
    }
}

#[test]
fn a_missing_cwd_is_silent() {
    let w = with_open_plan("nocwd");
    let mut p = w.payload(SESSION, serde_json::json!({}));
    p.as_object_mut().unwrap().remove("cwd");
    assert!(w.run(&p, &w.claude).silent());
}

#[test]
fn an_unwritable_config_dir_is_silent() {
    let w = with_open_plan("unwritable");
    let file = w.root.join("not-a-dir");
    std::fs::write(&file, "").unwrap();
    let r = w.run(
        &w.payload(SESSION, serde_json::json!({})),
        &file.join("claude"),
    );
    assert!(r.silent(), "{}", r.stdout);
}

#[test]
fn three_blocks_then_a_release_then_silence() {
    let w = with_open_plan("cap");
    for n in 1..=3 {
        let r = w.stop_with(SESSION, serde_json::json!({"stop_hook_active": n > 1}));
        assert!(r.blocks(), "block {n}: {}", r.stdout);
    }
    let r = w.stop();
    let v = r.json();
    assert!(v.get("decision").is_none(), "{}", r.stdout);
    let note = r.note().expect("a systemMessage");
    assert!(
        note.contains("\"Phase 2\"")
            && note.contains("2026-10-07-x.md")
            && note.contains("after 3 continuations"),
        "{note}"
    );
    assert!(w.stop().silent());
    assert!(w.stop().silent());
    let journal =
        std::fs::read_to_string(w.claude.join("amont-agent/journal.log")).unwrap_or_default();
    for outcome in ["denied", "released", "watched"] {
        assert!(journal.contains(outcome), "{outcome} in:\n{journal}");
    }
    assert!(journal.contains("stop_hook_active=true"), "{journal}");
}

#[test]
fn the_counter_resets_on_a_new_phase() {
    let w = with_open_plan("newphase");
    for _ in 0..3 {
        assert!(w.stop().blocks());
    }
    assert!(w.stop().note().is_some());
    w.plan(
        "2026-10-07-x.md",
        &plan_text(
            "feat/x",
            "active",
            "- [x] Phase 1\n- [x] Phase 2\n- [ ] Phase 3 — next",
        ),
    );
    for n in 1..=3 {
        let r = w.stop();
        assert!(r.blocks(), "block {n}: {}", r.stdout);
        assert!(r.json()["reason"].as_str().unwrap().contains("\"Phase 3\""));
    }
    assert!(w.stop().note().is_some());
}

#[test]
fn the_counter_resets_on_a_prompt() {
    let w = with_open_plan("prompt");
    for _ in 0..3 {
        assert!(w.stop().blocks());
    }
    assert!(w.stop().note().is_some());
    assert!(w.stop().silent());
    assert!(w.state_dir().join(SESSION).exists());
    assert!(w.prompt(SESSION).silent());
    assert!(!w.state_dir().join(SESSION).exists());
    assert!(w.stop().blocks());
}

#[test]
fn the_counter_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let w = with_open_plan("mode");
    assert!(w.stop().blocks());
    let mode = std::fs::metadata(w.state_dir().join(SESSION))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn a_detached_head_is_silent() {
    let w = with_open_plan("detached");
    git(&w.repo, &["checkout", "-q", "--detach"]);
    assert!(w.stop().silent());
}

#[test]
fn a_repository_without_plans_is_silent() {
    let w = World::new("noplans");
    std::fs::remove_dir_all(w.repo.join("docs")).unwrap();
    assert!(w.stop().silent());
}

#[test]
fn a_deleted_plan_beside_an_active_one_still_blocks() {
    let w = World::new("deleted");
    git(&w.repo, &["checkout", "-q", "main"]);
    w.plan(
        "2026-09-01-old.md",
        &plan_text("main", "active", "- [ ] Old — gone"),
    );
    w.commit();
    git(&w.repo, &["push", "-q", "origin", "main"]);
    git(&w.repo, &["fetch", "-q", "origin"]);
    git(&w.repo, &["checkout", "-q", "feat/x"]);
    git(&w.repo, &["merge", "-q", "--ff-only", "main"]);
    git(&w.repo, &["rm", "-q", "docs/plans/2026-09-01-old.md"]);
    w.plan("2026-10-07-x.md", &plan_text("feat/x", "active", OPEN_2));
    let r = w.stop();
    assert!(r.blocks(), "{}", r.stdout);
    assert!(r.json()["reason"]
        .as_str()
        .unwrap()
        .contains("2026-10-07-x.md"));
}

#[test]
fn at_advise_the_person_is_told_and_the_agent_stops() {
    let w = with_open_plan("advise");
    std::fs::write(
        &w.global,
        "[amont \"agent.plan-phases-open\"]\n\tstance = advise\n",
    )
    .unwrap();
    let r = w.stop();
    let v = r.json();
    assert!(v.get("decision").is_none(), "{}", r.stdout);
    let note = r.note().expect("a systemMessage");
    assert!(
        note.contains("\"Phase 2\"") && note.contains("2026-10-07-x.md"),
        "{note}"
    );
    assert!(!note.contains("WAITING:"), "{note}");
    // It does not count: every stop says so.
    assert!(w.stop().note().is_some());
}

#[test]
fn at_observe_it_only_journals() {
    let w = with_open_plan("observe");
    std::fs::write(
        &w.global,
        "[amont \"agent.plan-phases-open\"]\n\tstance = observe\n",
    )
    .unwrap();
    assert!(w.stop().silent());
}
