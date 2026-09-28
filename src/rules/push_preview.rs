//! `push-preview` — a push that would publish interface changes no approved
//! preview covers (ADR-0023, `work.preview-before-publish`).
//!
//! ```sh
//! git push -u origin feat/settings-toggle   # app/routes/settings.tsx changed
//! ```
//!
//! Interface regressions kept reaching pull requests and deployed sites after
//! the agent had checked its own screen in a browser. The person's eyes are
//! the check that does not share the agent's blind spots, and localhost is
//! where rejecting a screen costs one sentence instead of a fix pull request
//! and a release. So a UI-changing commit is published only after the person
//! approved THAT commit — registered with `amont-agent preview register` and
//! answered on a marked question (see `crate::preview`).
//!
//! ## Where it speaks
//!
//! `examine` fires on the shape of a push. `confirm` resolves what the push
//! would publish (`crate::push_target`, a documented subset), and fires only
//! when all hold: the repository has a user interface (a `dev` script, or
//! `git config amont.agent.push-preview.ui true`), a pushed branch carries a
//! file under `app/`, `src/`, `web/` or a `.tsx/.jsx/.css/.html` whose
//! nearest `package.json` looks like an interface and whose diff is not
//! comments only (`crate::preview`), and that commit has no approval. A push this guard cannot read is journalled with
//! its shape and passes — except under `deny`, where it is held so that
//! `--all` is not the way around the gate.
//!
//! The rule runs BEFORE the command, so it speaks of what the push would do.
//! Whether a push actually published anything is `push-published`'s record.

use crate::rules::{Confirmed, Context, Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "push-preview",
    // Advises from the start, `confirm`-backed like `push-preflight`, and
    // under review: the soak decides between deny, advise, or retiring the
    // gate while keeping the verification loop (ADR-0023).
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-09-28",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: Some(confirm),
};

fn examine(parsed: &Parsed) -> Option<Finding> {
    let cmd = crate::push_target::find(parsed)?;
    if cmd.is_dry_run() || cmd.has_short('n') {
        return None;
    }
    // A push to the default branch is amont's `branch-protect` refusal
    // before any preview could matter, and an ordinary shape this rule must
    // stay silent on.
    if cmd
        .operands()
        .iter()
        .skip(2)
        .any(|w| to_default_branch(&w.text))
    {
        return None;
    }
    Some(Finding {
        reason: "This push would publish interface changes that no approved preview covers."
            .to_string(),
        remedy: "Verify the final commit in a real browser, serve the clean worktree, then run \
                 `amont-agent preview register --url <url> --attestation <file outside the worktree>` \
                 as its own command and, in the same turn, ask the marked question \
                 (`[preview <id>]`, listing each repo@sha, options exactly Approve / Request changes / Hold). \
                 Push after the person approves. A push shape this guard cannot read is held under deny: \
                 push the branch explicitly (`git push <remote> <branch>`)."
            .to_string(),
        span: cmd.at..cmd.end,
    })
}

fn to_default_branch(refspec: &str) -> bool {
    let dst = refspec.rsplit(':').next().unwrap_or(refspec);
    let dst = dst.strip_prefix('+').unwrap_or(dst);
    let dst = dst.strip_prefix("refs/heads/").unwrap_or(dst);
    matches!(dst, "main" | "master")
}

fn confirm(ctx: &Context, _finding: &Finding) -> Confirmed {
    let Some(cmd) = crate::push_target::find(ctx.parsed) else {
        return Confirmed::No("the push could not be read a second time");
    };
    let stance = crate::stance::resolve(&RULE);
    match crate::preview::needs_preview(&ctx.cwd_at(cmd.at), cmd, stance) {
        Ok(()) => Confirmed::Yes,
        Err(why) => Confirmed::No(why),
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
    fn a_dry_run_and_other_commands_are_not() {
        assert!(!fires("git push --dry-run origin feat/x"));
        assert!(!fires("git push -n origin feat/x"));
        assert!(!fires("git pull"));
        assert!(!fires("echo git push"));
        assert!(!fires("git push origin main"));
        assert!(!fires("git push origin HEAD:refs/heads/master"));
    }

    #[test]
    fn the_span_is_the_push_clause() {
        let command = "npm test && git push -u origin feat/x";
        let f = examine(&lex(command)).expect("fires");
        assert_eq!(command[f.span].trim(), "git push -u origin feat/x");
    }
}
