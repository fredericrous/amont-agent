//! `suppressions-override`: the commit gate's switches stay the person's,
//! driven through the built binary's PreToolUse hook.

use std::io::Write;
use std::process::{Command, Stdio};

const DENY_TEXT: &str = "amont-agent/suppressions-override: this override is the person's \
(general.no-disabled-safety). Fix the finding, or stop and tell the person which hits the \
gate printed.";

fn pre_bash(command: &str) -> (String, String) {
    let home = std::env::temp_dir().join(format!("amont-agent-override-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("scratch dir");
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "cwd": "/tmp",
        "session_id": "override-session",
        "tool_use_id": "t1",
        "permission_mode": "default",
        "tool_input": {"command": command},
    })
    .to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
        .arg("hook")
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("GIT_CONFIG_GLOBAL", home.join("no-global-gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("AMONT_AGENT_OFF")
        .env_remove("GREENHARNESS_HOST_SLOT")
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
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_default();
    let output = &json["hookSpecificOutput"];
    let decision = output["permissionDecision"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let reason = output["permissionDecisionReason"]
        .as_str()
        .unwrap_or("")
        .to_string();
    (decision, reason)
}

fn denied_with_the_plans_text(command: &str) {
    let (decision, reason) = pre_bash(command);
    assert_eq!(decision, "deny", "{command}: {reason}");
    assert!(reason.contains(DENY_TEXT), "{command}: {reason}");
}

#[test]
fn a_dash_c_override_is_denied() {
    denied_with_the_plans_text("git -c amont.agent.suppressions=allow commit -m x");
}

#[test]
fn git_config_of_the_override_is_denied() {
    denied_with_the_plans_text("git config amont.agent.suppressions allow");
}

#[test]
fn git_config_key_n_is_denied() {
    denied_with_the_plans_text(
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=amont.agent.suppressions GIT_CONFIG_VALUE_0=allow git commit -m x",
    );
}

#[test]
fn git_config_parameters_is_denied() {
    denied_with_the_plans_text(
        "GIT_CONFIG_PARAMETERS=\"'amont.agent.suppressions'='allow'\" git commit -m x",
    );
}

#[test]
fn a_hook_skip_naming_the_gate_is_denied() {
    denied_with_the_plans_text("git -c hook.skip=pre-commit-suppressions commit -m x");
    denied_with_the_plans_text("git -c hook.skip=suppressions commit -m x");
}

#[test]
fn an_amont_severity_lowering_is_denied() {
    denied_with_the_plans_text("git -c amont.severity.pre-commit-suppressions=warn commit -m x");
    denied_with_the_plans_text("git config amont.severity.pre-commit-suppressions warn");
}

#[test]
fn a_stance_write_is_denied() {
    denied_with_the_plans_text("git config amont.agent.lint-suppression-added.stance observe");
}

#[test]
fn a_whole_pre_commit_skip_and_an_ordinary_commit_are_allowed() {
    for command in ["git -c hook.skip=pre-commit commit -m x", "git commit -m x"] {
        let (decision, reason) = pre_bash(command);
        assert_ne!(decision, "deny", "{command}: {reason}");
        assert!(!reason.contains("suppressions-override"), "{command}");
    }
}

#[test]
fn a_read_in_an_earlier_command_does_not_excuse_the_write() {
    denied_with_the_plans_text(
        "bash -c 'git config --get user.name; git config amont.agent.suppressions allow'",
    );
}
