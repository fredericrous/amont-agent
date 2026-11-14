//! `screenshot-in-main-session` — `take_screenshot` called from the main
//! conversation without a `filePath`.
//!
//! Each such screenshot puts an image of about 126 KB into the main context
//! (368 of them in past transcripts). The `browser-check` subagent takes it
//! in its own context and reports in words.
//!
//! Like `agent-guidance-written`, `examine` never fires — the backtester sees
//! no tool calls other than Bash — and the rule lives in the hook's
//! `PreToolUse` path for the chrome-devtools tool, which calls [`phrase`].
//! The hook advises once per session. A subagent (any `agent_id`) and a call
//! with a `filePath` (the image is saved, not attached) are left alone.

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "screenshot-in-main-session",
    prevents: "An image of about 126 KB entering the main context where a subagent could take it",
    irreversible: false,
    default_stance: Stance::Advise,
    max_stance: Stance::Advise,
    evidence: Evidence {
        // 368 calls in past transcripts, about 126 KB each; the per-1,000
        // rate was not computed.
        per_1000: 0.0,
        measured: "2026-11-12",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

/// The tool this rule is about.
pub const TOOL: &str = "mcp__chrome-devtools__take_screenshot";

fn examine(_parsed: &Parsed) -> Option<Finding> {
    None
}

/// The one-line advice.
pub fn phrase() -> String {
    crate::decision::phrase(
        RULE.id,
        "a screenshot here puts the image (~126 KB) into the main context.",
        "Delegate the check: Agent tool with subagent_type \"browser-check\", giving the URL \
         and what to verify.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_advice_is_the_plans_string() {
        assert_eq!(
            phrase(),
            "greenpiste/screenshot-in-main-session: a screenshot here puts the image (~126 KB) \
             into the main context. Delegate the check: Agent tool with subagent_type \
             \"browser-check\", giving the URL and what to verify."
        );
    }
}
