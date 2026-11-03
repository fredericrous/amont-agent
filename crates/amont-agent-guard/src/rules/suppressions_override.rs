//! `suppressions-override` — the commit gate's switches stay the person's.
//!
//! `amont-agent suppressions --staged` blocks a commit that adds a bare or
//! blanket lint suppression. The person commits those cases themselves with
//! `git -c amont.agent.suppressions=allow commit`; an agent that reaches the
//! same switch, or lowers the gate another way, has loosened a safety
//! mechanism (`general.no-disabled-safety`). This rule refuses every channel a
//! single command can use:
//!
//! - `-c amont.agent.suppressions=…`, `git config … amont.agent.suppressions`,
//!   and `GIT_CONFIG_KEY_<n>` / `GIT_CONFIG_PARAMETERS` carrying that key;
//! - `hook.skip` naming `suppressions` or `pre-commit-suppressions`
//!   (`hook.skip=pre-commit` as a whole is a different decision and passes);
//! - an `amont.severity.pre-commit-suppressions` other than `block`;
//! - a write of `amont.agent.lint-suppression-added.stance`, the rollback
//!   switch.
//!
//! An agent editing a config file with Edit or Write is not covered; that is a
//! known gap left to review.

use crate::rules::{Evidence, Examine, Finding, Input, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "suppressions-override",
    prevents: "An agent overriding the suppressions commit gate that only the person may loosen",
    irreversible: false,
    default_stance: Stance::Deny,
    max_stance: Stance::Deny,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-11-03",
        trend: Trend::Flat(0),
    },
    examine: Examine::Analysis(examine),
    confirm: None,
};

const OVERRIDE_KEY: &str = "amont.agent.suppressions";
const STANCE_KEY: &str = "amont.agent.lint-suppression-added.stance";
const SEVERITY_KEY: &str = "amont.severity.pre-commit-suppressions";
const SKIP_KEY: &str = "hook.skip";

fn examine(input: &Input) -> Option<Finding> {
    let refused = match input.legacy {
        Parsed::Opaque(_) => refuses(&loose_words(input.src)),
        Parsed::Clear(clauses) => clauses.iter().any(|clause| {
            if clause.opaque.is_some() {
                let end = clause.end.min(input.src.len());
                let text = input.src.get(clause.at..end).unwrap_or(input.src);
                refuses(&loose_words(text))
            } else {
                let words: Vec<String> = clause.words.iter().map(|w| w.text.clone()).collect();
                refuses(&words)
            }
        }),
    };
    refused.then(|| Finding {
        reason: "this override is the person's (general.no-disabled-safety).".to_string(),
        remedy: "Fix the finding, or stop and tell the person which hits the gate printed."
            .to_string(),
        span: 0..input.src.len(),
    })
}

/// Words of a text the lexer did not read: split on whitespace and quoting
/// characters, so `bash -c 'git -c amont.agent.suppressions=allow commit'`
/// is still seen.
fn loose_words(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || "'\"`;&|()".contains(c))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether one command's words reach a protected switch.
fn refuses(words: &[String]) -> bool {
    let lower: Vec<String> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
    lower.iter().enumerate().any(|(at, word)| {
        environment(word, &lower) || git_option(at, &lower) || config_write(at, &lower)
    })
}

/// A `GIT_CONFIG_PARAMETERS=…` or `GIT_CONFIG_KEY_<n>=…` assignment.
fn environment(word: &str, all: &[String]) -> bool {
    if let Some(value) = word.strip_prefix("git_config_parameters=") {
        return [OVERRIDE_KEY, STANCE_KEY]
            .iter()
            .any(|key| value.contains(key))
            || (value.contains(SKIP_KEY) && names_skip(value))
            || (value.contains(SEVERITY_KEY) && !value.contains("=block"));
    }
    let Some(rest) = word.strip_prefix("git_config_key_") else {
        return false;
    };
    let Some((number, key)) = rest.split_once('=') else {
        return false;
    };
    let value_prefix = format!("git_config_value_{number}=");
    let value = all
        .iter()
        .find_map(|w| w.strip_prefix(value_prefix.as_str()));
    protected(key, value)
}

/// `git -c key=value` and `git --config-env=key=VAR`.
fn git_option(at: usize, lower: &[String]) -> bool {
    let word = &lower[at];
    if let Some(pair) = word.strip_prefix("--config-env=") {
        let key = pair.split_once('=').map_or(pair, |(key, _)| key);
        return protected(key, None);
    }
    if word != "-c" || !lower.iter().any(|w| w == "git" || w.ends_with("/git")) {
        return false;
    }
    let Some(pair) = lower.get(at + 1) else {
        return false;
    };
    let (key, value) = match pair.split_once('=') {
        Some((key, value)) => (key, Some(value)),
        None => (pair.as_str(), None),
    };
    protected(key, value)
}

/// `git config <key> <value>` and the unset forms; a read is allowed.
fn config_write(at: usize, lower: &[String]) -> bool {
    let key = lower[at].as_str();
    if ![OVERRIDE_KEY, STANCE_KEY, SEVERITY_KEY, SKIP_KEY].contains(&key) {
        return false;
    }
    let configures = lower
        .iter()
        .take(at)
        .rev()
        .take_while(|w| *w != "git" && !w.ends_with("/git"))
        .any(|w| w == "config")
        && lower
            .iter()
            .take(at)
            .any(|w| w == "git" || w.ends_with("/git"));
    if !configures {
        return false;
    }
    let reads = lower
        .iter()
        .any(|w| ["--get", "--get-all", "--get-regexp", "--list", "-l"].contains(&w.as_str()));
    if reads {
        return false;
    }
    let value = lower.get(at + 1).map(String::as_str);
    let unsets = lower
        .iter()
        .any(|w| w.starts_with("--unset") || w == "--remove-section");
    if unsets {
        return true;
    }
    // `git config <key>` with no value reads it.
    value.is_some() && protected(key, value)
}

/// Whether setting `key` to `value` (`None` when it is not known) loosens
/// the gate.
fn protected(key: &str, value: Option<&str>) -> bool {
    match key {
        OVERRIDE_KEY | STANCE_KEY => true,
        SKIP_KEY => value.is_some_and(names_skip),
        SEVERITY_KEY => value.is_some_and(|v| v != "block"),
        _ => false,
    }
}

/// Whether a `hook.skip` value lists the suppressions gate by name.
fn names_skip(value: &str) -> bool {
    value
        .split(|c: char| c == ',' || c.is_whitespace() || c == '=' || c == '"' || c == '\'')
        .any(|name| name == "suppressions" || name == "pre-commit-suppressions")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Dialect;

    fn denied(command: &str) -> bool {
        let parsed = crate::shell::lex(command);
        let input = Input::new(command, Dialect::Unknown, &parsed);
        RULE.judge(&input).is_some()
    }

    #[test]
    fn each_channel_is_refused() {
        for command in [
            "git -c amont.agent.suppressions=allow commit -m x",
            "git config amont.agent.suppressions allow",
            "git config --global amont.agent.suppressions allow",
            "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=amont.agent.suppressions GIT_CONFIG_VALUE_0=allow git commit",
            "GIT_CONFIG_PARAMETERS=\"'amont.agent.suppressions'='allow'\" git commit",
            "git -c hook.skip=pre-commit-suppressions commit",
            "git -c hook.skip=pre-commit,suppressions commit",
            "git -c amont.severity.pre-commit-suppressions=warn commit",
            "git config amont.severity.pre-commit-suppressions warn",
            "git config amont.agent.lint-suppression-added.stance observe",
            "git -c amont.agent.lint-suppression-added.stance=observe commit",
            "GIT_CONFIG_KEY_0=amont.agent.lint-suppression-added.stance git commit",
            "bash -c 'git -c amont.agent.suppressions=allow commit'",
        ] {
            assert!(denied(command), "{command}");
        }
    }

    #[test]
    fn ordinary_commits_and_a_whole_pre_commit_skip_pass() {
        for command in [
            "git commit -m x",
            "git -c hook.skip=pre-commit commit -m x",
            "git config amont.agent.suppressions",
            "git config --get amont.agent.lint-suppression-added.stance",
            "git commit -m \"explain -c amont.agent.suppressions=allow\"",
            "git log --grep=amont.agent.suppressions",
            "git -c amont.severity.pre-commit-suppressions=block commit",
        ] {
            assert!(!denied(command), "{command}");
        }
    }
}
