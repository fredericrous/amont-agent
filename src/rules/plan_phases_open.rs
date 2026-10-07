//! `plan-phases-open`: a turn ended while the plan its branch carries still
//! has an open phase (ADR-0022, `work.commits-follow-phases`).
//!
//! It fires on `Stop`, not on a shell command, so `examine` never matches a
//! command; the judgement is `crate::plan_phases`, called from the hook's
//! `Stop` arm. The rule is declared here so its stance resolves, graduates
//! and reports like every other rule's.
//!
//! Default `deny`, the person's choice: the turn is kept going, with the
//! phase named, until the phase is ticked, a `WAITING:` line says why it
//! cannot be, or three continuations have passed.

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "plan-phases-open",
    default_stance: Stance::Deny,
    max_stance: Stance::Deny,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-10-07",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(_: &Parsed) -> Option<Finding> {
    None
}
