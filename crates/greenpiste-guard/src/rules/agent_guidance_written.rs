//! `agent-guidance-written` — a Write, Edit or MultiEdit to a file that an
//! agent reads as guidance.
//!
//! The `guidance.*` and `prose.*` rules (ADR-0026, ADR-0032) govern such
//! files: skills, the always-on `CLAUDE.md`, `AGENTS.md` and `MEMORY.md`, and
//! subagent definitions. The advice names the command that lists them.
//!
//! Like `lint-suppression-added`, `examine` never fires — the backtester sees
//! no Edit or Write calls — and the rule lives in the hook's file path, which
//! calls [`is_guidance`] (pure) and [`phrase`]. The hook tells it once per
//! session and file; later writes are journalled only.
//!
//! Memory files other than `MEMORY.md` are exempt: they are detail fetched on
//! demand, not text every session loads.

use std::path::Path;

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "agent-guidance-written",
    prevents: "Guidance an agent reads being edited without the guidance rules that govern it being checked",
    irreversible: false,
    default_stance: Stance::Advise,
    max_stance: Stance::Advise,
    evidence: Evidence {
        // `tools/agent-guidance-rate.py`, 131,285 tool calls: 86 writes to
        // guidance files (SKILL.md 29, CLAUDE.md 27, MEMORY.md 24, other 6).
        per_1000: 0.66,
        measured: "2026-10-13",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(_parsed: &Parsed) -> Option<Finding> {
    None
}

/// Is the file one that agents read as guidance?
pub fn is_guidance(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if matches!(
        name,
        "SKILL.md" | "CLAUDE.md" | "CLAUDE.md.tmpl" | "AGENTS.md" | "MEMORY.md"
    ) {
        return true;
    }
    if !name.ends_with(".md") {
        return false;
    }
    let mut parents = path.components().rev().skip(1);
    let dir = parents.next().and_then(|c| c.as_os_str().to_str());
    let up = parents.next().and_then(|c| c.as_os_str().to_str());
    dir == Some("agents") && matches!(up, Some(".claude") | Some("dot_claude"))
}

/// The one-line advice.
pub fn phrase(shown: &str) -> String {
    let file = Path::new(shown)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(shown);
    crate::decision::phrase(
        RULE.id,
        &format!("`{file}` is agent guidance: the `guidance.*` and `prose.*` rules apply (ADR-0026, ADR-0032)."),
        &format!("`aval relevant --path {shown}` lists them."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guidance_paths_are_recognised() {
        for p in [
            "a/SKILL.md",
            "CLAUDE.md",
            "dot_claude/CLAUDE.md.tmpl",
            "r/AGENTS.md",
            "m/MEMORY.md",
            "r/.claude/agents/reviewer.md",
            "r/dot_claude/agents/reviewer.md",
        ] {
            assert!(is_guidance(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn other_files_are_not_guidance() {
        for p in [
            "src/main.rs",
            "m/feedback_x.md",
            "README.md",
            "r/agents/reviewer.md",
            ".claude/agents/notes.txt",
            ".claude/agents",
        ] {
            assert!(!is_guidance(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn the_advice_names_the_file_the_adrs_and_the_command() {
        let t = phrase("/r/skills/x/SKILL.md");
        assert!(t.contains("`SKILL.md`"), "{t}");
        assert!(t.contains("ADR-0026") && t.contains("ADR-0032"), "{t}");
        assert!(
            t.contains("aval relevant --path /r/skills/x/SKILL.md"),
            "{t}"
        );
    }
}
