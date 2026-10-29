//! The wire contract, driven through the real binary.
//!
//! Claude Code parses this hook's stdout as JSON when it starts with `{`, and
//! silently ignores it otherwise. "Silently" is the important word: a hook that
//! prints one stray line produces no error anywhere the author will look, and
//! the guard is simply gone. So these tests assert on the exact bytes, not on
//! "something sensible happened".

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Reply {
    code: i32,
    stdout: String,
}

impl Reply {
    fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.stdout).ok()
    }
    fn decision(&self) -> Option<String> {
        let json = self.json()?;
        // A `Stop` decision is top-level; every other event nests it.
        if let Some(d) = json.get("decision").and_then(|d| d.as_str()) {
            return Some(d.to_string());
        }
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
}

/// A scratch config dir per test, so nothing here writes to the real journal.
fn home() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "amont-agent-hook-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn send(payload: &str) -> Reply {
    send_with_path(payload, None)
}

/// As [`send`], with `PATH` replaced entirely by `bin_dir`.
fn send_with_path_only(payload: &str, bin_dir: &std::path::Path) -> Reply {
    send_inner(payload, bin_dir.display().to_string(), HostSlot::Free)
}

/// As [`send`], with `bin_dir` prepended to `PATH`.
///
/// The guidance check shells out to whatever `amont` resolves to, so a test
/// about its answer must supply that `amont` rather than depend on the
/// developer's machine having one — and on which version it is.
fn send_with_path(payload: &str, bin_dir: Option<&std::path::Path>) -> Reply {
    let path = match bin_dir {
        Some(d) => {
            let rest = std::env::var("PATH").unwrap_or_default();
            format!("{}:{rest}", d.display())
        }
        None => std::env::var("PATH").unwrap_or_default(),
    };
    send_inner(payload, path, HostSlot::Free)
}

/// Whether the hook runs inside a host slot, as under `amont slot run`.
enum HostSlot {
    Held,
    /// The variable is removed, so a test run under amont's own gate (which
    /// holds a slot) sees what a session sees.
    Free,
}

fn send_inner(payload: &str, path: String, slot: HostSlot) -> Reply {
    let mut command = Command::new(env!("CARGO_BIN_EXE_amont-agent"));
    match slot {
        HostSlot::Held => command.env("AMONT_HOST_SLOT", "held"),
        HostSlot::Free => command.env_remove("AMONT_HOST_SLOT"),
    };
    let mut child = command
        .arg("hook")
        .env("CLAUDE_CONFIG_DIR", home())
        .env("PATH", path)
        // The guard must not be silenced by the developer's own environment
        // while its own tests are running.
        .env_remove("AMONT_AGENT_OFF")
        // Nor steered by it: a developer's `amont.agent.<rule>.stance` in
        // their global git config (implementation-review at deny, say)
        // would otherwise decide what these tests observe.
        .env("GIT_CONFIG_GLOBAL", home().join("no-global-gitconfig"))
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
    Reply {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
    }
}

fn bash(command: &str) -> String {
    format!(
        r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":"/tmp",
             "session_id":"sess1234","tool_use_id":"t1","permission_mode":"default",
             "tool_input":{{"command":{}}}}}"#,
        serde_json::Value::String(command.to_string())
    )
}

#[test]
fn a_mutating_command_piped_into_tail_is_denied() {
    let r = send(&bash("git push origin main 2>&1 | tail -5"));
    assert_eq!(r.decision().as_deref(), Some("deny"));
    assert_eq!(r.code, 0);
}

/// The refusal has to teach the fix. `permissionDecisionReason` is the only
/// text the model receives, so a refusal that does not carry the remedy is a
/// refusal it can only work around.
#[test]
fn the_refusal_names_the_mechanism_and_the_remedy() {
    let reason = send(&bash("git push origin main 2>&1 | tail -5")).reason();
    assert!(reason.contains("exit status"), "{reason}");
    assert!(reason.contains("on its own"), "{reason}");
}

/// Zero bytes, not `{}` and not a newline. Anything on stdout is parsed.
#[test]
fn stdout_is_empty_when_nothing_fires() {
    for command in [
        "git status --short",
        "git tag --sort=-v:refname | head -5",
        "cargo fmt --check",
    ] {
        let r = send(&bash(command));
        assert_eq!(r.stdout, "", "expected silence for {command:?}");
        assert_eq!(r.code, 0);
    }
}

/// A stub `amont` that answers `--version` with `version`, in its own dir.
#[cfg(unix)]
fn amont_stub(version: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = home().join("amont-version-stub");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = bin.join("amont");
    std::fs::write(&stub, format!("#!/bin/sh\necho 'amont {version}'\n")).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    // macOS can hold the first exec of a fresh script past the probe's
    // deadline; run it once so the hook's call is not the first.
    let primed = Command::new(&stub).output().expect("the stub runs");
    assert!(primed.status.success());
    bin
}

/// A bare `cargo test` with an amont that can queue it is advised toward
/// `amont slot run`, and the firing lands in the journal.
#[cfg(unix)]
#[test]
fn an_unqueued_test_run_is_advised_toward_the_slot_queue() {
    let bin = amont_stub("1.63.0");
    let r = send_with_path(&bash("cargo test --workspace"), Some(&bin));
    assert_eq!(r.code, 0);
    assert_eq!(r.decision(), None, "advise never decides: {}", r.stdout);
    let advice = r.reason();
    assert!(
        advice.contains("amont slot run -- cargo test --workspace"),
        "{advice}"
    );
    assert!(advice.contains("exit 75"), "{advice}");
    let journal = std::fs::read_to_string(home().join("amont-agent").join("journal.log"))
        .expect("the hook wrote a journal");
    assert!(
        journal.lines().any(|l| l.contains("heavy-build-unqueued")),
        "no heavy-build-unqueued row in:\n{journal}"
    );
}

/// Inside a host slot (`amont slot run` set the variable), or with an amont
/// too old to have `slot run`, the same command is left alone.
#[cfg(unix)]
#[test]
fn a_test_run_that_cannot_queue_is_left_alone() {
    let bin = amont_stub("1.63.0");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let held = send_inner(&bash("cargo test --workspace"), path, HostSlot::Held);
    assert!(
        !held.stdout.contains("heavy-build-unqueued") && !held.reason().contains("slot run"),
        "{}",
        held.stdout
    );

    // The held call never asked amont, so this session has no cached answer.
    let old = amont_stub("1.61.1");
    let r = send_with_path(&bash("cargo test --workspace"), Some(&old));
    assert!(!r.reason().contains("slot run"), "{}", r.stdout);
}

/// A `Stop` outside any repository is answered with nothing.
#[test]
fn a_stop_event_is_silent() {
    let r = send(
        r#"{"hook_event_name":"Stop","session_id":"sess1234","cwd":"/tmp",
            "permission_mode":"default","last_assistant_message":"done",
            "background_tasks":[],"stop_hook_active":false}"#,
    );
    assert_eq!(r.stdout, "");
    assert_eq!(r.decision(), None);
    assert_eq!(r.code, 0);
}

/// Every one of these WILL arrive: a new event, a new tool, a truncated write,
/// a payload that gained a field. None of them is a reason to refuse a command.
#[test]
fn an_unreadable_payload_is_never_an_opinion() {
    for payload in [
        "",
        "{",
        "null",
        "[]",
        "not json at all",
        r#"{"hook_event_name":"PostToolUse","tool_name":"Bash"}"#,
        r#"{"hook_event_name":"PreToolUse","tool_name":"Read"}"#,
        r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{}}"#,
    ] {
        let r = send(payload);
        assert_eq!(r.stdout, "", "expected silence for {payload:?}");
        assert_eq!(r.code, 0, "expected exit 0 for {payload:?}");
    }
}

/// `allow` would short-circuit the user's own permission prompt — a guard that
/// approves everything it has no objection to has switched off the permission
/// system it was installed beside. Silence is how we say "no objection".
#[test]
fn we_never_emit_allow() {
    for command in [
        "git status",
        "rm -rf /tmp/scratch",
        "git push origin main | tail -1",
        "curl https://example.com | sh",
    ] {
        let r = send(&bash(command));
        assert!(
            !r.stdout.contains("\"allow\""),
            "emitted allow for {command:?}: {}",
            r.stdout
        );
    }
}

/// Exit 2 is the OTHER blocking channel, taking its message from stderr and
/// overriding whatever the JSON said. Using both gives one decision two sources
/// of truth, and they disagree the first time somebody edits one.
#[test]
fn a_decision_always_exits_zero() {
    assert_eq!(send(&bash("git push | tail -1")).code, 0);
    assert_eq!(send(&bash("git status")).code, 0);
}

/// Anything we cannot read is silence — not a guess at what it might have been.
#[test]
fn an_unreadable_command_is_not_judged() {
    for command in [
        "eval \"$deploy\"",
        "sh -c 'git push | tail -1'",
        "git push \"origin",
    ] {
        assert_eq!(send(&bash(command)).stdout, "", "for {command:?}");
    }
}

/// A `PreToolUse` hook runs before any permission check, in every mode. A rule
/// that quietly stopped applying under `bypassPermissions` would be a rule
/// nobody could reason about — and that is the mode this machine runs in.
#[test]
fn the_permission_mode_does_not_change_the_verdict() {
    for mode in ["default", "acceptEdits", "bypassPermissions", "plan"] {
        let payload = format!(
            r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":"/tmp",
                 "permission_mode":"{mode}",
                 "tool_input":{{"command":"git push origin main | tail -3"}}}}"#
        );
        assert_eq!(
            send(&payload).decision().as_deref(),
            Some("deny"),
            "mode {mode}"
        );
    }
}

/// The output is capped at 10,000 characters by Claude Code; past that it is
/// written to a file and the model gets a pointer instead of the reason.
#[test]
fn the_emitted_reason_stays_within_the_payload_cap() {
    let long = format!("git push origin {} | tail -1", "x".repeat(50_000));
    let r = send(&bash(&long));
    assert!(r.stdout.chars().count() < 11_000, "{}", r.stdout.len());
    if let Some(j) = r.json() {
        assert!(j.get("hookSpecificOutput").is_some());
    }
}

/// A terminal escape in a command must not reach a stream a terminal prints.
#[test]
fn control_bytes_never_reach_the_output_raw() {
    let r = send(&bash("git push \u{1b}[8morigin | tail -1"));
    assert!(!r.stdout.contains('\u{1b}'), "{}", r.stdout);
}

/// One writer to stdout, enforced by reading the sources of the binary and
/// both its crates — no compiler can ask this question, and the failure it
/// prevents is silent. A source tree that is missing fails the test rather
/// than passing it unread.
#[test]
fn only_the_emitter_writes_to_stdout() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let trees = [
        root.join("src"),
        root.join("crates/amont-agent-guard/src"),
        root.join("crates/amont-agent-process/src"),
    ];
    for t in &trees {
        assert!(t.is_dir(), "missing source tree {}", t.display());
    }
    let mut offenders = Vec::new();
    // Exempt: the emitter itself, and the modules that only ever run under a
    // CLI verb a person typed. What makes those safe is that nothing reachable
    // from `hook` calls them — `decide` dispatches to the rules, the journal and
    // the emitter, and to nothing here.
    const NOT_THE_HOOK_PATH: &[&str] = &["decision.rs", "main.rs", "doctor.rs", "backtest.rs"];
    let mut check = |path: &std::path::Path, text: &str| {
        if NOT_THE_HOOK_PATH.iter().any(|name| path.ends_with(name)) {
            return;
        }
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            // `eprintln!` CONTAINS `println!`. Blank the stderr macros before
            // looking, or every diagnostic in the crate reads as a violation —
            // which is the same unbounded-substring mistake this crate exists
            // to stop making about shell commands.
            let code = code.replace("eprintln!", "").replace("eprint!", "");
            if code.contains("println!") || code.contains("print!") {
                offenders.push(format!("{}:{}", path.display(), n + 1));
            }
        }
    };
    for t in &trees {
        walk(t, &mut check);
    }
    assert!(
        offenders.is_empty(),
        "stdout is written outside decision.rs:\n{}",
        offenders.join("\n")
    );
}

fn walk(dir: &std::path::Path, f: &mut impl FnMut(&std::path::Path, &str)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, f);
        } else if p.extension().is_some_and(|x| x == "rs") {
            if let Ok(text) = std::fs::read_to_string(&p) {
                f(&p, &text);
            }
        }
    }
}

/// A clone whose origin has moved on. Two commits on the bare origin, the
/// clone taken after the first, so `HEAD` is exactly one behind.
fn a_stale_clone() -> (PathBuf, PathBuf) {
    let root = home().join("stale");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch root");
    let origin = root.join("origin.git");
    let work = root.join("work");
    let clone = root.join("clone");
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(
        &root,
        &[
            "init",
            "-q",
            "--bare",
            "--initial-branch=main",
            "origin.git",
        ],
    );
    git(&root, &["clone", "-q", origin.to_str().unwrap(), "work"]);
    git(&work, &["config", "user.email", "t@t.test"]);
    git(&work, &["config", "user.name", "t"]);
    git(&work, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(&work, &["push", "-q", "origin", "HEAD:main"]);
    git(&root, &["clone", "-q", origin.to_str().unwrap(), "clone"]);
    git(
        &work,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "feat: the thing that already exists",
        ],
    );
    git(&work, &["push", "-q", "origin", "HEAD:main"]);
    (clone, work)
}

fn session_start(cwd: &std::path::Path) -> String {
    format!(
        r#"{{"hook_event_name":"SessionStart","source":"startup","session_id":"sess1234","cwd":{}}}"#,
        serde_json::Value::String(cwd.to_string_lossy().into_owned())
    )
}

/// The whole point: a session opening in a checkout the remote has moved past
/// is told so, with the count and the newest commit it is missing — and the
/// remote ref was refreshed to find out, without touching the working tree.
#[test]
fn a_session_opening_in_a_stale_checkout_is_told_how_far_behind_it_is() {
    let (clone, _) = a_stale_clone();
    let r = send(&session_start(&clone));
    assert_eq!(r.code, 0);
    let doc = r.json().expect("a decision document");
    let out = &doc["hookSpecificOutput"];
    assert_eq!(out["hookEventName"], "SessionStart", "{doc}");
    let text = out["additionalContext"].as_str().unwrap_or_default();
    assert!(text.contains("1 commit behind origin/main"), "{text}");
    assert!(text.contains("the thing that already exists"), "{text}");
    assert!(text.starts_with("amont-agent/stale-base:"), "{text}");
    // Informed, not moved: HEAD is where it was.
    let head = Command::new("git")
        .args(["-C", clone.to_str().unwrap(), "log", "-1", "--format=%s"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "first");
}

/// Up to date is silence — zero bytes, like every other no-opinion.
#[test]
fn a_session_opening_in_a_current_checkout_says_nothing() {
    let (_, work) = a_stale_clone();
    let r = send(&session_start(&work));
    assert_eq!(r.stdout, "");
    assert_eq!(r.code, 0);
}

/// Not a repository, or a `cwd` that is gone: nothing to measure, nothing said.
#[test]
fn a_session_opening_outside_a_repository_says_nothing() {
    for cwd in [std::env::temp_dir(), PathBuf::from("/nonexistent/for/sure")] {
        let r = send(&session_start(&cwd));
        assert_eq!(r.stdout, "", "expected silence for {}", cwd.display());
        assert_eq!(r.code, 0);
    }
}

/// The branch-creation rule, end to end: a branch about to be started from a
/// stale HEAD is advised — and one started from `origin/main` is not.
#[test]
fn a_branch_started_from_a_stale_head_is_advised() {
    let (clone, _) = a_stale_clone();
    let payload = |command: &str| {
        format!(
            r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":{},
                 "session_id":"sess1234","tool_use_id":"t1","permission_mode":"default",
                 "tool_input":{{"command":{}}}}}"#,
            serde_json::Value::String(clone.to_string_lossy().into_owned()),
            serde_json::Value::String(command.to_string())
        )
    };
    let r = send(&payload("git worktree add ../clone-wt-x -b feat/x"));
    let doc = r.json().expect("a decision document");
    let text = doc["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(text.starts_with("amont-agent/stale-base:"), "{doc}");
    assert!(
        doc["hookSpecificOutput"]["permissionDecision"].is_null(),
        "advice refuses nothing: {doc}"
    );

    let r = send(&payload(
        "git worktree add ../clone-wt-x -b feat/x origin/main",
    ));
    assert_eq!(r.stdout, "", "the remedy must not trip the rule");
}

/// The guidance block is checked when the session opens — before an agent
/// has read and believed it.
///
/// This drives the shell-out, so it supplies its own `amont`: a stub whose
/// stderr and exit code are the contract this crate reads. Three cases,
/// because the middle one is the whole reason the decision is made on
/// stderr rather than on the exit code.
///
/// UNIX ONLY, and the reason is Windows' process creation rather than
/// anything about this crate. `CreateProcessW` appends `.exe` and nothing
/// else when the name it is given has no extension, so neither a `#!/bin/sh`
/// stub nor an `amont.bat` is reachable through `Command::new("amont")`
/// there — the stub would have to be a real compiled executable, which is
/// more machinery than this assertion is worth.
///
/// What is NOT lost on Windows: `guidance`'s own unit tests cover the stderr
/// parsing and every not-drift case on every platform, and
/// `a_marked_block_with_no_amont_installed_says_nothing` below covers the
/// no-amont path there too.
#[cfg(unix)]
#[test]
fn a_session_opening_on_a_stale_guidance_block_is_told() {
    let (_, work) = a_stale_clone(); // `work` is up to date with origin
                                     // The marker is what makes this crate bother to spawn anything at all.
    std::fs::write(
        work.join("AGENTS.md"),
        "# Project\n\n<!-- amont:start -->\nSTALE\n<!-- amont:end -->\n",
    )
    .unwrap();

    let bin = home().join("stub-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = bin.join("amont");
    let write_stub = |body: &str| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&stub, body).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        // macOS can hold the first exec of a freshly written script for
        // seconds, past the hook's wait for amont; run it once here so the
        // hook's call is not the first.
        let _ = std::process::Command::new(&stub).output();
    };

    // Drifted: amont exits 1 and says so on stderr.
    write_stub(
        "#!/bin/sh\necho \"$PWD/AGENTS.md: drifted from the generated block \
         — run \\`amont agents-md\\`\" >&2\nexit 1\n",
    );
    let r = send_with_path(&session_start(&work), Some(&bin));
    let doc = r.json().expect("a decision document");
    let text = doc["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(text.contains("amont-agent/agents-md: AGENTS.md"), "{text}");
    assert!(text.contains("amont agents-md"), "{text}");

    // Exit 1 WITHOUT a drift line — amont could not read the file. Same
    // exit code, and it must not be reported as staleness.
    write_stub("#!/bin/sh\necho \"$PWD/AGENTS.md: Permission denied\" >&2\nexit 1\n");
    let r = send_with_path(&session_start(&work), Some(&bin));
    assert_eq!(
        r.stdout, "",
        "exit 1 alone is not drift — amont uses it for unreadable files too"
    );

    // Up to date: nothing on stderr, exit 0.
    write_stub("#!/bin/sh\necho \"$PWD/AGENTS.md: up to date\"\nexit 0\n");
    let r = send_with_path(&session_start(&work), Some(&bin));
    assert_eq!(r.stdout, "", "a current block is not news");
}

/// Somebody who does not use amont has no `amont` on PATH, and the whole
/// feature must then be invisible rather than an error.
#[test]
fn a_marked_block_with_no_amont_installed_says_nothing() {
    let (_, work) = a_stale_clone();
    std::fs::write(
        work.join("AGENTS.md"),
        "# Project\n\n<!-- amont:start -->\nSTALE\n<!-- amont:end -->\n",
    )
    .unwrap();
    let empty = home().join("no-amont-here");
    std::fs::create_dir_all(&empty).unwrap();
    // An empty PATH: nothing resolves, `amont` least of all.
    let r = send_with_path_only(&session_start(&work), &empty);
    assert_eq!(r.stdout, "", "no amont, no opinion");
}

/// `push-preflight`, end to end: a push from an amont-guarded checkout whose
/// tree is not yet stamped is advised to rehearse; the same push once the
/// tree carries a push stamp is not. Drives the shell-out to `amont list`,
/// so it supplies its own `amont` stub (unix only, for the same reason as
/// the guidance test above).
#[cfg(unix)]
#[test]
fn a_push_from_an_unrehearsed_tree_is_advised_and_a_stamped_one_is_not() {
    let (_, work) = a_stale_clone(); // any real repo with a HEAD will do
                                     // amont's shim, by its one-word signature.
    let hooks = work.join(".git").join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(
        hooks.join("pre-push"),
        "#!/bin/sh\nexec amont --hooks-dir . pre-push \"$@\"\n",
    )
    .unwrap();
    // An `amont` whose `list --json --stage pre-push` says a JS suite runs.
    let bin = home().join("stub-bin-push");
    std::fs::create_dir_all(&bin).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let stub = bin.join("amont");
        std::fs::write(
            &stub,
            "#!/bin/sh\necho '{\"checks\":[{\"id\":\"pre-push-run-tests-js\",\"stage\":\"pre-push\",\"source\":\"builtin\",\"status\":\"runs\"}]}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let payload = format!(
        r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":{},
             "session_id":"sess1234","tool_use_id":"t1","permission_mode":"default",
             "tool_input":{{"command":"git push -u origin feat/x"}}}}"#,
        serde_json::Value::String(work.to_string_lossy().into_owned()),
    );
    let r = send_with_path(&payload, Some(&bin));
    let doc = r.json().expect("a decision document");
    let text = doc["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(text.starts_with("amont-agent/push-preflight:"), "{doc}");
    assert!(text.contains("amont rehearse --wait"), "{text}");
    assert!(
        doc["hookSpecificOutput"]["permissionDecision"].is_null(),
        "advice refuses nothing: {doc}"
    );

    // Rehearsed: a push stamp on HEAD's tree, as amont ≥ 1.27 writes it.
    let out = Command::new("git")
        .args([
            "-C",
            work.to_str().unwrap(),
            "notes",
            "--ref",
            "amont-gate",
            "add",
            "-f",
            "-m",
            "amont-gate-v1 pre-push-run-tests-js",
            "HEAD^{tree}",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = send_with_path(&payload, Some(&bin));
    assert_eq!(r.stdout, "", "a rehearsed tree is not nagged: {}", r.stdout);
}

/// A `confirm` that declines writes WHY, in the rule's words, and `status`
/// reads it back. This is the loop the crate runs on: a rule ships `observe`,
/// and the number that decides whether it may advise is how often `confirm`
/// agreed — a number the backtester cannot produce, because it never runs
/// `confirm`. Until this, the only reader of that number was `awk`.
#[test]
fn a_declined_confirm_records_why_and_status_reads_it_back() {
    // A primary checkout with no linked worktrees: `worktree-isolation`'s
    // `examine` fires on the shape and its `confirm` declines, naming why.
    let repo = home().join("solo-repo");
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).expect("scratch repo");
    let init = Command::new("git")
        .args(["init", "-q", "--initial-branch=main", "."])
        .current_dir(&repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(init.status.success());

    let payload = format!(
        r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":{},
             "session_id":"seen1234","tool_use_id":"t1","permission_mode":"default",
             "tool_input":{{"command":"git checkout -b feat/why"}}}}"#,
        serde_json::Value::String(repo.display().to_string())
    );
    let reply = send(&payload);
    assert_eq!(reply.code, 0);

    let journal = std::fs::read_to_string(home().join("amont-agent").join("journal.log"))
        .expect("the hook wrote a journal");
    let line = journal
        .lines()
        .find(|l| l.contains("worktree-isolation unconfirmed"))
        .unwrap_or_else(|| panic!("no unconfirmed record in:\n{journal}"));
    assert!(
        line.contains("nothing_else_is_checked_out_from_this_repository"),
        "the reason, in the rule's own words: {line}"
    );
    assert!(
        !line.contains(" skipped "),
        "`skipped` is not a reason: {line}"
    );

    let status = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
        .arg("status")
        .env("CLAUDE_CONFIG_DIR", home())
        .env_remove("AMONT_AGENT_OFF")
        .output()
        .expect("the binary runs");
    let text = String::from_utf8_lossy(&status.stdout);
    let row = text
        .lines()
        .find(|l| l.starts_with("worktree-isolation"))
        .unwrap_or_else(|| panic!("no status row in:\n{text}"));
    assert!(
        row.contains("1 unconfirmed (nothing else is checked out from this repository \u{d7}1)"),
        "status reads the journal back, reason and all: {row}"
    );
}

/// A scratch file worth reading, and the two halves of a Read on it: the
/// `PreToolUse` that asks whether the session already has the file, and the
/// `PostToolUse` that says the read happened.
struct ReadFixture {
    dir: PathBuf,
    file: PathBuf,
}

impl ReadFixture {
    fn new(name: &str) -> ReadFixture {
        ReadFixture::with_size(name, 6000)
    }

    /// A file of `bytes` bytes (a multiple of 100), in lines of 100.
    fn with_size(name: &str, bytes: usize) -> ReadFixture {
        let dir = home().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let file = dir.join("big.rs");
        let line = format!("{}\n", "x".repeat(99));
        std::fs::write(&file, line.repeat(bytes / 100)).expect("a file worth reading");
        ReadFixture { dir, file }
    }

    /// A scratch file named `file_name` holding `text`, for the write tools:
    /// the rule rebuilds the file from disk, so the file has to say something.
    fn with_content(name: &str, file_name: &str, text: &str) -> ReadFixture {
        let dir = home().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let file = dir.join(file_name);
        std::fs::write(&file, text).expect("a file worth editing");
        ReadFixture { dir, file }
    }

    /// As [`ReadFixture::file_event`], with the stance config pinned to `cfg`.
    fn file_event_with_global(
        &self,
        event: &str,
        tool: &str,
        session: &str,
        input: &str,
        cfg: &str,
    ) -> Reply {
        let cwd = serde_json::Value::String(self.dir.display().to_string());
        let path = serde_json::Value::String(self.file.display().to_string());
        send_with_global(
            &format!(
                r#"{{"hook_event_name":"{event}","tool_name":"{tool}","cwd":{cwd},
                 "session_id":"{session}","permission_mode":"default",
                 "tool_input":{{"file_path":{path}{input}}}}}"#
            ),
            cfg,
        )
    }

    fn file_event(&self, event: &str, tool: &str, session: &str, input: &str) -> Reply {
        let cwd = serde_json::Value::String(self.dir.display().to_string());
        let path = serde_json::Value::String(self.file.display().to_string());
        send(&format!(
            r#"{{"hook_event_name":"{event}","tool_name":"{tool}","cwd":{cwd},
                 "session_id":"{session}","permission_mode":"default",
                 "tool_input":{{"file_path":{path}{input}}}}}"#
        ))
    }

    /// About to Read.
    fn read(&self, session: &str) -> Reply {
        self.file_event("PreToolUse", "Read", session, "")
    }

    /// Read, and it worked.
    fn read_done(&self, session: &str) -> Reply {
        let done = self.file_event("PostToolUse", "Read", session, "");
        assert_eq!(done.stdout, "", "a finished read is remembered silently");
        done
    }

    fn bash_event(&self, event: &str, session: &str, command: &str) -> Reply {
        let cwd = serde_json::Value::String(self.dir.display().to_string());
        send(&format!(
            r#"{{"hook_event_name":"{event}","tool_name":"Bash","cwd":{cwd},
                 "session_id":"{session}","permission_mode":"default",
                 "tool_input":{{"command":"{command}"}}}}"#
        ))
    }
}

/// The file tier. A Read is remembered; a second Read of the same path with
/// nothing written to it since is advised against; an Edit in between makes
/// the next Read silent again; and a `cat` of the same file is the same
/// question asked from the shell.
#[test]
fn a_file_read_twice_is_advised_and_an_edit_between_resets_it() {
    let f = ReadFixture::new("reread-repo");
    let read = |session: &str| f.read(session);

    let first = read("rr-1");
    assert_eq!(first.code, 0);
    assert_eq!(first.stdout, "", "a first read is not commented on");
    f.read_done("rr-1");

    let second = read("rr-1");
    assert!(
        second.reason().contains("file-reread") && second.reason().contains("already read"),
        "second read: {}",
        second.stdout
    );
    assert_eq!(
        second.decision(),
        None,
        "advise, not deny: {}",
        second.stdout
    );

    // Another session has its own record.
    assert_eq!(read("rr-2").stdout, "");

    let edit = f.file_event(
        "PreToolUse",
        "Edit",
        "rr-1",
        r#","old_string":"x","new_string":"y""#,
    );
    assert_eq!(edit.stdout, "", "a write is remembered silently");
    assert_eq!(read("rr-1").stdout, "", "a read after an edit is right");
    f.read_done("rr-1");

    // From the shell: a dump of the file the session just read.
    let cat = f.bash_event("PreToolUse", "rr-1", "cat big.rs");
    let said = cat.reason();
    assert!(
        said.contains("file-reread"),
        "cat after Read: {}",
        cat.stdout
    );
    assert!(
        said.contains("whole-file-dump"),
        "a 6 KB cat is a dump too: {}",
        cat.stdout
    );
}

/// A first Read of a large file with no window is advised, never refused.
#[test]
fn a_large_whole_read_is_advised() {
    let f = ReadFixture::with_size("unbounded-large", 20_000);
    let first = f.read("ul-1");
    assert_eq!(first.code, 0);
    assert!(
        first.reason().contains("read-unbounded-large"),
        "{}",
        first.stdout
    );
    assert_eq!(first.decision(), None, "advise, not deny: {}", first.stdout);
}

/// A window, or a deliberate whole read spelled `offset: 1` plus a `limit`,
/// is the way through.
#[test]
fn a_windowed_or_deliberate_whole_read_is_silent() {
    let f = ReadFixture::with_size("unbounded-window", 20_000);
    let windowed = f.file_event("PreToolUse", "Read", "uw-1", r#","limit":200"#);
    assert!(
        !windowed.stdout.contains("read-unbounded-large"),
        "{}",
        windowed.stdout
    );
    let whole = f.file_event("PreToolUse", "Read", "uw-2", r#","offset":1,"limit":200"#);
    assert!(
        !whole.stdout.contains("read-unbounded-large"),
        "{}",
        whole.stdout
    );
}

/// Under the threshold the advice costs more than the read.
#[test]
fn a_small_first_read_stays_silent() {
    let f = ReadFixture::new("unbounded-small");
    assert_eq!(f.read("us-1").stdout, "");
}

/// A second whole Read is `file-reread`'s to answer, and only its.
#[test]
fn a_reread_says_file_reread_only() {
    let f = ReadFixture::with_size("unbounded-reread", 20_000);
    assert!(f.read("ur-1").reason().contains("read-unbounded-large"));
    f.read_done("ur-1");
    let second = f.read("ur-1");
    let said = second.reason();
    assert!(said.contains("file-reread"), "{}", second.stdout);
    assert!(!said.contains("read-unbounded-large"), "{}", second.stdout);
}

/// With `file-reread` observing, a re-read still keeps this rule quiet: it
/// yields to whether the file was seen, not to whether a message came back.
#[test]
fn an_observed_reread_still_suppresses() {
    let f = ReadFixture::with_size("unbounded-observed", 20_000);
    let cfg = "[amont \"agent.file-reread\"]\n\tstance = observe";
    let session = "obsrd-01";
    // The first read is only remembered, so the journal below holds what the
    // second one did and nothing else.
    f.file_event_with_global("PostToolUse", "Read", session, "", cfg);
    let second = f.file_event_with_global("PreToolUse", "Read", session, "", cfg);
    assert_eq!(second.stdout, "", "neither rule speaks: {}", second.stdout);

    let journal = std::fs::read_to_string(home().join("amont-agent").join("journal.log"))
        .expect("the hook wrote a journal");
    let rows: Vec<&str> = journal.lines().filter(|l| l.contains(session)).collect();
    let rereads = rows
        .iter()
        .filter(|l| l.contains("file-reread") && l.contains("watched"))
        .count();
    let large = rows
        .iter()
        .filter(|l| l.contains("read-unbounded-large"))
        .count();
    assert_eq!(rereads, 1, "{rows:?}");
    assert_eq!(large, 0, "{rows:?}");
}

/// A Read is remembered once it has HAPPENED. One that was refused, or that
/// failed on a path that is not there, put nothing in context — and a retry
/// told "its contents are already in context" is a retry that never learns
/// what the file says. The same for a `cat`: only the `PostToolUse` that
/// reports it ran is the read.
#[test]
fn a_read_that_never_completed_is_not_remembered() {
    let f = ReadFixture::new("reread-incomplete");

    assert_eq!(
        f.read("ri-1").stdout,
        "",
        "a first read is not commented on"
    );
    assert_eq!(
        f.read("ri-1").stdout,
        "",
        "no PostToolUse came back, so the first Read is not on record"
    );

    let cat = f.bash_event("PreToolUse", "ri-2", "cat big.rs");
    assert!(
        !cat.reason().contains("file-reread"),
        "nothing read yet: {}",
        cat.stdout
    );
    let again = f.bash_event("PreToolUse", "ri-2", "cat big.rs");
    assert!(
        !again.reason().contains("file-reread"),
        "the first cat never ran, so it is not a read: {}",
        again.stdout
    );
    assert_eq!(
        f.bash_event("PostToolUse", "ri-2", "cat big.rs").stdout,
        "",
        "a finished cat is remembered silently"
    );
    let third = f.bash_event("PreToolUse", "ri-2", "cat big.rs");
    assert!(
        third.reason().contains("file-reread"),
        "now it has run once: {}",
        third.stdout
    );
}

/// The Edit tool is not the only thing that writes a file: `sed -i`, a
/// formatter, `git checkout`, another session in the same checkout. A file
/// that changed on disk since it was read is read again without comment —
/// the record is checked against the file, not trusted on its own.
#[test]
fn a_file_changed_behind_the_sessions_back_is_read_again_without_comment() {
    let f = ReadFixture::new("reread-changed");
    assert_eq!(f.read("rc-1").stdout, "");
    f.read_done("rc-1");
    assert!(
        f.read("rc-1").reason().contains("file-reread"),
        "unchanged, so the second read is advised against"
    );

    std::fs::write(&f.file, "y".repeat(6001)).expect("rewrite the file");
    assert_eq!(
        f.read("rc-1").stdout,
        "",
        "the file is not what the session read any more"
    );
    f.read_done("rc-1");
    assert!(
        f.read("rc-1").reason().contains("file-reread"),
        "read again and unchanged since: advised again"
    );

    std::fs::remove_file(&f.file).expect("remove the file");
    assert_eq!(
        f.read("rc-1").stdout,
        "",
        "a file that is gone is not in context"
    );
}

/// The shape that was advised live on 2026-09-14: a background task's output
/// file, Read while still EMPTY, then read again once the task had written
/// to it. Nothing in the session wrote it, so the record alone says
/// "unchanged"; the file says otherwise, and the file wins. An empty read is
/// also the one case where "what it says is already in context" is exactly
/// nothing — worth pinning on its own, apart from the rewrite above.
#[test]
fn a_file_read_while_empty_and_filled_by_a_background_task_is_read_again() {
    let f = ReadFixture::new("reread-empty-then-written");
    std::fs::write(&f.file, "").expect("an empty output file");
    assert_eq!(f.read("bg-1").stdout, "");
    f.read_done("bg-1");
    assert!(
        f.read("bg-1").reason().contains("file-reread"),
        "still empty, still what the session saw: advised against"
    );

    // Nothing the hook sees writes it — a background process appends.
    let mut out = std::fs::OpenOptions::new()
        .append(true)
        .open(&f.file)
        .expect("append to the output file");
    std::io::Write::write_all(&mut out, b"21:07:41 state=success\n").expect("append");
    drop(out);
    assert_eq!(
        f.read("bg-1").stdout,
        "",
        "the task wrote to it since: read again without comment"
    );
}

/// A poll whose budget fits the call's timeout is left alone; one that can
/// outlast it is what the rule is about.
#[test]
fn a_poll_is_judged_against_the_calls_timeout() {
    // A directory that exists on every platform: `confirm` declines in one
    // that does not, and `/tmp` is not one on Windows.
    let cwd = serde_json::Value::String(home().display().to_string());
    let payload = |timeout: &str, cmd: &str| {
        send(&format!(
            r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":{cwd},
                 "session_id":"poll-1","permission_mode":"default",
                 "tool_input":{{"command":"{cmd}"{timeout}}}}}"#
        ))
    };
    let loop_ =
        "for i in $(seq 1 36); do gh pr checks 1 | grep -q pending || break; sleep 15; done";
    assert!(
        payload("", loop_).reason().contains("foreground-poll"),
        "nine minutes against a two-minute default"
    );
    assert_eq!(
        payload(r#","timeout":600000"#, loop_).stdout,
        "",
        "nine minutes inside an explicit ten"
    );
    assert!(
        payload(r#","timeout":600000"#, "while true; do sleep 5; done")
            .reason()
            .contains("foreground-poll"),
        "unbounded is over any clock"
    );
}

/// A push sent to the background has no clock but its own (amont-agent#86).
///
/// The pre-push gate runs inside the push: a test suite, a lint pass. In the
/// foreground the tool's timeout ends it; in the background nothing does, and
/// on 2026-10-08 a seeding push held a session for 40 minutes before anybody
/// asked. `timeout <s> git push …` bounds it, and the push guards read
/// through that wrapper.
#[test]
fn a_background_push_without_a_deadline_is_advised() {
    let cwd = serde_json::Value::String(home().display().to_string());
    let payload = |background: bool, cmd: &str| {
        let bg = if background {
            r#","run_in_background":true"#
        } else {
            ""
        };
        send(&format!(
            r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":{cwd},
                 "session_id":"bgpush-1","permission_mode":"default",
                 "tool_input":{{"command":"{cmd}"{bg}}}}}"#
        ))
    };
    assert!(
        payload(true, "git push origin dev > /tmp/push.log 2>&1")
            .reason()
            .contains("unbounded-background-push"),
        "a background push with no deadline"
    );
    assert!(
        !payload(
            true,
            "timeout 1800 git push origin dev > /tmp/push.log 2>&1"
        )
        .reason()
        .contains("unbounded-background-push"),
        "`timeout` is the deadline"
    );
    assert!(
        !payload(true, "timeout -k 30 1800 git push -u origin feat/x")
            .reason()
            .contains("unbounded-background-push"),
        "timeout's own flags do not hide it"
    );
    assert!(
        !payload(false, "git push origin dev")
            .reason()
            .contains("unbounded-background-push"),
        "in the foreground the tool's own timeout bounds it"
    );
    assert!(
        !payload(true, "git push --dry-run origin dev")
            .reason()
            .contains("unbounded-background-push"),
        "a dry run runs no gate"
    );
}

// `open` is a launcher only on macOS: off it the rule is silent, which the
// test below asserts, so the platform is checked on every CI host.
#[cfg(target_os = "macos")]
#[test]
fn a_background_gui_launch_without_a_deadline_is_advised() {
    let cwd = serde_json::Value::String(home().display().to_string());
    let payload = |background: bool, cmd: &str| {
        let bg = if background {
            r#","run_in_background":true"#
        } else {
            ""
        };
        send(&format!(
            r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":{cwd},
                 "session_id":"guiopen-1","permission_mode":"default",
                 "tool_input":{{"command":"{cmd}"{bg}}}}}"#
        ))
    };
    let fires = |background: bool, cmd: &str| {
        payload(background, cmd)
            .reason()
            .contains("gui-open-unbounded")
    };
    assert!(
        fires(
            true,
            "xcrun simctl boot X 2>/dev/null; open -a Simulator; yarn ios:dev > build.log 2>&1"
        ),
        "the 2026-10-14 chain"
    );
    assert!(fires(true, "open /Applications/Foo.app"), "an .app operand");
    assert!(
        fires(true, "open -g -a Simulator") && fires(true, "open -j -a Simulator"),
        "-g and -j are not a deadline"
    );
    assert!(
        !fires(true, "timeout 15 open -a Simulator"),
        "`timeout` is the deadline"
    );
    assert!(
        fires(true, "timeout 15 open -a Simulator; open -a Safari"),
        "one bounded launch does not hide another"
    );
    assert!(
        !fires(false, "open -a Simulator"),
        "in the foreground the tool's own timeout bounds it"
    );
    assert!(
        !fires(true, "open README.md") && !fires(true, "open https://example.com"),
        "only an app launch is flagged"
    );
    assert!(
        !fires(true, "open -R /Applications/Foo.app"),
        "revealing in Finder launches nothing"
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn a_background_gui_launch_is_silent_off_macos() {
    let cwd = serde_json::Value::String(home().display().to_string());
    let payload = |cmd: &str| {
        send(&format!(
            r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":{cwd},
                 "session_id":"guiopen-2","permission_mode":"default",
                 "tool_input":{{"command":"{cmd}","run_in_background":true}}}}"#
        ))
    };
    assert!(
        !payload("open -a Simulator")
            .reason()
            .contains("gui-open-unbounded"),
        "`open` launches an app only on macOS"
    );
}

/// A pipeline we cannot read costs us that pipeline, not the line.
///
/// The `xargs` run is unreadable; the `git commit … | tail -1` beside it is
/// not, and it is the shape the only blocking rule exists for. Before
/// per-pipeline opacity the whole command was silence.
#[test]
fn a_hidden_pipeline_does_not_silence_the_rest_of_the_line() {
    let dir = home();
    let cwd = serde_json::Value::String(dir.display().to_string());
    let cmd = serde_json::Value::String(
        "git status -s | xargs git add && git commit -q -m x 2>&1 | tail -1".to_string(),
    );
    let reply = send(&format!(
        r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":{cwd},
             "session_id":"partial-1","permission_mode":"default",
             "tool_input":{{"command":{cmd}}}}}"#
    ));
    assert_eq!(reply.code, 0);
    let said = reply.reason();
    assert!(
        said.contains("pipe-to-tail"),
        "the readable pipeline was not judged: {}",
        reply.stdout
    );
    // Capped: a command we only partly read may advise, never refuse.
    assert_eq!(
        reply.decision(),
        None,
        "a half-read command produced a refusal: {}",
        reply.stdout
    );
}

/// Nothing readable survived, so the guard stays as silent as it always was.
#[test]
fn a_wholly_unreadable_command_is_still_not_judged() {
    let cwd = serde_json::Value::String(home().display().to_string());
    for command in [
        "git status --short | xargs git add",
        "source ./setup.sh && git push origin main 2>&1 | tail -1",
    ] {
        let cmd = serde_json::Value::String(command.to_string());
        let reply = send(&format!(
            r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":{cwd},
                 "session_id":"partial-2","permission_mode":"default",
                 "tool_input":{{"command":{cmd}}}}}"#
        ));
        assert_eq!(reply.stdout, "", "{command} was judged: {}", reply.stdout);
    }
}

/// The hook, with its stance config pinned to `global` so the developer's own
/// `~/.gitconfig` cannot decide what the test sees.
fn send_with_global(payload: &str, global: &str) -> Reply {
    let dir = home();
    let cfg = dir.join("fanout.gitconfig");
    std::fs::write(&cfg, global).expect("global config");
    let mut child = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
        .arg("hook")
        .env("CLAUDE_CONFIG_DIR", &dir)
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("SHELL", "/bin/zsh")
        .env_remove("AMONT_AGENT_OFF")
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
    Reply {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
    }
}

fn journal_rows(session: &str) -> Vec<String> {
    std::fs::read_to_string(home().join("amont-agent").join("journal.log"))
        .expect("the hook wrote a journal")
        .lines()
        .filter(|l| l.contains(session))
        .map(str::to_string)
        .collect()
}

const ADD_IGNORE: &str =
    r#","old_string":"return g(a)","new_string":"return g(a)  # type: ignore""#;

/// An Edit that adds a suppression is advised, never refused, and the advice
/// names the line it is on.
#[test]
fn lint_suppression_an_edit_adding_a_type_ignore_is_advised() {
    let f = ReadFixture::with_content("lsa-add", "x.py", "def f(a):\n    return g(a)\n");
    let said = f.file_event("PreToolUse", "Edit", "lsa-add1", ADD_IGNORE);
    assert_eq!(said.code, 0);
    assert!(
        said.reason().contains("lint-suppression-added"),
        "{}",
        said.stdout
    );
    assert!(
        said.reason().contains("line 2: # type: ignore"),
        "{}",
        said.stdout
    );
    assert_eq!(said.decision(), None, "advise, not deny: {}", said.stdout);
    let rows = journal_rows("lsa-add1");
    assert!(
        rows.iter().any(|r| r.contains("lint-suppression-added")
            && r.contains(" advised ")
            && r.contains("x.py:2 type: ignore[] (+1) exact")),
        "{rows:?}"
    );
}

/// Changing code on a line that keeps its suppression is not adding one.
#[test]
fn lint_suppression_an_edit_on_an_already_suppressed_line_is_silent() {
    let f = ReadFixture::with_content("lsa-kept", "x.py", "x = f(a)  # type: ignore\n");
    let said = f.file_event(
        "PreToolUse",
        "Edit",
        "lsa-kept1",
        r#","old_string":"f(a)","new_string":"f(b)""#,
    );
    assert_eq!(said.stdout, "", "{}", said.stdout);
}

/// A Write has no file to read for a new path, and a config is looked at
/// too: loosening `strict` is advised.
#[test]
fn lint_suppression_a_new_tsconfig_with_strict_off_is_advised() {
    let f = ReadFixture::with_content("lsa-ts", "tsconfig.json", "");
    std::fs::remove_file(&f.file).expect("a new file");
    let said = f.file_event(
        "PreToolUse",
        "Write",
        "lsa-ts-1",
        r#","content":"{\"compilerOptions\": {\"strict\": false}}""#,
    );
    assert!(
        said.reason().contains("lint-suppression-added"),
        "{}",
        said.stdout
    );
    assert!(
        said.reason()
            .contains("loosens tsconfig.json strict = false"),
        "{}",
        said.stdout
    );
    assert_eq!(said.decision(), None, "advise, not deny: {}", said.stdout);
}

/// `observe` through the git config: nothing said, and the journal shows the
/// rule watched.
#[test]
fn lint_suppression_observed_is_silent_and_journalled_as_watched() {
    let f = ReadFixture::with_content("lsa-obs", "x.py", "def f(a):\n    return g(a)\n");
    let cfg = "[amont \"agent.lint-suppression-added\"]\n\tstance = observe";
    let session = "lsaobs-1";
    let said = f.file_event_with_global("PreToolUse", "Edit", session, ADD_IGNORE, cfg);
    assert_eq!(said.stdout, "", "{}", said.stdout);
    let rows = journal_rows(session);
    assert!(
        rows.iter()
            .any(|r| r.contains("lint-suppression-added") && r.contains(" watched ")),
        "{rows:?}"
    );
}

/// The implementation-review guard comes first and still refuses, whatever the
/// content says.
#[test]
fn lint_suppression_leaves_the_pass_file_guard_denying() {
    let f = ReadFixture::with_content("lsa-guard", "x.py", "");
    let target = home()
        .join("amont-agent")
        .join("implementation-review")
        .join("by-tree")
        .join("app")
        .join("z.py");
    let cwd = serde_json::Value::String(f.dir.display().to_string());
    let path = serde_json::Value::String(target.display().to_string());
    let said = send(&format!(
        r#"{{"hook_event_name":"PreToolUse","tool_name":"Write","cwd":{cwd},
             "session_id":"lsa-gd-1","permission_mode":"default",
             "tool_input":{{"file_path":{path},"content":"x = 1  # noqa\n"}}}}"#
    ));
    assert_eq!(said.decision().as_deref(), Some("deny"), "{}", said.stdout);
    assert!(
        said.reason().contains("implementation-review"),
        "{}",
        said.stdout
    );
    assert!(
        !said.reason().contains("lint-suppression-added"),
        "{}",
        said.stdout
    );
}

/// The write is recorded before any advice is built, so an advised Edit still
/// resets what the session knows of the file, and `file-reread` sees it.
#[test]
fn lint_suppression_an_advised_edit_is_a_write_for_file_reread() {
    let f = ReadFixture::with_content("lsa-reread", "x.py", "def f(a):\n    return g(a)\n");
    let session = "lsarr-01";
    f.read_done(session);
    let again = f.read(session);
    assert!(
        again.reason().contains("file-reread"),
        "control: {}",
        again.stdout
    );

    let edit = f.file_event("PreToolUse", "Edit", session, ADD_IGNORE);
    assert!(
        edit.reason().contains("lint-suppression-added"),
        "{}",
        edit.stdout
    );
    assert_eq!(
        f.read(session).stdout,
        "",
        "a read after the edit is a different read"
    );
}

/// `PATH` for the LSP tests: the stub directory, then the first directory
/// holding `git` (the stance is read with `git config`) and no real
/// `fleet-lsp`, so removing the stub really leaves none.
fn lsp_path(bin: &std::path::Path) -> std::ffi::OsString {
    let has = |d: &std::path::Path, name: &str| {
        d.join(name).is_file() || d.join(format!("{name}.exe")).is_file()
    };
    let system = std::env::var_os("PATH").unwrap_or_default();
    let git = std::env::split_paths(&system).find(|d| has(d, "git") && !has(d, "fleet-lsp"));
    std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(git))
        .expect("a PATH of two directories")
}

/// The world the LSP rules ask about, built per test: a git working tree
/// (`.git` is enough for the walk up), a `fleet-lsp` stub on `PATH`, and the
/// plugin enabled in the scratch config.
struct LspWorld {
    repo: PathBuf,
    bin: PathBuf,
    config: PathBuf,
}

impl LspWorld {
    fn new(name: &str, plugin_on: bool) -> LspWorld {
        let root = home().join(format!("lsp-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).expect("repo");
        std::fs::create_dir_all(repo.join("src")).expect("src");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let stub = bin.join("fleet-lsp");
        std::fs::write(&stub, "#!/bin/sh\n").expect("stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let config = root.join("config");
        std::fs::create_dir_all(&config).expect("config");
        std::fs::write(
            config.join("settings.json"),
            format!(r#"{{"enabledPlugins":{{"fleet-lsp@fleet-lsp":{plugin_on}}}}}"#),
        )
        .expect("settings");
        LspWorld { repo, bin, config }
    }

    fn file(&self, rel: &str, bytes: usize) -> PathBuf {
        let p = self.repo.join(rel);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("dirs");
        std::fs::write(&p, "x".repeat(bytes)).expect("file");
        p
    }

    /// Send a payload with this world's config, `PATH` = the stub plus the
    /// system dirs, and `global` as the git config (for a stance).
    fn send(&self, payload: &str, global: &str) -> Reply {
        let cfg = self.config.join("global.gitconfig");
        std::fs::write(&cfg, global).expect("global config");
        let mut child = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .arg("hook")
            .env("CLAUDE_CONFIG_DIR", &self.config)
            .env("PATH", lsp_path(&self.bin))
            .env("GIT_CONFIG_GLOBAL", &cfg)
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
            .expect("stdin")
            .write_all(payload.as_bytes())
            .expect("write the payload");
        let out = child.wait_with_output().expect("the hook exits");
        assert!(out.stderr.is_empty(), "nothing on stderr: {:?}", out.stderr);
        Reply {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        }
    }

    fn read(&self, session: &str, path: &std::path::Path, extra: &str) -> Reply {
        self.read_with(session, path, extra, "")
    }

    fn read_with(&self, session: &str, path: &std::path::Path, extra: &str, global: &str) -> Reply {
        let cwd = serde_json::Value::String(self.repo.display().to_string());
        let path = serde_json::Value::String(path.display().to_string());
        self.send(
            &format!(
                r#"{{"hook_event_name":"PreToolUse","tool_name":"Read","cwd":{cwd},
                     "session_id":"{session}","permission_mode":"default"{extra},
                     "tool_input":{{"file_path":{path}}}}}"#
            ),
            global,
        )
    }

    /// A `PostToolUse` LSP payload; `result` is the answer text and `agent`
    /// an `agent_id` (or none for the main conversation).
    fn lsp(&self, session: &str, agent: Option<&str>, op: &str, file: &str, result: &str) -> Reply {
        let mut v = serde_json::json!({
            "hook_event_name": "PostToolUse", "tool_name": "LSP",
            "cwd": self.repo.display().to_string(),
            "session_id": session, "permission_mode": "default",
            "tool_input": {"operation": op, "filePath": file, "line": 1, "character": 1},
            "tool_response": {"operation": op, "result": result, "filePath": file},
        });
        if let Some(a) = agent {
            v["agent_id"] = a.into();
        }
        self.send(&v.to_string(), "")
    }

    fn read_as(&self, session: &str, agent: &str, path: &std::path::Path, global: &str) -> Reply {
        let extra = format!(r#","agent_id":"{agent}","agent_type":"Explore""#);
        self.read_with(session, path, &extra, global)
    }

    fn journal_rows(&self, session: &str) -> Vec<String> {
        std::fs::read_to_string(self.config.join("amont-agent").join("journal.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains(session))
            .map(str::to_string)
            .collect()
    }

    fn grep_tool(&self, session: &str, input: &str) -> Reply {
        let cwd = serde_json::Value::String(self.repo.display().to_string());
        self.send(
            &format!(
                r#"{{"hook_event_name":"PreToolUse","tool_name":"Grep","cwd":{cwd},
                     "session_id":"{session}","permission_mode":"default",
                     "tool_input":{input}}}"#
            ),
            "",
        )
    }

    fn bash(&self, session: &str, command: &str) -> Reply {
        let cwd = serde_json::Value::String(self.repo.display().to_string());
        let command = serde_json::Value::String(command.to_string());
        self.send(
            &format!(
                r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":{cwd},
                     "session_id":"{session}","tool_use_id":"t1","permission_mode":"default",
                     "tool_input":{{"command":{command}}}}}"#
            ),
            "",
        )
    }
}

/// A whole Read of code LSP serves is advised toward LSP, once a session, and
/// `read-unbounded-large` stays quiet while it speaks.
#[test]
fn a_whole_read_of_served_code_is_advised_toward_lsp_once() {
    let w = LspWorld::new("read", true);
    let big = w.file("src/lib.rs", 20_000);
    let first = w.read("lr-1", &big, "");
    assert_eq!(first.code, 0);
    let said = first.reason();
    assert!(said.contains("code-read-not-lsp"), "{}", first.stdout);
    assert!(said.contains("documentSymbol"), "{said}");
    assert!(
        !said.contains("read-unbounded-large"),
        "one advisory: {said}"
    );
    assert_eq!(first.decision(), None, "advise, not deny");
    let again = w.read("lr-1", &big, "");
    assert!(
        !again.stdout.contains("code-read-not-lsp"),
        "once: {}",
        again.stdout
    );
    assert!(
        w.read("lr-2", &big, "")
            .reason()
            .contains("code-read-not-lsp"),
        "another session is told too"
    );
}

/// Turned off, the rule gives the Read back to `read-unbounded-large`, whose
/// remedy then names LSP rather than grep.
#[test]
fn an_observed_code_read_leaves_the_size_advice_speaking() {
    let w = LspWorld::new("observe", true);
    let big = w.file("src/lib.rs", 20_000);
    let r = w.read_with(
        "lo-1",
        &big,
        "",
        "[amont \"agent.code-read-not-lsp\"]\n\tstance = observe\n",
    );
    let said = r.reason();
    assert!(said.contains("read-unbounded-large"), "{}", r.stdout);
    assert!(!said.contains("code-read-not-lsp"), "{said}");
    assert!(
        said.contains("documentSymbol") && !said.contains("grep -n"),
        "{said}"
    );
}

/// Silent: small code, text, a window, a persisted result, the plugin off, no
/// repository, an agent without the LSP tool.
#[test]
fn a_code_read_is_silent_where_lsp_cannot_help() {
    let w = LspWorld::new("silent", true);
    let small = w.file("src/small.rs", 3_000);
    assert_eq!(w.read("ls-1", &small, "").stdout, "", "under 4 KB");
    let md = w.file("notes.md", 9_000);
    assert!(!w.read("ls-1", &md, "").stdout.contains("code-read-not-lsp"));
    let mid = w.file("src/mid.rs", 9_000);
    let windowed = w.send(
        &serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Read",
            "cwd": w.repo.display().to_string(),
            "session_id": "ls-1",
            "tool_input": {"file_path": mid.display().to_string(), "offset": 1, "limit": 40},
        })
        .to_string(),
        "",
    );
    assert_eq!(windowed.stdout, "", "a window");
    let persisted = w.file("tool-results/x.rs", 9_000);
    assert!(!w
        .read("ls-1", &persisted, "")
        .stdout
        .contains("code-read-not-lsp"));
    std::fs::create_dir_all(w.repo.join(".claude/agents")).expect("agents");
    std::fs::write(
        w.repo.join(".claude/agents/narrow.md"),
        "---\nname: narrow\ntools: Read, Grep\n---\nbody\n",
    )
    .expect("agent");
    let narrow = w.read("ls-2", &mid, r#","agent_id":"a1","agent_type":"narrow""#);
    assert_eq!(narrow.stdout, "", "an agent without LSP");
    let unnamed = w.read("ls-3", &mid, r#","agent_id":"a1""#);
    assert_eq!(unnamed.stdout, "", "a subagent of unknown type");
    let explore = w.read("ls-4", &mid, r#","agent_id":"a1","agent_type":"Explore""#);
    assert!(
        explore.reason().contains("code-read-not-lsp"),
        "{}",
        explore.stdout
    );

    let off = LspWorld::new("silent-off", false);
    let mid = off.file("src/mid.rs", 9_000);
    assert_eq!(off.read("ls-5", &mid, "").stdout, "", "plugin off");
    let nobin = LspWorld::new("silent-nobin", true);
    let mid = nobin.file("src/mid.rs", 9_000);
    std::fs::remove_file(nobin.bin.join("fleet-lsp")).expect("rm stub");
    assert_eq!(
        nobin.read("ls-7", &mid, "").stdout,
        "",
        "no fleet-lsp on PATH"
    );
    let outside = LspWorld::new("silent-nogit", true);
    let mid = outside.file("src/mid.rs", 9_000);
    std::fs::remove_dir_all(outside.repo.join(".git")).expect("rm .git");
    assert_eq!(
        outside.read("ls-6", &mid, "").stdout,
        "",
        "outside a repository"
    );
}

/// The Grep tool: a symbol over code is advised once; a count or a text
/// pattern is not.
#[test]
fn a_grep_tool_symbol_search_over_code_is_advised_once() {
    let w = LspWorld::new("grep-tool", true);
    let first = w.grep_tool(
        "gt-1",
        r#"{"pattern":"fn agent_type","path":".","output_mode":"content","-n":true}"#,
    );
    assert!(
        first.reason().contains("symbol-grep-not-lsp"),
        "{}",
        first.stdout
    );
    assert!(
        first.reason().contains("workspaceSymbol"),
        "{}",
        first.stdout
    );
    let again = w.grep_tool("gt-1", r#"{"pattern":"fn agent_type","path":"."}"#);
    assert_eq!(again.stdout, "", "once per pattern");
    assert_eq!(
        w.grep_tool("gt-2", r#"{"pattern":"agent_type","output_mode":"count"}"#)
            .stdout,
        ""
    );
    assert_eq!(
        w.grep_tool(
            "gt-2",
            r#"{"pattern":"ingestion_time|valid_from","path":"."}"#
        )
        .stdout,
        ""
    );
    assert_eq!(
        w.grep_tool("gt-2", r#"{"pattern":"agent_type","glob":"**/*.md"}"#)
            .stdout,
        ""
    );
}

/// The shell: a recursive symbol grep in the repository is advised once; a
/// pipe filter is not.
#[test]
fn a_shell_symbol_grep_over_code_is_advised_once() {
    let w = LspWorld::new("grep-bash", true);
    w.file("src/lib.rs", 100);
    let first = w.bash("gb-1", "grep -rn agent_types src");
    assert!(
        first.reason().contains("symbol-grep-not-lsp"),
        "{}",
        first.stdout
    );
    let again = w.bash("gb-1", "grep -rn agent_types src");
    assert!(
        !again.stdout.contains("symbol-grep-not-lsp"),
        "{}",
        again.stdout
    );
    let filter = w.bash("gb-1", "cargo test 2>&1 | grep agent_types");
    assert!(
        !filter.stdout.contains("symbol-grep-not-lsp"),
        "{}",
        filter.stdout
    );
}

const DENY: &str = "[amont \"agent.code-read-not-lsp\"]\n\tstance = deny\n";

/// A whole Read after a documentSymbol on the same file is silent, and the
/// journal holds the call, then the rule's `lsp-first`.
#[test]
fn a_read_after_a_document_symbol_call_is_silent_and_journalled_lsp_first() {
    let w = LspWorld::new("first", true);
    let big = w.file("src/lib.rs", 10_000);
    let call = w.lsp(
        "lf-1",
        None,
        "documentSymbol",
        "src/lib.rs",
        "Document symbols:\nx - Line 1",
    );
    assert_eq!(
        (call.code, call.stdout.as_str()),
        (0, ""),
        "PostToolUse never speaks"
    );
    let read = w.read("lf-1", &big, "");
    assert_eq!(read.stdout, "", "silent");
    let rows = w.journal_rows("lf-1");
    let lsp = rows
        .iter()
        .position(|r| r.contains("\tlsp\t") || r.contains(" lsp "))
        .unwrap_or_else(|| panic!("{rows:?}"));
    let first = rows
        .iter()
        .position(|r| r.contains("code-read-not-lsp") && r.contains("lsp-first"))
        .unwrap_or_else(|| panic!("{rows:?}"));
    assert!(lsp < first, "{rows:?}");
    assert!(rows[lsp].contains("documentSymbol"), "{rows:?}");
}

/// LSP first quiets the LSP advice only: a whole Read past 16 KiB still gets
/// the size advice, whose remedy is the window LSP pointed to.
#[test]
fn an_lsp_call_leaves_the_size_advice_speaking() {
    let w = LspWorld::new("first-large", true);
    let big = w.file("src/lib.rs", 20_000);
    w.lsp(
        "fl-1",
        None,
        "documentSymbol",
        "src/lib.rs",
        "Document symbols:\nx - Line 1",
    );
    let said = w.read("fl-1", &big, "").stdout;
    assert!(said.contains("read-unbounded-large"), "{said}");
    assert!(!said.contains("code-read-not-lsp"), "{said}");
}

/// A findReferences answer names other files; an error counts as asked.
#[test]
fn files_named_by_an_answer_and_failed_calls_count_as_asked() {
    let w = LspWorld::new("named", true);
    let other = w.file("src/other.rs", 10_000);
    let lib = w.file("src/lib.rs", 10_000);
    w.lsp("ln-1", None, "findReferences", "src/lib.rs", "Found 2 references across 2 files:\n\nsrc/other.rs:\n  Line 3:1\n\nsrc/lib.rs:\n  Line 9:2");
    assert_eq!(w.read("ln-1", &other, "").stdout, "");
    let err = w.file("src/err.rs", 10_000);
    w.lsp(
        "ln-2",
        None,
        "documentSymbol",
        "src/err.rs",
        "Error performing documentSymbol: server down",
    );
    assert_eq!(w.read("ln-2", &err, "").stdout, "");
    assert!(w
        .journal_rows("ln-2")
        .iter()
        .any(|r| r.contains("lsp-error")));
    // The same Read in a session that never asked is advised.
    assert!(w
        .read("ln-3", &lib, "")
        .reason()
        .contains("code-read-not-lsp"));
}

/// An answer without a text `result` is recorded by `filePath` alone.
#[test]
fn an_unparsed_answer_is_journalled_and_still_counts_the_file() {
    let w = LspWorld::new("unparsed", true);
    let big = w.file("src/lib.rs", 10_000);
    let payload = serde_json::json!({
        "hook_event_name": "PostToolUse", "tool_name": "LSP",
        "cwd": w.repo.display().to_string(), "session_id": "lu-1",
        "permission_mode": "default",
        "tool_input": {"operation": "hover", "filePath": "src/lib.rs"},
        "tool_response": ["not", "an", "object"],
    });
    assert_eq!(w.send(&payload.to_string(), "").stdout, "");
    assert!(w
        .journal_rows("lu-1")
        .iter()
        .any(|r| r.contains("lsp-unparsed")));
    assert_eq!(w.read("lu-1", &big, "").stdout, "");
}

/// The record is per session and per agent.
#[test]
fn an_lsp_call_silences_only_its_own_session_and_agent() {
    let w = LspWorld::new("scope", true);
    let big = w.file("src/lib.rs", 10_000);
    w.lsp(
        "lc-1",
        Some("a1"),
        "documentSymbol",
        "src/lib.rs",
        "Document symbols:",
    );
    assert!(
        w.read("lc-1", &big, "")
            .reason()
            .contains("code-read-not-lsp"),
        "main is advised"
    );
    assert!(
        w.read_as("lc-1", "a2", &big, "")
            .reason()
            .contains("code-read-not-lsp"),
        "a2 is advised"
    );
    assert_eq!(
        w.read_as("lc-1", "a1", &big, "").stdout,
        "",
        "a1's own call"
    );
    // Positive control for main, and another session.
    w.lsp(
        "lc-2",
        None,
        "documentSymbol",
        "src/lib.rs",
        "Document symbols:",
    );
    assert_eq!(w.read("lc-2", &big, "").stdout, "");
    assert!(
        w.read("lc-3", &big, "")
            .reason()
            .contains("code-read-not-lsp"),
        "another session"
    );
}

/// At `deny` a blind Read is refused with the way through; after an LSP call
/// the same Read passes.
#[test]
fn a_denied_code_read_passes_once_lsp_was_asked() {
    let w = LspWorld::new("deny", true);
    let big = w.file("src/lib.rs", 10_000);
    let refused = w.read_with("ld-1", &big, "", DENY);
    assert_eq!(
        refused.decision().as_deref(),
        Some("deny"),
        "{}",
        refused.stdout
    );
    let said = refused.reason();
    assert!(said.contains("documentSymbol"), "{said}");
    assert!(
        said.contains("once LSP was asked") && said.contains("if LSP errors"),
        "{said}"
    );
    assert!(
        w.read_with("ld-1", &big, "", DENY).decision().as_deref() == Some("deny"),
        "refused again"
    );
    w.lsp(
        "ld-1",
        None,
        "documentSymbol",
        "src/lib.rs",
        "Document symbols:",
    );
    let after = w.read_with("ld-1", &big, "", DENY);
    assert_eq!(after.decision(), None, "{}", after.stdout);
    assert_eq!(after.stdout, "");
}

// --- file-reread: a record per agent, exact windows, certain or uncertain ---

const REREAD_DENY: &str = "[amont \"agent.file-reread\"]\n\tstance = deny\n";

/// A scratch file, and every payload shape the per-agent read record sees.
struct Rr {
    dir: PathBuf,
    file: PathBuf,
    session: String,
}

impl Rr {
    fn new(name: &str, lines: usize) -> Rr {
        let dir = home().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let file = dir.join("f.txt");
        let text: String = (1..=lines).map(|i| format!("line {i:04}\n")).collect();
        std::fs::write(&file, text).expect("a file worth reading");
        Rr {
            dir,
            file,
            session: format!("{name}-sess"),
        }
    }

    /// `agent` is a JSON fragment such as `,"agent_id":"sub-1"`, or empty.
    fn payload(&self, event: &str, tool: &str, agent: &str, input: &str, response: &str) -> String {
        let cwd = serde_json::Value::String(self.dir.display().to_string());
        format!(
            r#"{{"hook_event_name":"{event}","tool_name":"{tool}","cwd":{cwd},
                 "session_id":"{}","permission_mode":"default","tool_use_id":"t1"{agent}
                 ,"tool_input":{input}{response}}}"#,
            self.session
        )
    }

    fn read_input(&self, extra: &str) -> String {
        let path = serde_json::Value::String(self.file.display().to_string());
        format!(r#"{{"file_path":{path}{extra}}}"#)
    }

    fn pre_read(&self, agent: &str, extra: &str) -> Reply {
        send_with_global(
            &self.payload("PreToolUse", "Read", agent, &self.read_input(extra), ""),
            REREAD_DENY,
        )
    }

    fn post_read(&self, agent: &str, extra: &str, response: &str) {
        let r = send_with_global(
            &self.payload(
                "PostToolUse",
                "Read",
                agent,
                &self.read_input(extra),
                response,
            ),
            REREAD_DENY,
        );
        assert_eq!(r.stdout, "");
    }

    fn command(command: &str) -> String {
        format!(
            r#"{{"command":{}}}"#,
            serde_json::Value::String(command.to_string())
        )
    }

    fn pre_bash(&self, agent: &str, command: &str) -> Reply {
        send_with_global(
            &self.payload("PreToolUse", "Bash", agent, &Rr::command(command), ""),
            REREAD_DENY,
        )
    }

    /// `stdout` is the JSON text of the result's `stdout`.
    fn post_bash(&self, agent: &str, command: &str, stdout: &str) {
        let response = format!(r#","tool_response":{{"stdout":{stdout},"stderr":""}}"#);
        send_with_global(
            &self.payload(
                "PostToolUse",
                "Bash",
                agent,
                &Rr::command(command),
                &response,
            ),
            REREAD_DENY,
        );
    }

    fn bash(&self, agent: &str, command: &str) -> Reply {
        let r = self.pre_bash(agent, command);
        self.post_bash(agent, command, r#""ok""#);
        r
    }

    fn sessions(&self) -> PathBuf {
        home().join("amont-agent").join("sessions")
    }
}

const SUB: &str = r#","agent_id":"sub-1","agent_type":"Explore""#;

fn said_reread(r: &Reply) -> bool {
    r.reason().contains("file-reread")
}

/// A subagent shares its parent's `session_id`: it must not be refused a file
/// only the parent read.
#[test]
fn a_subagent_is_not_told_file_reread_after_only_the_parent_read() {
    let f = Rr::new("rr-sub", 50);
    f.post_read("", "", "");
    let sub = f.pre_read(SUB, "");
    assert!(!said_reread(&sub), "{}", sub.stdout);
    assert_eq!(sub.stdout, "");
    // The parent itself is refused, under the deny stance.
    let main = f.pre_read("", "");
    assert!(said_reread(&main), "{}", main.stdout);
    assert_eq!(main.decision().as_deref(), Some("deny"), "{}", main.stdout);
}

#[test]
fn a_subagent_cat_is_keyed_to_the_agent() {
    let f = Rr::new("rr-subcat", 50);
    f.bash(SUB, "cat f.txt");
    let main = f.pre_bash("", "cat f.txt");
    assert!(!said_reread(&main), "main never read it: {}", main.stdout);
    let again = f.pre_bash(SUB, "cat f.txt");
    assert!(said_reread(&again), "{}", again.stdout);
    assert_eq!(again.decision().as_deref(), Some("deny"));
}

#[test]
fn an_agent_id_that_is_not_a_key_neither_records_nor_denies() {
    let f = Rr::new("rr-badid", 50);
    f.post_read("", "", "");
    for bad in [
        r#","agent_id":123"#,
        r#","agent_id":"""#,
        r#","agent_id":"../x""#,
    ] {
        let r = f.pre_read(bad, "");
        assert!(!said_reread(&r), "{bad}: {}", r.stdout);
        assert_eq!(r.decision(), None, "{bad}");
        f.post_read(bad, "", "");
        let r = f.pre_read(bad, "");
        assert!(!said_reread(&r), "{bad} after its own read: {}", r.stdout);
        let r = f.pre_bash(bad, "cat f.txt");
        assert!(!said_reread(&r), "{bad} cat: {}", r.stdout);
        f.post_bash(bad, "cat f.txt", r#""ok""#);
    }
    // Nothing escaped `sessions/`.
    let root = home().join("amont-agent");
    assert!(!root.join("x.log").exists());
    assert!(!root.join("x").exists());
    assert!(!home().join("x.log").exists());
    for e in std::fs::read_dir(f.sessions()).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        assert!(!name.contains("123") && !name.contains(".."), "{name}");
    }
}

#[test]
fn windows_are_exact_and_have_a_direction() {
    let f = Rr::new("rr-windows", 300);
    let told = |cmd: &str| f.bash("", cmd);
    assert!(!said_reread(&told("head -20 f.txt")));
    assert!(!said_reread(&told("tail -20 f.txt")));
    assert!(!said_reread(&told("sed -n '101,120p' f.txt")));
    let again = told("head -20 f.txt");
    assert!(said_reread(&again), "{}", again.stdout);
    assert_eq!(again.decision().as_deref(), Some("deny"));
    // A smaller head is inside the larger one; a larger one is not.
    assert!(said_reread(&told("head -5 f.txt")));
    assert!(!said_reread(&told("head -30 f.txt")));
}

#[test]
fn a_record_that_is_not_certain_never_speaks() {
    // `true || cat f`: the cat may not have run.
    let f = Rr::new("rr-cond", 50);
    f.bash("", "true || cat f.txt");
    let r = f.pre_bash("", "cat f.txt");
    assert!(!said_reread(&r), "{}", r.stdout);

    // A persisted result.
    let f = Rr::new("rr-persisted", 50);
    f.pre_bash("", "cat f.txt");
    f.post_bash(
        "",
        "cat f.txt",
        r#""<persisted-output>Output too large. Full output saved to: /x/tool-results/a.txt""#,
    );
    let r = f.pre_bash("", "cat f.txt");
    assert!(!said_reread(&r), "{}", r.stdout);

    // A result of exactly the cap length, counted in characters.
    let f = Rr::new("rr-cap", 50);
    f.pre_bash("", "cat f.txt");
    let capped = serde_json::Value::String("\u{e9}".repeat(30_000)).to_string();
    f.post_bash("", "cat f.txt", &capped);
    let r = f.pre_bash("", "cat f.txt");
    assert!(!said_reread(&r), "{}", r.stdout);
    // One character under it is whole.
    let under = serde_json::Value::String("\u{e9}".repeat(29_999)).to_string();
    f.post_bash("", "cat f.txt", &under);
    let r = f.pre_bash("", "cat f.txt");
    assert!(said_reread(&r), "{}", r.stdout);

    // An unwindowed Read of 3,000 lines delivers 2,000; the rest is a
    // different window.
    let f = Rr::new("rr-long", 3000);
    f.post_read("", "", "");
    let r = f.pre_read("", r#","offset":2000"#);
    assert!(!said_reread(&r), "{}", r.stdout);
    let r = f.pre_read("", "");
    assert!(said_reread(&r), "the same window again: {}", r.stdout);
}

#[test]
fn compaction_clears_the_main_record_and_keeps_the_subagents() {
    let f = Rr::new("rr-compact", 50);
    f.post_read("", "", "");
    f.post_read(SUB, "", "");
    let cwd = serde_json::Value::String(f.dir.display().to_string());
    let start = format!(
        r#"{{"hook_event_name":"SessionStart","source":"compact","cwd":{cwd},"session_id":"{}"}}"#,
        f.session
    );
    send_with_global(&start, REREAD_DENY);
    let main = f.pre_read("", "");
    assert!(!said_reread(&main), "{}", main.stdout);
    let sub = f.pre_read(SUB, "");
    assert!(said_reread(&sub), "{}", sub.stdout);
}

#[test]
fn the_same_agent_rereading_a_certain_window_is_refused_under_deny() {
    let f = Rr::new("rr-same", 50);
    f.post_read("", r#","offset":11,"limit":20"#, "");
    let r = f.pre_read("", r#","offset":11,"limit":20"#);
    assert_eq!(r.decision().as_deref(), Some("deny"), "{}", r.stdout);
    let other = f.pre_read("", r#","offset":41,"limit":5"#);
    assert!(!said_reread(&other), "{}", other.stdout);
}

// ---- agent-guidance-written and agent-guidance-shape ---------------------------

const GOOD_SKILL: &str =
    "---\nname: x\ndescription: Does x. Use when asked for x.\n---\nbody one\n";

fn edit_body(old: &str, new: &str) -> String {
    format!(r#","old_string":{old:?},"new_string":{new:?}"#)
}

fn write_body(content: &str) -> String {
    format!(
        r#","content":{}"#,
        serde_json::Value::String(content.to_string())
    )
}

#[test]
fn guidance_an_edit_to_a_skill_is_told_the_guidance_rules() {
    let f = ReadFixture::with_content("agw-1", "SKILL.md", GOOD_SKILL);
    let said = f.file_event(
        "PreToolUse",
        "Edit",
        "agw-s1",
        &edit_body("body one", "body two"),
    );
    assert!(
        said.reason().contains("agent-guidance-written"),
        "{}",
        said.stdout
    );
    assert!(said.reason().contains("ADR-0026"), "{}", said.stdout);
    assert!(
        said.reason().contains("aval relevant --path"),
        "{}",
        said.stdout
    );
    assert!(
        !said.reason().contains("agent-guidance-shape"),
        "{}",
        said.stdout
    );
    assert_eq!(said.decision(), None, "advise, not deny: {}", said.stdout);
}

#[test]
fn guidance_five_edits_in_a_session_give_one_advice() {
    let f = ReadFixture::with_content("agw-5", "SKILL.md", GOOD_SKILL);
    let mut told = 0;
    for i in 0..5 {
        let said = f.file_event(
            "PreToolUse",
            "Edit",
            "agw-s5",
            &edit_body("body one", &format!("body one {i}")),
        );
        if said.reason().contains("agent-guidance-written") {
            told += 1;
        }
    }
    assert_eq!(told, 1);
    let rows = journal_rows("agw-s5");
    assert!(
        rows.iter()
            .any(|r| r.contains("agent-guidance-written") && r.contains("told-before")),
        "{rows:?}"
    );
}

#[test]
fn guidance_a_multi_line_or_block_description_is_flagged() {
    for (i, fm) in [
        "---\nname: x\ndescription: >\n  Does x. Use when asked.\n---\n",
        "---\nname: x\ndescription: Does x.\n  Use when asked.\n---\n",
        "---\nname: x\ndescription: Does x.\n---\n",
    ]
    .iter()
    .enumerate()
    {
        let f = ReadFixture::with_content(&format!("ags-{i}"), "SKILL.md", GOOD_SKILL);
        let said = f.file_event("PreToolUse", "Write", &format!("ags-s{i}"), &write_body(fm));
        assert!(
            said.reason().contains("agent-guidance-shape"),
            "{i}: {}",
            said.stdout
        );
        assert!(
            said.reason().contains("guidance.pointer-names-when"),
            "{i}: {}",
            said.stdout
        );
    }
}

#[test]
fn guidance_a_skill_that_disables_model_invocation_is_silent_on_shape() {
    let f = ReadFixture::with_content("ags-dmi", "SKILL.md", GOOD_SKILL);
    let said = f.file_event(
        "PreToolUse",
        "Write",
        "ags-sd",
        &write_body(
            "---\nname: x\ndescription: >\n  Does x.\ndisable-model-invocation: true\n---\n",
        ),
    );
    assert!(
        !said.reason().contains("agent-guidance-shape"),
        "{}",
        said.stdout
    );
}

#[test]
fn guidance_an_edit_that_matches_nothing_is_silent_on_shape_and_journalled() {
    let f = ReadFixture::with_content("ags-frag", "SKILL.md", "---\nname: x\n---\n");
    let said = f.file_event(
        "PreToolUse",
        "Edit",
        "ags-sf",
        &edit_body("not there", "new"),
    );
    assert!(
        !said.reason().contains("agent-guidance-shape"),
        "{}",
        said.stdout
    );
    let rows = journal_rows("ags-sf");
    assert!(
        rows.iter()
            .any(|r| r.contains("agent-guidance-shape") && r.contains("fragment")),
        "{rows:?}"
    );
}

fn shape_edit(name: &str, file: &str, before: usize, after: usize, session: &str) -> Reply {
    let f = ReadFixture::with_content(name, file, &format!("{}\nmark\n", "x".repeat(before)));
    f.file_event(
        "PreToolUse",
        "Edit",
        session,
        &edit_body("mark", &format!("mark{}", "y".repeat(after - before))),
    )
}

#[test]
fn guidance_growing_an_oversized_file_advises_with_numbers() {
    let said = shape_edit("ags-big", "CLAUDE.md", 9000, 9100, "ags-sb");
    assert!(
        said.reason().contains("agent-guidance-shape"),
        "{}",
        said.stdout
    );
    assert!(said.reason().contains("8192"), "{}", said.stdout);
    assert!(said.reason().contains("100 bytes"), "{}", said.stdout);
    assert!(
        said.reason().contains("guidance.always-on-is-an-index"),
        "{}",
        said.stdout
    );
}

#[test]
fn guidance_crossing_the_budget_advises() {
    let said = shape_edit("ags-cross", "CLAUDE.md", 7000, 9000, "ags-sc");
    assert!(
        said.reason().contains("agent-guidance-shape"),
        "{}",
        said.stdout
    );
}

#[test]
fn guidance_a_template_crossing_the_budget_advises() {
    let dir = home().join("ags-tmpl").join("dot_claude");
    let _ = std::fs::remove_dir_all(home().join("ags-tmpl"));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let file = dir.join("CLAUDE.md.tmpl");
    std::fs::write(&file, "x".repeat(8000)).expect("a template");
    let f = ReadFixture { dir, file };
    let said = f.file_event(
        "PreToolUse",
        "Write",
        "ags-st",
        &write_body(&"x".repeat(8300)),
    );
    assert!(
        said.reason().contains("agent-guidance-shape"),
        "{}",
        said.stdout
    );
}

#[test]
fn guidance_shrinking_an_oversized_file_is_silent_on_shape() {
    let f = ReadFixture::with_content("ags-shrink", "CLAUDE.md", &"x".repeat(9000));
    let said = f.file_event(
        "PreToolUse",
        "Write",
        "ags-ss",
        &write_body(&"x".repeat(8500)),
    );
    assert!(
        !said.reason().contains("agent-guidance-shape"),
        "{}",
        said.stdout
    );
}

#[test]
fn guidance_a_shape_finding_is_told_once_per_file() {
    let f = ReadFixture::with_content("ags-once", "SKILL.md", GOOD_SKILL);
    let bad = write_body("---\nname: x\ndescription: Does x.\n---\n");
    let first = f.file_event("PreToolUse", "Write", "ags-so", &bad);
    let second = f.file_event("PreToolUse", "Write", "ags-so", &bad);
    assert!(
        first.reason().contains("agent-guidance-shape"),
        "{}",
        first.stdout
    );
    assert!(
        !second.reason().contains("agent-guidance-shape"),
        "{}",
        second.stdout
    );
}

#[test]
fn guidance_a_write_to_source_is_silent() {
    let f = ReadFixture::with_content("agw-src", "main.rs", "fn main() {}\n");
    let said = f.file_event(
        "PreToolUse",
        "Write",
        "agw-ss",
        &write_body("fn main() { }\n"),
    );
    assert_eq!(said.stdout, "", "{}", said.stdout);
}

#[test]
fn guidance_other_memory_files_are_exempt() {
    let f = ReadFixture::with_content("agw-mem", "feedback_x.md", "a\n");
    let said = f.file_event("PreToolUse", "Write", "agw-sm", &write_body("b\n"));
    assert_eq!(said.stdout, "", "{}", said.stdout);
}

#[test]
fn guidance_a_denying_lint_rule_still_denies() {
    let f = ReadFixture::with_content("agw-deny", "x.py", "def f(a):\n    return g(a)\n");
    let cfg = "[amont \"agent.lint-suppression-added\"]\n\tstance = deny";
    let said = f.file_event_with_global("PreToolUse", "Edit", "agw-sd", ADD_IGNORE, cfg);
    assert_eq!(said.decision().as_deref(), Some("deny"), "{}", said.stdout);
    assert!(
        said.reason().contains("lint-suppression-added"),
        "{}",
        said.stdout
    );
}

// --- file-reread: a command that writes or waits is not a certain read ---

/// After a whole Read of `f.txt`, run `cmd` under the deny stance.
fn bash_after_whole_read(cmd: &str) -> Reply {
    let mut h = 0u64;
    for b in cmd.bytes() {
        h = h.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    let f = Rr::new(&format!("rr-excuse-{h}"), 50);
    f.post_read("", "", "");
    f.bash("", cmd)
}

fn assert_allowed(cmd: &str) {
    let r = bash_after_whole_read(cmd);
    assert!(!said_reread(&r), "{cmd}: {}", r.stdout);
    assert_eq!(r.decision(), None, "{cmd}");
}

fn assert_refused(cmd: &str) {
    let r = bash_after_whole_read(cmd);
    assert!(said_reread(&r), "{cmd}: {}", r.stdout);
    assert_eq!(r.decision().as_deref(), Some("deny"), "{cmd}");
}

/// Guards the redirect branch of `writes_or_waits`: a write onto the dumped
/// path, compared after resolving both sides.
#[test]
fn a_redirect_onto_the_dumped_file_is_not_a_reread() {
    assert_allowed("printf x >> ./f.txt && tail -1 f.txt");
    assert_allowed("printf x > f.txt; cat ./f.txt");
}

/// The redirect names another file, or comes after the dump: still a re-read.
#[test]
fn a_redirect_elsewhere_is_still_a_reread() {
    assert_refused("printf x >> other.txt && tail -1 f.txt");
    assert_refused("cat f.txt; printf x >> f.txt");
}

/// Guards the wait branch: a `sleep` loop naming the file, dump after or in it.
#[test]
fn a_sleep_loop_on_the_dumped_file_is_a_wait() {
    assert_allowed("until test -s f.txt; do sleep 1; done; cat f.txt");
    assert_allowed("while ! grep -q done f.txt; do sleep 1; cat f.txt; done");
}

/// The word `sleep` outside a loop that names the file changes nothing.
#[test]
fn the_word_sleep_elsewhere_changes_nothing() {
    assert_refused("cat f.txt # sleep");
    assert_refused("echo sleep; cat f.txt");
    assert_refused("cat sleep.log f.txt");
    assert_refused("until test -s other.txt; do sleep 1; done; cat f.txt");
    assert_refused("sleep 1; cat f.txt");
}

/// `sed -i` on another file excuses nothing.
#[test]
fn sed_in_place_elsewhere_is_still_a_reread() {
    assert_refused("sed -i s/a/b/ other.txt; cat f.txt");
}

/// Guards the interpreter-heredoc branch. Shapes of real denied commands of
/// 2026-10-12..10-20 (journal.log rows, full commands from the session
/// transcripts), paths anonymised: a python heredoc rewrote the file, then a
/// `sed -n`/`tail` showed the result.
#[test]
fn a_python_heredoc_edit_then_a_dump_is_not_a_reread() {
    let fixtures = [
        // A changelog edit, then `sed -n 3,36p CHANGELOG.md`.
        "python3 - <<'EOF'\np=\"f.txt\"; s=open(p).read()\ns=s.replace(\"line\",\"LINE\")\nopen(p,\"w\").write(s)\nEOF\nsed -n 3,36p f.txt; awk 'length>78' f.txt | head -3",
        // A screen file rewrite, then `tail -5`.
        "python3 -I - <<'EOF'\np=\"f.txt\"; t=open(p).read()\nt=t[:9].rstrip()+\"\\n\"\nopen(p,\"w\").write(t)\nEOF\ntail -5 f.txt; echo tc=$?",
        // A `rep(p, a, b)` helper with single quotes, then `sed -n 44,56p`.
        "python3 - <<'EOF'\ndef rep(p,a,b):\n    s=open(p).read(); assert s.count(a)==1,(p,a); open(p,'w').write(s.replace(a,b))\nrep('f.txt','line','LINE')\nEOF\nsed -n 44,56p f.txt; grep -n LINE f.txt | head -3",
        // `sed -i` on another file, then a python edit of the dumped one.
        "sed -i 's/x/y/' other.rs\npython3 - <<'EOF'\np='f.txt'\ns=open(p).read()\nassert s.count('line')>0\nopen(p,'w').write(s.replace('line','LINE'))\nEOF\nsed -n 2020,2030p f.txt",
    ];
    for cmd in fixtures {
        assert_allowed(cmd);
    }
}

/// A python heredoc that only reads the file, or writes another, excuses nothing.
#[test]
fn a_python_heredoc_that_does_not_write_the_file_is_a_reread() {
    assert_refused("python3 - <<'EOF'\np='f.txt'\nprint(open(p).read())\nEOF\ncat f.txt");
    assert_refused("python3 - <<'EOF'\nopen('g.txt','w').write('x')\nEOF\ncat f.txt");
    // A write of another file beside a read of the dumped one.
    assert_refused(
        "python3 - <<'EOF'\nopen('g.txt','w').write(open('f.txt').read())\nEOF\ncat f.txt",
    );
    // A write mode outside the heredoc body.
    assert_refused("python3 - <<'EOF'\nprint(open('f.txt').read())\nEOF\necho 'w'; cat f.txt");
}

/// A dump the PreToolUse side excuses leaves no certain read behind: the
/// record of a dump beside a write, a loop or a heredoc is `uncertain`, so the
/// next `cat` of the unchanged file is not refused. No PostToolUse skip is
/// needed for that (measured 2026-10-21: removing one changed no record).
#[test]
fn an_excused_dump_is_not_recorded_as_a_read() {
    for (name, cmd) in [
        (
            "rr-post-wait",
            "until test -s f.txt; do sleep 1; done; cat f.txt",
        ),
        (
            "rr-post-heredoc",
            "python3 - <<'EOF'\np='f.txt'\ns=open(p).read()\nopen(p,'w').write(s)\nEOF\ncat f.txt",
        ),
    ] {
        let f = Rr::new(name, 50);
        f.post_bash("", cmd, r#""ok""#);
        let r = f.pre_bash("", "cat f.txt");
        assert!(!said_reread(&r), "{cmd}: {}", r.stdout);
    }
}

/// The control: a second whole Read of an unchanged file is still refused.
#[test]
fn a_second_whole_read_is_still_denied_at_deny() {
    let f = Rr::new("rr-control", 50);
    f.post_read("", "", "");
    let r = f.pre_read("", "");
    assert!(said_reread(&r), "{}", r.stdout);
    assert_eq!(r.decision().as_deref(), Some("deny"));
}

// ---- bug-nudge: a red check run asks for the diagnosing-bugs skill once ----

/// A Bash payload for a post-call event of a check run. `transcript` is the
/// payload's `transcript_path`, absent when `None`.
fn post_bash(
    event: &str,
    session: &str,
    command: &str,
    stdout: &str,
    error: &str,
    transcript: Option<&std::path::Path>,
) -> String {
    let mut v = serde_json::json!({
        "hook_event_name": event,
        "tool_name": "Bash",
        "cwd": "/tmp",
        "session_id": session,
        "tool_use_id": "t1",
        "permission_mode": "default",
        "tool_input": {"command": command},
    });
    if event == "PostToolUseFailure" {
        v["error"] = serde_json::Value::String(error.to_string());
    } else {
        v["tool_response"] =
            serde_json::json!({"stdout": stdout, "stderr": "", "interrupted": false});
    }
    if let Some(t) = transcript {
        v["transcript_path"] = serde_json::Value::String(t.display().to_string());
    }
    v.to_string()
}

/// A transcript of one human turn that never loaded the skill. The assertion
/// stays silent when it has no transcript to read, so a firing needs one.
fn unloaded_transcript(name: &str) -> PathBuf {
    let t = home().join(name);
    let human = serde_json::json!({"type":"user","message":{"role":"user","content":"go"},"origin":{"kind":"human"}});
    std::fs::write(&t, human.to_string() + "\n").expect("write the transcript");
    t
}

/// How many times the reply's reason text carries the bug-nudge advisory.
fn advisory_count(r: &Reply) -> usize {
    r.reason().matches("amont-agent/bug-nudge").count()
}

#[test]
fn bug_nudge_fires_once_on_a_failed_cargo_test() {
    let t = unloaded_transcript("bugnudge-fires.jsonl");
    let payload = post_bash(
        "PostToolUseFailure",
        "bugnudge-fires",
        "cargo test",
        "",
        "Exit code 101",
        Some(&t),
    );
    let first = send(&payload);
    assert_eq!(first.code, 0);
    assert_eq!(advisory_count(&first), 1, "{}", first.stdout);
    assert_ne!(
        first.decision().as_deref(),
        Some("deny"),
        "{}",
        first.stdout
    );
    let second = send(&payload);
    assert_eq!(advisory_count(&second), 0, "{}", second.stdout);
}

#[test]
fn bug_nudge_is_silent_on_a_grep_that_prints_error() {
    let t = unloaded_transcript("bugnudge-grep.jsonl");
    let payload = post_bash(
        "PostToolUseFailure",
        "bugnudge-grep",
        "grep -n error src/main.rs",
        "",
        "Exit code 1\nsrc/main.rs:3: error",
        Some(&t),
    );
    let r = send(&payload);
    assert_eq!(advisory_count(&r), 0, "{}", r.stdout);
}

#[test]
fn bug_nudge_is_silent_when_the_skill_was_loaded() {
    let t = home().join("bugnudge-skill.jsonl");
    let lines = [
        serde_json::json!({"type":"user","message":{"role":"user","content":"go"},"origin":{"kind":"human"}}),
        serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"s1","name":"Skill","input":{"skill":"diagnosing-bugs"}}]}}),
    ];
    let body: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    std::fs::write(&t, body.join("\n") + "\n").expect("write the transcript");
    let payload = post_bash(
        "PostToolUseFailure",
        "bugnudge-loaded",
        "cargo test",
        "",
        "Exit code 101\ntest result: FAILED",
        Some(&t),
    );
    let r = send(&payload);
    assert_eq!(advisory_count(&r), 0, "{}", r.stdout);
}

#[test]
fn bug_nudge_reads_a_pipeline_exit_0_with_failure_text() {
    let t = unloaded_transcript("bugnudge-pipe.jsonl");
    let payload = post_bash(
        "PostToolUse",
        "bugnudge-pipe",
        "cargo test 2>&1 | tail -5",
        "test result: FAILED. 0 passed; 1 failed",
        "",
        Some(&t),
    );
    let r = send(&payload);
    assert_eq!(advisory_count(&r), 1, "{}", r.stdout);
}

#[test]
fn bug_nudge_is_silent_for_a_failed_pipeline_with_no_failure_text() {
    let t = unloaded_transcript("bugnudge-nopipe.jsonl");
    let payload = post_bash(
        "PostToolUseFailure",
        "bugnudge-nopipe",
        "cargo test 2>&1 | grep FAILED",
        "",
        "",
        Some(&t),
    );
    let r = send(&payload);
    assert_eq!(advisory_count(&r), 0, "{}", r.stdout);
}

#[test]
fn bug_nudge_reads_the_error_text_of_a_failed_pipeline() {
    let t = unloaded_transcript("bugnudge-errtext.jsonl");
    let payload = post_bash(
        "PostToolUseFailure",
        "bugnudge-errtext",
        "cargo test 2>&1 | grep FAILED",
        "",
        "Exit code 1\ntest result: FAILED",
        Some(&t),
    );
    let r = send(&payload);
    assert_eq!(advisory_count(&r), 1, "{}", r.stdout);
}

#[test]
fn bug_nudge_is_silent_when_a_subagent_loaded_the_skill() {
    let parent = unloaded_transcript("bugnudge-sub-parent.jsonl");
    let agent = home().join("bugnudge-sub-agent.jsonl");
    let line = serde_json::json!({"type":"assistant","message":{"content":[
        {"type":"tool_use","id":"s1","name":"Skill","input":{"skill":"diagnosing-bugs"}}]}});
    std::fs::write(&agent, line.to_string() + "\n").expect("write the agent transcript");
    let mut v: serde_json::Value = serde_json::from_str(&post_bash(
        "PostToolUseFailure",
        "bugnudge-sub",
        "cargo test",
        "",
        "Exit code 101\ntest result: FAILED",
        Some(&parent),
    ))
    .expect("payload");
    v["agent_id"] = "a1".into();
    v["agent_type"] = "worker".into();
    v["agent_transcript_path"] = agent.display().to_string().into();
    let r = send(&v.to_string());
    assert_eq!(advisory_count(&r), 0, "{}", r.stdout);
}

#[test]
fn bug_nudge_is_silent_and_unknown_on_an_unreadable_transcript() {
    let gone = home().join("bugnudge-gone.jsonl");
    let _ = std::fs::remove_file(&gone);
    let payload = post_bash(
        "PostToolUseFailure",
        "bugnudge-gone",
        "cargo test",
        "",
        "Exit code 101\ntest result: FAILED",
        Some(&gone),
    );
    let r = send(&payload);
    assert_eq!(advisory_count(&r), 0, "{}", r.stdout);
    assert_ne!(r.decision().as_deref(), Some("deny"), "{}", r.stdout);
}

#[test]
fn bug_nudge_is_silent_without_a_transcript_path() {
    let payload = post_bash(
        "PostToolUseFailure",
        "bugnudge-nopath",
        "cargo test",
        "",
        "Exit code 101\ntest result: FAILED",
        None,
    );
    let r = send(&payload);
    assert_eq!(advisory_count(&r), 0, "{}", r.stdout);
    assert_ne!(r.decision().as_deref(), Some("deny"), "{}", r.stdout);
}
