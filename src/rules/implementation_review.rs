//! `implementation-review` — a push of a branch that carries a plan, whose
//! diff no independent reviewer has read (ADR-0022,
//! `work.implementation-review`).
//!
//! ```sh
//! git push -u origin feat/toast   # docs/plans/2026-09-29-toast.md on the branch
//! ```
//!
//! The quality loop was the author checking its own work: a browser pass or
//! a piloted run, then the tests, then CI. Nothing independent read the
//! diff, so what an author is blind to went through — code drifting from the
//! plan's Verification section, a swallowed error, a disabled lint, a test
//! that cannot fail, scope past the Non-goals. One reviewer, findings only,
//! reads the diff against the plan and the active rules before the push,
//! and its verdict is bound to the tree it read (see
//! `crate::implementation_review`).
//!
//! ## Where it speaks
//!
//! `examine` fires on the shape of a push. `confirm` resolves the push
//! (`crate::push_target`), declines silently when the branch carries no
//! plan or the tree was reviewed, and speaks when the review is missing,
//! stale, said `rework`, or cannot be told — naming the tree, and the next
//! step. The pass-file store is guarded outside this rule's stance, by the
//! hook itself (`crate::hook`), because only the hook writes there.
//!
//! Ships `advise`. The 2026-10-06 soak review reads the journal's
//! `review=` field and decides between `deny`, `advise` and retiring it.

use crate::rules::{Confirmed, Context, Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: crate::implementation_review::RULE_ID,
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-09-29",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: Some(confirm),
};

const REMEDY: &str = "worktree-task F4b: `amont-agent tree-sha --block`, then launch the \
                      implementation-review agent with the plan, the diff and the active rules; \
                      a stale or rework review gets ONE delta with the round-1 findings, and a \
                      rework that survives it goes to the person on a marked question \
                      (`[implementation-review <repo>@<sha>]`, options exactly Overrule / Fix / Hold).";

fn examine(parsed: &Parsed) -> Option<Finding> {
    if let Some(cmd) = crate::push_target::find(parsed) {
        if cmd.is_dry_run() || cmd.has_short('n') {
            return None;
        }
        // A push to the default branch is amont's `branch-protect` refusal
        // before any review could matter.
        if cmd
            .operands()
            .iter()
            .skip(2)
            .any(|w| to_default_branch(&w.text))
        {
            return None;
        }
        return Some(Finding {
            reason:
                "This push carries a plan whose implementation no independent reviewer has read."
                    .to_string(),
            remedy: REMEDY.to_string(),
            span: cmd.at..cmd.end,
        });
    }
    None
}

fn to_default_branch(refspec: &str) -> bool {
    let dst = refspec.rsplit(':').next().unwrap_or(refspec);
    let dst = dst.strip_prefix('+').unwrap_or(dst);
    let dst = dst.strip_prefix("refs/heads/").unwrap_or(dst);
    matches!(dst, "main" | "master")
}

fn confirm(ctx: &Context, _finding: &Finding) -> Confirmed {
    let stance = crate::stance::resolve(&RULE);
    match crate::push_target::find(ctx.parsed) {
        Some(cmd) => crate::implementation_review::confirm_push(ctx, cmd, stance),
        None => Confirmed::No("the push could not be read a second time"),
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
    fn a_push_is_examined() {
        assert!(fires("git push -u origin feat/x"));
        assert!(fires("cd ../wt && git push origin HEAD:feat/x"));
        assert!(fires("git -C ../wt push origin feat/x"));
    }

    #[test]
    fn a_dry_run_the_default_branch_and_other_commands_are_not() {
        assert!(!fires("git push --dry-run origin feat/x"));
        assert!(!fires("git push -n origin feat/x"));
        assert!(!fires("git pull"));
        assert!(!fires("echo git push"));
        assert!(!fires("git push origin main"));
        assert!(!fires("git push origin HEAD:refs/heads/master"));
    }
}
