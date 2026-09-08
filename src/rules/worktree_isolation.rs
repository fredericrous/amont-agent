//! `worktree-isolation` — starting task work in the shared checkout of a
//! repository that is already being worked in parallel.
//!
//! A working directory has one HEAD. When an agent and a person are both in
//! the primary checkout, a branch created or a `--hard` reset run by one of
//! them moves the ground under the other: uncommitted edits are carried onto
//! a branch they were never meant for, or reverted outright. Nothing reports
//! this. The first sign is a file that "went missing" or an edit that
//! "undid itself", found much later and usually blamed on the editor.
//!
//! ## What makes this a rule rather than advice in a prompt
//!
//! The failure is silent, which is this crate's admission test. `git` prints
//! nothing unusual — the checkout succeeds, the reset succeeds — so no
//! correcting loop can form from the outcome, and the convention is followed
//! or forgotten depending on whether anyone remembered to read it.
//!
//! ## `confirm` is what keeps it quiet
//!
//! `examine` fires on shape alone, and the shape is ordinary: creating a
//! branch is most of what anyone does. Two facts turn it into a finding, and
//! both are about the repository rather than the command:
//!
//! 1. the command runs in the **primary** checkout, not a linked worktree —
//!    `--git-dir` and `--git-common-dir` are the same path only there;
//! 2. that repository **already has linked worktrees**, which is the evidence
//!    that parallel work happens here and that there is someone to collide
//!    with. In a repository nobody else is standing in, this is not a mistake
//!    and the rule says nothing.
//!
//! Plain navigation — `git checkout main`, `git switch main` — is deliberately
//! not matched. Returning the shared checkout to its default branch is what
//! you do when you are *finished*, and firing there would put the rule in
//! front of the remedy.

use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "worktree-isolation",
    // Observing, which is where a rule goes to earn its case. The harm is
    // real and silent, but the shape is common enough that its rate has to be
    // looked at before it is allowed to speak: `Advise` would put text in
    // front of every branch creation in a primary checkout, and that text
    // would contaminate the very rate this is here to measure.
    default_stance: Stance::Observe,
    evidence: Evidence {
        // Backtested over 31,119 Bash calls / 302 transcripts. The shape is
        // flat across four full weeks — 5.6, 4.8, 3.3, 5.2 per thousand — so
        // it is not a habit correcting itself. The number prices `examine`
        // alone; `confirm` rejects the great majority of them, because most
        // are `cd <repo>-wt-<slug> && git checkout -b …`, which is the
        // convention being followed rather than broken.
        per_1000: 5.6,
        measured: "2026-09-08",
        trend: Trend::Flat(4),
    },
    examine,
    confirm: Some(confirm),
};

/// What the command is about to do to the shared HEAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Move {
    /// `git checkout -b` / `git switch -c` — the moment task work starts here.
    Creating(String),
    /// `git reset --hard` — the collision itself, not its prelude.
    HardReset,
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    let (cmd, mv) = detect(parsed)?;
    let reason = match &mv {
        Move::Creating(name) => format!(
            "`{name}` is being created in the checkout everything else here shares, \
             and a working directory has one HEAD — whatever else is uncommitted in \
             it comes along onto the new branch."
        ),
        Move::HardReset => "`git reset --hard` in the shared checkout discards \
             whatever is uncommitted there, including work that arrived from \
             somebody else's session rather than this one."
            .to_string(),
    };
    let remedy = match &mv {
        Move::Creating(name) => format!(
            "Give the task its own HEAD: `git fetch origin -q && git worktree add \
             ../<repo>-wt-<slug> -b {name} origin/main`, then work there."
        ),
        Move::HardReset => "Check whose work is there first — `git status --short` — \
             and if the intent was a clean start, take it in a new worktree off \
             `origin/main` instead of resetting this one."
            .to_string(),
    };
    Some(Finding {
        reason,
        remedy,
        span: cmd.at..cmd.end,
    })
}

/// The first clause that creates a branch or resets the working tree.
pub fn detect(parsed: &Parsed) -> Option<(&Simple, Move)> {
    for cmd in parsed.clauses() {
        if cmd.program() != Some("git") || cmd.is_dry_run() {
            continue;
        }
        // `let else` rather than `?`: a bare `git` mid-script must skip this
        // clause, not abandon the scan of the ones after it.
        let Some(sub) = cmd.subcommand() else {
            continue;
        };
        let mv = match sub {
            "checkout" => creating(cmd, &["-b", "-B"], &[]),
            "switch" => creating(cmd, &["-c", "-C"], &["--create", "--force-create"]),
            "reset" if cmd.has_flag("--hard") => Some(Move::HardReset),
            _ => None,
        };
        if let Some(mv) = mv {
            return Some((cmd, mv));
        }
    }
    None
}

/// `git checkout -b <name>` / `git switch -c <name>`, and their `-B`/`-C`
/// force-creating spellings.
fn creating(cmd: &Simple, shorts: &[&str], longs: &[&str]) -> Option<Move> {
    if cmd.has_flag("--detach") {
        return None;
    }
    let sub = cmd.subcommand()?;
    let mut words = cmd.words.iter().skip_while(|w| w.text != sub).skip(1);
    while let Some(w) = words.next() {
        if w.quoted {
            continue; // a quoted `-b` is an operand, not a flag
        }
        let t = w.text.as_str();
        if shorts.contains(&t) || longs.contains(&t) {
            let name = words.next()?;
            // A name that came out of a substitution is unknowable; quoting
            // the whole command in the finding would name a variable, not a
            // branch.
            if name.expanded {
                return None;
            }
            return Some(Move::Creating(name.text.clone()));
        }
    }
    None
}

/// The shape is ordinary; the repository is what makes it a finding.
fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    let cwd = ctx.cwd_at(f.span.start);
    let cwd = cwd.as_path();
    if !cwd.is_dir() {
        return Confirmed::No("the directory the command moves to does not exist");
    }
    let (Some(git_dir), Some(common)) = (
        crate::git::stdout_in(cwd, &["rev-parse", "--git-dir"]),
        crate::git::stdout_in(cwd, &["rev-parse", "--git-common-dir"]),
    ) else {
        return Confirmed::No("not a git repository");
    };
    // In a linked worktree these differ — `.git/worktrees/<name>` against
    // `.git`. Equal means the primary checkout, which is the shared one.
    if git_dir != common {
        return Confirmed::No("already in a linked worktree");
    }
    let Some(list) = crate::git::stdout_in(cwd, &["worktree", "list", "--porcelain"]) else {
        return Confirmed::No("git could not list the worktrees");
    };
    // The primary itself is always one line. Anything beyond it is evidence
    // that this repository is worked in parallel, and therefore that the
    // shared checkout has someone to collide with.
    if list.lines().filter(|l| l.starts_with("worktree ")).count() < 2 {
        return Confirmed::No("nothing else is checked out from this repository");
    }
    Confirmed::Yes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn move_of(command: &str) -> Option<Move> {
        detect(&lex(command)).map(|(_, m)| m)
    }

    #[test]
    fn creating_a_branch_names_it() {
        assert_eq!(
            move_of("git checkout -b feat/thing"),
            Some(Move::Creating("feat/thing".into()))
        );
        assert_eq!(
            move_of("git switch -c fix/x origin/main"),
            Some(Move::Creating("fix/x".into()))
        );
        assert_eq!(
            move_of("git switch --create fix/x"),
            Some(Move::Creating("fix/x".into()))
        );
        assert_eq!(
            move_of("cd ~/Developer/Perso/homelab && git checkout -B chore/bump"),
            Some(Move::Creating("chore/bump".into()))
        );
    }

    #[test]
    fn a_hard_reset_is_the_collision_itself() {
        assert_eq!(
            move_of("git reset --hard origin/main"),
            Some(Move::HardReset)
        );
        assert_eq!(move_of("git reset --hard"), Some(Move::HardReset));
    }

    /// Navigation is not task work, and the remedy must not trip the rule.
    #[test]
    fn navigation_and_the_remedy_stay_silent() {
        for c in [
            "git checkout main",
            "git switch main",
            "git checkout -- src/lib.rs",
            "git checkout --detach origin/main",
            "git reset --soft HEAD~1",
            "git reset HEAD src/lib.rs",
            "git worktree add ../x -b feat/y origin/main",
            "git fetch origin -q && git worktree add ../amont-wt-thing -b fix/z origin/main",
            "git branch -D feat/old",
            "git status --short",
        ] {
            assert_eq!(move_of(c), None, "{c}");
        }
    }

    /// A branch name out of a substitution cannot be quoted back at anyone.
    #[test]
    fn an_unknowable_branch_name_is_not_guessed() {
        assert_eq!(move_of("git checkout -b $(date +%s)"), None);
    }

    /// A `git` with no subcommand must not abandon the rest of the script.
    #[test]
    fn a_bare_git_does_not_end_the_scan() {
        assert_eq!(
            move_of("git; git checkout -b feat/after"),
            Some(Move::Creating("feat/after".into()))
        );
    }
}
