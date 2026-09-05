//! `push-preflight` — a `git push` whose pre-push test gate has not been
//! rehearsed.
//!
//! ```sh
//! git push -u origin feat/thing      # gate runs for 4 min INSIDE the push
//! ```
//!
//! git opens its connection to the remote *before* it runs `pre-push`, then
//! holds that connection idle for as long as the gate takes. A four-minute
//! suite is longer than some remotes keep an idle session — Forgejo's own git
//! command timeout is six minutes; a cloud front door may be shorter — and
//! the push then dies AFTER the gate passed: `Connection closed by … port 22`
//! over SSH, `RPC failed; HTTP 504` over HTTPS. Measured on 2026-09-04: three
//! pushes in a row, each one paying the full suite and each one dying on the
//! wire, before a `--no-verify` retry of the already-attested tree went
//! through. That retry is the failure this rule exists to prevent — the gate
//! bypassed for a reason that had nothing to do with the gate.
//!
//! ## The remedy is amont's, this rule only points at it
//!
//! amont 1.27 stamps the tree a passed push gate ran against, and `amont run
//! pre-push` drives the same gate with no connection open and stamps `HEAD`.
//! A push whose tips carry the stamp skips the suite and holds the remote for
//! the seconds the transport takes. So: rehearse, then push.
//!
//! ## What is not a fire
//!
//! `examine` fires on the shape of a push. `confirm` is where the facts live,
//! and it stays silent unless all three hold: the repository is one amont
//! guards (an amont shim in `hooks/pre-push`), amont would actually run a
//! test gate on this push (`amont list --json --stage pre-push`), and `HEAD`'s
//! tree carries no push stamp yet. A `--dry-run` sends nothing. A push that
//! only carries a notes ref is amont's own attestation push. A push that
//! already skips its hooks is `no-verify`'s business, not this rule's. And a
//! push of `main` or `master`, or of tags, is not judged: amont's
//! branch-protect refuses the former before any gate runs, and a tag carries
//! content the branch push already proved — both are also the ordinary
//! shapes a shape-only `examine` must stay silent on.

use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "push-preflight",
    // Advises from the start: it refuses nothing, it speaks only when
    // `confirm` has established that a slow gate is about to run inside the
    // transport window, and the failure it names is one no correcting loop
    // can see — the push fails for a reason the model reads as "network".
    default_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-09-04",
        trend: Trend::Rare,
    },
    examine,
    confirm: Some(confirm),
};

/// The push clause, if this command has one worth judging.
fn detect(parsed: &Parsed) -> Option<&Simple> {
    parsed.clauses().iter().find(|cmd| {
        cmd.program() == Some("git")
            && cmd.subcommand() == Some("push")
            && !cmd.has_flag("--dry-run")
            && !cmd.has_short('n')
            && !cmd.has_flag("--no-verify")
            && !cmd.has_flag("--tags")
            && !cmd.has_flag("--delete")
            && !cmd.has_short('d')
            // operands()[0] is `push`; the rest name a remote and refspecs.
            && !cmd.operands().iter().skip(1).any(|w| not_a_branch_push(&w.text))
    })
}

/// A refspec this rule leaves alone: amont's own notes push, a tag, the
/// default branch, or a deletion.
fn not_a_branch_push(refspec: &str) -> bool {
    let dst = refspec.rsplit(':').next().unwrap_or(refspec);
    refspec.starts_with("refs/notes/")
        || refspec.starts_with("refs/tags/")
        || refspec.starts_with(':')
        || matches!(
            dst,
            "main" | "master" | "refs/heads/main" | "refs/heads/master"
        )
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    let cmd = detect(parsed)?;
    Some(Finding {
        reason: "git opens its connection to the remote BEFORE running pre-push and \
                 holds it idle while the test gate runs; a remote that closes idle \
                 sessions (Forgejo's git timeout is 6 minutes) kills the push after \
                 the gate has already passed."
            .to_string(),
        remedy: "Rehearse first: `amont rehearse --wait` runs the same gate on a \
                 snapshot of HEAD with no connection open and stamps the tree — or \
                 follows the rehearsal a commit already started — so this push then \
                 skips the suite and holds the remote for seconds (`amont run \
                 pre-push` on amont 1.27). Run it, read its verdict, then push."
            .to_string(),
        span: cmd.at..cmd.end,
    })
}

/// A test gate amont would run at push time here, by id.
fn slow_gate_would_run(cwd: &std::path::Path) -> bool {
    // `--pushed` measures the actual push (and, on amont ≥ 1.27, a never-
    // pushed branch against origin's default); an older amont refuses that
    // without an upstream, so fall back to the repository-level answer.
    let listed = list_json(cwd, true).or_else(|| list_json(cwd, false));
    let Some(json) = listed else { return false };
    // No JSON dependency in this crate: the shape is `"id":"…"` followed,
    // some fields later, by `"status":"runs"`. Walk id by id.
    json.split("\"id\":\"").skip(1).any(|entry| {
        let id = entry.split('"').next().unwrap_or_default();
        let status = entry
            .split("\"status\":\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or_default();
        status == "runs" && (TEST_GATES.contains(&id) || !entry.contains("\"source\":\"builtin\""))
    })
}

/// The built-in push gates that run a suite. A declared (non-builtin)
/// pre-push check counts too: those exist to run something slow.
const TEST_GATES: &[&str] = &[
    "pre-push-run-tests-js",
    "pre-push-cargo-test",
    "pre-push-go-test",
    "pre-push-pytest",
];

fn list_json(cwd: &std::path::Path, pushed: bool) -> Option<String> {
    let mut cmd = std::process::Command::new("amont");
    cmd.args(["list", "--json", "--stage", "pre-push"]);
    if pushed {
        cmd.arg("--pushed");
    }
    let out = cmd
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Does `HEAD`'s tree (or `HEAD` itself) already carry a push stamp?
fn head_is_stamped(cwd: &std::path::Path) -> bool {
    ["HEAD^{tree}", "HEAD"].iter().any(|key| {
        crate::git::stdout_in(cwd, &["notes", "--ref", "amont-gate", "show", key]).is_some_and(
            |note| {
                note.lines()
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .any(|t| t.starts_with("pre-push-"))
            },
        )
    })
}

fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    if detect(ctx.parsed).is_none() {
        return Confirmed::No("the command no longer matches");
    }
    let cwd = ctx.cwd_at(f.span.start);
    let cwd = cwd.as_path();
    if !cwd.is_dir() {
        return Confirmed::No("the directory the command moves to does not exist");
    }
    let Some(hook) = crate::git::stdout_in(cwd, &["rev-parse", "--git-path", "hooks/pre-push"])
    else {
        return Confirmed::No("not a git repository");
    };
    let hook_path = if std::path::Path::new(&hook).is_absolute() {
        std::path::PathBuf::from(&hook)
    } else {
        cwd.join(&hook)
    };
    let guarded = std::fs::read_to_string(&hook_path).is_ok_and(|s| s.contains("amont"));
    if !guarded {
        return Confirmed::No("amont does not guard this repository's pushes");
    }
    if head_is_stamped(cwd) {
        return Confirmed::No("HEAD's tree already carries a push stamp");
    }
    if !slow_gate_would_run(cwd) {
        return Confirmed::No("no test gate runs at push time here");
    }
    Confirmed::Yes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn a_plain_push_has_the_shape() {
        assert!(fires("git push"));
        assert!(fires("git push -u origin feat/x"));
        assert!(fires("cd ../repo && git push origin HEAD"));
    }

    #[test]
    fn a_dry_run_sends_nothing() {
        assert!(!fires("git push --dry-run origin main"));
        assert!(!fires("git push -n"));
    }

    #[test]
    fn a_push_that_skips_its_hooks_is_another_rules_business() {
        assert!(!fires("git push --no-verify origin feat/x"));
    }

    #[test]
    fn amonts_own_notes_push_is_not_judged() {
        assert!(!fires(
            "git push origin refs/notes/amont-attest:refs/notes/amont-attest"
        ));
    }

    #[test]
    fn the_default_branch_and_tags_are_not_judged() {
        // branch-protect refuses these before a gate could run.
        assert!(!fires("git push origin main"));
        assert!(!fires("git push origin HEAD:master"));
        // A tag carries a tree the branch push already proved.
        assert!(!fires("git push origin v2.2.0 --tags"));
        assert!(!fires("git push origin refs/tags/v2.2.0"));
        assert!(!fires("git push origin --delete feat/x"));
        assert!(!fires("git push origin :feat/x"));
    }

    #[test]
    fn other_git_verbs_are_silent() {
        assert!(!fires("git pull"));
        assert!(!fires("git fetch origin"));
        assert!(!fires("git log --oneline -1"));
    }

    #[test]
    fn the_span_is_the_push_clause() {
        let parsed = lex("git status && git push origin feat/x");
        let f = examine(&parsed).expect("fires");
        // A clause span starts at the separator's trailing whitespace, like
        // every other rule's; the excerpt is what matters.
        assert_eq!(
            "git status && git push origin feat/x"[f.span.clone()].trim(),
            "git push origin feat/x"
        );
    }
}
