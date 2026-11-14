//! `ask-unopened-page`: a question about a page the person was never shown
//! (decisions ADR-0058, `work.page-shown-before-asked`).
//!
//! It fires on `AskUserQuestion`, not on a shell command, so `examine` never
//! matches a command; the judgement is `crate::page_open`, called from the
//! hook's `PreAsk` arm. The rule is declared here so its stance resolves,
//! graduates and reports like every other rule's.
//!
//! Default `deny`, the person's choice: a question naming a local page that no
//! `amont-agent open` showed since the last answered question about it is
//! refused with one command per page. Without a readable transcript the hook
//! cannot see what was opened, and advises instead.

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "ask-unopened-page",
    prevents: "Asking the person to choose between pages they were never shown",
    irreversible: false,
    default_stance: Stance::Deny,
    max_stance: Stance::Deny,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-11-02",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(_: &Parsed) -> Option<Finding> {
    None
}
