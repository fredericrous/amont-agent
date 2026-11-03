//! `ask-unopened-page`, driven through the real binary with transcript
//! fixtures (decisions ADR-0058, `work.page-shown-before-asked`).
//!
//! The transcripts are synthesised: each tool call is an assistant `tool_use`
//! line followed by the user `tool_result` line that completes it, in the
//! shape Claude Code writes them. The opens in them are records, not real
//! browser launches.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const CWD: &str = "/work/repo";
const MOCKUP: &str = "docs/mockups/part-toggles/options.html";

struct Reply {
    stdout: String,
}

impl Reply {
    fn json(&self) -> Option<Value> {
        serde_json::from_str(&self.stdout).ok()
    }
    fn decision(&self) -> Option<String> {
        let json = self.json()?;
        Some(
            json.get("hookSpecificOutput")?
                .get("permissionDecision")?
                .as_str()?
                .to_string(),
        )
    }
    fn reason(&self) -> String {
        self.json()
            .and_then(|v| {
                let o = v.get("hookSpecificOutput")?.clone();
                Some(
                    o.get("permissionDecisionReason")
                        .or_else(|| o.get("additionalContext"))?
                        .as_str()?
                        .to_string(),
                )
            })
            .unwrap_or_default()
    }
    fn denied(&self) -> bool {
        self.decision().as_deref() == Some("deny")
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("amont-agent-page-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// One assistant `tool_use` line and the user `tool_result` completing it.
fn call(id: &str, name: &str, input: Value, tool_output: &str, is_error: bool) -> Vec<String> {
    vec![
        json!({"type":"assistant","cwd":CWD,"message":{"content":[
            {"type":"tool_use","id":id,"name":name,"input":input}]}})
        .to_string(),
        json!({"type":"user","cwd":CWD,"message":{"content":[
            {"type":"tool_result","tool_use_id":id,"content":tool_output,"is_error":is_error}]}})
        .to_string(),
    ]
}

fn bash(id: &str, command: &str) -> Vec<String> {
    call(id, "Bash", json!({ "command": command }), "", false)
}

fn failed_bash(id: &str, command: &str) -> Vec<String> {
    call(id, "Bash", json!({ "command": command }), "boom", true)
}

fn ask_input(question: &str) -> Value {
    json!({"questions":[{"question":question,"options":[{"label":"A"},{"label":"B"}]}]})
}

fn answered(id: &str, question: &str) -> Vec<String> {
    call(id, "AskUserQuestion", ask_input(question), "answers", false)
}

fn denied_ask(id: &str, question: &str) -> Vec<String> {
    call(id, "AskUserQuestion", ask_input(question), "denied", true)
}

const REGISTER_OPEN: &str =
    "amont-agent preview register --url http://localhost:5173 --guide g.md --open";

fn register_command(id: &str, command: &str, page: &str, opened: bool) -> Vec<String> {
    let registration = json!({"id":"aa7ad80ae8ea6b","page":page,"opened":opened}).to_string();
    call(
        id,
        "Bash",
        json!({ "command": command }),
        &registration,
        false,
    )
}

fn register(id: &str, page: &str, opened: bool) -> Vec<String> {
    register_command(id, REGISTER_OPEN, page, opened)
}

fn transcript(name: &str, calls: &[Vec<String>]) -> PathBuf {
    let path = scratch(name).join("session.jsonl");
    let mut text = calls.concat().join("\n");
    text.push('\n');
    std::fs::write(&path, text).expect("write the transcript");
    path
}

fn payload(question: &str, transcript: Option<&PathBuf>) -> String {
    let mut v = json!({
        "hook_event_name":"PreToolUse","tool_name":"AskUserQuestion",
        "session_id":"s1","prompt_id":"p1","tool_use_id":"ask-now","cwd":CWD,
        "tool_input": ask_input(question),
    });
    if let Some(t) = transcript {
        v["transcript_path"] = json!(t);
    }
    v.to_string()
}

fn send(payload: &str) -> Reply {
    let config = scratch("config");
    let mut child = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
        .arg("hook")
        .env("CLAUDE_CONFIG_DIR", &config)
        .env_remove("AMONT_AGENT_OFF")
        .env_remove("AMONT_HOST_SLOT")
        .env("GIT_CONFIG_GLOBAL", config.join("no-global-gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write the payload");
    let out = child.wait_with_output().expect("the hook exits");
    assert_eq!(out.status.code(), Some(0));
    Reply {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
    }
}

fn ask(name: &str, calls: &[Vec<String>], question: &str) -> Reply {
    let path = transcript(name, calls);
    send(&payload(question, Some(&path)))
}

fn abc() -> String {
    format!("Placement: A, B or C? See {MOCKUP}")
}

#[test]
fn a_question_naming_an_unopened_mockup_is_denied() {
    let reply = ask("unopened", &[bash("b1", "ls")], &abc());
    assert!(reply.denied(), "{}", reply.stdout);
    let why = reply.reason();
    assert!(why.contains(MOCKUP), "{why}");
    assert!(why.contains("amont-agent open 'docs/mockups/part-toggles/options.html'"));
    assert!(why.contains("ask the question again"));
    assert!(why.contains("work.page-shown-before-asked"));
}

#[test]
fn a_question_with_no_page_is_allowed() {
    let reply = ask("nopage", &[bash("b1", "ls")], "Fix source: rule or hook?");
    assert_eq!(reply.stdout, "");
}

#[test]
fn an_opened_page_may_be_asked_about() {
    let calls = [bash("b1", &format!("amont-agent open {MOCKUP}"))];
    assert_eq!(ask("opened", &calls, &abc()).stdout, "");
}

#[test]
fn a_page_named_only_in_an_option_is_found() {
    let mut v: Value = serde_json::from_str(&payload("Which?", None)).unwrap();
    v["tool_input"]["questions"][0]["options"][0]["description"] = json!(format!("see {MOCKUP}"));
    let reply = send(&v.to_string());
    assert!(reply.reason().contains(MOCKUP), "{}", reply.stdout);
}

/// The open records below are synthesised: they stand for the person's
/// browser having been launched by `register --open` and `open`.
#[test]
fn a_preview_question_after_register_and_open_is_allowed() {
    let question =
        "[preview aa7ad80ae8ea6b] Approve http://localhost:5173/app ? Guide: /work/repo/g.html";
    let calls = [
        register("r1", "/work/repo/g.html", true),
        bash("b2", "amont-agent open 'http://localhost:5173/app'"),
    ];
    assert_eq!(ask("preview-ok", &calls, question).stdout, "");
}

#[test]
fn a_preview_question_after_register_only_is_denied_naming_the_url() {
    let question =
        "[preview aa7ad80ae8ea6b] Approve http://localhost:5173/app ? Guide: /work/repo/g.html";
    let calls = [register("r1", "/work/repo/g.html", true)];
    let reply = ask("preview-url", &calls, question);
    assert!(reply.denied(), "{}", reply.stdout);
    let why = reply.reason();
    assert!(
        why.contains("amont-agent open 'http://localhost:5173/app'"),
        "{why}"
    );
    assert!(
        !why.contains("amont-agent open '/work/repo/g.html'"),
        "{why}"
    );
}

#[test]
fn open_ask_ask_again_is_denied() {
    let calls = [
        bash("b1", &format!("amont-agent open {MOCKUP}")),
        answered("a1", &abc()),
    ];
    assert!(ask("window", &calls, &abc()).denied());
}

#[test]
fn a_denied_ask_is_not_a_boundary() {
    let calls = [
        bash("b1", &format!("amont-agent open {MOCKUP}")),
        denied_ask("a1", &abc()),
    ];
    assert_eq!(ask("denied-ask", &calls, &abc()).stdout, "");
}

#[test]
fn another_fragment_or_route_is_another_page() {
    let calls = [bash("b1", "amont-agent open 'http://localhost:3000/#/A'")];
    assert!(ask("frag", &calls, "Pick http://localhost:3000/#/B").denied());
    let calls = [bash("b1", "amont-agent open http://localhost:3000/options")];
    assert!(ask("slash", &calls, "Pick http://localhost:3000/options/").denied());
    assert!(ask(
        "index",
        &calls,
        "Pick http://localhost:3000/options/index.html"
    )
    .denied());
    assert_eq!(
        ask("same", &calls, "Pick http://localhost:3000/options").stdout,
        ""
    );
}

#[test]
fn an_open_that_may_not_have_run_does_not_count() {
    let page = format!("Pick {MOCKUP}");
    let skipped = [bash("b1", &format!("true || amont-agent open {MOCKUP}"))];
    assert!(ask("skipped", &skipped, &page).denied());
    let masked = [bash("b1", &format!("amont-agent open {MOCKUP}; true"))];
    assert!(ask("masked", &masked, &page).denied());
    let failed = [failed_bash("b1", &format!("amont-agent open {MOCKUP}"))];
    assert!(ask("failed", &failed, &page).denied());
    let not_opened = [register("r1", &format!("{CWD}/{MOCKUP}"), false)];
    assert!(ask("not-opened", &not_opened, &page).denied());
    let opened = [register("r1", &format!("{CWD}/{MOCKUP}"), true)];
    assert_eq!(ask("opened-reg", &opened, &page).stdout, "");
}

#[test]
fn a_register_open_in_a_compound_command_does_not_count() {
    let page = format!("Pick {MOCKUP}");
    let url = format!("{CWD}/{MOCKUP}");
    let echoed_first = format!("echo '{{\"opened\":true,\"page\":\"{url}\"}}'; {REGISTER_OPEN}");
    let calls = [register_command("r1", &echoed_first, &url, true)];
    assert!(ask("echo-first", &calls, &page).denied());
    let echoed_after = format!("{REGISTER_OPEN}; echo done");
    let calls = [register_command("r1", &echoed_after, &url, true)];
    assert!(ask("echo-after", &calls, &page).denied());
    let cd_first = format!("cd {CWD} && {REGISTER_OPEN}");
    let calls = [register_command("r1", &cd_first, &url, true)];
    assert_eq!(ask("cd-register", &calls, &page).stdout, "");
}

#[test]
fn a_cd_prefixed_open_counts_for_the_page_under_that_directory() {
    let calls = [bash("b1", "cd dir && amont-agent open x.html")];
    assert_eq!(ask("cd", &calls, "Pick /work/repo/dir/x.html").stdout, "");
    assert!(ask("cd-wrong", &calls, "Pick /work/repo/x.html").denied());
}

#[test]
fn a_schemeless_host_and_a_remote_link_are_not_pages() {
    let reply = ask(
        "narrow",
        &[bash("b1", "ls")],
        "Try 127.0.0.1:8080 or https://github.com/a/b/pull/1 ?",
    );
    assert_eq!(reply.stdout, "");
}

#[test]
fn no_transcript_advises() {
    let reply = send(&payload(&abc(), None));
    assert!(!reply.denied(), "{}", reply.stdout);
    assert!(
        reply.reason().contains("amont-agent open"),
        "{}",
        reply.stdout
    );
}

#[test]
fn a_transcript_still_being_written_advises() {
    let path = transcript("truncated", &[bash("b1", "ls")]);
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("{\"type\":\"assistant\",\"mess");
    std::fs::write(&path, text).unwrap();
    let reply = send(&payload(&abc(), Some(&path)));
    assert!(!reply.denied(), "{}", reply.stdout);
    assert!(!reply.reason().is_empty());
}

#[test]
fn the_guard_encodes_a_file_url_as_the_opener_does() {
    use amont_agent_guard::page_open::key_of;
    use std::path::Path;
    for path in [
        "/a b/c.html",
        "/x#y/z?.html",
        "/100%/é.html",
        "/plain/p.html",
    ] {
        assert_eq!(
            key_of(path, Path::new("/")),
            amont_agent_process::guide::file_url(Path::new(path)),
            "{path}"
        );
    }
}
