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

/// One budget for every plan lookup of one piece of advice.
const PLAN_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// The title of the first active plan the peer's branch added under
/// `docs/plans` relative to the repository's default branch, read in the
/// peer's worktree; `None` for a peer on the default branch, a lookup that
/// failed, or the `deadline`.
fn peer_plan(peer: &Peer, deadline: std::time::Instant) -> Option<String> {
    if on_default_branch_by_files(peer) {
        return None;
    }
    let (default, default_name) = crate::git::default_ref(&peer.worktree, deadline)?;
    if peer.branch == default_name {
        return None;
    }
    crate::git::active_added_plans(&peer.worktree, &default, "HEAD", deadline)
        .into_iter()
        .next()
        .map(|plan| plan.title)
}

/// Whether the peer's branch is its repository's default, read from files
/// with no git process: `refs/remotes/origin/HEAD` in the common dir, else the
/// usual names. `false` means "not known", and the caller asks git.
///
/// Without `origin/HEAD`, `master` is the default only when no `main` exists:
/// `git::default_ref` prefers `main`, and this shortcut must agree with it.
fn on_default_branch_by_files(peer: &Peer) -> bool {
    let Some(common) = crate::activity::resolve(&peer.worktree).map(|place| place.repo_identity)
    else {
        return false;
    };
    let origin_head = std::fs::read_to_string(common.join("refs/remotes/origin/HEAD")).ok();
    match origin_head
        .as_deref()
        .and_then(|text| text.trim().strip_prefix("ref: refs/remotes/origin/"))
    {
        Some(name) => peer.branch == name,
        None => match peer.branch.as_str() {
            "main" => true,
            "master" => !ref_exists_by_files(&common, "main"),
            _ => false,
        },
    }
}

/// Whether `refs/heads/<name>` or `refs/remotes/origin/<name>` exists, loose
/// or packed, in the common dir. A ref that cannot be read counts as present,
/// so a doubt leaves the answer to git.
fn ref_exists_by_files(common: &std::path::Path, name: &str) -> bool {
    let refs = [
        format!("refs/heads/{name}"),
        format!("refs/remotes/origin/{name}"),
    ];
    if refs.iter().any(|r| common.join(r).exists()) {
        return true;
    }
    match std::fs::read_to_string(common.join("packed-refs")) {
        Ok(packed) => packed.lines().any(|line| {
            line.split_once(' ')
                .is_some_and(|(_, r)| refs.iter().any(|want| want == r))
        }),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// The one-line advice, naming every live peer (at most three, freshest first)
/// and the plan each one's branch added; the plan lookups share one 2 s budget
/// and a peer reached after it is named without a plan.
pub fn phrase(repo_name: &str, peers: &[Peer], now: std::time::SystemTime) -> String {
    const NAMED: usize = 3;
    let deadline = std::time::Instant::now() + PLAN_BUDGET;
    let named: Vec<String> = peers
        .iter()
        .take(NAMED)
        .map(|peer| {
            let plan = peer_plan(peer, deadline)
                .map(|title| format!(", active plan \"{title}\""))
                .unwrap_or_default();
            format!(
                "session `{}` (worktree `{}`, branch `{}`{plan}, last seen {} ago)",
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
        assert!(!text.contains("active plan"), "{text}");
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
