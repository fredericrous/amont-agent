//! `persisted-output-dump` — reading back an output that was too big once.
//!
//! When a tool result is too large, the harness saves it to a file under
//! `tool-results/` and returns the path. Reading that file back whole pays
//! for the same output twice; measured, 31 of 83 persisted results were read
//! back within five calls. The part that was wanted is almost always a
//! handful of lines: `grep`, `head -c`, or a Read with `offset`/`limit`.

use crate::rules::dump::{dumps, Extent};
use crate::rules::{Evidence, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "persisted-output-dump",
    default_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 0.7,
        measured: "2026-09-09",
        trend: Trend::Rare,
    },
    examine,
    confirm: None,
};

pub fn is_persisted(path: &str) -> bool {
    path.contains("/tool-results/")
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    let all = dumps(parsed);
    // A projection — a window of lines or bytes — is the remedy, not the shape.
    let d = all.iter().find(|d| {
        is_persisted(&d.path)
            && match d.extent {
                Extent::Whole => true,
                Extent::Lines(n) => n >= 100,
                Extent::Bytes(n) => n >= 8192,
            }
    })?;
    Some(Finding {
        reason: reason(),
        remedy: remedy(),
        span: d.at..d.end,
    })
}

pub fn reason() -> String {
    "this is a tool result the harness saved to a file because it was too large for the \
     context; reading it back whole pays for that output a second time."
        .to_string()
}

pub fn remedy() -> String {
    "Take the part that is wanted: `grep -n '<pattern>' <file> | head`, `head -c 2000 <file>`, \
     or Read with `offset`/`limit`."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn a_saved_result_read_whole_fires() {
        assert!(fires(
            "cat /Users/x/.claude/projects/p/s/tool-results/bb3af8fql.txt"
        ));
        assert!(fires(
            "head -200 /Users/x/.claude/projects/p/s/tool-results/bb3af8fql.txt"
        ));
    }

    #[test]
    fn a_projection_of_it_is_silent() {
        assert!(!fires(
            "grep -n conclusion /Users/x/.claude/projects/p/s/tool-results/a.txt"
        ));
        assert!(!fires(
            "head -c 2000 /Users/x/.claude/projects/p/s/tool-results/a.txt | head"
        ));
        assert!(!fires(
            "cat /Users/x/.claude/projects/p/s/tool-results/a.txt | grep -c error"
        ));
        assert!(!fires("cat src/main.rs"));
    }
}
