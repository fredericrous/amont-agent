//! `branch-force-delete` — `git branch -D` on work that exists nowhere else.
//!
//! `-d` refuses to delete a branch whose commits are not reachable from its
//! upstream or from HEAD, and that refusal is the point: it is the one
//! moment git says "these commits are on no other branch". `-D` skips it.
//! After a merged pull request the branch is on `origin/main` and `-D` is
//! harmless — which is how it becomes the default spelling, so that the one
//! time the PR was never opened, the branch goes with the reflog as the only
//! way back. Measured over forty-two sessions: 213 forced deletions.
//!
//! ## `confirm` asks where else the commits are
//!
//! `examine` fires on every `-D`; `confirm` asks whether the branch's tip is
//! contained in any remote-tracking branch or in the remote's default
//! branch, and stays silent when it is — the merged case.
//!
//! Ships observing: the shape is common and mostly harmless, and this crate
//! measures before it speaks.

use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "branch-force-delete",
    default_stance: Stance::Observe,
    evidence: Evidence {
        per_1000: 11.0,
        measured: "2026-09-05",
        trend: Trend::Flat(8),
    },
    examine,
    confirm: Some(confirm),
};

fn branches_of(cmd: &Simple) -> Vec<String> {
    if cmd.program() != Some("git") || cmd.subcommand() != Some("branch") {
        return Vec::new();
    }
    let forced = cmd.has_short('D')
        || ((cmd.has_flag("--delete") || cmd.has_short('d')) && cmd.has_flag("--force"));
    if !forced {
        return Vec::new();
    }
    cmd.operands()
        .iter()
        .skip(1)
        .filter(|w| !w.expanded)
        .map(|w| w.text.clone())
        .collect()
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.clauses() {
        let branches = branches_of(cmd);
        if branches.is_empty() {
            continue;
        }
        return Some(Finding {
            reason: format!(
                "`git branch -D` deletes {} without checking that its commits exist \
                 anywhere else; `-d` refuses exactly the deletions that would lose work, \
                 and the reflog is then the only way back.",
                branches.join(", ")
            ),
            remedy: "Use `-d`, which refuses only an unmerged branch, or check first: \
                     `git branch -r --contains <branch>` names the remote branches that \
                     already hold its commits."
                .to_string(),
            span: cmd.at..cmd.end,
        });
    }
    None
}

/// The remote's default branch, as `origin/HEAD` says it, or the usual
/// names when it does not.
fn default_base(cwd: &std::path::Path) -> Option<String> {
    for candidate in ["origin/HEAD", "origin/main", "origin/master"] {
        if crate::git::succeeds_in(
            cwd,
            &[
                "rev-parse",
                "-q",
                "--verify",
                &format!("{candidate}^{{commit}}"),
            ],
        ) {
            return Some(candidate.to_string());
        }
    }
    None
}

fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    let branches: Vec<String> = ctx
        .parsed
        .clauses()
        .iter()
        .find(|c| c.at == f.span.start)
        .map(branches_of)
        .unwrap_or_default();
    if branches.is_empty() {
        return Confirmed::No("the command no longer matches");
    }
    let cwd = ctx.cwd_at(f.span.start);
    if !cwd.is_dir() {
        return Confirmed::No("the directory the command moves to does not exist");
    }
    let base = default_base(&cwd);
    for b in &branches {
        if !crate::git::succeeds_in(
            &cwd,
            &["rev-parse", "-q", "--verify", &format!("refs/heads/{b}")],
        ) {
            continue; // no such branch: git will say so itself
        }
        let on_remote = crate::git::stdout_in(&cwd, &["branch", "-r", "--contains", b])
            .is_some_and(|s| !s.trim().is_empty());
        if on_remote {
            continue;
        }
        let merged = base.as_deref().is_some_and(|base| {
            crate::git::succeeds_in(&cwd, &["merge-base", "--is-ancestor", b, base])
        });
        if !merged {
            return Confirmed::Yes;
        }
    }
    Confirmed::No("every branch named is on a remote or merged")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn names(command: &str) -> Vec<String> {
        lex(command)
            .clauses()
            .iter()
            .flat_map(branches_of)
            .collect()
    }

    #[test]
    fn a_forced_delete_names_its_branches() {
        assert_eq!(names("git branch -D feat/x"), vec!["feat/x"]);
        assert_eq!(
            names("git worktree remove ../x && git branch -D feat/y fix/z 2>&1 | tail -1"),
            vec!["feat/y", "fix/z"]
        );
        assert_eq!(names("git branch --delete --force feat/z"), vec!["feat/z"]);
    }

    #[test]
    fn a_safe_delete_and_other_verbs_are_silent() {
        assert!(names("git branch -d feat/x").is_empty());
        assert!(names("git push origin --delete feat/x").is_empty());
        assert!(names("git branch -r --contains HEAD").is_empty());
        assert!(names("git branch -vv").is_empty());
        assert!(names("git branch -D").is_empty());
    }
}
