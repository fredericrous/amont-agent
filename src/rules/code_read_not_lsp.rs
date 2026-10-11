//! `code-read-not-lsp` — code Read whole where the LSP tool would answer.
//!
//! Measured 2026-09-01..10-11 (`/tmp/lspmeasure/rate.py`): whole-file Reads
//! of `.rs .go .py .ts/.js` files went from 6.9 to 21.2 per 1,000 tool calls
//! over five weeks, while the LSP tool was called 52 times in all. fleet-lsp
//! 0.5.2 tells every session and subagent to navigate with LSP; this rule is
//! the note repeated at the moment it is ignored.
//!
//! Like `read-unbounded-large`, `examine` never fires — the backtester sees
//! no Read calls — and the rule lives in the hook's file path, which calls
//! [`applies`] (pure) and then [`crate::lsp`] (the world). It yields to
//! `file-reread` and `persisted-output-dump`, and while it speaks,
//! `read-unbounded-large` does not.
//!
//! An Edit is accepted after a Read of another window of the same file
//! (checked live with Claude Code, 2026-10-11), so the remedy can say to Read
//! only the window about to change.

use std::path::Path;

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "code-read-not-lsp",
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        // W37–W41: 6.9, 15.5, 17.7, 11.2, 21.2 per 1,000 tool calls; any
        // size, so the 4 KB floor below makes the rate this rule fires at
        // lower.
        per_1000: 21.2,
        measured: "2026-10-11",
        trend: Trend::Flat(5),
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(_parsed: &Parsed) -> Option<Finding> {
    None
}

/// Below this the advice costs more than the read: the `whole-file-dump`
/// threshold.
const SMALL: u64 = 4096;

/// `Some(bytes)` when this Read has no window and names a regular code file
/// over [`SMALL`] that `read-unbounded-large` would not exempt. Stats the
/// file, never opens it.
pub fn applies(path: &Path, window: &str) -> Option<u64> {
    if window != "full" || !crate::lsp::is_code(&path.to_string_lossy()) {
        return None;
    }
    if crate::rules::read_unbounded_large::exempt(path) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    (meta.is_file() && meta.len() > SMALL).then_some(meta.len())
}

/// What the hook says: the mechanism, then the way through.
pub fn phrase(shown: &str, bytes: u64) -> (String, String) {
    (
        format!(
            "`{shown}` is code, {} KB, Read whole: every line lands in the context, \
             and every later turn carries it.",
            bytes.div_ceil(1024)
        ),
        "Ask LSP first: documentSymbol for the outline; goToDefinition, findReferences \
         or hover for one symbol. Then Read with offset/limit around that line; to edit, \
         Read the window you change. On an LSP error, apply its fix or say so."
            .to_string(),
    )
}

/// The refusal: the same mechanism, and the way through spelled out.
pub fn phrase_refusal(shown: &str, bytes: u64) -> (String, String) {
    let (reason, _) = phrase(shown, bytes);
    (
        reason,
        "Ask LSP about this file (documentSymbol), then Read it once LSP has answered: \
         the Read passes once LSP was asked, or if LSP errors. Then Read with offset/limit \
         around the line you need."
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("amont-agent-crnl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_whole_read_of_code_over_4_kb_applies() {
        let dir = scratch("applies");
        let rs = dir.join("lib.rs");
        std::fs::write(&rs, "x".repeat(5000)).expect("file");
        assert_eq!(applies(&rs, "full"), Some(5000));
        assert_eq!(applies(&rs, "0:40"), None, "a window");
        std::fs::write(&rs, "x".repeat(4096)).expect("file");
        assert_eq!(applies(&rs, "full"), None, "the floor is exclusive");
        let md = dir.join("notes.md");
        std::fs::write(&md, "x".repeat(9000)).expect("file");
        assert_eq!(applies(&md, "full"), None, "not code");
        let plan = dir.join("docs/plans/x.ts");
        std::fs::create_dir_all(plan.parent().expect("parent")).expect("dirs");
        std::fs::write(&plan, "x".repeat(9000)).expect("file");
        assert_eq!(
            applies(&plan, "full"),
            None,
            "exempt like read-unbounded-large"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_refusal_names_the_way_through() {
        let (_, remedy) = phrase_refusal("a.rs", 9000);
        assert!(remedy.contains("documentSymbol"), "{remedy}");
        assert!(remedy.contains("once LSP was asked"), "{remedy}");
        assert!(remedy.contains("if LSP errors"), "{remedy}");
    }

    #[test]
    fn the_advice_stays_under_sixty_words() {
        let (reason, remedy) = phrase("crates/relais/src/runner/mod.rs", 412_000);
        let words = reason.split_whitespace().count() + remedy.split_whitespace().count();
        assert!(words <= 60, "{words} words");
        assert!(reason.contains("403 KB"), "{reason}");
    }
}
