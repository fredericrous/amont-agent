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
//! A THIRD door stayed open until this reader named its scopes. Git's own
//! search order ends at `--local` and `--worktree`, and the last file wins —
//! so a `.git/config` in the repository the agent is standing in outranked
//! the machine's answer without any of amont's machinery being involved, and
//! the agent could write one. [`read`] reads `--global` and `--system` only,
//! and says there why.
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

/// This crate's settings, from the MACHINE's config and nowhere else.
///
/// `--global`, then `--system` — git's own precedence with one file removed:
/// the repository's. A bare `git config --get` searches system, global, local
/// and worktree, and the last one wins; `--local` and `--worktree` are both
/// files a repository can carry or a process standing in it can write. That
/// is one scope too many for a guard whose whole job is to refuse commands a
/// coding agent is about to run:
///
///   * the agent that just had a command refused can write
///     `git config amont.agent.pipe-to-tail.stance observe` into `.git/config`
///     — an ordinary command no rule here objects to — and the next call is
///     allowed. A guard a guarded process can switch off is decoration.
///   * `amont-agent graduate` and `demote` write `--global` (see `graduate`),
///     so a stale `--local` key silently outranked the very command that
///     exists to move a stance. A stance you believe you set and did not is
///     the failure this crate is about.
///   * `git clone` does not copy `.git/config`, but a restored backup, a
///     copied working copy and a linked worktree all do.
///
/// The module docs above already claimed this ("a stance answers to the
/// user's own git config and to nothing a `git clone` can carry"); dropping
/// the policy ladder closed the door amont's own reader left open, and this
/// closes the one git's search order left open.
///
/// Precedence between the two that remain is git's: `--global` wins, and
/// `--system` answers only when the user's own file is silent. A value the
/// user's file sets to something git refuses is reported as [`Value::Bad`]
/// rather than falling through to the machine's — a mistake in the file you
/// edited must not be answered by a file you have never seen.
fn read(key: &str, ty: Option<&str>) -> Value<String> {
    match read_scope(0, "--global", key, ty) {
        Value::Set(v) => Value::Set(v),
        Value::Bad { why } => Value::Bad { why },
        Value::Unset => read_scope(1, "--system", key, ty),
    }
}

/// One scope's answer: from its snapshot for this crate's own keys, from
/// git directly for anything else.
fn read_scope(index: usize, scope: &str, key: &str, ty: Option<&str>) -> Value<String> {
    if key.starts_with(PREFIX) {
        read_cached(index, scope, key, ty)
    } else {
        ask_git(scope, key, ty)
    }
}

/// Every key this crate reads lives under this prefix, so one
/// `--get-regexp` per scope answers all of them.
const PREFIX: &str = "amont.agent.";

/// One scope's `amont.agent.*` keys, read once per process.
///
/// Measured (#70): a `git config` spawn costs ~40 ms on a loaded macOS
/// machine, and the firing path read three keys in two scopes — six spawns,
/// 120 ms of a 140 ms hook call whose silent path costs 17 ms. A snapshot is
/// two spawns, whatever the number of keys. The scope argument above is
/// unchanged: the same two files, with what they include, and nothing else.
enum Snapshot {
    /// `(key, value)` in file order; `None` is a valueless key.
    Keys(Vec<(String, Option<String>)>),
    /// Git refused the scope (a malformed file, an unreadable include):
    /// every key read from it is that mistake, as a per-key read reported.
    Refused(String),
}

type Snapshots = [Option<Snapshot>; 2];

fn snapshots() -> &'static Mutex<Snapshots> {
    static CELL: OnceLock<Mutex<Snapshots>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new([None, None]))
}

/// Drop the snapshots, so the next read sees a value this process just
/// wrote (`graduate`).
pub fn forget() {
    if let Ok(mut s) = snapshots().lock() {
        *s = [None, None];
    }
}

fn snapshot_of(scope: &str) -> Snapshot {
    let Some(out) = git::output(&[
        "config",
        scope,
        "--includes",
        "--null",
        "--get-regexp",
        r"^amont\.agent\.",
    ]) else {
        // Git did not run: every key takes its default, as before.
        return Snapshot::Keys(Vec::new());
    };
    match out.code {
        0 => Snapshot::Keys(parse_null(&out.stdout)),
        1 => Snapshot::Keys(Vec::new()),
        _ => Snapshot::Refused(first_line(&out.stderr)),
    }
}

/// Read both scopes at once. Nearly every lookup needs both — `--system`
/// answers whenever the user's file is silent, which for most keys is
/// always — and a git spawn is the whole cost (~30 ms each on a loaded
/// machine), so two in parallel cost the wall time of one.
fn fill(all: &mut Snapshots) {
    let want = [all[0].is_none(), all[1].is_none()];
    if want == [false, false] {
        return;
    }
    let read = |i: usize| {
        let scope = if i == 0 { "--global" } else { "--system" };
        snapshot_of(scope)
    };
    let got = std::thread::scope(|sc| {
        let handles = [0, 1].map(|i| want[i].then(|| sc.spawn(move || read(i))));
        // A reader that panicked answers nothing: the defaults, as when git
        // does not run at all.
        handles.map(|h| h.map(|h| h.join().unwrap_or(Snapshot::Keys(Vec::new()))))
    });
    for (slot, snap) in all.iter_mut().zip(got) {
        if let Some(snap) = snap {
            *slot = Some(snap);
        }
    }
}

/// `--null` output: `key\nvalue\0` per entry, `key\0` for a valueless key.
fn parse_null(raw: &str) -> Vec<(String, Option<String>)> {
    raw.split('\0')
        .filter(|e| !e.is_empty())
        .map(|e| match e.split_once('\n') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (e.to_string(), None),
        })
        .collect()
}

/// Git's key equality: section and variable name ignore case, the
/// subsection between them does not.
fn canonical(key: &str) -> String {
    match (key.find('.'), key.rfind('.')) {
        (Some(first), Some(last)) if first < last => format!(
            "{}{}{}",
            key[..first].to_ascii_lowercase(),
            &key[first..last],
            key[last..].to_ascii_lowercase()
        ),
        _ => key.to_ascii_lowercase(),
    }
}

/// What one scope says about `key`, untyped: the LAST value, as `--get`.
fn lookup(scope_index: usize, scope: &str, key: &str) -> Value<Option<String>> {
    let Ok(mut all) = snapshots().lock() else {
        return Value::Unset;
    };
    fill(&mut all);
    let snap = all[scope_index].get_or_insert_with(|| snapshot_of(scope));
    match snap {
        Snapshot::Refused(why) => Value::Bad { why: why.clone() },
        Snapshot::Keys(keys) => {
            let want = canonical(key);
            match keys.iter().rev().find(|(k, _)| canonical(k) == want) {
                Some((_, v)) => Value::Set(v.clone()),
                None => Value::Unset,
            }
        }
    }
}

/// [`ask_git`], answered from the scope's snapshot. A typed read of a key
/// that IS set still asks git, so git alone parses the dialect; the common
/// case — the key is unset — costs no spawn at all.
fn read_cached(scope_index: usize, scope: &str, key: &str, ty: Option<&str>) -> Value<String> {
    match lookup(scope_index, scope, key) {
        Value::Unset => Value::Unset,
        Value::Bad { why } => Value::Bad { why },
        Value::Set(_) if ty.is_some() => ask_git(scope, key, ty),
        // A valueless key reads as the empty string, as `--get` prints it.
        Value::Set(v) => Value::Set(v.unwrap_or_default()),
    }
}

/// `git config <scope> --includes [--type=<ty>] --get <key>`, with the three
/// exits kept apart.
///
/// `--includes` is not the default for a scoped read. Measured on git 2.55:
/// `[include] path = extra` in `~/.gitconfig` was invisible to
/// `git config --global --get`, and so was an `includeIf "gitdir:…"` — the
/// shape a person with a work and a personal identity keeps their config
/// in. A stance kept in a file the user's own config names is still the
/// user's own choice, and the scope argument stays: only the global and
/// system files, and what THEY include, are ever opened.
///
/// Git failing to run at all is reported as `Unset`: this crate's standing
/// posture is that an unanswerable question takes the default rather than
/// interfering with what the agent is doing. A scope whose file does not
/// exist at all exits 1, the same as a key nobody set — which is the answer
/// we want for a machine with no `/etc/gitconfig`.
fn ask_git(scope: &str, key: &str, ty: Option<&str>) -> Value<String> {
    let type_flag = ty.map(|t| format!("--type={t}"));
    let mut args: Vec<&str> = vec!["config", scope, "--includes"];
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
    fn null_output_keeps_values_whole_and_valueless_keys_apart() {
        let raw = "amont.agent.enabled\0amont.agent.stance\nadvise\0\
                   amont.agent.x.stance\nline one\nline two\0amont.agent.y.stance\n\0";
        assert_eq!(
            parse_null(raw),
            vec![
                ("amont.agent.enabled".to_string(), None),
                ("amont.agent.stance".to_string(), Some("advise".to_string())),
                (
                    "amont.agent.x.stance".to_string(),
                    Some("line one\nline two".to_string())
                ),
                ("amont.agent.y.stance".to_string(), Some(String::new())),
            ]
        );
    }

    /// `git config` folds the section and the variable name to lower case
    /// and keeps the subsection as written.
    #[test]
    fn keys_compare_the_way_git_compares_them() {
        assert_eq!(
            canonical("amont.agent.agentsMdNotice"),
            canonical("amont.agent.agentsmdnotice")
        );
        assert_eq!(
            canonical("AMONT.agent.pipe-to-tail.Stance"),
            "amont.agent.pipe-to-tail.stance"
        );
        assert_ne!(
            canonical("amont.Agent.stance"),
            canonical("amont.agent.stance")
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
