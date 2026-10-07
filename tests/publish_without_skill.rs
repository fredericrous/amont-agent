//! `publish-without-skill`, end to end through the real binary.
//!
//! Each test writes a transcript in the shapes captured from real sessions on
//! 2026-10-07 (see `src/skill_window.rs`) and sends a `PreToolUse` Bash
//! payload that names it. The rule's own stance key is `deny`, as a person
//! who wants the refusal sets it; the other rules keep their defaults, as on
//! the machine this was built on.

use std::path::{Path, PathBuf};
use std::process::Command;

const RULE: &str = "amont-agent/publish-without-skill";

#[derive(Debug, PartialEq)]
enum Said {
    Silent,
    Advise(String),
    Deny(String),
}

impl Said {
    fn text(&self) -> &str {
        match self {
            Said::Silent => "",
            Said::Advise(t) | Said::Deny(t) => t,
        }
    }
    fn ours(&self) -> bool {
        self.text().contains(RULE)
    }
}

struct World {
    root: PathBuf,
}

impl World {
    fn new(name: &str) -> World {
        let root = std::env::temp_dir().join(format!(
            "amont-agent-pws-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for skill in ["tag-release", "merge-when-green"] {
            let d = root.join("claude/skills").join(skill);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("SKILL.md"), format!("---\nname: {skill}\n---\n")).unwrap();
        }
        std::fs::create_dir_all(root.join("work")).unwrap();
        std::fs::write(root.join("global"), "").unwrap();
        let w = World { root };
        w.stance("publish-without-skill", "deny");
        w
    }

    fn stance(&self, rule: &str, stance: &str) {
        let out = Command::new("git")
            .args(["config", "--file"])
            .arg(self.root.join("global"))
            .args([&format!("amont.agent.{rule}.stance"), stance])
            .output()
            .expect("git config");
        assert!(out.status.success());
    }

    fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    /// A transcript of `lines`, written under the world.
    fn transcript(&self, name: &str, lines: &[String]) -> PathBuf {
        let p = self.root.join(format!("{name}.jsonl"));
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        p
    }

    fn run(&self, cmd: &str, cwd: &Path, transcript: Option<&Path>, agent: Option<&Path>) -> Said {
        let mut payload = serde_json::json!({
            "hook_event_name": "PreToolUse", "tool_name": "Bash",
            "cwd": cwd, "session_id": "s1", "prompt_id": "p1",
            "tool_use_id": "t1", "permission_mode": "default",
            "tool_input": {"command": cmd}
        });
        if let Some(t) = transcript {
            payload["transcript_path"] = serde_json::json!(t);
        }
        if let Some(a) = agent {
            payload["agent_transcript_path"] = serde_json::json!(a);
            payload["agent_id"] = serde_json::json!("a1");
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .arg("hook")
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("GIT_CONFIG_GLOBAL", self.root.join("global"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("AMONT_AGENT_OFF")
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
        if let Some(reason) = o["permissionDecisionReason"].as_str() {
            return Said::Deny(reason.to_string());
        }
        Said::Advise(
            o["additionalContext"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    }

    fn publish(&self, cmd: &str, transcript: &Path) -> Said {
        self.run(cmd, &self.work(), Some(transcript), None)
    }

    fn journal(&self) -> String {
        std::fs::read_to_string(self.root.join("claude/amont-agent/journal.log"))
            .unwrap_or_default()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// The shapes, as Claude Code writes them.

fn human(text: &str) -> String {
    serde_json::json!({"type":"user","promptId":"p","message":{"role":"user","content":text},
        "origin":{"kind":"human"}})
    .to_string()
}

fn typed(skill: &str) -> String {
    human(&format!(
        "<command-message>{skill}</command-message>\n<command-name>/{skill}</command-name>"
    ))
}

/// A model's Skill call, its result, and the skill body Claude Code adds.
fn skill(name: &str) -> Vec<String> {
    vec![
        serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"toolu_s","name":"Skill","input":{"skill":name}}]}})
        .to_string(),
        serde_json::json!({"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"toolu_s","content":format!("Launching skill: {name}")}]},
            "toolUseResult":{"success":true,"commandName":name}})
        .to_string(),
        serde_json::json!({"type":"user","isMeta":true,"turnCompanion":true,
            "message":{"role":"user","content":[{"type":"text",
            "text":format!("Base directory for this skill: /x/skills/{name}\n\n# {name}")}]}})
        .to_string(),
    ]
}

fn notification() -> String {
    serde_json::json!({"type":"user","message":{"role":"user",
        "content":"<task-notification>\n<status>completed</status>\n</task-notification>"},
        "origin":{"kind":"task-notification"}})
    .to_string()
}

fn lines(parts: &[Vec<String>]) -> Vec<String> {
    parts.concat()
}

const TAG: &str = "git push origin v0.10.0";
const MERGE: &str = "gh pr merge 23 --repo fredericrous/relais --squash";
const CURL: &str = "curl -sS -X POST \"https://git.daddyshome.fr/api/v1/repos/fredericrous/relais/pulls/23/merge\" -d '{\"Do\":\"merge\"}'";

#[test]
fn no_skill_call_is_refused() {
    let w = World::new("none");
    let t = w.transcript("t", &[human("release relais 0.10.0")]);
    let said = w.publish(TAG, &t);
    assert!(matches!(said, Said::Deny(_)), "{said:?}");
    assert!(
        said.text().contains("Call the tag-release skill first"),
        "{said:?}"
    );
    assert!(w.journal().contains("window=outside"), "{}", w.journal());
}

#[test]
fn a_call_in_this_turn_passes() {
    let w = World::new("current");
    let t = w.transcript("t", &lines(&[vec![human("release")], skill("tag-release")]));
    let said = w.publish(TAG, &t);
    assert!(!said.ours(), "{said:?}");
    assert!(
        w.journal()
            .contains("publish-without-skill unconfirmed skill_called_in_this_turn"),
        "{}",
        w.journal()
    );
}

#[test]
fn one_reply_passes_two_prompts_do_not() {
    let w = World::new("window");
    let one = lines(&[
        vec![human("merge")],
        skill("merge-when-green"),
        vec![human("yes")],
    ]);
    let t = w.transcript("one", &one);
    assert!(!w.publish(MERGE, &t).ours());
    let two = lines(&[one, vec![human("and the other one?")]]);
    let t = w.transcript("two", &two);
    let said = w.publish(MERGE, &t);
    assert!(matches!(said, Said::Deny(_)), "{said:?}");
    assert!(
        said.text().contains("this turn or the one before"),
        "{said:?}"
    );
    assert!(w.journal().contains("window=outside"), "{}", w.journal());
}

#[test]
fn notifications_inside_the_turn_do_not_end_it() {
    let w = World::new("notify");
    let t = w.transcript(
        "t",
        &lines(&[
            vec![human("merge when green")],
            skill("merge-when-green"),
            vec![notification(), notification()],
        ]),
    );
    assert!(!w.publish(MERGE, &t).ours());
}

#[test]
fn a_skill_the_person_typed_passes() {
    let w = World::new("typed");
    let t = w.transcript("t", &[human("x"), typed("tag-release")]);
    assert!(!w.publish(TAG, &t).ours());
}

#[test]
fn the_other_skill_does_not_count() {
    let w = World::new("other");
    let t = w.transcript("t", &lines(&[vec![human("x")], skill("merge-when-green")]));
    assert!(matches!(w.publish(TAG, &t), Said::Deny(_)));
}

#[test]
fn not_knowing_advises_and_never_refuses() {
    let w = World::new("unknown");
    // No transcript at all.
    let said = w.run(TAG, &w.work(), None, None);
    assert!(matches!(said, Said::Advise(_)) && said.ours(), "{said:?}");
    // A transcript that cannot be opened.
    let said = w.publish(TAG, &w.root.join("missing.jsonl"));
    assert!(matches!(said, Said::Advise(_)) && said.ours(), "{said:?}");
    // A transcript whose last line is still being written.
    let p = w.root.join("cut.jsonl");
    let call = &skill("tag-release")[0];
    std::fs::write(&p, format!("{}\n{}", human("x"), &call[..call.len() / 2])).unwrap();
    let said = w.publish(TAG, &p);
    assert!(matches!(said, Said::Advise(_)) && said.ours(), "{said:?}");
    assert!(said.text().contains("cannot be told"), "{said:?}");
}

#[test]
fn a_skill_not_installed_advises() {
    let w = World::new("absent");
    std::fs::remove_dir_all(w.root.join("claude/skills/tag-release")).unwrap();
    let t = w.transcript("t", &[human("x")]);
    let said = w.publish(TAG, &t);
    assert!(matches!(said, Said::Advise(_)) && said.ours(), "{said:?}");
    assert!(
        w.journal().contains("window=unknown:not-installed"),
        "{}",
        w.journal()
    );
}

#[test]
fn a_project_skill_is_found_where_the_command_runs() {
    let w = World::new("project");
    std::fs::remove_dir_all(w.root.join("claude/skills/tag-release")).unwrap();
    let repo = w.root.join("repo");
    let skill = repo.join(".claude/skills/tag-release");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "---\nname: tag-release\n---\n").unwrap();
    let t = w.transcript("t", &[human("x")]);
    // From the session's directory, `cd` into the repository: refused, not
    // "not installed". A relative path, because a Windows absolute path's
    // backslashes are escapes to the shell lexer.
    let said = w.publish("cd ../repo && git push origin v0.10.0", &t);
    assert!(matches!(said, Said::Deny(_)), "{said:?}");
}

#[test]
fn a_missing_working_directory_is_still_judged() {
    let w = World::new("nocwd");
    let gone = w.root.join("removed-worktree");
    let t = w.transcript("t", &lines(&[vec![human("x")], skill("merge-when-green")]));
    assert!(!w.run(MERGE, &gone, Some(&t), None).ours());
    let t = w.transcript("u", &[human("x")]);
    let said = w.run(MERGE, &gone, Some(&t), None);
    assert!(matches!(said, Said::Deny(_)) && said.ours(), "{said:?}");
    assert!(
        !said.text().contains("directory that does not exist"),
        "{said:?}"
    );
}

#[test]
fn a_partly_read_command_only_advises() {
    let w = World::new("partial");
    let t = w.transcript("t", &[human("x")]);
    let said = w.publish(
        "git status -s | xargs git add && git push origin v0.10.0",
        &t,
    );
    assert!(said.ours(), "{said:?}");
    assert!(!matches!(said, Said::Deny(_)), "{said:?}");
}

#[test]
fn observe_turns_it_off() {
    let w = World::new("observe");
    w.stance("publish-without-skill", "observe");
    let t = w.transcript("t", &[human("x")]);
    assert!(!w.publish(TAG, &t).ours());
    assert!(
        w.journal()
            .contains("publish-without-skill observe watched"),
        "{}",
        w.journal()
    );
}

#[test]
fn a_subagent_counts_its_own_call_and_its_parents() {
    let w = World::new("agent");
    let parent = w.transcript("parent", &[human("ship it")]);
    // The subagent's file has no human prompt; its own call covers its run.
    let own = w.transcript(
        "agent",
        &lines(&[
            vec![serde_json::json!({"type":"user","isSidechain":true,
            "message":{"role":"user","content":"merge PR 23"}})
            .to_string()],
            skill("merge-when-green"),
        ]),
    );
    assert!(!w.run(MERGE, &w.work(), Some(&parent), Some(&own)).ours());
    // The parent called it this turn; the subagent did not.
    let parent = w.transcript(
        "parent2",
        &lines(&[vec![human("ship it")], skill("merge-when-green")]),
    );
    let bare = w.transcript(
        "agent2",
        &[serde_json::json!({"type":"user","isSidechain":true,
        "message":{"role":"user","content":"merge PR 23"}})
        .to_string()],
    );
    assert!(!w.run(MERGE, &w.work(), Some(&parent), Some(&bare)).ours());
    // Neither did.
    let parent = w.transcript("parent3", &[human("ship it")]);
    assert!(matches!(
        w.run(MERGE, &w.work(), Some(&parent), Some(&bare)),
        Said::Deny(_)
    ));
}

#[test]
fn gh_pr_merge_auto_still_refuses_on_its_own() {
    let w = World::new("auto");
    w.stance("gh-pr-merge-auto", "deny");
    let cmd = "gh pr merge --auto 23";
    let t = w.transcript("t", &[human("x")]);
    let said = w.publish(cmd, &t);
    let text = said.text();
    let (a, b) = (text.find("amont-agent/gh-pr-merge-auto"), text.find(RULE));
    assert!(matches!(said, Said::Deny(_)), "{said:?}");
    assert!(a.is_some() && b.is_some() && a < b, "{text}");
    let t = w.transcript("u", &lines(&[vec![human("x")], skill("merge-when-green")]));
    let said = w.publish(cmd, &t);
    assert!(matches!(said, Said::Deny(_)) && !said.ours(), "{said:?}");
}

#[test]
fn a_curl_merge_is_refused_alone_or_advised_first() {
    let w = World::new("curl");
    let t = w.transcript("t", &[human("x")]);
    let said = w.publish(CURL, &t);
    assert!(matches!(said, Said::Deny(_)), "{said:?}");
    // A refusal drops advice: `forge-merge-by-hand` is journalled, not said.
    assert!(!said.text().contains("forge-merge-by-hand"), "{said:?}");
    let first: String = said.text().chars().take(80).collect();
    assert!(first.contains("merge-when-green"), "{first}");

    w.stance("publish-without-skill", "advise");
    let said = w.publish(CURL, &t);
    let text = said.text();
    let (a, b) = (
        text.find(RULE),
        text.find("amont-agent/forge-merge-by-hand"),
    );
    assert!(matches!(said, Said::Advise(_)), "{said:?}");
    assert!(a.is_some() && b.is_some() && a < b, "{text}");
}

#[test]
fn a_dry_run_or_a_branch_push_is_not_a_publish() {
    let w = World::new("quiet");
    let t = w.transcript("t", &[human("x")]);
    assert!(!w.publish("git push --dry-run origin v0.10.0", &t).ours());
    assert!(!w.publish("git push -u origin feat/x", &t).ours());
    assert!(!w.publish("git push --follow-tags origin main", &t).ours());
}
