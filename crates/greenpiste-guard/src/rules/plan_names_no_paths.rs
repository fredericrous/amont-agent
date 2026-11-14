//! `plan-names-no-paths`: a plan whose Phases name no file, so its review
//! panel was seated from every area of its repositories (decisions ADR-0061,
//! `work.review-seats`).
//!
//! It fires on `ExitPlanMode`, beside `plan-review-panel`, never on a shell
//! command, so `examine` never matches one; the judgement is
//! `crate::plan_review`, called from the hook's `PrePlanExit` arm. The rule
//! is declared here so its stance resolves and reports like every other
//! rule's.
//!
//! `advise` by default and at most: the plan template does not ask for the
//! files of each phase yet, so a refusal would block every session in plan
//! mode. The advice also names each phase that names no file when others do.

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "plan-names-no-paths",
    prevents: "A review panel seated from every area of a repository because the plan's Phases name no file",
    irreversible: false,
    default_stance: Stance::Advise,
    max_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-11-07",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(_: &Parsed) -> Option<Finding> {
    None
}
