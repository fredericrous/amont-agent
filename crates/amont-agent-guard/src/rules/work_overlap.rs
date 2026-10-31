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

/// The title of the first plan (by file name) under `docs/plans` of the
/// worktree whose front matter says `status: active`, read from disk; `None`
/// when there is none. A pointer file that says active counts.
pub fn active_plan_title(worktree: &std::path::Path) -> Option<String> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(worktree.join("docs").join("plans"))
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    files.iter().find_map(|file| {
        let text = std::fs::read_to_string(file).ok()?;
        plan_title(&text)
    })
}

fn plan_title(text: &str) -> Option<String> {
    let rest = text.strip_prefix("---\n")?;
    let (front, body) = rest.split_once("\n---\n")?;
    let active = front.lines().any(|line| {
        line.split_once(':').is_some_and(|(key, value)| {
            key.trim() == "status" && value.split('#').next().unwrap_or("").trim() == "active"
        })
    });
    if !active {
        return None;
    }
    let mut fenced = false;
    body.lines().find_map(|line| {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            return None;
        }
        if fenced {
            return None;
        }
        line.strip_prefix("# ")
            .map(|title| title.trim().to_string())
    })
}

/// The one-line advice, naming every live peer (at most three, freshest first).
pub fn phrase(repo_name: &str, peers: &[Peer], now: std::time::SystemTime) -> String {
    const NAMED: usize = 3;
    let named: Vec<String> = peers
        .iter()
        .take(NAMED)
        .map(|peer| {
            let plan = active_plan_title(&peer.worktree)
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

    #[test]
    fn the_title_is_the_first_active_plan_h1_by_file_name() {
        let dir = std::env::temp_dir().join(format!("wo-plan-{}", std::process::id()));
        let plans = dir.join("docs").join("plans");
        std::fs::create_dir_all(&plans).expect("plans dir");
        std::fs::write(plans.join("a.md"), "---\nstatus: done\n---\n# Old\n").expect("a");
        std::fs::write(plans.join("c.md"), "---\nstatus: active\n---\n# Third\n").expect("c");
        std::fs::write(
            plans.join("b.md"),
            "---\nstatus: active\n---\n```\n# no\n```\n# Second\n",
        )
        .expect("b");
        assert_eq!(active_plan_title(&dir).as_deref(), Some("Second"));
        std::fs::remove_dir_all(&dir).expect("cleanup");
        assert_eq!(active_plan_title(&dir), None);
    }
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
