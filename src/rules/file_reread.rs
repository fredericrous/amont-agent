//! `file-reread` — opening a file this session already has in context.
//!
//! Measured over 41,700 tool calls (2026-09-09): 3,381 second-or-later
//! opens of a path already read in the same session, in two sessions of
//! every three — 15% of every byte the tools returned. Nothing fails; the
//! model simply reads again what it could have scrolled up to, and the
//! context pays twice.
//!
//! This rule cannot be answered from a command string. It is answered from
//! the session's own record ([`crate::session_state`]): the hook records
//! every Read, every `cat`-shaped dump, and every Edit or Write, and asks
//! before a read whether the same path was read earlier with nothing written
//! to it since. A write in between makes the re-read correct, and the rule
//! stays silent. So `examine` here never fires — the backtester has no
//! session to consult — and the rule lives in the hook's file path; its
//! evidence is the transcript measurement, and its stance is honoured the
//! same way as every other rule's.

use crate::rules::{Evidence, Finding, Rule, Stance, Trend};
use crate::session_state::Seen;
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "file-reread",
    default_stance: Stance::Advise,
    evidence: Evidence {
        // 3,381 re-reads over 41,700 calls of every tool.
        per_1000: 81.1,
        measured: "2026-09-09",
        trend: Trend::Flat(4),
    },
    examine,
    confirm: None,
};

fn examine(_parsed: &Parsed) -> Option<Finding> {
    None
}

/// What the hook says when a read repeats one the session already made.
pub fn phrase(path: &str, seen: &Seen) -> (String, String) {
    let window = if seen.window == "full" {
        "whole".to_string()
    } else {
        format!("lines {}", seen.window.replace(':', " to +"))
    };
    (
        format!(
            "`{path}` was already read {} file operation{} ago in this session ({window}, \
             {} bytes) and nothing has written to it since — what it says is already in \
             context.",
            seen.calls_ago,
            if seen.calls_ago == 1 { "" } else { "s" },
            seen.bytes
        ),
        "Use what was read, or Read only the window you need with `offset`/`limit`. A file \
         edited since is read again without comment."
            .to_string(),
    )
}
