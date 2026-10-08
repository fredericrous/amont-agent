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
        // The PreToolUse stamp comes first, as it does in a session: the
        // bind requires a page written after the call began.
        self.pre_bash(session, "reg", &command);
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
fn a_register_that_is_not_last_is_not_bound_and_says_how() {
    let w = World::new("not-last");
    w.commit("app/a.tsx", "1\n");
    let record = w.guide();
    let command = format!(
        "amont-agent preview register --url http://localhost:1/ --guide '{}' && echo done",
        record.display()
    );
    let (id, said) = stamped(&w, &w.work, "reg", &command, "", "done\n");
    // Seen 2026-09-29 (PR #302): this failure was silent, so the session
    // asked, the person approved, and the approval bound nothing.
    assert!(said.contains("NOT bound"), "the session is told: {said}");
    assert!(said.contains("LAST command"), "{said}");
    w.answer("s", "p1", &id, Some("Approve"));
    assert!(w.push_advised("s"));
    assert!(
        w.journal().contains("not the last command"),
        "{}",
        w.journal()
    );

    // The command it prints binds when run as printed, and the refusal
    // quotes the question to ask.
    let v: serde_json::Value = serde_json::from_str(&said).unwrap();
    let context = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    let again = context
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("cd "))
        .map(|rest| format!("cd {rest}"))
        .expect("a command to run");
    assert!(again.contains("amont-agent preview register"), "{again}");
    assert!(!again.contains("echo done"), "{again}");
    let (_, bound) = stamped(&w, &w.work, "reg2", &again, "", "");
    assert!(!bound.contains("NOT bound"), "{bound}");
    assert!(w.journal().contains("registered"), "{}", w.journal());
}

#[test]
fn a_refusal_quotes_the_question_prefix() {
    let w = World::new("refusal-prefix");
    w.commit("app/a.tsx", "1\n");
    let command = format!(
        "amont-agent preview register --url http://localhost:1/ --guide '{}' | tee /dev/null",
        w.guide().display()
    );
    let (id, said) = stamped(&w, &w.work, "reg", &command, "", "");
    assert!(said.contains("NOT bound"), "{said}");
    assert!(
        said.contains(&format!("[preview {id}] {}", w.label())),
        "{said}"
    );
}

#[test]
fn a_register_chained_after_other_commands_is_bound() {
    let w = World::new("chained");
    w.commit("app/a.tsx", "1\n");
    let record = w.guide();
    let command = format!(
        "curl -s http://localhost:1/ ; cd '{}' && amont-agent preview register --url http://localhost:1/ --guide '{}'",
        w.work.display(),
        record.display()
    );
    // curl printed first; the JSON is the last line.
    let (id, said) = stamped(&w, &w.root, "reg", &command, "<html>ok</html>\n", "\n");
    assert!(!said.contains("NOT bound"), "{said}");
    assert!(w.journal().contains("registered"), "{}", w.journal());
    w.answer("s", "p1", &id, Some("Approve"));
    assert!(!w.push_advised("s"), "{}", w.journal());
}

#[test]
fn a_plain_register_binds() {
    let w = World::new("plain");
    w.commit("app/a.tsx", "1\n");
    let command = format!(
        "amont-agent preview register --url http://localhost:1/ --guide '{}'",
        w.guide().display()
    );
    let (id, said) = stamped(&w, &w.work, "reg", &command, "", "");
    assert!(!said.contains("NOT bound"), "{said}");
    w.answer("s", "p1", &id, Some("Approve"));
    assert!(!w.push_advised("s"), "{}", w.journal());
}

#[test]
fn a_register_with_no_pretooluse_stamp_is_not_bound() {
    let w = World::new("no-stamp");
    w.commit("app/a.tsx", "1\n");
    let record = w.guide();
    let command = format!(
        "amont-agent preview register --url http://localhost:1/ --guide '{}'",
        record.display()
    );
    let (_, out, _) = w.register_cli("http://localhost:1/", &record);
    let said = w.post_bash("s", "p1", "reg", &command, &out);
    assert!(said.contains("NOT bound"), "{said}");
    assert!(
        w.journal().contains("no record of the call"),
        "{}",
        w.journal()
    );
}

#[test]
fn a_json_line_printed_before_a_failed_register_is_not_bound() {
    // `printf '<json naming the real HEAD>'; amont-agent preview register
    // --bad`: the line is a real registration's, of the real HEAD, but its
    // page was written before this call began.
    let w = World::new("forged");
    w.commit("app/a.tsx", "1\n");
    let (_, out, _) = w.register_cli("http://localhost:1/", &w.guide());
    std::thread::sleep(std::time::Duration::from_millis(20));
    let command = format!(
        "printf '%s\\n' '{}'; amont-agent preview register --bad",
        out.trim()
    );
    w.pre_bash("s", "reg", &command);
    let said = w.post_bash("s", "p1", "reg", &command, &out);
    assert!(said.contains("NOT bound"), "{said}");
    assert!(
        w.journal().contains("not written by this call"),
        "{}",
        w.journal()
    );

    // A line whose id does not start with the commit's sha7.
    let mut v: serde_json::Value = serde_json::from_str(&out).unwrap();
    v["id"] = serde_json::json!("0000000aaaa");
    let forged = v.to_string();
    w.pre_bash("s", "reg2", &command);
    let said = w.post_bash("s", "p1", "reg2", &command, &forged);
    assert!(said.contains("NOT bound"), "{said}");
    assert!(
        w.journal().contains("does not start with the commit"),
        "{}",
        w.journal()
    );
    assert!(!w.journal().contains("registered"), "{}", w.journal());
}

#[test]
fn a_chained_register_help_is_not_a_registration() {
    let w = World::new("register-help");
    let said = w.post_bash(
        "s",
        "p1",
        "help",
        "amont-agent --version && amont-agent preview register --help | grep -c Mockup",
        "usage: amont-agent preview register …\n",
    );
    assert!(!said.contains("NOT bound"), "{said}");
}

#[test]
fn a_marked_question_with_the_id_and_no_label_approves() {
    // Seen 2026-10-08 (Duro 5.5): the question carried `[preview <id>]` and
    // no label, the approval bound nothing, and the person was asked twice.
    let w = World::new("id-only");
    w.commit("app/a.tsx", "1\n");
    let id = w.register("s", "p1");
    let q = format!("[preview {id}] Ship the 28px small controls?");
    assert!(!q.contains(&w.label()));
    w.ask("PreToolUse", ("s", "p1"), "q1", &q, None, false);
    let said = w.ask("PostToolUse", ("s", "p1"), "q1", &q, Some("Approve"), false);
    assert!(!said.contains("bound NOTHING"), "{said}");
    assert!(!w.push_advised("s"), "{}", w.journal());
    assert!(w.journal().contains("approved"), "{}", w.journal());
}

#[test]
fn the_register_json_carries_the_question_prefix() {
    let w = World::new("question-prefix");
    w.commit("app/a.tsx", "1\n");
    let (code, out, err) = w.register_cli("http://localhost:1/", &w.guide());
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim().lines().count(), 1, "one line: {out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let id = v["id"].as_str().unwrap();
    assert_eq!(
        v["question_prefix"].as_str(),
        Some(format!("[preview {id}] {}", w.label()).as_str()),
        "{out}"
    );
}

#[test]
fn a_marker_naming_no_pending_id_approves_nothing() {
    let w = World::new("id-unknown");
    w.commit("app/a.tsx", "1\n");
    let id = w.register("s", "p1");
    let sha = git(&w.work, &["rev-parse", "HEAD"]);
    // The right sha7 and label, but an id nobody registered.
    let q = format!("[preview {}fffffff] Ship {}?", &sha[..7], w.label());
    assert!(!q.contains(&id));
    w.ask("PreToolUse", ("s", "p1"), "q1", &q, None, false);
    w.ask("PostToolUse", ("s", "p1"), "q1", &q, Some("Approve"), false);
    assert!(w.push_advised("s"), "{}", w.journal());
    assert!(!w.journal().contains("approved"), "{}", w.journal());
}

#[test]
fn a_worktree_commit_is_approved_under_the_main_checkouts_name() {
    // Seen 2026-09-29: the question said `application-landscape@2fcffec`, the
    // registration's label was the worktree's `al-wt-history-mockup@2fcffec`.
    let w = World::new("alias");
    let wt = w.root.join("app-wt-x");
    git(
        &w.work,
        &["worktree", "add", "-q", "-b", "feat/wt"]
            .iter()
            .copied()
            .chain([wt.to_str().unwrap()])
            .collect::<Vec<_>>(),
    );
    git(&wt, &["config", "user.email", "t@t"]);
    git(&wt, &["config", "user.name", "t"]);
    std::fs::create_dir_all(wt.join("app")).unwrap();
    std::fs::write(wt.join("app/a.tsx"), "1\n").unwrap();
    git(&wt, &["add", "."]);
    git(&wt, &["commit", "-qm", "a"]);
    let sha = git(&wt, &["rev-parse", "HEAD"]);

    let record = w.guide();
    let command = format!(
        "cd '{}' && amont-agent preview register --url http://localhost:1/ --guide '{}'",
        wt.display(),
        record.display()
    );
    w.pre_bash_in(&wt, "s", "reg", &command);
    let (code, out, err) = w.run(
        &[
            "preview",
            "register",
            "--repo",
            wt.to_str().unwrap(),
            "--url",
            "http://localhost:1/",
            "--guide",
            record.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["label"], format!("app-wt-x@{}", &sha[..7]));
    let aliases: Vec<&str> = v["aliases"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a.as_str())
        .collect();
    assert!(
        aliases.contains(&format!("app@{}", &sha[..7]).as_str()),
        "{aliases:?}"
    );
    let id = v["id"].as_str().unwrap().to_string();
    w.post_bash_in(&wt, "s", "p1", "reg", &command, &out);

    let q = format!("[preview {id}] Ship app@{}?", &sha[..7]);
    w.ask("PreToolUse", ("s", "p1"), "q1", &q, None, false);
    w.ask("PostToolUse", ("s", "p1"), "q1", &q, Some("Approve"), false);
    assert!(w.journal().contains("approved"), "{}", w.journal());
    let pushed = w.pre_bash_in(&wt, "s", "push1", "git push -u origin feat/wt");
    assert!(!pushed.contains("amont-agent/push-preview"), "{pushed}");
}

/// A register call as a session makes it, from a session sitting in `cwd`:
/// the PreToolUse stamp, the CLI run in the worktree, then the PostToolUse
/// bind of `command` with `before` + the printed JSON + `after` as stdout.
/// Returns (id, what the bind said).
fn stamped(
    w: &World,
    cwd: &Path,
    tool_use_id: &str,
    command: &str,
    before: &str,
    after: &str,
) -> (String, String) {
    w.pre_bash_in(cwd, "s", tool_use_id, command);
    let (code, out, err) = w.register_cli("http://localhost:1/", &w.guide());
    assert_eq!(code, 0, "{err}");
    let id = serde_json::from_str::<serde_json::Value>(&out).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let stdout = format!("{before}{out}{after}");
    let said = w.post_bash_in(cwd, "s", "p1", tool_use_id, command, &stdout);
    (id, said)
}

#[test]
fn a_register_after_leading_cds_is_bound_in_the_repository_they_reach() {
    // Seen 2026-09-28: `cd /path/to/worktree && amont-agent preview register …`
    // from a session whose cwd was another directory was `unbound`.
    let w = World::new("cd-chained");
    w.commit("app/a.tsx", "1\n");
    let command = format!(
        "cd '{}' && cd app && amont-agent preview register --url http://localhost:1/ --guide '{}'",
        w.root.display(),
        w.guide().display()
    );
    // The session sits in the claude dir, not in the repository.
    let (id, _) = stamped(&w, &w.root.join("claude"), "reg", &command, "", "");
    assert!(w.journal().contains("registered"), "{}", w.journal());
    w.answer("s", "p1", &id, Some("Approve"));
    assert!(!w.push_advised("s"), "{}", w.journal());
}

#[test]
fn a_register_joined_by_semicolons_or_after_a_build_is_bound() {
    let w = World::new("cd-other-bound");
    w.commit("app/a.tsx", "1\n");
    let work = w.work.display();
    let tail = format!(
        "amont-agent preview register --url http://localhost:1/ --guide '{}'",
        w.guide().display()
    );
    for (i, command) in [
        format!("cd '{work}'; {tail}"),
        format!("cd '{work}' && npm test && {tail}"),
    ]
    .iter()
    .enumerate()
    {
        let (_, said) = stamped(&w, &w.root, &format!("reg{i}"), command, "", "");
        assert!(!said.contains("NOT bound"), "{command}: {said}");
    }
}

#[test]
fn a_register_that_is_not_last_piped_or_substituted_stays_unbound() {
    let w = World::new("cd-other");
    w.commit("app/a.tsx", "1\n");
    let tail = format!(
        "amont-agent preview register --url http://localhost:1/ --guide '{}'",
        w.guide().display()
    );
    let work = w.work.display();
    let mut last = String::new();
    for (i, command) in [
        format!("cd '{work}' || {tail}"),
        format!("cd $(git rev-parse --show-toplevel) && {tail}"),
        format!("cd '{work}' && {tail} && echo done"),
        format!("cd '{work}' && {tail} | tee /dev/null"),
        format!("cd '{work}' && echo $({tail})"),
        format!("cd '{work}' && {tail} > /tmp/out.json"),
    ]
    .iter()
    .enumerate()
    {
        let (id, said) = stamped(&w, &w.root, &format!("reg{i}"), command, "", "");
        assert!(said.contains("NOT bound"), "{command}: {said}");
        last = id;
    }
    assert!(!w.journal().contains("registered"), "{}", w.journal());
    w.answer("s", "p1", &last, Some("Approve"));
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
fn under_deny_a_delete_only_push_passes() {
    let w = World::new("deny-delete");
    w.set_global("amont.agent.push-preview.stance", "deny");
    w.commit("app/a.tsx", "1\n");
    for command in [
        "git push origin --delete old other",
        "git push -d origin old",
        "git push origin :old",
    ] {
        let out = w.pre_bash("s", "t", command);
        assert!(!out.contains("push-preview"), "{command}: {out}");
    }
    let out = w.pre_bash("s", "t", "git push origin feat/x :old");
    assert!(out.contains("\"deny\""), "deleting and publishing: {out}");
}

/// #67: a session left in a removed worktree. The push's own `cd` reaches a
/// real repository, so the push is judged there; without one, the shell runs
/// somewhere the hook cannot name, and deny holds it.
#[test]
fn a_removed_session_directory_neither_hides_nor_excuses_a_push() {
    let w = World::new("gone-cwd");
    w.commit("app/a.tsx", "1\n");
    let gone = w.root.join("removed-worktree");
    // Quoted, so a Windows path's backslashes reach `cd` as written.
    let cd = format!("cd '{}' && git push -u origin feat/x", w.work.display());
    let out = w.pre_bash_in(&gone, "s", "t", &cd);
    assert!(
        out.contains("amont-agent/push-preview"),
        "judged in the repository the cd reaches: {out}"
    );
    let out = w.pre_bash_in(&gone, "s", "t", "git push -u origin feat/x");
    assert!(
        !out.contains("push-preview"),
        "advise: nothing to look at: {out}"
    );

    w.set_global("amont.agent.push-preview.stance", "deny");
    let out = w.pre_bash_in(&gone, "s", "t", &cd);
    assert!(out.contains("\"deny\""), "{out}");
    let out = w.pre_bash_in(&gone, "s", "t", "git push -u origin feat/x");
    assert!(
        out.contains("\"deny\"") && out.contains("does not exist"),
        "held: {out}"
    );
    // A command that is not a push still declines quietly.
    let out = w.pre_bash_in(&gone, "s", "t", "ls");
    assert!(!out.contains("\"deny\""), "{out}");
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

// --- mockup mode (plan 2026-09-29-mockup-fidelity-checks) -----------------

const MOCKUP_GUIDE: &str = "\
## Where we are
app, branch `feat/x`, direction B.

## What you should see
A pill.

## Try it
1. Open http://localhost:5173/

## Reference
Artboards: docs/mockups/pill
Viewport: 1120px · light

![Mockup B](artboard.png) ![Built](after.png)

## Already checked
- console clean

## Differences from the mockup
- fixed: the separate History segment is gone
";

/// A branch that commits a picked mockup's artboard and the screen it built.
fn mockup_branch(w: &World) {
    w.commit("docs/mockups/pill/Main.dc.html", "<x-dc></x-dc>\n");
    w.commit("app/routes/home.tsx", "1\n");
}

fn guide_with_images(w: &World, text: &str, images: &[&str]) -> PathBuf {
    let g = w.guide_with(text);
    for i in images {
        std::fs::write(g.parent().unwrap().join(i), b"png").unwrap();
    }
    g
}

#[test]
fn a_mockup_branch_without_the_side_by_side_is_refused() {
    // Seen 2026-09-29 (PR #302): the build diverged from the picked
    // artboard, and the preview went up without the two side by side.
    let w = World::new("mockup-refused");
    mockup_branch(&w);
    let (code, out, err) = w.register_cli("http://localhost:1/", &w.guide());
    assert_eq!(code, 1, "{err}");
    assert!(out.is_empty(), "nothing on stdout: {out}");
    for piece in [
        "mockup mode",
        "docs/mockups/pill",
        "viewport line",
        "artboard.png",
        "## Differences from the mockup",
        "Paste and fill",
    ] {
        assert!(err.contains(piece), "{piece} in:\n{err}");
    }
}

#[test]
fn a_complete_mockup_guide_registers_and_renders_the_pair() {
    let w = World::new("mockup-ok");
    mockup_branch(&w);
    let g = guide_with_images(&w, MOCKUP_GUIDE, &["artboard.png", "after.png"]);
    let (code, _, err) = w.register_cli("http://localhost:1/", &g);
    assert_eq!(code, 0, "{err}");
    let html = std::fs::read_to_string(g.parent().unwrap().join("index.html")).unwrap();
    assert!(html.contains("class=\"design\""), "{html}");
    assert!(html.contains("class=\"built\""));
    assert!(html.contains("Mockup, direction B"));
}

#[test]
fn a_mockup_branch_already_pushed_is_still_in_mockup_mode() {
    // The range starts at the merge base with the default branch, never at
    // the upstream ref, which already holds the artboards after a push.
    let w = World::new("mockup-pushed");
    mockup_branch(&w);
    git(&w.work, &["push", "-q", "-u", "origin", "feat/x"]);
    let (code, _, err) = w.register_cli("http://localhost:1/", &w.guide());
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("mockup mode"), "{err}");
}

#[test]
fn artboards_with_no_default_branch_to_compare_are_refused_not_skipped() {
    let w = World::new("mockup-no-remote");
    mockup_branch(&w);
    git(&w.work, &["remote", "remove", "origin"]);
    let (code, _, err) = w.register_cli("http://localhost:1/", &w.guide());
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("no default remote branch"), "{err}");
}

#[test]
fn artboards_already_on_main_do_not_put_a_later_branch_in_mockup_mode() {
    let w = World::new("mockup-on-main");
    git(&w.work, &["checkout", "-q", "main"]);
    w.commit("docs/mockups/pill/Main.dc.html", "<x-dc></x-dc>\n");
    git(&w.work, &["push", "-q", "origin", "main"]);
    git(&w.work, &["checkout", "-q", "-b", "feat/later"]);
    w.commit("app/routes/home.tsx", "2\n");
    let (code, _, err) = w.register_cli("http://localhost:1/", &w.guide());
    assert_eq!(code, 0, "the old guide still registers: {err}");
}

#[test]
fn an_unapproved_mockup_commit_is_held_and_a_plain_ui_one_is_advised() {
    let w = World::new("mockup-deny");
    w.commit("app/routes/home.tsx", "1\n");
    let plain = w.pre_bash("s", "push1", "git push -u origin feat/x");
    assert!(plain.contains("amont-agent/push-preview"), "{plain}");
    assert!(!plain.contains("\"deny\""), "advised, not held: {plain}");

    w.commit("docs/mockups/pill/Main.dc.html", "<x-dc></x-dc>\n");
    let held = w.pre_bash("s", "push2", "git push -u origin feat/x");
    assert!(held.contains("\"deny\""), "held: {held}");

    // Observe is how a person turns the rule off; mockup mode respects it.
    w.set_global("amont.agent.push-preview.stance", "observe");
    let off = w.pre_bash("s", "push3", "git push -u origin feat/x");
    assert!(!off.contains("\"deny\""), "{off}");
}

#[test]
fn a_register_on_the_default_branch_itself_is_not_refused() {
    // Seen 2026-10-08 (duro-design-system v5.5.0): main carried artboards,
    // the commit to tag WAS main, and `git diff <c>..<c>` printing nothing
    // was taken for a failure.
    let w = World::new("mockup-at-main");
    git(&w.work, &["checkout", "-q", "main"]);
    w.commit("docs/mockups/pill/Main.dc.html", "<x-dc></x-dc>\n");
    git(&w.work, &["push", "-q", "origin", "main"]);
    let (code, _, err) = w.register_cli("http://localhost:1/", &w.guide());
    assert_eq!(code, 0, "nothing new since main, so no screen: {err}");
}

#[test]
fn an_unreadable_push_is_held_as_unreadable_not_as_unapproved() {
    // Seen 2026-10-08: `git tag v5.5.0 && git push origin v5.5.0` judged
    // before the tag existed; the hold said "no approved preview covers",
    // and the session went looking for an approval it did not need.
    let w = World::new("deny-unborn-tag");
    w.set_global("amont.agent.push-preview.stance", "deny");
    w.commit("app/a.tsx", "1\n");
    let out = w.pre_bash("s", "t", "git tag v9.9.9 && git push origin v9.9.9");
    assert!(out.contains("\"deny\""), "still held: {out}");
    assert!(
        !out.contains("no approved preview covers"),
        "not called unapproved: {out}"
    );
    assert!(out.contains("does not resolve to a commit"), "{out}");
    assert!(out.contains("its own command"), "says how: {out}");
    assert!(
        !out.contains("preview register"),
        "no preview steps for a push that needs none: {out}"
    );
}

#[test]
fn under_deny_a_push_of_an_existing_tag_passes() {
    let w = World::new("deny-tag");
    w.set_global("amont.agent.push-preview.stance", "deny");
    w.commit("app/a.tsx", "1\n");
    git(&w.work, &["tag", "v9.9.9"]);
    let out = w.pre_bash("s", "t", "git push origin v9.9.9");
    assert!(!out.contains("amont-agent/push-preview"), "{out}");
}

// --- the plan approval record (ADR-0028) -----------------------------------

/// The PostToolUse payloads of an `ExitPlanMode`. The approve shape is the
/// one recorded in real transcripts (`toolUseResult` = `{plan, isAgent,
/// filePath}`, no `is_error`).
const EXIT_APPROVE: &str = include_str!("fixtures/exitplanmode-approve.json");
// The reject shape is inferred, not captured: a rejection is recorded in
// transcripts as an `is_error` tool result whose content is an "Error: …"
// string. holds-until: the ExitPlanMode payloads are captured live before
// v2.30.0 is tagged (plan 2026-10-08-approve-only-what-needs-judging, Phase 2
// step 1); this fixture is then replaced by the captured one.
const EXIT_REJECT: &str = include_str!("fixtures/exitplanmode-reject.json");

/// Send an ExitPlanMode PostToolUse fixture whose plan file is `plan`.
fn plan_answered(w: &World, fixture: &str, plan: &Path) -> String {
    let mut v: serde_json::Value = serde_json::from_str(fixture).unwrap();
    v["tool_input"]["planFilePath"] = serde_json::json!(plan);
    if v["tool_response"].is_object() {
        v["tool_response"]["filePath"] = serde_json::json!(plan);
    }
    v["cwd"] = serde_json::json!(w.work);
    w.hook(v)
}

fn plan_sha(w: &World, plan: &Path) -> String {
    let (code, out, err) = w.run(&["plan-sha", plan.to_str().unwrap()], None);
    assert_eq!(code, 0, "{err}");
    out.trim().to_string()
}

fn approved_plans(w: &World) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(w.root.join("claude/amont-agent/plan-approved"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

const PLAN: &str = "\
# Small controls at 28px

## Context

The person decided the small controls grow to 28px.

## Preview

evidence: the person already decided the only visible change, 28px small controls.
";

#[test]
fn an_approved_plan_is_recorded_by_its_body_sha_as_the_file_reads_now() {
    let w = World::new("plan-approved");
    let plan = w.root.join("plan.md");
    // The file, not the payload's `plan`: the person may edit it first.
    std::fs::write(&plan, PLAN).unwrap();
    let said = plan_answered(&w, EXIT_APPROVE, &plan);
    assert!(said.trim().is_empty(), "silent: {said}");
    assert_eq!(approved_plans(&w), vec![plan_sha(&w, &plan)]);
    assert!(w.journal().contains("plan-approved"), "{}", w.journal());
}

#[test]
fn a_rejected_plan_records_nothing() {
    let w = World::new("plan-rejected");
    let plan = w.root.join("plan.md");
    std::fs::write(&plan, PLAN).unwrap();
    plan_answered(&w, EXIT_REJECT, &plan);
    assert!(approved_plans(&w).is_empty());

    // An object with no `plan` is not the approve shape either.
    let mut odd: serde_json::Value = serde_json::from_str(EXIT_APPROVE).unwrap();
    odd["tool_response"] = serde_json::json!({"filePath": "x"});
    plan_answered(&w, &odd.to_string(), &plan);
    assert!(approved_plans(&w).is_empty());
}

#[test]
fn a_subagents_exit_plan_records_nothing() {
    let w = World::new("plan-subagent");
    let plan = w.root.join("plan.md");
    std::fs::write(&plan, PLAN).unwrap();
    let mut agent: serde_json::Value = serde_json::from_str(EXIT_APPROVE).unwrap();
    agent["tool_response"]["isAgent"] = serde_json::json!(true);
    plan_answered(&w, &agent.to_string(), &plan);
    assert!(approved_plans(&w).is_empty());
}

// --- planned evidence (ADR-0028, work.preview-unless-planned-evidence) -----

/// The person approves `body` through ExitPlanMode, from a plan file
/// outside the repository, as plan mode writes it.
fn approve_plan(w: &World, body: &str) {
    let file = w.root.join("approved-plan.md");
    std::fs::write(&file, body).unwrap();
    plan_answered(w, EXIT_APPROVE, &file);
    assert_eq!(approved_plans(w).len(), 1, "{}", w.journal());
}

/// The plan as it lands on the branch: front matter added.
fn landed(body: &str) -> String {
    format!("---\nstatus: active\nbranch: feat/x\n---\n\n{body}")
}

#[test]
fn an_approved_plan_declaring_evidence_lets_a_later_ui_push_through() {
    let w = World::new("evidence");
    approve_plan(&w, PLAN);
    w.commit("docs/plans/2026-10-08-small.md", &landed(PLAN));
    w.commit("app/routes/home.tsx", "export default 1\n");
    w.commit("app/routes/home.tsx", "export default 2\n");
    assert!(!w.push_advised("s"), "{}", w.journal());
    let journal = w.journal();
    let line = journal
        .lines()
        .find(|l| l.contains(" evidence "))
        .expect("an evidence line");
    let head = git(&w.work, &["rev-parse", "HEAD"]);
    assert!(line.contains("docs/plans/2026-10-08-small.md"), "{line}");
    assert!(line.contains(&format!("commit={}", &head[..7])), "{line}");
    assert!(
        line.contains(&format!("sha={}", &approved_plans(&w)[0][..12])),
        "{line}"
    );
}

#[test]
fn evidence_the_person_did_not_approve_is_held_as_before() {
    let w = World::new("evidence-unapproved");
    w.commit("docs/plans/2026-10-08-small.md", &landed(PLAN));
    w.commit("app/a.tsx", "1\n");
    assert!(w.push_advised("s"), "{}", w.journal());
    assert!(!w.journal().contains(" evidence "), "{}", w.journal());
}

#[test]
fn a_preview_section_added_after_the_plan_landed_does_not_count() {
    let w = World::new("evidence-later");
    let without = PLAN.split("## Preview").next().unwrap().to_string();
    // The person approved both bodies; the plan landed without the section.
    approve_plan(&w, PLAN);
    let first = w.root.join("first-plan.md");
    std::fs::write(&first, &without).unwrap();
    plan_answered(&w, EXIT_APPROVE, &first);
    assert_eq!(approved_plans(&w).len(), 2);
    w.commit("docs/plans/2026-10-08-small.md", &landed(&without));
    w.commit("docs/plans/2026-10-08-small.md", &landed(PLAN));
    w.commit("app/a.tsx", "1\n");
    assert!(w.push_advised("s"), "{}", w.journal());
}

#[test]
fn no_plan_a_pointer_plan_or_a_plan_already_on_main_is_held_as_before() {
    // No plan on the branch.
    let w = World::new("evidence-no-plan");
    approve_plan(&w, PLAN);
    w.commit("app/a.tsx", "1\n");
    assert!(w.push_advised("s"), "{}", w.journal());

    // A pointer plan, with the approved body under it.
    let w = World::new("evidence-pointer");
    approve_plan(&w, PLAN);
    w.commit(
        "docs/plans/2026-10-08-small.md",
        &format!("---\ncanonical: decisions:docs/plans/2026-10-08-small.md\n---\n\n{PLAN}"),
    );
    w.commit("app/a.tsx", "1\n");
    assert!(w.push_advised("s"), "{}", w.journal());

    // The plan landed on main before the branch started.
    let w = World::new("evidence-on-main");
    approve_plan(&w, PLAN);
    git(&w.work, &["checkout", "-q", "main"]);
    w.commit("docs/plans/2026-10-08-small.md", &landed(PLAN));
    git(&w.work, &["push", "-q", "origin", "main"]);
    git(&w.work, &["checkout", "-q", "-b", "feat/y"]);
    w.commit("app/a.tsx", "1\n");
    let out = w.pre_bash("s", "push1", "git push -u origin feat/y");
    assert!(out.contains("amont-agent/push-preview"), "{out}");
    assert!(!w.journal().contains(" evidence "), "{}", w.journal());
}

#[test]
fn with_no_base_ref_the_evidence_is_not_looked_for() {
    let w = World::new("evidence-no-base");
    approve_plan(&w, PLAN);
    w.commit("docs/plans/2026-10-08-small.md", &landed(PLAN));
    w.commit("app/a.tsx", "1\n");
    // The only remote has no default branch to diff against.
    let empty = w.root.join("empty.git");
    Command::new("git")
        .args(["init", "-q", "--bare", "--template="])
        .arg(&empty)
        .output()
        .unwrap();
    git(&w.work, &["remote", "remove", "origin"]);
    git(
        &w.work,
        &["remote", "add", "fresh", &empty.display().to_string()],
    );
    let out = w.pre_bash("s", "push1", "git push -u fresh feat/x");
    assert!(out.contains("amont-agent/push-preview"), "{out}");
    assert!(!w.journal().contains(" evidence "), "{}", w.journal());
}

#[test]
fn evidence_never_stands_for_a_picked_mockups_preview() {
    let w = World::new("evidence-mockup");
    approve_plan(&w, PLAN);
    w.commit("docs/plans/2026-10-08-small.md", &landed(PLAN));
    mockup_branch(&w);
    let out = w.pre_bash("s", "push1", "git push -u origin feat/x");
    assert!(out.contains("\"deny\""), "held: {out}");
    assert!(!w.journal().contains(" evidence "), "{}", w.journal());
}

#[test]
fn a_preview_heading_inside_a_code_fence_is_not_a_declaration() {
    let w = World::new("evidence-fenced");
    let fenced = PLAN.replace(
        "## Preview\n\nevidence: the person already decided the only visible change, 28px small controls.\n",
        "```md\n## Preview\n\nevidence: an example in a template\n```\n",
    );
    assert_ne!(fenced, PLAN);
    approve_plan(&w, &fenced);
    w.commit("docs/plans/2026-10-08-small.md", &landed(&fenced));
    w.commit("app/a.tsx", "1\n");
    assert!(w.push_advised("s"), "{}", w.journal());
}
