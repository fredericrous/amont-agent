//! amont-agent's work process: the plan-review panel, the implementation
//! review, preview approvals, provenance, `shipped` and the attestations
//! sweep, whose verdicts hold or release an agent's tool call (decisions
//! ADR-0039). It builds on the guard (`amont-agent-guard`) and never names the
//! binary.

pub mod agents;
pub mod attestations;
pub mod candidates_block;
pub mod content_id;
pub mod guide;
pub mod impl_review_outcomes;
pub mod implementation_review;
pub mod implementation_review_rule;
pub mod plan_apply;
pub mod plan_phases;
pub mod plan_review;
pub mod plan_text;
pub mod plan_words;
pub mod plans;
pub mod preview;
pub mod provenance;
pub mod push_preview;
pub mod review_brief;
pub mod review_outcomes;
pub mod shipped;
