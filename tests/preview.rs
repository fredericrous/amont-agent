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

/// A guide with every section `work.preview-is-guided` asks for.
const GUIDE: &str = "\
# Settings panel

## 📍 Where we are
duro-app, branch `feat/x`, plan **Settings panel**: the Save button moves.

## What you should see
The **Save** button sits top right instead of at the bottom.

## 👉 Try it
1. Open http://localhost:5173/settings
   - You should see the Settings form.
2. Click **Save**, top right.
   - A green toast says Saved.

![the open panel](panel.png)

## Reference
![before](before.png)
![after](after.png)

## Already checked
- the console is clean on /settings
- look especially at the toast on a narrow window
";

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

    /// Write `text` as a guide outside the worktree and return its path.
    fn guide_with(&self, text: &str) -> PathBuf {
        let dir = self.root.join("guides");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guide.md");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn guide(&self) -> PathBuf {
        self.guide_with(GUIDE)
    }

    /// `preview register --url <url> --guide <guide>`, in the worktree.
    fn register_cli(&self, url: &str, guide: &Path) -> (i32, String, String) {
        self.run(
            &[
                "preview",
                "register",
                "--url",
                url,
                "--guide",
                &guide.display().to_string(),
            ],
            None,
        )
    }

    fn hook(&self, payload: serde_json::Value) -> String {
        self.run(&["hook"], Some(&payload.to_string())).1
    }

    fn pre_bash(&self, session: &str, tool_use_id: &str, command: &str) -> String {
        self.pre_bash_in(&self.work, session, tool_use_id, command)
    }

    /// A PreToolUse Bash payload whose session cwd is `cwd`.
    fn pre_bash_in(&self, cwd: &Path, session: &str, tool_use_id: &str, command: &str) -> String {
        self.hook(serde_json::json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "cwd": cwd, "session_id": session, "prompt_id": "p1",
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
        self.post_bash_in(&self.work, session, prompt, tool_use_id, command, stdout)
    }

    /// A PostToolUse Bash payload whose session cwd is `cwd`.
    fn post_bash_in(
        &self,
        cwd: &Path,
        session: &str,
        prompt: &str,
        tool_use_id: &str,
        command: &str,
        stdout: &str,
    ) -> String {
        self.hook(serde_json::json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "cwd": cwd, "session_id": session, "prompt_id": prompt,
            "tool_use_id": tool_use_id, "permission_mode": "default",
            "tool_input": {"command": command},
            "tool_response": {"stdout": stdout, "stderr": "", "interrupted": false}
        }))
    }

    /// Validate a preview through the CLI and bind it through the hook, the
    /// way a session does. Returns the registration id.
    fn register(&self, session: &str, prompt: &str) -> String {
        let record = self.guide();
        // Quoted as a session writes it: an unquoted Windows path's
        // backslashes are shell escapes.
        let command = format!(
            "amont-agent preview register --url http://localhost:5173/ --guide '{}'",
            record.display()
        );
        let (code, out, err) = self.register_cli("http://localhost:5173/", &record);
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
fn a_change_is_judged_by_its_nearest_package_not_the_repository() {
    // Seen 2026-09-28 in duro-design-system: the root has a `dev` script, a
    // push touched only `packages/cli/src/**` (a CLI) and was advised.
    let w = World::new("per-package");
    w.commit(
        "packages/cli/package.json",
        r#"{"name":"cli","bin":{"x":"dist/bin.js"},
            "devDependencies":{"@duro-app/ui":"workspace:^"},
            "peerDependencies":{"@duro-app/ui":"workspace:^"},
            "peerDependenciesMeta":{"@duro-app/ui":{"optional":true}}}"#,
    );
    w.commit(
        "packages/ui/package.json",
        r#"{"name":"ui","peerDependencies":{"react":"^19"}}"#,
    );
    w.commit(
        "packages/cli/src/commands/doctor.ts",
        "export const x = 1\n",
    );
    assert!(!w.push_advised("s"), "{}", w.journal());

    w.commit(
        "packages/ui/src/Button.tsx",
        "export const B = () => null\n",
    );
    assert!(w.push_advised("s"));
}

#[test]
fn the_package_is_read_from_the_pushed_commit_not_the_worktree() {
    let w = World::new("per-package-commit");
    w.commit("packages/cli/package.json", r#"{"name":"cli"}"#);
    w.commit("packages/cli/src/a.ts", "export const x = 1\n");
    // The worktree now claims a dev script the commit does not carry.
    std::fs::write(
        w.work.join("packages/cli/package.json"),
        r#"{"scripts":{"dev":"vite"}}"#,
    )
    .unwrap();
    assert!(!w.push_advised("s"));
}

#[test]
fn a_comment_only_change_is_not_an_interface_change() {
    // Seen 2026-09-28: application-landscape #301 changed only comments
    // under app/ and was advised.
    let w = World::new("comment-only");
    w.commit(
        "app/routes/graph.tsx",
        "/* graph (plan-90d C). */\nexport default () => <p>Graph</p>\n",
    );
    git(&w.work, &["push", "-q", "-u", "origin", "feat/x"]);
    w.commit(
        "app/routes/graph.tsx",
        "/* graph (90-day-table-stakes-plan C). */\nexport default () => <p>Graph</p>\n",
    );
    assert!(!w.push_advised("s"), "{}", w.journal());

    w.commit(
        "app/routes/graph.tsx",
        "/* graph (90-day-table-stakes-plan C). */\nexport default () => <p>The graph</p>\n",
    );
    assert!(w.push_advised("s"));
}

#[test]
fn a_comment_only_change_on_a_new_branch_is_judged_commit_by_commit() {
    // No tracking ref and no `origin/HEAD`: the files come from `git log`.
    let w = World::new("comment-only-log");
    w.commit("app/a.tsx", "// one\n");
    w.commit("app/a.tsx", "// two\n");
    assert!(!w.push_advised("s"), "{}", w.journal());
    w.commit("app/a.tsx", "// two\nexport const a = 1\n");
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
    let outside = w.guide();
    let (code, _, err) = w.register_cli("http://localhost:1/", &outside);
    assert_eq!(code, 1);
    assert!(err.contains("not clean"), "{err}");

    git(&w.work, &["checkout", "--", "app/a.tsx"]);
    let inside = w.work.join("guide.md");
    std::fs::write(&inside, GUIDE).unwrap();
    git(&w.work, &["add", "guide.md"]);
    git(&w.work, &["commit", "-qm", "guide"]);
    let (code, _, err) = w.register_cli("http://localhost:1/", &inside);
    assert_eq!(code, 1);
    assert!(err.contains("inside the worktree"), "{err}");
    assert!(!w.work.join("index.html").exists(), "no page was written");
}

#[test]
fn a_chained_register_is_not_bound() {
    let w = World::new("chained");
    w.commit("app/a.tsx", "1\n");
    let record = w.guide();
    let (_, out, _) = w.register_cli("http://localhost:1/", &record);
    let id = serde_json::from_str::<serde_json::Value>(&out).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let command = format!(
        "npm test && amont-agent preview register --url http://localhost:1/ --guide '{}'",
        record.display()
    );
    w.post_bash("s", "p1", "reg", &command, &out);
    w.answer("s", "p1", &id, Some("Approve"));
    assert!(w.push_advised("s"));
    assert!(w.journal().contains("not a standalone command"));
}

/// Run `preview register` in the worktree and return (id, printed JSON,
/// guide path).
fn validated(w: &World) -> (String, String, PathBuf) {
    let record = w.guide();
    let (code, out, err) = w.register_cli("http://localhost:1/", &record);
    assert_eq!(code, 0, "{err}");
    let id = serde_json::from_str::<serde_json::Value>(&out).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    (id, out, record)
}

#[test]
fn a_register_after_leading_cds_is_bound_in_the_repository_they_reach() {
    // Seen 2026-09-28: `cd /path/to/worktree && amont-agent preview register …`
    // from a session whose cwd was another directory was `unbound`.
    let w = World::new("cd-chained");
    w.commit("app/a.tsx", "1\n");
    let (id, out, record) = validated(&w);
    let command = format!(
        "cd '{}' && cd app && amont-agent preview register --url http://localhost:1/ --guide '{}'",
        w.root.display(),
        record.display()
    );
    // The session sits in the claude dir, not in the repository.
    w.post_bash_in(&w.root.join("claude"), "s", "p1", "reg", &command, &out);
    assert!(w.journal().contains("registered"), "{}", w.journal());
    w.answer("s", "p1", &id, Some("Approve"));
    assert!(!w.push_advised("s"), "{}", w.journal());
}

#[test]
fn a_cd_chain_with_anything_else_stays_unbound() {
    let w = World::new("cd-other");
    w.commit("app/a.tsx", "1\n");
    let (id, out, record) = validated(&w);
    let tail = format!(
        "amont-agent preview register --url http://localhost:1/ --guide '{}'",
        record.display()
    );
    let work = w.work.display();
    for command in [
        format!("cd '{work}'; {tail}"),
        format!("cd '{work}' && npm test && {tail}"),
        format!("cd '{work}' || {tail}"),
        format!("cd $(git rev-parse --show-toplevel) && {tail}"),
        format!("cd '{work}' && {tail} | tee /dev/null"),
    ] {
        w.post_bash_in(&w.root, "s", "p1", "reg", &command, &out);
    }
    assert!(!w.journal().contains("registered"), "{}", w.journal());
    w.answer("s", "p1", &id, Some("Approve"));
    assert!(w.push_advised("s"));
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

#[test]
fn preview_journal_lines_name_the_repository_pushed_not_the_sessions() {
    // Seen 2026-09-28: an application-landscape push was journalled under
    // the name of the worktree the session sat in.
    let w = World::new("attribution");
    let elsewhere = w.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    git(&elsewhere, &["init", "-q", "--template=", "."]);
    w.commit("app/a.tsx", "export const a = 1\n");

    // Quoted as a session writes it: an unquoted Windows path's backslashes
    // are shell escapes.
    let push = format!("cd '{}' && git push -q -u origin feat/x", w.work.display());
    let out = w.pre_bash_in(&elsewhere, "s", "push1", &push);
    assert!(out.contains("amont-agent/push-preview"), "{out}");
    git(&w.work, &["push", "-q", "-u", "origin", "feat/x"]);
    w.post_bash_in(&elsewhere, "s", "p1", "push1", &push, "");

    let record = w.guide();
    let unbound = format!(
        "cd '{}'; amont-agent preview register --url http://localhost:1/ --guide '{}'",
        w.work.display(),
        record.display()
    );
    w.post_bash_in(&elsewhere, "s", "p1", "reg", &unbound, "{}");

    let journal = w.journal();
    let lines: Vec<&str> = journal
        .lines()
        .filter(|l| l.contains("push-preview") || l.contains("push-published"))
        .collect();
    for needle in ["advised", "published-unapproved", "unbound"] {
        assert!(
            lines.iter().any(|l| l.contains(needle)),
            "{needle} missing:\n{journal}"
        );
    }
    for l in &lines {
        let fields: Vec<&str> = l.split_whitespace().collect();
        assert!(fields.contains(&"app"), "{l}");
        assert!(!fields.contains(&"elsewhere"), "{l}");
    }
}

#[test]
fn the_answer_latency_is_measured_not_read_from_duration_ms() {
    // Seen 2026-09-28: an approval that took minutes was journalled
    // `option,0s` from the payload's `duration_ms`.
    let w = World::new("latency");
    w.commit("app/a.tsx", "export const a = 1\n");
    let id = w.register("s", "p1");
    let q = format!("[preview {id}] Ship {}?", w.label());
    w.ask("PreToolUse", ("s", "p1"), "q1", &q, None, false);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    w.ask("PostToolUse", ("s", "p1"), "q1", &q, Some("Approve"), false);
    // The payload says 4200 ms; the person took just over a second.
    let journal = w.journal();
    let at = journal.find("by option,").expect("an approval line") + "by option,".len();
    let (secs, rest) = journal[at..].split_once("s,").expect("<secs>s,");
    let secs: u64 = secs.parse().expect("whole seconds");
    assert!((1..=3).contains(&secs), "{journal}");
    assert!(rest.starts_with("dur=4200ms"), "{journal}");
}

// --- the guide (work.preview-is-guided) ------------------------------------

/// The bullet lines of a refusal: exactly what the guide lacks.
fn lacks(err: &str) -> Vec<String> {
    err.lines()
        .filter_map(|l| l.strip_prefix("  - "))
        .map(str::to_string)
        .collect()
}

#[test]
fn a_register_without_a_guide_is_refused() {
    let w = World::new("no-guide");
    w.commit("app/a.tsx", "1\n");
    let (code, out, err) = w.run(
        &["preview", "register", "--url", "http://localhost:1/"],
        None,
    );
    assert_eq!(code, 2, "{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("--guide"), "{err}");
}

#[test]
fn a_guide_missing_a_section_is_refused_naming_it() {
    let w = World::new("sections");
    w.commit("app/a.tsx", "1\n");
    let headings = [
        ("Where we are", "## 📍 Where we are"),
        ("What you should see", "## What you should see"),
        ("Try it", "## 👉 Try it"),
        ("Reference", "## Reference"),
        ("Already checked", "## Already checked"),
    ];
    for (name, heading) in headings {
        assert!(GUIDE.contains(heading), "{heading}");
        let text = GUIDE.replace(heading, "## Notes");
        let (code, out, err) = w.register_cli("http://localhost:1/", &w.guide_with(&text));
        assert_eq!(code, 1, "{name}: {err}");
        assert!(out.is_empty(), "{out}");
        assert_eq!(
            lacks(&err),
            vec![format!("the section `## {name}`")],
            "{name}: {err}"
        );
    }
}

#[test]
fn try_it_needs_a_numbered_step_and_a_url() {
    let w = World::new("try-it");
    w.commit("app/a.tsx", "1\n");
    let unnumbered = GUIDE
        .replace("1. Open", "- Open")
        .replace("2. Click", "- Click");
    let (code, _, err) = w.register_cli("http://localhost:1/", &w.guide_with(&unnumbered));
    assert_eq!(code, 1, "{err}");
    let l = lacks(&err);
    assert_eq!(l.len(), 1, "{err}");
    assert!(l[0].starts_with("a numbered step"), "{err}");

    let no_url = GUIDE.replace("http://localhost:5173/settings", "the settings page");
    let (code, _, err) = w.register_cli("http://localhost:1/", &w.guide_with(&no_url));
    assert_eq!(code, 1, "{err}");
    let l = lacks(&err);
    assert_eq!(l.len(), 1, "{err}");
    assert!(l[0].starts_with("an http(s) URL"), "{err}");
}

#[test]
fn the_deprecated_attestation_flag_is_read_as_a_guide() {
    let w = World::new("attestation-alias");
    w.commit("app/a.tsx", "1\n");
    let old = w.root.join("attest.md");
    std::fs::write(&old, "drove /settings; 0 console errors\n").unwrap();
    let args = |p: &Path| {
        vec![
            "preview".to_string(),
            "register".to_string(),
            "--url".to_string(),
            "http://localhost:1/".to_string(),
            "--attestation".to_string(),
            p.display().to_string(),
        ]
    };
    let a = args(&old);
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    let (code, _, err) = w.run(&a, None);
    assert_eq!(code, 1, "a bare attestation is not a guide: {err}");
    assert!(err.contains("deprecated"), "{err}");
    assert_eq!(lacks(&err).len(), 5, "{err}");

    let a = args(&w.guide());
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    let (code, out, err) = w.run(&a, None);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("deprecated"), "{err}");
    assert!(out.contains("\"page\""), "{out}");
}

#[test]
fn a_complete_guide_renders_its_page_beside_it() {
    let w = World::new("render");
    w.commit("app/a.tsx", "1\n");
    let text = GUIDE.replace(
        "## Already checked\n",
        "## Already checked\n- a guide that says <script>alert(1)</script> is text\n",
    );
    let guide = w.guide_with(&text);
    let dir = guide.parent().unwrap();
    for img in ["panel.png", "before.png", "after.png"] {
        std::fs::write(dir.join(img), b"\x89PNG\r\n\x1a\n").unwrap();
    }
    let (code, out, err) = w.register_cli("http://localhost:5173/settings", &guide);
    assert_eq!(code, 0, "{err}");

    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let canon = std::fs::canonicalize(&guide).unwrap();
    let page = canon.parent().unwrap().join("index.html");
    assert_eq!(v["guide"].as_str(), Some(canon.to_str().unwrap()), "{out}");
    assert_eq!(v["page"].as_str(), Some(page.to_str().unwrap()), "{out}");
    let page_url = v["page_url"].as_str().unwrap();
    assert!(page_url.starts_with("file://"), "{page_url}");
    assert!(page_url.ends_with("/index.html"), "{page_url}");
    assert!(Path::new(v["page"].as_str().unwrap()).is_absolute());

    let html = std::fs::read_to_string(&page).unwrap();
    let sha = git(&w.work, &["rev-parse", "HEAD"]);
    assert!(
        html.contains(&format!("<title>app@{} — duro-app", &sha[..7])),
        "{html}"
    );
    assert!(
        html.contains("class=\"open\" href=\"http://localhost:5173/settings\""),
        "{html}"
    );
    assert!(
        html.contains("<li>Click <strong>Save</strong>, top right."),
        "{html}"
    );
    assert!(
        html.contains("<li>A green toast says Saved.</li>"),
        "{html}"
    );
    assert!(html.contains("<img src=\"panel.png\""), "{html}");
    let pair = html
        .find("<div class=\"pair\">")
        .expect("the before/after pair");
    let reference = html.find("<section id=\"reference\">").unwrap();
    assert!(pair > reference, "under Reference: {html}");
    let block = &html[pair..pair + html[pair..].find("</div>").unwrap()];
    assert!(block.contains("src=\"before.png\""), "{block}");
    assert!(block.contains("src=\"after.png\""), "{block}");
    assert_eq!(html.matches("src=\"before.png\"").count(), 1, "{html}");
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{html}"
    );
    assert!(!html.contains("<script"), "{html}");
    // The sections in the order the person reads them.
    let at = |id: &str| html.find(&format!("<section id=\"{id}\">")).unwrap();
    assert!(at("where-we-are") < at("what-you-should-see"));
    assert!(at("what-you-should-see") < at("try-it"));
    assert!(at("try-it") < at("reference"));
    assert!(at("reference") < at("already-checked"));
}
