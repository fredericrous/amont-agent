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
//! ## Ships at `Advise`, ceiling `Deny`
//!
//! The fleet pattern. A person who wants the refusal sets
//! `amont.agent.publish-without-skill.stance deny`; `observe` turns it off.

use crate::publish_cmd::{self, Kind};
use crate::rules::{Confirmed, Context, Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;
use crate::skill_window::{self, Window};

pub const RULE: Rule = Rule {
    id: "publish-without-skill",
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-10-07",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: Some(confirm),
};

fn examine(parsed: &Parsed) -> Option<Finding> {
    let p = publish_cmd::classify(parsed)?;
    Some(Finding {
        reason: missing(p.kind, &p.what, None),
        remedy: remedy(p.kind.skill()),
        span: p.span,
    })
}

/// What the publish does, and that the skill was not called.
fn missing(kind: Kind, what: &str, ago: Option<u32>) -> String {
    let skill = kind.skill();
    let when = match ago {
        None => "no call of it is in this turn or the one before".to_string(),
        Some(n) => format!("its last call was {n} prompts ago"),
    };
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
        floor: None,
        ceiling: Some(Stance::Advise),
        reason: format!(
            "Whether the {skill} skill ran cannot be told ({why}), and `{}` publishes: \
             make sure its steps were followed.",
            p.what
        ),
        excerpt: format!("skill={skill} window=unknown:{tag} {}", p.what),
    };
    if !installed(skill, ctx.cwd) {
        return unknown("it is not installed here", "not-installed");
    }
    let mut best: Option<Window> = None;
    let mut failed: Option<&'static str> = None;
    for path in [ctx.transcript, ctx.agent_transcript].into_iter().flatten() {
        match std::fs::File::open(path)
            .map_err(|_| "the transcript could not be opened")
            .and_then(|f| skill_window::skill_window(std::io::BufReader::new(f), skill))
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
        Window::Stale(n) => Confirmed::YesSaying {
            floor: None,
            ceiling: None,
            reason: missing(p.kind, &p.what, Some(n)),
            excerpt: format!("skill={skill} window=stale-turn:{n} {}", p.what),
        },
        Window::Absent => Confirmed::YesSaying {
            floor: None,
            ceiling: None,
            reason: missing(p.kind, &p.what, None),
            excerpt: format!("skill={skill} window=no-skill {}", p.what),
        },
    }
}

/// Whether `skill` can be called here: a `SKILL.md` under the user's skills,
/// the project's (`.claude/skills` in the working directory or a parent), or
/// an installed plugin's. A refusal that asks for a skill this machine does
/// not have could not be obeyed.
fn installed(skill: &str, cwd: &std::path::Path) -> bool {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".claude")));
    let leaf = std::path::Path::new("skills").join(skill).join("SKILL.md");
    if let Some(config) = &config {
        if config.join(&leaf).is_file() {
            return true;
        }
    }
    if cwd
        .ancestors()
        .any(|d| d.join(".claude").join(&leaf).is_file())
    {
        return true;
    }
    // Plugins: `plugins/cache/<marketplace>/<plugin>/<version>/skills/…` and
    // `plugins/marketplaces/<marketplace>/…`. Walked to a bounded depth, and
    // only on a publish, which is a few commands in a thousand.
    config.is_some_and(|c| plugin_has(&c.join("plugins"), &leaf, 6))
}

fn plugin_has(dir: &std::path::Path, leaf: &std::path::Path, depth: u8) -> bool {
    if dir.join(leaf).is_file() {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries
        .flatten()
        .take(200)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| e.file_name() != "node_modules" && e.file_name() != ".git")
        .any(|e| plugin_has(&e.path(), leaf, depth - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_leads_with_the_skill() {
        // `amont-agent/publish-without-skill: ` then the reason: the skill
        // and the next step fit in the first 80 columns.
        let head = format!(
            "amont-agent/{}: {}",
            RULE.id,
            missing(Kind::PrMerge, "gh pr merge", None)
        );
        let first: String = head.chars().take(80).collect();
        assert!(
            first.contains("Call the merge-when-green skill first"),
            "{first}"
        );
    }

    #[test]
    fn a_stale_call_says_how_long_ago() {
        assert!(missing(Kind::TagPush, "v1.0.0", Some(3)).contains("3 prompts ago"));
    }
}
