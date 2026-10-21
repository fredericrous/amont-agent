//! Is the guard actually running?
//!
//! This command exists because of one property of Claude Code hooks: **they
//! fail open, quietly, in almost every way that matters.** A hook whose command
//! does not exist exits 127, which lands in the non-blocking bucket. A hook that
//! times out does not block. A hook that emits JSON the schema rejects is a
//! non-blocking error. None of those reach the person who installed it.
//!
//! So a broken guard and a quiet week look identical from the outside, and the
//! failure is silent in exactly the way the rule this crate was built for is
//! silent. Answering "is it alive?" is therefore not a nicety; it is the same
//! problem again, one level up.
//!
//! ## The liveness check is the one that earns the command
//!
//! Transcripts prove that sessions happened. The heartbeat proves the guard ran
//! inside one. If there are sessions newer than the newest heartbeat, the hook
//! stopped firing at a moment we can name.
//!
//! Note that this is the one place file mtime is the RIGHT question to ask.
//! `transcript.rs` refuses mtime on principle, because "when did this tool call
//! happen" must come from the entry's own timestamp. Here the question is
//! literally "when was a transcript last written", and mtime is precisely that.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::ui;

use crate::journal;
use crate::rules;
use crate::settings;
use crate::stance;
use crate::transcript;

struct Install {
    path: PathBuf,
    command: String,
    event: String,
    /// The block's `matcher`; `None` for an event that takes none.
    matcher: Option<String>,
}

pub struct Finding {
    pub ok: bool,
    /// A finding that is not `ok` but also not fatal — worth saying, not worth
    /// a non-zero exit.
    pub advisory: bool,
    pub line: String,
    pub detail: Option<String>,
}

impl Finding {
    pub fn good(line: impl Into<String>) -> Finding {
        Finding {
            ok: true,
            advisory: false,
            line: line.into(),
            detail: None,
        }
    }
    pub(crate) fn bad(line: impl Into<String>, detail: impl Into<String>) -> Finding {
        Finding {
            ok: false,
            advisory: false,
            line: line.into(),
            detail: Some(detail.into()),
        }
    }
    pub fn warn(line: impl Into<String>, detail: impl Into<String>) -> Finding {
        Finding {
            ok: false,
            advisory: true,
            line: line.into(),
            detail: Some(detail.into()),
        }
    }
}

/// Every check, in order. `table` is the rule table; `extra` is the line the
/// work process contributes (the attestations store), last.
pub fn run(table: &[rules::Rule], extra: Option<Finding>) -> Vec<Finding> {
    let mut out = Vec::new();
    let installs = installed_in();
    out.push(configured(&installs));
    out.push(executable(&installs));
    out.push(self_test());
    out.push(liveness(&installs));
    out.push(stances(table));
    out.push(journal_span());
    out.push(lsp_rules());
    out.extend(shadow_settings(table));
    out.extend(extra);
    out
}

/// The shadow rates of ADR-0042: nothing when none is set, the active ones
/// when some are, and a warning for each that does not parse (it is off) or
/// that sits on an irreversible part (refused: such a part is never shadowed).
fn shadow_settings(table: &[rules::Rule]) -> Vec<Finding> {
    shadow_findings(
        &crate::shadow::bad_settings(),
        crate::shadow::configured(),
        |id| {
            table.iter().any(|r| r.id == id && r.irreversible)
                || crate::assertions::ASSERTIONS
                    .iter()
                    .any(|a| a.id == id && a.irreversible)
        },
    )
}

fn shadow_findings(
    bad: &[(String, String)],
    configured: &std::collections::BTreeMap<String, String>,
    irreversible: impl Fn(&str) -> bool,
) -> Vec<Finding> {
    if !bad.is_empty() {
        let list = bad
            .iter()
            .map(|(id, v)| format!("{id}={v}"))
            .collect::<Vec<_>>()
            .join(", ");
        return vec![Finding::warn(
            "shadow: a rate does not parse, so that part is not shadowed",
            format!("{list}: set a number in [0,1], or unset amont.agent.<id>.shadow"),
        )];
    }
    let mut on = Vec::new();
    let mut refused = Vec::new();
    for (id, v) in configured {
        if let crate::shadow::Rate::On(r) = crate::shadow::parse_rate(v) {
            if irreversible(id) {
                refused.push(id.clone());
            } else {
                on.push(format!("{id} at {r}"));
            }
        }
    }
    let mut out = Vec::new();
    if !on.is_empty() {
        out.push(Finding::good(format!("shadow: {}", on.join(", "))));
    }
    if !refused.is_empty() {
        out.push(Finding::warn(
            format!(
                "shadow: a rate is set on an irreversible part, so it is refused (irreversible): {}",
                refused.join(", ")
            ),
            "an irreversible part is judged by replay and never switched off live; unset amont.agent.<id>.shadow",
        ));
    }
    out
}

/// Whether `code-read-not-lsp` and `symbol-grep-not-lsp` can speak here: they
/// stay silent without fleet-lsp on PATH, outside a git working tree, or with
/// the plugin off, and nothing else would say so.
fn lsp_rules() -> Finding {
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match (crate::lsp::on_path("fleet-lsp"), crate::lsp::serves(&here)) {
        (Some(bin), Ok(())) => Finding::good(format!(
            "LSP rules: fleet-lsp at {}, plugin enabled",
            bin.display()
        )),
        (_, Err(why)) => Finding::warn(
            "LSP rules are silent here",
            format!(
                "{why}: `code-read-not-lsp` and `symbol-grep-not-lsp` speak only where fleet-lsp \
                 would answer"
            ),
        ),
        (None, Ok(())) => Finding::warn("LSP rules are silent here", "fleet-lsp is not on PATH"),
    }
}

/// Every settings file that names us, and the command each one points at.
fn installed_in() -> Vec<Install> {
    let project = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut found: Vec<Install> = Vec::new();
    for scope in [
        settings::Scope::User,
        settings::Scope::Project,
        settings::Scope::ProjectLocal,
    ] {
        let Some(path) = scope.path(&project) else {
            continue;
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        // Each event once: TARGETS names PreToolUse several times, and
        // scanning it per entry counted every PreToolUse handler thrice,
        // hiding a missing event behind the surplus.
        let mut seen: Vec<&str> = Vec::new();
        for (event, _) in settings::TARGETS {
            if seen.contains(event) {
                continue;
            }
            seen.push(event);
            let Some(list) = doc
                .get("hooks")
                .and_then(|h| h.get(*event))
                .and_then(|p| p.as_array())
            else {
                continue;
            };
            for block in list {
                let Some(handlers) = block.get("hooks").and_then(|h| h.as_array()) else {
                    continue;
                };
                for h in handlers {
                    if settings::is_ours(h) {
                        found.push(Install {
                            path: path.clone(),
                            command: h
                                .get("command")
                                .and_then(|c| c.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            event: (*event).to_string(),
                            matcher: block
                                .get("matcher")
                                .and_then(|m| m.as_str())
                                .map(str::to_string),
                        });
                    }
                }
            }
        }
    }
    found
}

fn configured(installs: &[Install]) -> Finding {
    if installs.is_empty() {
        return Finding::bad(
            "not installed in any settings file",
            "run `amont-agent install --write` (it prints the block first)".to_string(),
        );
    }
    let files: Vec<&PathBuf> = {
        let mut v: Vec<&PathBuf> = installs.iter().map(|i| &i.path).collect();
        v.dedup();
        v
    };
    // Hooks from different settings files MERGE rather than override, so two
    // installs mean the guard runs twice per command — every reason printed
    // twice, every journal line doubled.
    if files.len() > 1 {
        return Finding::warn(
            format!("installed in {} settings files", files.len()),
            format!(
                "hooks merge across files rather than overriding, so this runs twice:\n      {}",
                files
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n      ")
            ),
        );
    }
    // Each (event, matcher) pair, not each event: a release that adds a
    // matcher to an event already installed (PreToolUse Grep, 2.32.0) would
    // otherwise count as complete.
    let missing: Vec<String> = settings::TARGETS
        .iter()
        .filter(|(event, matcher)| {
            !installs
                .iter()
                .any(|i| i.event == *event && i.matcher.as_deref() == *matcher)
        })
        .map(|(event, matcher)| match matcher {
            Some(m) => format!("{event} {m}"),
            None => (*event).to_string(),
        })
        .collect();
    if !missing.is_empty() {
        let want = settings::TARGETS.len();
        return Finding::warn(
            format!(
                "installed for {} of {want} hook entries",
                want - missing.len()
            ),
            format!(
                "missing: {}. Re-run `amont-agent install --write`: the SessionStart entry is \
                 what proves the guard is alive, the Read|Edit|Write and PostToolUse Read \
                 entries let `file-reread` see a file opened twice, the Grep entry lets \
                 `symbol-grep-not-lsp` see the Grep tool, and the PostToolUse LSP entry lets \
                 `code-read-not-lsp` see what LSP was asked",
                missing.join(", ")
            ),
        );
    }
    Finding::good(format!("installed in {}", files[0].display()))
}

/// The exit-127 case: a command that does not resolve is a hook that silently
/// does nothing, because 127 is a NON-blocking status.
fn executable(installs: &[Install]) -> Finding {
    let Some(cmd) = installs.first().map(|i| &i.command) else {
        return Finding::bad("no command to check", "nothing is installed".to_string());
    };
    let path = Path::new(cmd);
    if !path.is_absolute() {
        return Finding::bad(
            format!("`{cmd}` is not an absolute path"),
            "a command resolved from PATH exits 127 the moment PATH differs, and 127 does \
             not block — the guard would be gone with nothing to notice"
                .to_string(),
        );
    }
    if !path.exists() {
        return Finding::bad(
            format!("`{cmd}` does not exist"),
            "the hook exits 127 on every call, which Claude Code treats as non-blocking"
                .to_string(),
        );
    }
    match Command::new(path).arg("--version").output() {
        Ok(o) if o.status.success() => {
            let reported = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let mine = format!("amont-agent {}", env!("CARGO_PKG_VERSION"));
            if reported != mine {
                return Finding::warn(
                    format!("the installed binary is {reported}"),
                    format!("this one is {mine} — re-install to point at the newer binary"),
                );
            }
            Finding::good(format!("{reported} at {cmd}"))
        }
        _ => Finding::bad(
            format!("`{cmd}` will not run"),
            "it exists but could not be executed — check its permissions".to_string(),
        ),
    }
}

/// Drive a synthetic payload through the real decision path. Catches the
/// schema-invalid-on-exit-0 case in process, before Claude Code has to.
fn self_test() -> Finding {
    let Ok(me) = std::env::current_exe() else {
        return Finding::warn("could not find my own binary", "skipping the self-test");
    };
    let probe = r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":"/tmp",
        "tool_input":{"command":"git push origin main 2>&1 | tail -5"}}"#;
    let out = crate::doctor::pipe(&me, probe);
    let Some(stdout) = out else {
        return Finding::bad("the self-test could not run", "could not spawn myself");
    };
    if stdout.trim().is_empty() {
        return Finding::bad(
            "a command that should be refused produced no decision",
            "every rule may be demoted, or the guard is switched off — check \
             `amont-agent status` and $AMONT_AGENT_OFF"
                .to_string(),
        );
    }
    match serde_json::from_str::<serde_json::Value>(&stdout) {
        Ok(v)
            if v.get("hookSpecificOutput")
                .and_then(|o| o.get("hookEventName"))
                .is_some() =>
        {
            Finding::good("a refused command produces a valid decision document")
        }
        _ => Finding::bad(
            "the decision was not a document Claude Code accepts",
            format!("emitted: {}", stdout.trim()),
        ),
    }
}

fn pipe(bin: &Path, payload: &str) -> Option<String> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(bin)
        .arg("hook")
        // Do not let the self-test count as a firing: see `journal::record`.
        // Without this the probe writes a denial to the journal on every
        // `doctor` run, which both inflates the evidence behind every rule
        // and makes the liveness check below unfalsifiable — the journal
        // would always be seconds old because doctor had just written it.
        .env(crate::journal::ENV_PROBE, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(payload.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Sessions prove work happened; the heartbeat proves the guard saw it.
fn liveness(installs: &[Install]) -> Finding {
    // Without a SessionStart entry no heartbeat is ever written, and the
    // absence of one says nothing about whether the guard is firing. Reporting
    // "never run" here was wrong on this machine's very first doctor run: the
    // guard had refused a command minutes earlier.
    let can_beat = installs.iter().any(|i| i.event == "SessionStart");

    let beat = journal::dir()
        .map(|d| d.join("heartbeat"))
        .and_then(|p| std::fs::metadata(&p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    let sessions = transcript::roots(&[])
        .ok()
        .map(|r| transcript::files(&r))
        .unwrap_or_default();
    let stamp = |t: std::io::Result<std::time::SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    };
    let newest_session = sessions
        .iter()
        .filter_map(|p| stamp(std::fs::metadata(p).ok()?.modified()))
        .max();
    // When a session BEGAN, not when it was last written to. A session that is
    // open right now has its transcript appended to continuously, so its mtime
    // races ahead of any install and makes every ongoing session look like a
    // new one. Creation time is the question actually being asked, and where
    // the filesystem will not answer it we stay lenient rather than accuse.
    let newest_start = sessions
        .iter()
        .filter_map(|p| stamp(std::fs::metadata(p).ok()?.created()))
        .max();

    // The journal is written on every firing, so a recent entry is independent
    // proof of life — weaker than the heartbeat, because a genuinely quiet
    // period leaves none, but it can only ever confirm, never accuse.
    let newest_fire = journal::path()
        .and_then(|p| std::fs::metadata(&p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    if newest_session.is_none() {
        return Finding::warn(
            "no transcripts found, so liveness cannot be judged",
            "this is normal on a machine that has not run Claude Code".to_string(),
        );
    }
    if !can_beat {
        return match newest_fire {
            Some(f) => Finding::warn(
                format!(
                    "no heartbeat, but a rule fired {} ago",
                    ago(now().saturating_sub(f))
                ),
                "liveness cannot be judged properly without the SessionStart entry — \
                 re-run `amont-agent install --write`"
                    .to_string(),
            ),
            None => Finding::warn(
                "liveness cannot be judged",
                "no SessionStart entry is installed, so no heartbeat is ever written — \
                 re-run `amont-agent install --write`"
                    .to_string(),
            ),
        };
    }
    // When was the hook installed? A guard cannot have run in a session that
    // began before it existed, so "no heartbeat" is only an accusation if a
    // session STARTED AFTER the install. Immediately after installing, silence
    // is simply the truth — reporting it as a fault made the fresh-install
    // case red on the very first run.
    let installed_at = installs
        .first()
        .and_then(|i| std::fs::metadata(&i.path).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let (Some(session), Some(beat)) = (newest_session, beat) else {
        // `None` means the filesystem does not report creation times; treat
        // that as "cannot prove a session started", which is the lenient read.
        let started_since = newest_start.is_some_and(|s| s > installed_at);
        if !started_since {
            return Finding::warn(
                "installed, but no session has started since",
                "start a new Claude Code session and run this again to confirm it fires"
                    .to_string(),
            );
        }
        return Finding::bad(
            "the guard has never run",
            "a session started after the hook was installed and no heartbeat was written, \
             so it is not firing. Check `claude --debug-file <path>` for its exit code."
                .to_string(),
        );
    };

    // The best evidence the guard is alive, from EITHER source.
    //
    // `newest_fire` was computed above and then never consulted here, which
    // made the accusation below contradict the promise its own comment makes
    // — "it can only ever confirm, never accuse". A rule that fired after the
    // last heartbeat is proof the hook did not stop firing, and the journal
    // records every firing.
    //
    // The failure that exposed this: one Claude Code session open for more
    // than six hours. `SessionStart` writes the heartbeat once, at the
    // beginning, so the heartbeat ages while the session file keeps being
    // written — and `doctor` announced "the guard has not run in 14h" with
    // the journal sitting in the same directory, last written seconds
    // earlier by a `pipe-to-tail` denial. A long session is the NORMAL case
    // for this tool; the check was accusing it of the one thing it was
    // demonstrably not doing.
    //
    // Taking the max only ever moves liveness FORWARD, so the journal keeps
    // its "confirm, never accuse" property: a genuinely quiet period leaves
    // no entries and the heartbeat still decides.
    let alive = newest_fire.map_or(beat, |fired| fired.max(beat));

    // A session file is written continuously while a session is open, so the
    // newest one is almost always newer than the last heartbeat by seconds.
    // Only a gap wide enough to span a whole session means anything.
    const GRACE: u64 = 6 * 60 * 60;
    if session > alive + GRACE {
        return Finding::bad(
            format!("the guard has not run in {}", ago(session - alive)),
            format!(
                "a session was active {} after the guard last showed any sign of life, \
                 so the hook stopped firing. Check `claude --debug-file <path>` for its \
                 exit code.",
                ago(session - alive)
            ),
        );
    }
    Finding::good(format!("last ran {} ago", ago(now().saturating_sub(alive))))
}

/// How much history the journal holds: generation count and the age of its
/// oldest record, so a window shorter than a week is visible. Liveness above
/// still reads `journal.log`'s own mtime.
fn journal_span() -> Finding {
    let gens = journal::paths().len();
    match journal::oldest_record() {
        Some(ts) => Finding::good(format!(
            "journal: {gens} generation{}, oldest record {} ago",
            if gens == 1 { "" } else { "s" },
            ago(now().saturating_sub(ts))
        )),
        None => Finding::good(format!(
            "journal: {gens} generation{}, no records yet",
            if gens == 1 { "" } else { "s" }
        )),
    }
}

/// What is actually armed, so "installed" is never mistaken for "blocking".
fn stances(table: &[rules::Rule]) -> Finding {
    if stance::switched_off() {
        return Finding::warn(
            format!(
                "${} is set — every rule is clamped to observe",
                stance::ENV_OFF
            ),
            "unset it to arm the guard".to_string(),
        );
    }
    let (armed, observing): (Vec<&str>, Vec<&str>) = table
        .iter()
        .map(|r| (r.id, stance::resolve(r)))
        .fold((Vec::new(), Vec::new()), |(mut a, mut o), (id, st)| {
            if st == rules::Stance::Observe {
                o.push(id)
            } else {
                a.push(id)
            }
            (a, o)
        });
    if armed.is_empty() {
        return Finding::warn(
            "every rule is observing — nothing will be refused",
            "the journal still records what fires; `amont-agent status` shows each stance"
                .to_string(),
        );
    }
    // Both halves, because "installed" was being mistaken for "armed" in the
    // other direction too: a rule that ships `observe` is live and journaling
    // from its first release, and a line that names only what refuses reads
    // as if it were not there at all.
    if observing.is_empty() {
        Finding::good(format!("acting on {}", armed.join(", ")))
    } else {
        Finding::good(format!(
            "acting on {}; observing {}",
            armed.join(", "),
            observing.join(", ")
        ))
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn ago(secs: u64) -> String {
    match secs {
        s if s < 90 => format!("{s}s"),
        s if s < 5400 => format!("{}m", s / 60),
        s if s < 172_800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// Render, and say whether the guard can be trusted to be doing anything.
pub fn report(findings: &[Finding]) -> bool {
    let mut healthy = true;
    for f in findings {
        let sign = if f.ok {
            ui::valid_sign()
        } else if f.advisory {
            ui::warning_sign()
        } else {
            healthy = false;
            ui::error_sign()
        };
        println!("  {sign} {}", ui::sanitize(&f.line));
        if let Some(d) = &f.detail {
            println!("    {}", ui::sanitize(d));
        }
    }
    healthy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shadow_rate_on_an_irreversible_part_is_a_warning_naming_it_refused() {
        let mut configured = std::collections::BTreeMap::new();
        configured.insert("no-verify".to_string(), "0.1".to_string());
        configured.insert("sed-in-place".to_string(), "0.2".to_string());
        let found = shadow_findings(&[], &configured, |id| id == "no-verify");
        let warn = found
            .iter()
            .find(|f| !f.ok && f.advisory)
            .expect("a warning");
        assert!(
            warn.line.contains("no-verify") && warn.line.contains("refused (irreversible)"),
            "{}",
            warn.line
        );
        let good = found.iter().find(|f| f.ok).expect("the good line");
        assert!(good.line.contains("sed-in-place"), "{}", good.line);
        assert!(!good.line.contains("no-verify"), "{}", good.line);
    }
}
