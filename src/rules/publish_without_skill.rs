//! `publish-without-skill` — a release tag pushed, or a pull request merged,
//! with no call of the skill that carries the procedure.
//!
//! Two skills hold the steps a release and a merge take here: `tag-release`
//! (HEAD advanced before the tag is cut, then the PUBLISHED ARTEFACT checked,
//! not the workflow; ADR-0008 `release.trigger`) and `merge-when-green` (the
//! checks polled to completion and their conclusion read as its own step;
//! `ci.gates-the-merge`). Both are named in the standing instructions, and on
//! 2026-10-07 a session still cut relais v0.10.0 by hand: every merge and the
//! tag push done from memory, which is the copy that skips steps. Nothing in
//! the outcome showed it — the release was fine — so no correcting loop could
//! form, which is this crate's admission test.
//!
//! `release-tag-push` and `forge-merge-by-hand` already see these commands and
//! judge the command itself. This rule judges what came before it: whether the
//! skill was called in this turn or the human turn before it
//! ([`crate::skill_window`]). The window allows one prompt in between because
//! both skills ask the person to confirm, and a "status?" typed during a poll
//! is not a new task.
//!
//! ## Never refuses on not knowing
//!
//! A transcript that is missing, unreadable or still being written, or a skill
//! that is not installed on this machine, leaves the question open. The rule
//! then advises — capped by `ceiling` — whatever its stance: an unreadable
//! file must never block a release.
//!
//! ## Measured 2026-10-07, and shipping at `Advise`
//!
//! `tools/skill-rate.py` over 79,267 Bash calls: 1,560 publishes. Of those,
//! 270 had no skill call in the transcript before them, 909 had one two or
//! more prompts back, 82 were covered only by the one-prompt allowance, and
//! 299 had one in the same turn. By week, uncovered publishes per 1,000
//! calls:
//!
//! ```text
//! 2026-08-24  17.3     2026-09-21  14.2
//! 2026-08-31  28.3     2026-09-28  11.6
//! 2026-09-07  16.7     2026-10-05   8.4
//! 2026-09-14  19.9
//! ```
//!
//! Falling, and still roughly one Bash call in 120. Most of the misses are
//! "stale", not "never": the skill was called earlier in the session, and
//! every merge after it ran from memory. That is the 2026-10-07 incident.
//!
//! It ships at `Advise`, the fleet pattern, with a `Deny` ceiling. A person
//! who wants the refusal sets
//! `amont.agent.publish-without-skill.stance deny`, and `observe` turns it off.

use crate::publish_cmd::{self, Kind};
use crate::rules::{Confirmed, Context, Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;
use crate::skill_window::{self, Window};

pub const RULE: Rule = Rule {
    id: "publish-without-skill",
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        // p95 of the weekly uncovered rate (weeks of 300+ Bash calls).
        per_1000: 28.3,
        measured: "2026-10-07",
        trend: Trend::Improving,
    },
    examine: Examine::Legacy(examine),
    confirm: Some(confirm),
};

fn examine(parsed: &Parsed) -> Option<Finding> {
    let p = publish_cmd::classify(parsed)?;
    Some(Finding {
        reason: missing(p.kind, &p.what),
        remedy: remedy(p.kind.skill()),
        span: p.span,
    })
}

/// What the publish does, and that the skill was not called.
fn missing(kind: Kind, what: &str) -> String {
    let skill = kind.skill();
    let when = "no call of it is in this turn or the one before";
    match kind {
        Kind::TagPush => format!(
            "Call the {skill} skill first: `{what}` publishes a release, and {when}. \
             The skill holds the steps memory skips: HEAD advanced before the tag \
             was cut, then the published artefact checked, not the workflow \
             (ADR-0008, release.trigger)."
        ),
        Kind::PrMerge => format!(
            "Call the {skill} skill first: `{what}` merges a pull request, and {when}. \
             The skill holds the steps memory skips: the checks polled to \
             completion, their conclusion read as its own step, then the merge \
             (ci.gates-the-merge)."
        ),
    }
}

fn remedy(skill: &str) -> String {
    format!(
        "Run the Skill tool with `{skill}`, follow it, then re-run this command. If \
         the skill does not apply here, ask the person; they can run `git config \
         --global amont.agent.publish-without-skill.stance observe`."
    )
}

fn confirm(ctx: &Context, _finding: &Finding) -> Confirmed {
    let Some(p) = publish_cmd::classify(ctx.parsed) else {
        return Confirmed::No("not a publish");
    };
    let skill = p.kind.skill();
    let unknown = |why: &str, tag: &str| Confirmed::YesSaying {
        remedy: None,
        floor: None,
        ceiling: Some(Stance::Advise),
        reason: format!(
            "Whether the {skill} skill ran cannot be told ({why}), and `{}` publishes: \
             make sure its steps were followed.",
            p.what
        ),
        excerpt: format!("skill={skill} window=unknown:{tag} {}", p.what),
    };
    // The directory the publish runs in, `cd`s followed: a project skill
    // lives in that repository, not in the session's directory.
    match installed(skill, &ctx.cwd_at(p.span.start)) {
        Found::Yes => {}
        Found::No => return unknown("it is not installed here", "not-installed"),
        Found::Unreadable => {
            return unknown("the skill folders could not be read", "skills-unreadable");
        }
    }
    let mut best: Option<Window> = None;
    let mut failed: Option<&'static str> = None;
    for path in [ctx.transcript, ctx.agent_transcript].into_iter().flatten() {
        match std::fs::File::open(path)
            .map_err(|_| "the transcript could not be opened")
            .and_then(|f| skill_window::skill_window(f, skill))
        {
            Ok(w) => best = Some(best.map_or(w, |b| b.nearer(w))),
            Err(why) => failed = Some(why),
        }
    }
    let window = match (best, failed) {
        (Some(w), _) if w.covers() => w,
        (_, Some(why)) => return unknown(why, "transcript"),
        (Some(w), None) => w,
        (None, None) => return unknown("the session named no transcript", "no-transcript"),
    };
    match window {
        Window::Current => Confirmed::No("skill called in this turn"),
        Window::Previous => Confirmed::No("skill called in the previous turn"),
        Window::Outside => Confirmed::YesSaying {
            remedy: None,
            floor: None,
            ceiling: None,
            reason: missing(p.kind, &p.what),
            excerpt: format!("skill={skill} window=outside {}", p.what),
        },
    }
}

/// Whether a skill's `SKILL.md` was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    Yes,
    No,
    /// A folder that should have been searched could not be read, so "not
    /// installed" cannot be claimed.
    Unreadable,
}

/// Whether `skill` can be called here: a `SKILL.md` under the user's skills,
/// the project's (`.claude/skills` in the working directory or a parent), or
/// an installed plugin's. A refusal that asks for a skill this machine does
/// not have could not be obeyed.
fn installed(skill: &str, cwd: &std::path::Path) -> Found {
    let config = crate::settings::config_dir();
    let leaf = std::path::Path::new("skills").join(skill).join("SKILL.md");
    if let Some(config) = &config {
        if config.join(&leaf).is_file() {
            return Found::Yes;
        }
    }
    if cwd
        .ancestors()
        .any(|d| d.join(".claude").join(&leaf).is_file())
    {
        return Found::Yes;
    }
    // Plugins: `plugins/cache/<marketplace>/<plugin>/<version>/skills/…` and
    // `plugins/marketplaces/<marketplace>/…`. Walked to a bounded depth, and
    // only on a publish, which is a few commands in a thousand.
    match config {
        Some(c) => plugin_has(&c.join("plugins"), &leaf, 6),
        None => Found::No,
    }
}

/// At most this many subfolders are followed per folder: a plugin cache is a
/// handful of marketplaces, plugins and versions, never thousands.
const PLUGIN_FANOUT: usize = 200;

fn plugin_has(dir: &std::path::Path, leaf: &std::path::Path, depth: u8) -> Found {
    if dir.join(leaf).is_file() {
        return Found::Yes;
    }
    if depth == 0 {
        return Found::No;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Found::No,
        Err(_) => return Found::Unreadable,
    };
    let mut found = Found::No;
    let mut followed = 0;
    for entry in entries {
        let Ok(entry) = entry else {
            found = Found::Unreadable;
            continue;
        };
        let name = entry.file_name();
        if !entry.file_type().is_ok_and(|t| t.is_dir()) || name == "node_modules" || name == ".git"
        {
            continue;
        }
        if followed == PLUGIN_FANOUT {
            break;
        }
        followed += 1;
        match plugin_has(&entry.path(), leaf, depth - 1) {
            Found::Yes => return Found::Yes,
            Found::Unreadable => found = Found::Unreadable,
            Found::No => {}
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugin_skill_is_found_past_many_files() {
        let root = std::env::temp_dir().join(format!("pws-plugins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let market = root.join("cache/market");
        // More files than the fan-out, which counts folders only.
        std::fs::create_dir_all(&market).unwrap();
        for i in 0..(PLUGIN_FANOUT + 5) {
            std::fs::write(market.join(format!("f{i}")), "").unwrap();
        }
        let skill = market.join("tools/1.0.0/skills/tag-release");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "").unwrap();
        let leaf = std::path::Path::new("skills/tag-release/SKILL.md");
        assert_eq!(plugin_has(&root, leaf, 6), Found::Yes);
        let other = std::path::Path::new("skills/merge-when-green/SKILL.md");
        assert_eq!(plugin_has(&root, other, 6), Found::No);
        assert_eq!(plugin_has(&root.join("absent"), leaf, 6), Found::No);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_refusal_leads_with_the_skill() {
        // `amont-agent/publish-without-skill: ` then the reason: the skill
        // and the next step fit in the first 80 columns.
        let head = format!(
            "amont-agent/{}: {}",
            RULE.id,
            missing(Kind::PrMerge, "gh pr merge")
        );
        let first: String = head.chars().take(80).collect();
        assert!(
            first.contains("Call the merge-when-green skill first"),
            "{first}"
        );
    }
}
