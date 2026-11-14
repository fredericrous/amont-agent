//! `agent-guidance-shape` — a guidance file written in a shape that costs
//! every session or hides the skill from the model.
//!
//! Two checks, both on the text the write produces (rebuilt by the hook with
//! `lint_suppression_added::reconstruct`, and only when that rebuild is exact):
//!
//! - a `SKILL.md` whose front matter lacks `disable-model-invocation: true`
//!   needs a one-line `description:` with a trigger clause, because the
//!   description is what the model matches a task against
//!   (`guidance.pointer-names-when`);
//! - an always-on file (`CLAUDE.md`, `CLAUDE.md.tmpl`, `AGENTS.md`,
//!   `MEMORY.md`) over [`BUDGET`] bytes that this write makes larger is an
//!   index that stopped being one (`guidance.always-on-is-an-index`). A
//!   ratchet: a write that shrinks the file or leaves its size passes, one
//!   that crosses the budget fires.
//!
//! The front matter is parsed by hand, with no YAML crate: the lines between
//! the first two `---` lines. `examine` never fires; the rule lives in the
//! hook's file path, which calls [`kind`] and [`check`] (both pure).

use std::path::Path;

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "agent-guidance-shape",
    prevents: "A guidance file written in a shape that costs every session or hides the skill from the model",
    irreversible: false,
    default_stance: Stance::Advise,
    // Advise until the index-rewrite plan brings MEMORY.md and CLAUDE.md
    // under budget: auto-memory grows MEMORY.md on every save.
    max_stance: Stance::Advise,
    evidence: Evidence {
        // `tools/agent-guidance-rate.py`, 131,285 tool calls: an upper bound,
        // every write to a SKILL.md or an always-on file (0.65); the rule
        // fires only on a failed check, once per session and file.
        per_1000: 0.65,
        measured: "2026-10-13",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(_parsed: &Parsed) -> Option<Finding> {
    None
}

/// The size, in UTF-8 bytes, above which an always-on file is no longer an
/// index.
pub const BUDGET: usize = 8192;

/// Whether a skill description names when to use it: a `Use <word>` clause
/// (`Use when`, `Use for`, `Use in plan mode`, …) or a `whenever` clause
/// (`Load this first … whenever …`). Both forms are measured triggers in the
/// fleet's own skills (guidance.pointer-names-when, ADR-0032).
///
/// holds-until: a description gets past it with a `Use` that names no case
/// ("Use the CLI.") — accepted, since the rule only advises and a reviewer
/// reads the description. Replace it with the trigger rate from
/// `skill-trigger-test` if such descriptions show up in the journal.
fn has_trigger(value: &str) -> bool {
    value.to_lowercase().contains("whenever")
        || value.match_indices("Use ").any(|(at, _)| {
            value[at + 4..]
                .chars()
                .next()
                .is_some_and(char::is_lowercase)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Skill,
    AlwaysOn,
}

/// Which check applies to the file, if any.
pub fn kind(path: &Path) -> Option<Kind> {
    match path.file_name().and_then(|n| n.to_str())? {
        "SKILL.md" => Some(Kind::Skill),
        "CLAUDE.md" | "CLAUDE.md.tmpl" | "AGENTS.md" | "MEMORY.md" => Some(Kind::AlwaysOn),
        _ => None,
    }
}

/// The advice for the text this write produces, or `None` when it passes.
pub fn check(path: &Path, kind: Kind, before: &str, after: &str) -> Option<String> {
    let shown = path.to_string_lossy();
    let file = path.file_name().and_then(|n| n.to_str()).unwrap_or(&shown);
    match kind {
        Kind::Skill => {
            let problem = skill_problem(after)?;
            Some(crate::decision::phrase(
                RULE.id,
                &format!("`{file}` {problem}, so the model cannot tell when to load it."),
                "Give it a one-line `description:` with a `Use when …` clause, or set \
`disable-model-invocation: true` (guidance.pointer-names-when).",
            ))
        }
        Kind::AlwaysOn => {
            let size = normalised(after).len();
            let was = normalised(before).len();
            if size <= BUDGET || size <= was {
                return None;
            }
            Some(crate::decision::phrase(
                RULE.id,
                &format!(
                    "`{file}` is loaded into every session and is now {size} bytes, over the \
{BUDGET} byte budget; this write adds {} bytes.",
                    size - was
                ),
                "Move the text behind a pointer: one line that names when to fetch the detail \
(guidance.always-on-is-an-index).",
            ))
        }
    }
}

/// The journal excerpt for a finding.
pub fn excerpt(shown: &str, kind: Kind) -> String {
    let what = match kind {
        Kind::Skill => "skill",
        Kind::AlwaysOn => "size",
    };
    format!("{what} {shown}")
}

fn normalised(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// What is wrong with a skill's front matter, if anything.
fn skill_problem(text: &str) -> Option<&'static str> {
    let text = normalised(text);
    let mut fences = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.trim_end() == "---")
        .map(|(i, _)| i);
    let front: Vec<&str> = match (fences.next(), fences.next()) {
        (Some(a), Some(b)) => text.lines().skip(a + 1).take(b - a - 1).collect(),
        _ => Vec::new(),
    };
    let mut description: Option<usize> = None;
    for (i, line) in front.iter().enumerate() {
        if let Some(value) = line.strip_prefix("disable-model-invocation:") {
            if value.trim().trim_matches(['"', '\'']) == "true" {
                return None;
            }
        } else if line.starts_with("description:") && description.is_none() {
            description = Some(i);
        }
    }
    let Some(at) = description else {
        return Some("has no `description:` in its front matter");
    };
    let value = front[at]["description:".len()..].trim();
    let continues = front
        .get(at + 1)
        .is_some_and(|l| l.starts_with(' ') || l.starts_with('\t'));
    if value.starts_with('>') || value.starts_with('|') {
        Some("has a block-scalar `description:`")
    } else if continues {
        Some("has a `description:` that spans more than one line")
    } else if value.is_empty() {
        Some("has an empty `description:`")
    } else if !has_trigger(value) {
        Some("has a `description:` with no `Use when …` trigger clause")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str =
        "---\nname: x\ndescription: Does x. Use when the person asks for x.\n---\nbody\n";

    fn skill(text: &str) -> Option<String> {
        check(Path::new("a/SKILL.md"), Kind::Skill, "", text)
    }

    #[test]
    fn a_one_line_description_with_a_trigger_passes() {
        assert_eq!(skill(GOOD), None);
    }

    #[test]
    fn every_trigger_form_counts() {
        for d in [
            "Does x. Use when y.",
            "Does x. Use whenever y.",
            "Does x. Use for y.",
            "Does x. Use at the start of a task.",
            "Does x. Use before a push.",
            "Does x. Use after a merge.",
            "Does x. Use proactively, without being asked.",
            "Run the panel. Use in plan mode every time a plan is presented.",
            "Load this FIRST, before any command, whenever a bug's cause is not known yet.",
        ] {
            let text = format!("---\ndescription: {d}\n---\n");
            assert_eq!(skill(&text), None, "{d}");
        }
    }

    #[test]
    fn use_followed_by_a_name_is_not_a_trigger() {
        assert!(skill("---\ndescription: Does x. Use Foo.\n---\n").is_some());
    }

    #[test]
    fn a_description_without_a_trigger_is_flagged() {
        let t = skill("---\nname: x\ndescription: Does x.\n---\n").expect("flagged");
        assert!(
            t.contains("agent-guidance-shape") && t.contains("trigger"),
            "{t}"
        );
        assert!(t.contains("guidance.pointer-names-when"), "{t}");
    }

    #[test]
    fn block_and_multi_line_descriptions_are_flagged() {
        assert!(skill("---\ndescription: >\n  Use when y.\n---\n").is_some());
        assert!(skill("---\ndescription: |\n  Use when y.\n---\n").is_some());
        assert!(skill("---\ndescription: Does x.\n  Use when y.\n---\n").is_some());
        assert!(skill("---\ndescription:\n  Use when y.\n---\n").is_some());
    }

    #[test]
    fn a_missing_description_or_front_matter_is_flagged() {
        assert!(skill("---\nname: x\n---\n").is_some());
        assert!(skill("# Title\n").is_some());
    }

    #[test]
    fn disable_model_invocation_exempts_the_skill() {
        assert_eq!(skill("---\ndisable-model-invocation: true\n---\n"), None);
        assert_eq!(
            skill("---\ndescription: >\n  x\ndisable-model-invocation: \"true\"\n---\n"),
            None
        );
        assert!(skill("---\ndisable-model-invocation: false\n---\n").is_some());
    }

    fn always(before: usize, after: usize) -> Option<String> {
        check(
            Path::new("CLAUDE.md"),
            Kind::AlwaysOn,
            &"x".repeat(before),
            &"x".repeat(after),
        )
    }

    #[test]
    fn growing_past_the_budget_is_flagged_with_numbers() {
        let t = always(9000, 9100).expect("flagged");
        assert!(
            t.contains("9100") && t.contains("8192") && t.contains("100 bytes"),
            "{t}"
        );
        assert!(t.contains("guidance.always-on-is-an-index"), "{t}");
    }

    #[test]
    fn crossing_the_budget_is_flagged_and_a_new_file_starts_at_zero() {
        assert!(always(7000, 9000).is_some());
        assert!(always(0, 9000).is_some());
    }

    #[test]
    fn shrinking_staying_equal_or_staying_under_passes() {
        assert_eq!(always(9000, 8900), None);
        assert_eq!(always(9000, 9000), None);
        assert_eq!(always(100, 8192), None);
    }

    #[test]
    fn crlf_counts_as_one_byte() {
        let before = "a\n".repeat(4000);
        let after = "a\r\n".repeat(4096);
        assert_eq!(
            check(Path::new("MEMORY.md"), Kind::AlwaysOn, &before, &after),
            None
        );
    }

    #[test]
    fn kinds() {
        assert_eq!(kind(Path::new("x/SKILL.md")), Some(Kind::Skill));
        assert_eq!(kind(Path::new("CLAUDE.md.tmpl")), Some(Kind::AlwaysOn));
        assert_eq!(kind(Path::new("feedback.md")), None);
        assert_eq!(kind(Path::new(".claude/agents/a.md")), None);
    }
}
