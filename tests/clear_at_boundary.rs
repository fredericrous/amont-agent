//! `clear-at-boundary`, end to end through the real binary: `Stop` payloads
//! with a synthetic transcript, against a temporary repository with a bare
//! `origin` whose default branch is `main`.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

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

const OPEN_PLAN: &str = "---\nstatus: active\nbranch: feat/x\n---\n# Plan\n\n## Phases\n- [ ] Phase 1 — the rule\n- [ ] Phase 2 — docs\n\n## Verification\n- [ ] drive it\n";
const DONE_PLAN: &str =
    "---\nstatus: active\nbranch: feat/x\n---\n# Plan\n\n## Phases\n- [x] Phase 1 — the rule\n";

struct Reply {
    stdout: String,
}

impl Reply {
    fn silent(&self) -> bool {
        self.stdout.is_empty()
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("{e}: {:?}", self.stdout))
    }
    fn blocks(&self) -> bool {
        !self.silent() && self.json()["decision"] == "block"
    }
    fn reason(&self) -> String {
        self.json()["reason"].as_str().unwrap_or("").to_string()
    }
    fn note(&self) -> Option<String> {
        self.json()["systemMessage"].as_str().map(str::to_string)
    }
}

struct World {
    root: PathBuf,
    repo: PathBuf,
    claude: PathBuf,
    global: PathBuf,
}

impl World {
    fn new(name: &str) -> World {
        let root = std::env::temp_dir().join(format!(
            "amont-agent-clear-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root); // a world left by a killed run, if any
        let repo = root.join("repo");
        let remote = root.join("origin.git");
        let claude = root.join("claude");
        for dir in [&repo, &claude] {
            std::fs::create_dir_all(dir).unwrap();
        }
        // The rule ships at `advise`; most cases here judge the Continue.
        let global = root.join("global");
        std::fs::write(
            &global,
            "[amont \"agent.clear-at-boundary\"]\n\tstance = deny\n",
        )
        .unwrap();
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
        git(&repo, &["remote", "set-head", "origin", "main"]);
        git(&repo, &["checkout", "-q", "-b", "feat/x"]);
        World {
            root,
            repo,
            claude,
            global,
        }
    }

    fn commit_file(&self, relative: &str, text: &str, message: &str) {
        let path = self.repo.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        git(&self.repo, &["add", "."]);
        git(&self.repo, &["commit", "-qm", message]);
    }

    fn stance(&self, rule: &str, stance: &str) {
        std::fs::write(
            &self.global,
            format!("[amont \"agent.{rule}\"]\n\tstance = {stance}\n"),
        )
        .unwrap();
    }

    fn transcript(&self, name: &str, entries: &[Value]) -> PathBuf {
        let path = self.root.join(name);
        let text: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn run_with_path(&self, payload: &Value, bin_dir: Option<&Path>) -> Reply {
        let mut command = Command::new(env!("CARGO_BIN_EXE_amont-agent"));
        command
            .arg("hook")
            .env("CLAUDE_CONFIG_DIR", &self.claude)
            .env("GIT_CONFIG_GLOBAL", &self.global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("AMONT_AGENT_OFF")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = bin_dir {
            let rest = std::env::var("PATH").unwrap_or_default();
            command.env("PATH", format!("{}:{rest}", dir.display()));
        }
        let mut child = command.spawn().expect("the binary runs");
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

    fn stop(&self, session: &str, transcript: &Path, extra: Value) -> Reply {
        self.stop_in(&self.repo, session, transcript, extra, None)
    }

    fn stop_in(
        &self,
        cwd: &Path,
        session: &str,
        transcript: &Path,
        extra: Value,
        bin_dir: Option<&Path>,
    ) -> Reply {
        let mut payload = json!({
            "hook_event_name": "Stop",
            "session_id": session,
            "cwd": cwd,
            "transcript_path": transcript,
        });
        for (key, value) in extra.as_object().unwrap() {
            payload[key] = value.clone();
        }
        self.run_with_path(&payload, bin_dir)
    }

    fn journal(&self) -> Vec<String> {
        std::fs::read_to_string(self.claude.join("amont-agent/journal.log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(" clear-at-boundary "))
            .map(str::to_string)
            .collect()
    }

    fn journal_has(&self, outcome: &str, excerpt: &str) -> bool {
        self.journal()
            .iter()
            .any(|line| line.contains(&format!(" {outcome} ")) && line.contains(excerpt))
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root); // a leftover temp directory is harmless
    }
}

fn prompt(uuid: &str) -> Value {
    json!({"type": "user", "uuid": uuid, "message": {"role": "user", "content": "go"}})
}

fn bash(id: &str, command: &str) -> Value {
    json!({"type": "assistant", "message": {"role": "assistant", "content": [
        {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}}]}})
}

fn tool_use(id: &str, name: &str) -> Value {
    json!({"type": "assistant", "message": {"role": "assistant", "content": [
        {"type": "tool_use", "id": id, "name": name, "input": {}}]}})
}

fn tool_result(id: &str, text: &str) -> Value {
    json!({"type": "user", "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": id, "content": [{"type": "text", "text": text}]}]}})
}

fn usage(tokens: u64, sidechain: bool) -> Value {
    json!({"type": "assistant", "isSidechain": sidechain, "message": {"role": "assistant",
        "content": [{"type": "text", "text": "done"}],
        "usage": {"input_tokens": 10, "cache_creation_input_tokens": 0,
                  "cache_read_input_tokens": tokens - 10}}})
}

/// One turn: a prompt, a Bash call, its result, and the closing usage.
fn turn(command: &str, tokens: u64) -> Vec<Value> {
    vec![
        prompt("p1"),
        bash("b1", command),
        tool_result("b1", "ok"),
        usage(tokens, false),
    ]
}

const TEARDOWN: &str = "git worktree remove ../feat-x";

fn git_path() -> String {
    let out = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn each_boundary_shape_at_250k_continues_once() {
    for (name, command, what) in [
        ("teardown", TEARDOWN, "worktree remove"),
        (
            "sweep",
            "python3 ~/bin/sweep.py --prune --apply",
            "sweep.py --apply",
        ),
        (
            "plancommit",
            "git commit -m \"docs(plan): the plan\"",
            "plan commit",
        ),
    ] {
        let world = World::new(name);
        if what == "plan commit" {
            world.commit_file(
                "docs/plans/2026-11-13-y.md",
                DONE_PLAN,
                "docs(plan): the plan",
            );
        }
        let transcript = world.transcript("t.jsonl", &turn(command, 250_000));
        let first = world.stop("s1", &transcript, json!({}));
        assert!(first.blocks(), "{name}: {}", first.stdout);
        let reason = first.reason();
        assert!(
            reason.starts_with("amont-agent/clear-at-boundary:"),
            "{reason}"
        );
        assert!(reason.contains(what), "{reason}");
        assert!(reason.contains("250k tokens"), "{reason}");
        assert!(reason.contains("safe to /clear"), "{reason}");
        assert!(world.journal_has("denied", what), "{:?}", world.journal());
        let again = world.stop("s1", &transcript, json!({}));
        assert!(again.silent(), "{name} again: {}", again.stdout);
    }
}

#[cfg(unix)] // the stub git is a #!/bin/sh script
#[test]
fn below_the_size_nothing_is_spawned_or_journalled() {
    let world = World::new("small");
    let bin = world.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = world.root.join("git.log");
    let real = git_path();
    common::write_fake(
        &bin.join("git"),
        &format!(
            "#!/bin/sh\necho \"$@\" >> {}\nexec {real} \"$@\"\n",
            log.display()
        ),
    );
    let outside = world.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let transcript = world.transcript("t.jsonl", &turn(TEARDOWN, 150_000));
    let reply = world.stop_in(&outside, "s1", &transcript, json!({}), Some(&bin));
    assert!(reply.silent(), "{}", reply.stdout);
    let spawned = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(spawned.is_empty(), "git was spawned: {spawned}");
    assert!(world.journal().is_empty(), "{:?}", world.journal());
}

#[test]
fn what_the_payload_shows_is_silent_and_not_journalled() {
    let world = World::new("payload");
    let transcript = world.transcript("t.jsonl", &turn(TEARDOWN, 250_000));
    for (at, extra) in [
        json!({"stop_hook_active": true}),
        json!({"permission_mode": "plan"}),
        json!({"last_assistant_message": "Shall I go on?"}),
        json!({"last_assistant_message": "Done.\n  WAITING: preview at :5173"}),
        json!({"last_assistant_message": "safe to /clear; next: `x`"}),
    ]
    .into_iter()
    .enumerate()
    {
        let reply = world.stop(&format!("s{at}"), &transcript, extra.clone());
        assert!(reply.silent(), "{extra}: {}", reply.stdout);
    }
    assert!(world.journal().is_empty(), "{:?}", world.journal());
}

#[test]
fn a_dirty_tree_or_unpushed_work_is_silent_and_journalled() {
    let world = World::new("dirty");
    let transcript = world.transcript("t.jsonl", &turn(TEARDOWN, 250_000));
    std::fs::write(world.repo.join("scratch.txt"), "x").unwrap();
    assert!(world.stop("s1", &transcript, json!({})).silent());
    assert!(
        world.journal_has("watched", "uncommitted changes"),
        "{:?}",
        world.journal()
    );
    std::fs::remove_file(world.repo.join("scratch.txt")).unwrap();
    world.commit_file("src/lib.rs", "fn f() {}", "feat: f");
    assert!(world.stop("s2", &transcript, json!({})).silent());
    assert!(
        world.journal_has("watched", "unpushed commits outside docs/plans/"),
        "{:?}",
        world.journal()
    );
}

const STARTED: &str =
    "Started relais run for: /world/.relais/tasks/abc123.json (in /world/repo). Waiting.";

fn plugin_message(text: &str) -> Value {
    json!({"type": "user", "uuid": "m", "message": {"role": "user",
        "content": format!("The relais plugin sent a message:\n{text}")}})
}

fn decide(run: &str) -> Value {
    json!({"type": "assistant", "message": {"role": "assistant", "content": [
        {"type": "tool_use", "id": "d1", "name": "mcp__relais__decide", "input": {"run": run}}]}})
}

fn started() -> Vec<Value> {
    vec![
        prompt("p0"),
        tool_use("r1", "mcp__relais__run"),
        tool_result("r1", STARTED),
    ]
}

fn with_teardown(mut entries: Vec<Value>) -> Vec<Value> {
    entries.extend(turn(TEARDOWN, 250_000));
    entries
}

#[test]
fn an_unfinished_start_keeps_it_silent_even_from_an_earlier_turn() {
    for (name, extra) in [("same", vec![]), ("earlier", vec![prompt("p0b")])] {
        let world = World::new(name);
        let mut entries = started();
        entries.extend(extra);
        let transcript = world.transcript("t.jsonl", &with_teardown(entries));
        assert!(world.stop("s1", &transcript, json!({})).silent(), "{name}");
        assert!(
            world.journal_has("watched", "relais pending 1"),
            "{name}: {:?}",
            world.journal()
        );
    }
}

#[test]
fn a_run_that_needs_a_decision_keeps_it_silent_until_decided() {
    let finished = plugin_message("relais run run-65abc-1f2e finished: needs_decision (x)");
    let world = World::new("needsdecision");
    let mut entries = started();
    entries.push(finished.clone());
    let transcript = world.transcript("t.jsonl", &with_teardown(entries));
    assert!(world.stop("s1", &transcript, json!({})).silent());
    assert!(world.journal_has("watched", "relais run-65abc-1f2e needs_decision"));

    let world = World::new("decided");
    let mut entries = started();
    entries.push(finished);
    entries.push(decide("run-65abc-1f2e"));
    let transcript = world.transcript("t.jsonl", &with_teardown(entries));
    assert!(world.stop("s1", &transcript, json!({})).blocks());
}

#[test]
fn a_cancelled_run_with_a_successor_is_in_flight_until_the_exit() {
    let successor = plugin_message(
        "relais run run-65abc-1f2e finished: cancelled (scope_widened_for_tests). relais launches successor intent x.",
    );
    let world = World::new("successor");
    let mut entries = started();
    entries.push(successor.clone());
    let transcript = world.transcript("t.jsonl", &with_teardown(entries));
    assert!(world.stop("s1", &transcript, json!({})).silent());
    assert!(world.journal_has("watched", "relais pending 1"));

    let world = World::new("exited");
    let mut entries = started();
    entries.push(successor);
    entries.push(plugin_message(
        "relais exited with code 9 without an outcome.",
    ));
    let transcript = world.transcript("t.jsonl", &with_teardown(entries));
    assert!(world.stop("s1", &transcript, json!({})).blocks());
}

#[test]
fn a_finished_run_does_not_hold_it_back() {
    let world = World::new("accepted");
    let mut entries = started();
    entries.push(plugin_message(
        "relais run run-65abc-1f2e finished: accepted.",
    ));
    let transcript = world.transcript("t.jsonl", &with_teardown(entries));
    assert!(world.stop("s1", &transcript, json!({})).blocks());
}

#[test]
fn a_refused_start_does_not_hold_it_back() {
    let world = World::new("refused");
    let entries = vec![
        prompt("p0"),
        tool_use("r1", "mcp__relais__run"),
        tool_result("r1", "Nothing ran: the scope is empty."),
    ];
    let transcript = world.transcript("t.jsonl", &with_teardown(entries));
    assert!(world.stop("s1", &transcript, json!({})).blocks());
}

#[cfg(unix)] // the stub git is a #!/bin/sh script
#[test]
fn a_git_that_outlasts_the_deadline_is_a_timeout() {
    let world = World::new("timeout");
    let bin = world.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let real = git_path();
    common::write_fake(
        &bin.join("git"),
        &format!(
            "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = status ] && sleep 10; done\nexec {real} \"$@\"\n"
        ),
    );
    let transcript = world.transcript("t.jsonl", &turn(TEARDOWN, 250_000));
    let started = std::time::Instant::now();
    let reply = world.stop_in(&world.repo, "s1", &transcript, json!({}), Some(&bin));
    assert!(reply.silent(), "{}", reply.stdout);
    assert!(started.elapsed() < std::time::Duration::from_secs(8));
    assert!(
        world.journal_has("watched", "timeout"),
        "{:?}",
        world.journal()
    );
}

#[test]
fn working_is_in_flight_whatever_the_stance() {
    let world = World::new("working");
    world.commit_file("docs/plans/2026-11-13-x.md", OPEN_PLAN, "docs(plan): x");
    world.commit_file("docs/plans/notes.md", "n", "feat: work after the plan");
    let transcript = world.transcript("t.jsonl", &turn(TEARDOWN, 250_000));
    for attempt in 1..=3 {
        let reply = world.stop("s1", &transcript, json!({}));
        assert!(reply.blocks(), "{attempt}: {}", reply.stdout);
        assert!(
            reply.reason().starts_with("amont-agent/plan-phases-open:"),
            "{}",
            reply.reason()
        );
    }
    assert!(
        world.stop("s1", &transcript, json!({})).note().is_some(),
        "the cap's note"
    );
    assert!(
        world.stop("s1", &transcript, json!({})).silent(),
        "past the cap"
    );
    assert!(world.journal().is_empty(), "{:?}", world.journal());

    world.stance("plan-phases-open", "observe");
    assert!(world.stop("s2", &transcript, json!({})).silent());
    assert!(world.journal().is_empty(), "{:?}", world.journal());
}

fn boundary_one() -> World {
    let world = World::new("one");
    world.commit_file("docs/plans/2026-11-13-x.md", OPEN_PLAN, "docs(plan): x");
    world
}

#[test]
fn boundary_one_is_judged_before_plan_phases_open() {
    let world = boundary_one();
    let big = world.transcript(
        "big.jsonl",
        &turn("git commit -m \"docs(plan): x\"", 250_000),
    );
    let first = world.stop("s1", &big, json!({}));
    assert!(first.blocks(), "{}", first.stdout);
    assert!(
        first.reason().starts_with("amont-agent/clear-at-boundary:"),
        "{}",
        first.reason()
    );
    // The Stop after the handoff Continue: plan-phases-open stays silent.
    let after = world.stop(
        "s1",
        &big,
        json!({"stop_hook_active": true, "last_assistant_message": "safe to /clear; next: `go`"}),
    );
    assert!(after.silent(), "{}", after.stdout);
    let still = world.stop("s1", &big, json!({"stop_hook_active": true}));
    assert!(still.silent(), "{}", still.stdout);

    let small = world.transcript(
        "small.jsonl",
        &turn("git commit -m \"docs(plan): x\"", 150_000),
    );
    let carried = world.stop("s2", &small, json!({}));
    assert!(carried.blocks(), "{}", carried.stdout);
    assert!(carried
        .reason()
        .starts_with("amont-agent/plan-phases-open:"));
}

#[test]
fn at_observe_boundary_one_falls_through_to_plan_phases_open() {
    let world = boundary_one();
    world.stance("clear-at-boundary", "observe");
    let big = world.transcript(
        "big.jsonl",
        &turn("git commit -m \"docs(plan): x\"", 250_000),
    );
    let reply = world.stop("s1", &big, json!({}));
    assert!(reply.blocks(), "{}", reply.stdout);
    assert!(reply.reason().starts_with("amont-agent/plan-phases-open:"));
    assert!(
        world.journal_has("watched", "plan commit (observe)"),
        "{:?}",
        world.journal()
    );
}

#[test]
fn at_advise_boundary_one_lets_plan_phases_open_carry_the_plan() {
    let world = boundary_one();
    world.stance("clear-at-boundary", "advise");
    let big = world.transcript(
        "big.jsonl",
        &turn("git commit -m \"docs(plan): x\"", 250_000),
    );
    let reply = world.stop("s1", &big, json!({}));
    assert!(reply.blocks(), "{}", reply.stdout);
    assert!(
        reply.reason().starts_with("amont-agent/plan-phases-open:"),
        "{}",
        reply.reason()
    );
    assert!(
        world.journal_has("watched", "plan-phases-open carries the plan"),
        "{:?}",
        world.journal()
    );
    // The next Stop of the same turn is plan-phases-open's to judge too.
    let next = world.stop("s1", &big, json!({"stop_hook_active": true}));
    assert!(
        next.silent() || next.reason().starts_with("amont-agent/plan-phases-open:"),
        "{}",
        next.stdout
    );
    assert!(
        !world.journal_has("advised", "plan commit"),
        "{:?}",
        world.journal()
    );
}

/// The Continue asks for the line the clear-relay plugin reads: the line it
/// quotes must match the plugin's `HANDOFF` pattern, `/^<prefix>(.+)<suffix>$/`.
#[test]
fn the_continue_asks_for_the_line_the_relay_reads() {
    let source = include_str!("../plugin/clear-relay/hooks/register.ts");
    let pattern = source
        .lines()
        .find_map(|line| line.trim().strip_prefix("const HANDOFF = /^"))
        .and_then(|rest| rest.strip_suffix("$/"))
        .expect("register.ts declares const HANDOFF = /^…$/")
        .replace("\\/", "/");
    let (prefix, suffix) = pattern
        .split_once("(.+)")
        .expect("HANDOFF captures the prompt with (.+)");

    let world = World::new("contract");
    let transcript = world.transcript("t.jsonl", &turn(TEARDOWN, 250_000));
    let reason = world.stop("s1", &transcript, json!({})).reason();
    let asked = reason
        .split('"')
        .find(|quoted| quoted.starts_with("safe to /clear"))
        .unwrap_or_else(|| panic!("no quoted handoff line in: {reason}"));
    let prompt = asked
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
        .unwrap_or_else(|| panic!("{asked:?} does not match /^{prefix}(.+){suffix}$/"));
    assert!(!prompt.is_empty(), "{asked:?}");
}

#[test]
fn the_rule_ships_at_advise() {
    let world = World::new("default");
    std::fs::write(&world.global, "").unwrap();
    let transcript = world.transcript("t.jsonl", &turn(TEARDOWN, 250_000));
    let reply = world.stop("s1", &transcript, json!({}));
    assert!(reply.json().get("decision").is_none(), "{}", reply.stdout);
    assert!(
        reply.note().is_some_and(
            |note| note.contains("write the handoff") && note.contains("safe to /clear; next:")
        ),
        "{}",
        reply.stdout
    );
}

#[test]
fn a_plan_commit_with_a_non_ascii_path_is_advised_with_a_note() {
    let world = World::new("accent");
    world.stance("clear-at-boundary", "advise");
    world.commit_file("docs/plans/é.md", DONE_PLAN, "docs(plan): é");
    let transcript = world.transcript("t.jsonl", &turn("git commit -m \"docs(plan): é\"", 250_000));
    let reply = world.stop("s1", &transcript, json!({}));
    assert!(reply.json().get("decision").is_none(), "{}", reply.stdout);
    let note = reply.note().expect("a systemMessage");
    assert!(note.contains("write the handoff"), "{note}");
    assert!(note.contains("plan commit"), "{note}");
    assert!(
        world.journal_has("advised", "plan commit"),
        "{:?}",
        world.journal()
    );
}

#[test]
fn a_removed_cwd_with_a_teardown_is_advised() {
    let world = World::new("removed");
    let gone = world.root.join("removed-worktree");
    let transcript = world.transcript("t.jsonl", &turn(TEARDOWN, 250_000));
    let reply = world.stop_in(&gone, "s1", &transcript, json!({}), None);
    assert!(reply.blocks(), "{}", reply.stdout);
    assert!(reply.reason().contains("worktree remove"));
}

#[test]
fn a_sidechain_usage_after_the_main_chain_one_is_ignored() {
    let world = World::new("sidechain");
    let mut entries = turn(TEARDOWN, 250_000);
    entries.push(usage(20_000, true));
    let transcript = world.transcript("t.jsonl", &entries);
    assert!(world.stop("s1", &transcript, json!({})).blocks());

    let world = World::new("sidechain-big");
    let mut entries = turn(TEARDOWN, 150_000);
    entries.push(usage(400_000, true));
    let transcript = world.transcript("t.jsonl", &entries);
    assert!(world.stop("s1", &transcript, json!({})).silent());
}

#[test]
fn a_first_line_cut_by_the_chunk_is_tolerated() {
    let world = World::new("cut");
    let filler = json!({"type": "assistant", "message": {"role": "assistant",
        "content": [{"type": "text", "text": "x".repeat(150_000)}]}});
    let mut entries = vec![filler];
    entries.extend(turn(TEARDOWN, 250_000));
    let transcript = world.transcript("t.jsonl", &entries);
    assert!(world.stop("s1", &transcript, json!({})).blocks());
}

#[test]
fn no_person_prompt_in_the_tail_is_silent_and_journalled() {
    let world = World::new("noprompt");
    let transcript = world.transcript(
        "t.jsonl",
        &[
            bash("b1", TEARDOWN),
            tool_result("b1", "ok"),
            usage(250_000, false),
        ],
    );
    assert!(world.stop("s1", &transcript, json!({})).silent());
    assert!(
        world.journal_has("watched", "turn-too-long"),
        "{:?}",
        world.journal()
    );
}

#[test]
fn no_boundary_is_silent_and_journalled() {
    let world = World::new("noboundary");
    let transcript = world.transcript("t.jsonl", &turn("ls", 250_000));
    assert!(world.stop("s1", &transcript, json!({})).silent());
    assert!(
        world.journal_has("watched", "no boundary"),
        "{:?}",
        world.journal()
    );
}

#[test]
fn a_session_start_by_clear_leaves_one_journal_line() {
    let world = World::new("sessionstart");
    let start = |source: &str| {
        world.run_with_path(
            &json!({"hook_event_name": "SessionStart", "session_id": "s1",
                    "cwd": world.repo, "source": source}),
            None,
        )
    };
    start("startup");
    start("resume");
    assert!(world.journal().is_empty(), "{:?}", world.journal());
    start("clear");
    let lines = world.journal();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains(" watched "), "{lines:?}");
    assert!(lines[0].contains("session-start clear"), "{lines:?}");
}

#[test]
fn when_the_cap_silences_a_plan_only_boundary_plan_phases_open_runs() {
    let world = boundary_one();
    for at in 1..=3 {
        let mut entries = turn(&format!("git commit -m \"docs(plan): x {at}\""), 250_000);
        entries[0] = prompt(&format!("turn-{at}"));
        let transcript = world.transcript("t.jsonl", &entries);
        let reply = world.stop("s1", &transcript, json!({}));
        assert!(
            reply.reason().starts_with("amont-agent/clear-at-boundary:"),
            "boundary {at}: {}",
            reply.stdout
        );
    }
    let mut entries = turn("git commit -m \"docs(plan): x 4\"", 250_000);
    entries[0] = prompt("turn-4");
    let transcript = world.transcript("t.jsonl", &entries);
    let fourth = world.stop("s1", &transcript, json!({}));
    assert!(fourth.blocks(), "{}", fourth.stdout);
    assert!(
        fourth.reason().starts_with("amont-agent/plan-phases-open:"),
        "{}",
        fourth.reason()
    );
    assert!(world.journal_has("watched", "cap"), "{:?}", world.journal());
}

fn with_uuid(mut entry: Value, uuid: &str) -> Value {
    entry["uuid"] = json!(uuid);
    entry
}

fn busy() -> Value {
    json!({"background_tasks": [{"id": "b1"}]})
}

fn notification() -> Value {
    json!({"type": "user", "uuid": "n1", "message": {"role": "user",
        "content": "<task-notification>\n<task-id>b1</task-id>\n</task-notification>"}})
}

/// A turn that tears a worktree down under background work, then the Stop
/// that records it as pending.
fn pending_world(name: &str) -> (World, Vec<Value>) {
    let world = World::new(name);
    let entries = vec![
        prompt("p1"),
        with_uuid(bash("b1", TEARDOWN), "boundary-1"),
        tool_result("b1", "ok"),
        usage(250_000, false),
    ];
    let transcript = world.transcript("t.jsonl", &entries);
    let reply = world.stop("s1", &transcript, busy());
    assert!(reply.silent(), "{}", reply.stdout);
    assert!(
        world.journal_has("watched", "pending background"),
        "{:?}",
        world.journal()
    );
    (world, entries)
}

fn later(world: &World, entries: &[Value], more: Vec<Value>, extra: Value) -> Reply {
    let mut all = entries.to_vec();
    all.extend(more);
    all.push(usage(250_000, false));
    let transcript = world.transcript("t.jsonl", &all);
    world.stop("s1", &transcript, extra)
}

#[test]
fn a_boundary_under_background_work_is_judged_at_the_first_idle_stop() {
    let (world, entries) = pending_world("pendingidle");
    let still_busy = later(&world, &entries, vec![notification()], busy());
    assert!(still_busy.silent(), "{}", still_busy.stdout);
    let idle = later(&world, &entries, vec![notification()], json!({}));
    assert!(idle.blocks(), "{}", idle.stdout);
    assert!(
        idle.reason().contains("worktree remove"),
        "{}",
        idle.reason()
    );
    assert!(
        world.journal_has("denied", "worktree remove"),
        "{:?}",
        world.journal()
    );
    let again = later(&world, &entries, vec![notification()], json!({}));
    assert!(again.silent(), "{}", again.stdout);
}

#[test]
fn work_after_the_pending_boundary_leaves_it_pending_and_silent() {
    for (name, work) in [
        ("edit", tool_use("w1", "Edit")),
        ("write", tool_use("w1", "Write")),
        ("worktreeadd", bash("w1", "git -C r worktree add ../x -b y")),
        ("relaisrun", tool_use("w1", "mcp__relais__run")),
    ] {
        let (world, entries) = pending_world(name);
        let reply = later(&world, &entries, vec![work, notification()], json!({}));
        assert!(reply.silent(), "{name}: {}", reply.stdout);
        assert!(
            world.journal_has("watched", "pending-work"),
            "{name}: {:?}",
            world.journal()
        );
    }
}

#[test]
fn a_person_prompt_drops_the_pending_boundary_and_a_plugin_message_does_not() {
    let (world, entries) = pending_world("pendingprompt");
    let plugin = plugin_message("relais run run-65abc-1f2e finished: accepted.");
    let kept = later(&world, &entries, vec![plugin], json!({}));
    assert!(kept.blocks(), "{}", kept.stdout);

    let (world, entries) = pending_world("pendingperson");
    let dropped = later(&world, &entries, vec![prompt("p2")], json!({}));
    assert!(dropped.silent(), "{}", dropped.stdout);
    let after = later(&world, &entries, vec![notification()], json!({}));
    assert!(after.silent(), "{}", after.stdout);
}

#[test]
fn a_pending_boundary_missing_from_the_tail_is_lost() {
    let (world, _) = pending_world("pendinglost");
    let transcript = world.transcript(
        "other.jsonl",
        &[prompt("p9"), notification(), usage(250_000, false)],
    );
    let reply = world.stop("s1", &transcript, json!({}));
    assert!(reply.silent(), "{}", reply.stdout);
    assert!(
        world.journal_has("watched", "pending-lost"),
        "{:?}",
        world.journal()
    );
}

#[test]
fn a_state_file_in_the_text_format_still_reads() {
    let world = World::new("oldstate");
    let transcript = world.transcript("t.jsonl", &turn(TEARDOWN, 250_000));
    let dir = world.claude.join("amont-agent/clear-at-boundary");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("capped"), "turn-1 3\nsome key").unwrap();
    let capped = world.stop("capped", &transcript, json!({}));
    assert!(capped.silent(), "{}", capped.stdout);
    assert!(world.journal_has("watched", "cap"), "{:?}", world.journal());
    let fresh = world.stop("fresh", &transcript, json!({}));
    assert!(fresh.blocks(), "{}", fresh.stdout);
}
