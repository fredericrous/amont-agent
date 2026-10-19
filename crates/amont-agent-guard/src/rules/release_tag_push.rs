//! `release-tag-push` — pushing a version tag, which publishes.
//!
//! In every repository here a `v*` tag drives a build-and-publish workflow.
//! `git push origin v1.2.3` is therefore not a bookkeeping step: it is the
//! release. What comes back is a workflow conclusion, and a workflow's
//! `success` means only that no step exited non-zero — the artefact it
//! uploaded can still be wrong.
//!
//! Two ways it is wrong, both seen:
//!
//! - the tag names the WRONG COMMIT, because the commit before it was refused
//!   and the tag landed on the previous HEAD. `tag-after-commit` catches that
//!   when the two are chained; typed as separate commands, nothing does.
//! - the workflow is green and the artefact is stale — a chart publish that
//!   merged an old manifest, a Flux reconcile still serving the prior
//!   revision.
//!
//! On an immutable registry the mistake cannot be withdrawn. npm and crates.io
//! will not take a version back; only the next version supersedes it, and
//! every consumer that already resolved is left on the bad one.
//!
//! ## Why the moment of the push
//!
//! Everything useful about a release is verifiable only afterwards, and
//! "afterwards" is exactly when a session declares itself finished. Said at
//! the push, the reminder still has somewhere to go. Said later, there is no
//! later.
//!
//! ## Measured 2026-09-10, and shipping at `Observe` for what it showed
//!
//! 178 matches in 34,950 Bash calls, per 1,000: `6.9, 4.6, 6.3, 3.8`. Noisy,
//! not climbing — which is a different animal from `forge-merge-by-hand`, whose
//! rate rose twentyfold over the same window and earned `Advise` on it.
//!
//! The stronger reason to stay quiet is what this rule matches. Pushing a
//! version tag is not a mistake; it is the correct final step of a release, and
//! this fires on every one of them, the careful ones included. `Advise` would
//! put a paragraph in front of an action that is usually right, which is a
//! checklist rather than a defect detector — and the crate's own position is
//! that a rule that talks is intervening.
//!
//! It is kept for cost-of-a-miss rather than frequency: an immutable registry
//! cannot take a version back. `Observe` is where that case gets built, with a
//! real rate underneath it, and `amont-agent graduate release-tag-push --to
//! advise` is one command if the journal later says the follow-through is
//! actually being skipped.
//!
//! ## No `confirm`
//!
//! Whether a `v*` tag drives CI in THIS repository is a question about a
//! workflow file, and the honest answer needs the remote's default branch, not
//! the checkout's. That is a round-trip on a path that runs before every shell
//! command. The reason states the condition instead.

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "release-tag-push",
    default_stance: Stance::Observe,
    max_stance: Stance::Deny,
    evidence: Evidence {
        // p95 of the weekly rate. `Rare` rather than `Flat`: at ~5 per
        // thousand the weeks are too noisy to call a trend, and the reason to
        // keep it is the cost of one miss, not how often it happens.
        per_1000: 6.9,
        measured: "2026-09-10",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.judgeable() {
        let Some((what, span)) = crate::publish_cmd::tag_push(cmd) else {
            continue;
        };
        return Some(Finding {
            reason: format!(
                "`{what}` publishes. A `v*` tag drives the build-and-publish workflow, \
                 and that workflow reporting `success` means only that no step exited \
                 non-zero — the artefact it uploaded can still be stale, or built from \
                 the wrong commit if the tag landed on the previous HEAD."
            ),
            remedy: "Use the tag-release skill: confirm HEAD actually advanced before \
                     the tag was cut, then verify the PUBLISHED ARTEFACT — the image \
                     digest, `npm view <pkg>@<version>`, the release assets — not the \
                     workflow's conclusion. On npm or crates.io a bad version cannot be \
                     withdrawn, only superseded."
                .to_string(),
            span,
        });
    }
    None
}
