//! `tag-after-commit` — a release tag chained onto a commit that may not
//! have happened.
//!
//! `git commit … && git tag vX && git push origin vX` reads as one step and
//! is three. If the commit is refused — a commit-msg hook, a pre-commit
//! gate, nothing staged — `&&` stops the chain, but `;` does not, and either
//! way the retry often becomes `git tag vX` on its own, now pointing at the
//! PREVIOUS commit. A tag-driven release then builds the new version number
//! from stale code, and an immutable registry (npm, crates.io) cannot take
//! it back; only the next version can supersede it. That happened once, and
//! it is why `~/.claude/CLAUDE.md` says to commit, confirm HEAD moved, then
//! tag. Measured over forty-two sessions: 78 commit-and-tag chains.
//!
//! The failure is silent by construction: every command in the chain
//! reports success, and the tag really does exist.

use crate::rules::{Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "tag-after-commit",
    // Advises from the start: pure shape, and the cost of a miss is a
    // published artefact nobody can unpublish.
    default_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 4.0,
        measured: "2026-09-05",
        trend: Trend::Rare,
    },
    examine,
    confirm: None,
};

fn is_commit(cmd: &Simple) -> bool {
    cmd.program() == Some("git") && cmd.subcommand() == Some("commit")
}

/// `git tag <name>` creating a tag — not listing, deleting or verifying one.
fn creates_tag(cmd: &Simple) -> bool {
    if cmd.program() != Some("git") || cmd.subcommand() != Some("tag") {
        return false;
    }
    if cmd.has_flag("--list")
        || cmd.has_flag("--delete")
        || cmd.has_flag("--verify")
        || cmd.has_flag("--contains")
        || cmd.has_flag("--points-at")
        || cmd.has_short('l')
        || cmd.has_short('d')
        || cmd.has_short('v')
        || cmd.has_short('n')
    {
        return false;
    }
    // The tag name is the first operand after `tag`.
    cmd.operands().len() >= 2
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    let clauses = parsed.clauses();
    let commit = clauses.iter().position(is_commit)?;
    let tag = clauses.iter().skip(commit + 1).find(|c| creates_tag(c))?;
    Some(Finding {
        reason: "`git tag` names whatever HEAD is when it runs; if the commit before it \
                 is refused by a hook, the shell carries on (or the retry runs `git tag` \
                 alone) and the tag lands on the previous commit — a release built from \
                 stale code, which an immutable registry cannot take back."
            .to_string(),
        remedy: "Commit as its own step, confirm HEAD moved (`git log --oneline -1`), then \
                 tag that commit by hash and push the tag."
            .to_string(),
        span: clauses[commit].at..tag.end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn a_commit_then_tag_chain_has_the_shape() {
        assert!(fires(
            "git commit -q -m \"chore(release): 1.2.3\" && git tag v1.2.3 && git push origin v1.2.3"
        ));
        assert!(fires("git add -A && git commit -m x; git tag -a v2 -m v2"));
        assert!(fires(
            "cd /r && git commit -F msg && git tag v0.1.0 && git push --tags"
        ));
    }

    #[test]
    fn tagging_an_existing_commit_is_the_remedy() {
        assert!(!fires("git tag v0.10.0 a278ecb && git push origin v0.10.0"));
        assert!(!fires("git tag v1.28.0 3c7a31d && git push origin v1.28.0"));
    }

    #[test]
    fn listing_or_deleting_after_a_commit_is_not_tagging() {
        assert!(!fires("git commit -m x && git tag -l | tail -3"));
        assert!(!fires("git commit -m x && git tag -d v1.9.0"));
        assert!(!fires("git commit -m x && git tag --contains HEAD"));
    }

    #[test]
    fn text_is_not_a_command() {
        assert!(!fires(
            "git commit -m \"tag: v1 && git tag v1\" && git push"
        ));
        assert!(!fires("git tag v1 && git commit --allow-empty -m 'after'"));
    }
}
