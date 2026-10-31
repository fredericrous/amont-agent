//! `work-overlap` — a session's first Edit or Write in a repository, while
//! another session was lately working in the same repository.
//!
//! Like `agent-guidance-written`, `examine` never fires — the backtester sees
//! no Edit or Write calls — and the rule lives in the hook's file path, which
//! asks [`crate::activity`] who else is live and calls [`phrase`]. Told once
//! per session and repository.

use crate::activity::Peer;
use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "work-overlap",
    prevents: "Two sessions editing the same repository without either knowing about the other",
    irreversible: false,
    default_stance: Stance::Advise,
    max_stance: Stance::Advise,
    evidence: Evidence {
        // Not yet measured: the activity record this rule reads starts with it.
        per_1000: 0.0,
        measured: "2026-10-31",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(_parsed: &Parsed) -> Option<Finding> {
    None
}

/// The one-line advice, naming every live peer (at most three, freshest first).
pub fn phrase(repo_name: &str, peers: &[Peer], now: std::time::SystemTime) -> String {
    const NAMED: usize = 3;
    let named: Vec<String> = peers
        .iter()
        .take(NAMED)
        .map(|peer| {
            format!(
                "session `{}` (worktree `{}`, branch `{}`, last seen {} ago)",
                peer.session,
                peer.worktree.display(),
                peer.branch,
                crate::activity::ago(now, peer.last_seen),
            )
        })
        .collect();
    let more = peers.len().saturating_sub(NAMED);
    let tail = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    crate::decision::phrase(
        RULE.id,
        &format!(
            "another session is working in this repository: {}{tail}.",
            named.join("; ")
        ),
        &format!(
            "Run `amont-agent board --repo {repo_name}`, then message that session (SendMessage) or ask the person."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn the_advice_names_the_session_worktree_branch_age_and_remedy() {
        let now = UNIX_EPOCH + Duration::from_secs(10_000);
        let peer = Peer {
            session: "s-other".to_string(),
            worktree: PathBuf::from("/w/feat"),
            branch: "feat/x".to_string(),
            last_seen: now - Duration::from_secs(25 * 60),
        };
        let text = phrase("amont", &[peer], now);
        for part in [
            "s-other",
            "/w/feat",
            "feat/x",
            "25m",
            "amont-agent board --repo amont",
            "SendMessage",
        ] {
            assert!(text.contains(part), "{part}: {text}");
        }
    }
}
