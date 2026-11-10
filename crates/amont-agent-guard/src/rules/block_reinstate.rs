//! `block-reinstate` — an agent running `amont-agent block reinstate`.
//!
//! Revoking a reviewed-block fingerprint records that a defect was found in
//! a copy (decisions ADR-0067). Lifting that revocation re-admits every copy
//! of the block without a full review, so the call is the person's: the agent
//! may list and revoke, never reinstate.

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "block-reinstate",
    prevents: "An agent lifting the revocation of a reviewed block, which re-admits every copy of a block with a known defect",
    irreversible: false,
    default_stance: Stance::Deny,
    max_stance: Stance::Deny,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-11-10",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.judgeable() {
        if cmd.program() != Some("amont-agent") {
            continue;
        }
        // Quoted words are operands too, so `echo "amont-agent block reinstate"`
        // is not a match: only the program position starts the command.
        let operands = cmd.operands();
        let verb = |at: usize| operands.get(at).map(|word| word.text.as_str());
        if verb(0) != Some("block") || verb(1) != Some("reinstate") {
            continue;
        }
        return Some(Finding {
            reason: "`amont-agent block reinstate` lifts the revocation of a reviewed \
                     block, so every copy of a block with a known defect is recognised \
                     again without a full review. That decision is the person's."
                .to_string(),
            remedy: "Leave the block revoked. Tell the person which fingerprint to \
                     reinstate (`amont-agent block list`); they run \
                     `amont-agent block reinstate <fp>` in their own shell."
                .to_string(),
            span: cmd.at..cmd.end,
        });
    }
    None
}
