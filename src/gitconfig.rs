//! Reading this guard's settings out of git config.
//!
//! Vendored from `amont-runtime::config`, trimmed to the two readers this
//! crate calls — and with one deliberate behavioural difference.
//!
//! # No policy layer, on purpose
//!
//! amont's reader consults a repository's committed policy file and lets its
//! `set` lines outrank the machine's `system` and `global` git config. That
//! is right for a hook manager: a team commits its conventions and they
//! apply. It is exactly wrong here. This guard's whole job is to refuse
//! commands a coding agent is about to run, and a repository that could set
//!
//!     amont.agent.pipe-to-tail.stance = observe
//!
//! could disarm the guard on the machine of anybody who cloned it, without
//! that person ever choosing it. So there is no policy ladder in this
//! module: a stance answers to the user's own git config and to nothing a
//! `git clone` can carry.
//!
//! `stance`'s module docs already made this argument — "promotion power
//! stays on the machine, with the person" — and rested it on `amont.conf`'s
//! parser refusing to name these rules. That closed one door. amont's
//! policy `set` lines were a second one, reaching any `amont.*` key through
//! the reader rather than through the parser, and outranking `global`
//! while they did it. Vendoring the reader without the ladder closes it.
//!
//! # Git parses the values, not us
//!
//! The founding argument of the module this came from, and the reason
//! [`boolean`] shells out with `--type=bool` rather than comparing strings:
//! git's boolean dialect is `true`/`false`/`yes`/`no`/`on`/`off`/`1`/`0`,
//! case-insensitively, plus a valueless key meaning true. A hand-rolled
//! `== "true"` would silently turn `amont.agent.enabled = yes` into a
//! no-op — a setting that reads as working and does nothing. Letting git
//! normalise means there is one config dialect, and it is git's.
//!
//! [`enumerated`] is the other shape and takes an UNTYPED read: there is no
//! `--type` for a closed set of words, so it reads the literal and does its
//! own case-insensitive match, reporting a non-member as a mistake rather
//! than as absence.

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

use crate::git;
use crate::ui::{highlight, warning_sign};

/// The three answers a config read can give, kept apart.
///
/// `Unset` and `Bad` must not collapse into each other: one is a default,
/// the other is a mistake somebody needs to be told about.
pub enum Value<T> {
    Set(T),
    Unset,
    Bad { why: String },
}

/// `git config [--type=<ty>] --get <key>`, with the three exits kept apart.
///
/// Git failing to run at all is reported as `Unset`: this crate's standing
/// posture is that an unanswerable question takes the default rather than
/// interfering with what the agent is doing.
fn read(key: &str, ty: Option<&str>) -> Value<String> {
    let type_flag = ty.map(|t| format!("--type={t}"));
    let mut args: Vec<&str> = vec!["config"];
    if let Some(tf) = &type_flag {
        args.push(tf);
    }
    args.extend(["--get", key]);
    let Some(out) = git::output(&args) else {
        return Value::Unset;
    };
    match out.code {
        // 1 is "no such key". Anything else is git refusing the value it
        // found — a `--type=bool` it cannot parse exits 128, not 1.
        0 => Value::Set(out.stdout),
        1 => Value::Unset,
        _ => Value::Bad {
            why: first_line(&out.stderr),
        },
    }
}

fn first_line(stderr: &str) -> String {
    let line = stderr.lines().next().unwrap_or("").trim();
    let line = line.strip_prefix("fatal: ").unwrap_or(line);
    if line.is_empty() {
        "git could not read the value".to_string()
    } else {
        line.to_string()
    }
}

/// A boolean in git's own dialect — see the module docs.
fn boolean(key: &str) -> Value<bool> {
    match read(key, Some("bool")) {
        Value::Set(v) => match v.as_str() {
            "true" => Value::Set(true),
            "false" => Value::Set(false),
            other => Value::Bad {
                why: format!("git normalised it to {other:?}, which is neither true nor false"),
            },
        },
        Value::Unset => Value::Unset,
        Value::Bad { why } => Value::Bad { why },
    }
}

/// One of `allowed`, matched case-insensitively. Untyped: git has no
/// `--type` for a closed set of words.
fn enumerated(key: &str, allowed: &[&'static str]) -> Value<&'static str> {
    match read(key, None) {
        Value::Set(v) => {
            let got = v.trim().to_ascii_lowercase();
            match allowed.iter().find(|a| a.eq_ignore_ascii_case(&got)) {
                Some(hit) => Value::Set(hit),
                None => Value::Bad {
                    why: format!("{got:?} is not one of {}", allowed.join(", ")),
                },
            }
        }
        Value::Unset => Value::Unset,
        Value::Bad { why } => Value::Bad { why },
    }
}

/// Say once, per key, that a configured value could not be used.
///
/// Deduplicated because a key read twice in one run is a detail of how the
/// code is arranged, and repeating the warning would make it look like two
/// separate mistakes.
fn complain(key: &str, why: &str, using: &str) {
    static SAID: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let said = SAID.get_or_init(|| Mutex::new(BTreeSet::new()));
    // A poisoned mutex means another thread panicked mid-insert; warning
    // twice is strictly better than joining it in panicking.
    let fresh = match said.lock() {
        Ok(mut set) => set.insert(key.to_string()),
        Err(_) => true,
    };
    if fresh {
        eprintln!(
            "{} {}: {why} — using {using}",
            warning_sign().trim(),
            highlight(key)
        );
    }
}

pub fn boolean_or(key: &str, default: bool) -> bool {
    match boolean(key) {
        Value::Set(v) => v,
        Value::Unset => default,
        Value::Bad { why } => {
            complain(key, &why, &default.to_string());
            default
        }
    }
}

pub fn enumerated_or(key: &str, allowed: &[&'static str], default: &'static str) -> &'static str {
    match enumerated(key, allowed) {
        Value::Set(v) => v,
        Value::Unset => default,
        Value::Bad { why } => {
            complain(key, &why, default);
            default
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The normalisation contract, without spawning git: whatever git hands
    /// back for `--type=bool` is one of exactly two strings, and anything
    /// else is a bug worth reporting rather than guessing at.
    #[test]
    fn boolean_accepts_only_gits_normalised_pair() {
        fn classify(v: &str) -> Option<bool> {
            match v {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            }
        }
        assert_eq!(classify("true"), Some(true));
        assert_eq!(classify("false"), Some(false));
        // What `--type=bool` exists to prevent us from ever seeing here.
        assert_eq!(classify("yes"), None);
        assert_eq!(classify("on"), None);
        assert_eq!(classify("1"), None);
    }

    #[test]
    fn enumerated_matches_case_insensitively_and_names_the_alternatives() {
        const ALLOWED: &[&str] = &["observe", "advise", "deny"];
        fn pick(v: &str) -> Result<&'static str, String> {
            let got = v.trim().to_ascii_lowercase();
            ALLOWED
                .iter()
                .find(|a| a.eq_ignore_ascii_case(&got))
                .copied()
                .ok_or_else(|| format!("{got:?} is not one of {}", ALLOWED.join(", ")))
        }
        assert_eq!(pick("deny"), Ok("deny"));
        assert_eq!(pick("DENY"), Ok("deny"));
        assert_eq!(pick("  Observe  "), Ok("observe"));
        assert_eq!(
            pick("nonsense"),
            Err("\"nonsense\" is not one of observe, advise, deny".to_string())
        );
    }

    #[test]
    fn first_line_strips_fatal_and_survives_empty_stderr() {
        assert_eq!(
            first_line("fatal: bad boolean config value 'maybe'\nsecond line"),
            "bad boolean config value 'maybe'"
        );
        assert_eq!(first_line(""), "git could not read the value");
        assert_eq!(first_line("   \n  "), "git could not read the value");
    }
}
