//! `whole-file-dump` — a file poured into the tool result with no window.
//!
//! ```sh
//! cat app/lib/landscapeDoc.ts          # 56 KB into context
//! sed -n '1,400p' crates/x/src/lib.rs   # four hundred lines, most unread
//! ```
//!
//! Measured over 41,700 tool calls (2026-09-09): files opened through
//! `cat`, `sed -n` and `head` were 31.5% of every byte the tools returned,
//! four times the Read tool's share, and the largest single family in the
//! transcripts. Nothing fails; the bytes simply arrive, and every later
//! turn of the session pays for them again. The Read tool returns the same
//! text with line numbers, a size cap and `offset`/`limit`.
//!
//! `examine` fires on the shape — a reader whose output goes nowhere but
//! the tool result (see [`crate::rules::dump`]). `confirm` looks at the file:
//! a whole dump of anything over 4 KB, or a bounded read of a hundred lines
//! or more, is what this rule is about; a `cat` of a 300-byte config is not.

use crate::rules::dump::{dumps, Extent};
use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "whole-file-dump",
    default_stance: Stance::Advise,
    evidence: Evidence {
        // 7,278 dumps in 41,700 calls; 1,571 of them unbounded over a file
        // the Read tool would have windowed.
        per_1000: 174.5,
        measured: "2026-09-09",
        trend: Trend::Flat(4),
    },
    examine,
    confirm: Some(confirm),
};

/// Below this a dump costs less than the advice about it.
const SMALL: u64 = 4096;
/// A bounded read this long is a dump with extra steps.
const MANY_LINES: u64 = 100;

fn examine(parsed: &Parsed) -> Option<Finding> {
    let all = dumps(parsed);
    let d = all.iter().find(|d| match d.extent {
        Extent::Whole => true,
        Extent::Lines(n) => n >= MANY_LINES,
        Extent::Bytes(n) => n >= 2 * SMALL,
    })?;
    Some(Finding {
        reason: format!(
            "`{}` is poured whole into the tool result: no line numbers, no size cap, and \
             every later turn of this session carries it again. Files opened this way were \
             31% of all tool-result bytes measured.",
            d.path
        ),
        remedy: format!(
            "Open it with the Read tool, with `offset`/`limit` around the part you need — \
             `grep -n '<anchor>' {} | head` finds the line first. `cat` is fine inside a pipe \
             or a heredoc.",
            d.path
        ),
        span: d.at..d.end,
    })
}

fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    let cwd = ctx.cwd_at(f.span.start);
    let all = dumps(ctx.parsed);
    let Some(d) = all.iter().find(|d| d.at == f.span.start) else {
        return Confirmed::No("the command no longer matches");
    };
    let path = if d.path.starts_with('/') {
        std::path::PathBuf::from(&d.path)
    } else if let Some(rest) = d.path.strip_prefix("~/") {
        match std::env::var_os("HOME") {
            Some(h) => std::path::PathBuf::from(h).join(rest),
            None => return Confirmed::No("no home to expand `~` against"),
        }
    } else {
        cwd.join(&d.path)
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return Confirmed::No("the file is not there to read");
    };
    match d.extent {
        Extent::Whole if meta.len() >= SMALL => Confirmed::Yes,
        Extent::Whole => Confirmed::No("the file is small"),
        Extent::Lines(_) | Extent::Bytes(_) => Confirmed::Yes,
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
    fn a_whole_or_long_dump_fires() {
        assert!(fires("cat app/lib/landscapeDoc.ts"));
        assert!(fires("cd /x && cat CLAUDE.md"));
        assert!(fires("cat -n src/main.rs"));
        assert!(fires("sed -n '1,400p' crates/x/src/lib.rs"));
        assert!(fires("head -200 build.log"));
        assert!(fires("tail -n 300 build.log"));
    }

    #[test]
    fn a_window_a_pipe_or_an_edit_is_silent() {
        assert!(!fires("sed -n '115,140p' app/x.tsx"));
        assert!(!fires("head -20 build.log"));
        assert!(!fires("tail -5 build.log"));
        assert!(!fires("cat f.rs | grep -n fn"));
        assert!(!fires("cat a.txt > b.txt"));
        assert!(!fires("cat > notes.md <<'EOF'\nhi\nEOF\n"));
        assert!(!fires("sed -i 's/a/b/' f.rs"));
        assert!(!fires("cat $F"));
        assert!(!fires("git log --oneline | head"));
    }
}
