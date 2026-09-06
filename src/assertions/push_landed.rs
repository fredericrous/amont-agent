//! `git push` exited 0. Is the ref actually on the remote?
//!
//! 2026-08-20, and again the day after: several pushes reported exit 0 and had
//! never left the machine. `git ls-remote` showed the remote still on the old
//! commit. One cause was a pipeline whose exit status belonged to `tail`, and
//! `pipe-to-tail` refuses that shape now — but the class is wider than its
//! causes. "Everything up-to-date" is exit 0. A push of a branch you were not
//! on is exit 0. A push to a remote you did not mean is exit 0.
//!
//! The check is one round trip, and the answer is a fact rather than advice:
//! the remote's SHA beside the local one. That is not something a model can
//! read as a suggestion.
//!
//! ## What it refuses to judge
//!
//! Only the unambiguous shapes. `HEAD:refs/heads/other`, a tag push, several
//! refspecs at once, `--delete`, `--mirror`, `--all`: each needs a different
//! question asked of the remote, and getting one of them subtly wrong produces
//! a confident false accusation. A wrong assertion is worse than no assertion —
//! it costs the channel its credibility, and the channel is the point.

use crate::assertions::{read_briefly, Assertion, Claim, Verdict};
use crate::rules::{Context, Evidence, Stance, Trend};
use crate::shell::{Parsed, Simple};

pub const ASSERTION: Assertion = Assertion {
    id: "push-landed",
    // Ships observing, and the number below is the cost of ASKING, not of
    // speaking: 1,007 pushes in 29,758 Bash calls across five weeks, so roughly
    // one call in twenty-six pays for a `git ls-remote`. How often the claim is
    // BROKEN is the question the journal answers going forward, because a
    // verdict cannot be replayed — the remotes have moved on since.
    default_stance: Stance::Observe,
    evidence: Evidence {
        per_1000: 38.7,
        measured: "2026-09-05",
        trend: Trend::Routine,
    },
    examine,
    verify,
};

/// What we managed to read out of the command.
struct Push<'a> {
    remote: Option<&'a str>,
    branch: Option<&'a str>,
    at: usize,
    end: usize,
}

/// Flags that change the QUESTION, not just the details. Any of them and we
/// have nothing to say.
const REWRITES_THE_QUESTION: &[&str] = &[
    "--delete",
    "--mirror",
    "--all",
    "--tags",
    "--follow-tags",
    "--prune",
];

fn read_push(parsed: &Parsed) -> Option<Push<'_>> {
    let Parsed::Clear(clauses) = parsed else {
        return None;
    };
    let cmd: &Simple = clauses
        .iter()
        .find(|c| c.program() == Some("git") && c.subcommand() == Some("push"))?;
    if cmd.is_dry_run() || cmd.has_short('n') {
        return None;
    }
    if REWRITES_THE_QUESTION.iter().any(|f| cmd.has_flag(f)) {
        return None;
    }

    // operands() drops the program and its flags, leaving `push`, then the
    // remote, then any refspecs.
    let operands: Vec<&str> = cmd
        .operands()
        .iter()
        .map(|w| w.text.as_str())
        .filter(|t| *t != "push")
        .collect();

    let (remote, branch) = match operands.as_slice() {
        [] => (None, None),
        [remote] => (Some(*remote), None),
        [remote, refspec] => {
            // A colon is a source:destination pair and a `+` is a force
            // refspec; both mean the local ref and the remote ref can differ,
            // which is a different question than the one below.
            if refspec.contains(':') || refspec.starts_with('+') {
                return None;
            }
            (Some(*remote), Some(*refspec))
        }
        // Several refspecs in one push: more than one claim, and this asserts
        // about one.
        _ => return None,
    };

    Some(Push {
        remote,
        branch,
        at: cmd.at,
        end: cmd.end,
    })
}

fn examine(parsed: &Parsed) -> Option<Claim> {
    let push = read_push(parsed)?;
    Some(Claim {
        what: "push".to_string(),
        span: push.at..push.end,
    })
}

fn verify(ctx: &Context, _claim: &Claim) -> Verdict {
    let Some(push) = read_push(ctx.parsed) else {
        return Verdict::Unknown("the push could not be read a second time");
    };
    let dir = ctx.cwd_at(push.at);
    if !dir.is_dir() {
        return Verdict::Unknown("the working directory is gone");
    }

    // A branch named on the command line wins; otherwise it is whatever HEAD
    // points at. A detached HEAD has no branch to compare and is not judged.
    let branch = match push.branch {
        Some(b) => b.to_string(),
        None => match read_briefly(&dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
            Some(b) if !b.is_empty() => b,
            _ => return Verdict::Unknown("HEAD is detached, so no branch was claimed"),
        },
    };

    // A tag and a branch of the same name are different refs on the remote.
    // Rather than guess, only judge a name that resolves as a local branch.
    let Some(local) = read_briefly(
        &dir,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
    ) else {
        return Verdict::Unknown("the pushed ref is not a local branch");
    };

    let remote = match push.remote {
        Some(r) => r.to_string(),
        None => read_briefly(&dir, &["config", &format!("branch.{branch}.remote")])
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "origin".to_string()),
    };
    // `git push /some/path` and `git push git@host:repo` are both legal. Only a
    // configured remote NAME is worth asking about by name.
    if remote.contains('/') || remote.contains(':') {
        return Verdict::Unknown("the push named a URL rather than a configured remote");
    }

    let refname = format!("refs/heads/{branch}");
    let Some(answer) = read_briefly(&dir, &["ls-remote", "--", &remote, &refname]) else {
        // Unreachable, unauthenticated, timed out: all the same answer here.
        // The push may well have landed; we simply cannot say.
        return Verdict::Unknown("the remote could not be reached without prompting");
    };

    let remote_sha = answer.split_whitespace().next().unwrap_or_default();
    if remote_sha == local {
        return Verdict::Held;
    }
    if remote_sha.is_empty() {
        return Verdict::Broken {
            reason: format!(
                "`git push` exited 0, but {remote} has no {refname}. The branch is not on the remote."
            ),
            remedy: format!(
                "Check `git ls-remote {remote} {refname}` before reporting this pushed."
            ),
        };
    }
    Verdict::Broken {
        reason: format!(
            "`git push` exited 0, but {remote}/{branch} is at {} while the local branch is at {}. The push did not land.",
            short(remote_sha),
            short(&local)
        ),
        remedy: "Run the push again on its own and read its output, then confirm with `git ls-remote`.".to_string(),
    }
}

fn short(sha: &str) -> String {
    sha.chars().take(9).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn claims(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn a_plain_push_is_a_claim() {
        assert!(claims("git push"));
        assert!(claims("git push origin"));
        assert!(claims("git push origin main"));
        assert!(claims("git push -u origin feat/x"));
    }

    /// Each of these asks the remote a different question than "is this branch
    /// at this commit", and a confident wrong answer is the one outcome worth
    /// avoiding.
    #[test]
    fn the_ambiguous_shapes_are_not_claims() {
        assert!(!claims("git push origin HEAD:refs/heads/other"));
        assert!(!claims("git push origin +main"));
        assert!(!claims("git push origin main next"));
        assert!(!claims("git push --delete origin old"));
        assert!(!claims("git push --mirror backup"));
        assert!(!claims("git push --tags"));
        assert!(!claims("git push --dry-run origin main"));
        assert!(!claims("git push -n origin main"));
    }

    #[test]
    fn other_commands_are_not_claims() {
        assert!(!claims("git pull"));
        assert!(!claims("git commit -m push"));
        assert!(!claims("echo git push"));
    }

    /// The excerpt has to be centred on the push, not on the head of whatever
    /// script it was buried in.
    #[test]
    fn the_span_covers_the_push_clause() {
        let command = "cargo build --release && git push origin main";
        let claim = examine(&lex(command)).expect("a claim");
        // The clause offset starts where the connector left off, so the slice
        // can carry the separating space — the same shape every rule's span has.
        assert_eq!(command[claim.span.clone()].trim(), "git push origin main");
    }
}
