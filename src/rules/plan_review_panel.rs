//! `plan-review-panel`: a plan presented for approval before its review
//! panel ran (ADR-0022, `work.plan-review-panel`).
//!
//! It fires on `ExitPlanMode`, not on a shell command, so `examine` never
//! matches a command; the judgement is `crate::plan_review`, called from the
//! hook's `PrePlanExit` arm. The rule is declared here so its stance
//! resolves, graduates and reports like every other rule's.
//!
//! Default `deny`, the person's choice: a plan whose panel did not run is
//! refused with the missing roles; after two refusals of the same plan, and
//! whenever the hook cannot check (no transcript, git failed), the call goes
//! to the person as `ask`.

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "plan-review-panel",
    default_stance: Stance::Deny,
    max_stance: Stance::Deny,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-09-29",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(_: &Parsed) -> Option<Finding> {
    None
}
