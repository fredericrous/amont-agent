//! `clear-at-boundary`: a turn ended at a boundary, with nothing in flight,
//! and the main chain's context is past 200k tokens, so a `/clear` would cost
//! less than carrying on.
//!
//! It fires on `Stop`, not on a shell command, so `examine` never matches a
//! command; the judgement is `amont_agent_process::clear_boundary`, called
//! from the hook's `Stop` arm. The rule is declared here so its stance
//! resolves, graduates and reports like every other rule's.
//!
//! Default `advise`, ceiling `deny`: a `Stop` continuation is the only way to
//! have the handoff written, so `deny` sends it, `advise` tells the person
//! instead, and `observe` only journals. It ships at `advise` because its
//! backtest missed two bars (25 of 31 compacted sessions advised before their
//! first compaction against 28; 46 of 140 advices followed by an editing
//! turn against at most 10%); `deny` waits on the Outcome's 7- and 14-day
//! counts.

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

/// What the handoff's last line says; the handoff step prints this constant.
pub const MARKER: &str = "safe to /clear";

pub const RULE: Rule = Rule {
    id: "clear-at-boundary",
    prevents:
        "A turn ending at a boundary past 200k tokens of context without a handoff and a clear",
    irreversible: false,
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-11-13",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(_: &Parsed) -> Option<Finding> {
    None
}
