//! `amend-pushed` — rewriting a commit the remote already has.
//!
//! `git commit --amend` replaces HEAD with a new commit. On a branch nobody
//! has seen that is tidy; on one already pushed it is history rewriting: the
//! next push is refused as non-fast-forward, the fix reached for is
//! `--force`, and anyone — a CI run, a worktree, a reviewer — holding the
//! old commit now holds one that no longer exists. Measured over forty-two
//! sessions: 35 amends.
//!
//! ## `confirm` asks whether the remote has HEAD
//!
//! `examine` fires on every `--amend`; `confirm` asks `git branch -r
//! --contains HEAD` and stays silent when no remote-tracking branch has the
//! commit — the local-only case, which is the safe one.

use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "amend-pushed",
    default_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 1.8,
        measured: "2026-09-05",
        trend: Trend::Rare,
    },
    examine,
    confirm: Some(confirm),
};

fn examine(parsed: &Parsed) -> Option<Finding> {
    let cmd = parsed.clauses().iter().find(|c| {
        c.program() == Some("git") && c.subcommand() == Some("commit") && c.has_flag("--amend")
    })?;
    Some(Finding {
        reason: "`--amend` replaces HEAD with a new commit; the old one is already on the \
                 remote, so the branch now needs a force-push and anything that fetched it \
                 — CI, a worktree, a reviewer — holds a commit that no longer exists."
            .to_string(),
        remedy: "Make a new commit instead (`git commit --fixup HEAD`, squashed before \
                 merge). If the branch is yours alone and must be rewritten, amend and push \
                 with `--force-with-lease`."
            .to_string(),
        span: cmd.at..cmd.end,
    })
}

fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    let cwd = ctx.cwd_at(f.span.start);
    if !cwd.is_dir() {
        return Confirmed::No("the directory the command moves to does not exist");
    }
    match crate::git::stdout_in(&cwd, &["branch", "-r", "--contains", "HEAD"]) {
        Some(s) if !s.trim().is_empty() => Confirmed::Yes,
        Some(_) => Confirmed::No("HEAD is on no remote branch"),
        None => Confirmed::No("not a git repository"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn an_amend_has_the_shape() {
        assert!(fires("git commit --amend --no-edit"));
        assert!(fires("git add f && git commit --amend -q -m \"fix: x\""));
        assert!(fires("cd ../x-wt-y && git commit --amend -F msg.txt"));
    }

    #[test]
    fn a_new_commit_is_not_an_amend() {
        assert!(!fires("git commit -m \"fix\""));
        assert!(!fires("git commit --fixup HEAD"));
        assert!(!fires("grep -rn \"commit --amend\" docs/"));
        assert!(!fires("git rebase -i --autosquash origin/main"));
    }
}
