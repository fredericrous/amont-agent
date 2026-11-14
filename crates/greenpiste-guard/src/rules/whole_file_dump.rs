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
use crate::rules::{Confirmed, Context, Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "whole-file-dump",
    prevents: "A file poured into the tool result with no window, most of it unread",
    irreversible: false,
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        // 7,278 dumps in 41,700 calls; 1,571 of them unbounded over a file
        // the Read tool would have windowed.
        per_1000: 174.5,
        measured: "2026-09-09",
        trend: Trend::Flat(4),
    },
    examine: Examine::Legacy(examine),
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

/// Past this a whole dump may be refused (once: the hook lets the same
/// command through when it is repeated); a smaller one, or a bounded read,
/// only ever advises. The same threshold as `read-unbounded-large`, measured
/// for the Read tool's whole reads (deliberate-output plan, Phase 3).
const REFUSABLE: u64 = 8 * 1024;

/// The file a finding dumps: where it is, resolved against the directory its
/// clause runs in, the path as the command wrote it, and how much is read.
pub struct Target {
    pub path: std::path::PathBuf,
    pub shown: String,
    pub extent: Extent,
}

/// The [`Target`] of `finding`. Fails when the command no longer holds that
/// dump, or a `~/` path has no `HOME` to expand against.
pub fn target(context: &Context, finding: &Finding) -> Result<Target, &'static str> {
    let cwd = context.cwd_at(finding.span.start);
    let all = dumps(context.parsed);
    let Some(dump) = all.into_iter().find(|d| d.at == finding.span.start) else {
        return Err("the command no longer matches");
    };
    let path = if dump.path.starts_with('/') {
        std::path::PathBuf::from(&dump.path)
    } else if let Some(rest) = dump.path.strip_prefix("~/") {
        match std::env::var_os("HOME") {
            Some(home) => std::path::PathBuf::from(home).join(rest),
            None => return Err("no home to expand `~` against"),
        }
    } else {
        cwd.join(&dump.path)
    };
    Ok(Target {
        path,
        shown: dump.path,
        extent: dump.extent,
    })
}

/// Whether a dump may be refused: a whole read of a regular file over
/// [`REFUSABLE`]: `regular_len` is its size when it is a regular file,
/// `None` for a FIFO, a device or a directory. Anything else is held at
/// advise, whatever the stance.
fn refusable(extent: Extent, regular_len: Option<u64>) -> bool {
    matches!(extent, Extent::Whole) && regular_len.is_some_and(|len| len > REFUSABLE)
}

fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    let Target {
        path,
        shown,
        extent,
    } = match target(ctx, f) {
        Ok(found) => found,
        Err(why) => return Confirmed::No(why),
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return Confirmed::No("the file is not there to read");
    };
    if matches!(extent, Extent::Whole) && meta.len() < SMALL {
        return Confirmed::No("the file is small");
    }
    let may_refuse = refusable(extent, meta.is_file().then_some(meta.len()));
    // A dump that may be refused says why this one and not a smaller one.
    let reason = if may_refuse {
        format!(
            "{} It is {} KB, over the 8 KB past which a whole dump is refused.",
            f.reason,
            meta.len().div_ceil(1024)
        )
    } else {
        f.reason.clone()
    };
    // Code LSP answers for is found with LSP, not with the grep this
    // rule's remedy names for any other file.
    let remedy = (crate::lsp::is_code(&shown) && crate::lsp::serves(&path).is_ok()).then(|| {
        "Ask LSP first (`documentSymbol`, `goToDefinition`), then open the file with \
         the Read tool, with `offset`/`limit` around that line. `cat` is fine inside \
         a pipe or a heredoc."
            .to_string()
    });
    Confirmed::YesSaying {
        floor: None,
        ceiling: (!may_refuse).then_some(Stance::Advise),
        reason,
        excerpt: shown,
        remedy,
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

    /// Only a whole read of a regular file past 8 KiB may be refused; the
    /// threshold itself, a bounded read and a directory never.
    #[test]
    fn only_a_whole_read_of_a_large_regular_file_is_refusable() {
        assert!(!refusable(Extent::Whole, Some(8_192)));
        assert!(refusable(Extent::Whole, Some(8_193)));
        assert!(!refusable(Extent::Whole, None), "not a regular file");
        assert!(!refusable(Extent::Lines(400), Some(20_480)));
        assert!(!refusable(Extent::Bytes(20_000), Some(20_480)));
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
