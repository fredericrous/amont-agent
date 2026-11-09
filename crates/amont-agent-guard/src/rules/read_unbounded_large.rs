//! `read-unbounded-large` — a Read with no window over a large file.
//!
//! Measured over 93,890 tool calls (2026-10-05): Read results over 16 KB were
//! 37% of the Read tool's bytes, and 584 of the 812 had no `offset`/`limit`.
//! The model followed `whole-file-dump`'s advice by switching from `cat` to
//! Read, then read the whole file anyway.
//!
//! Like `file-reread`, `examine` never fires — the backtester sees no Read
//! calls — and the rule lives in the hook's file path, which calls the
//! helpers below. [`applies`] only stats the file and never opens it:
//! `metadata` does not block, and `is_file()` rejects a FIFO or a device.

use std::path::Path;

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "read-unbounded-large",
    prevents: "A Read with no window over a large file, pouring most of it into context unread",
    irreversible: false,
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        // `tools/read-rate.py`, with this rule's exemptions: 445 unbounded
        // Reads over 16 KB in 96,950 calls, W36–W41 at 5.0, 2.2, 5.6, 5.8,
        // 3.6, 3.3. Counted without the exemptions the tail looked like it
        // was rising (W40 7.8, W41 11.1); that was review agents reading
        // plans whole, which is exempt.
        per_1000: 4.6,
        measured: "2026-10-05",
        trend: Trend::Flat(6),
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

fn examine(_parsed: &Parsed) -> Option<Finding> {
    None
}

/// Below this the advice costs more than the read. Measured over
/// [2026-10-23, 2026-11-06) (`amont-agent output-audit`): subagents' whole
/// Reads over 8 KiB were 1,711 calls and 40.4 MB, about 27% of every byte
/// the tools returned; over 16 KiB, half of that.
const LARGE: u64 = 8 * 1024;

/// `Some(bytes)` when this Read has no window, is not exempt, and names a
/// regular file over [`LARGE`]. A failed stat is silence.
pub fn applies(path: &Path, window: &str) -> Option<u64> {
    if window != "full" || exempt(path) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    (meta.is_file() && meta.len() > LARGE).then_some(meta.len())
}

pub fn exempt(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let is = |names: &[&str]| names.iter().any(|n| ext.eq_ignore_ascii_case(n));
    // Media the Read tool renders, not text: their size in bytes does not say
    // what lands in the context.
    if is(&["png", "jpg", "jpeg", "gif", "webp", "pdf", "ipynb"]) {
        return true;
    }
    // A diff or patch is handed to a reviewer to read in full.
    if is(&["diff", "patch"]) {
        return true;
    }
    // Plan-review and implementation-review agents are handed plans to read
    // whole; windowing them would review half a plan. Components, not
    // substrings, so `myplans/` is not a plans directory.
    let parts: Vec<_> = path.components().map(|c| c.as_os_str()).collect();
    if parts
        .windows(2)
        .any(|w| w[1] == "plans" && (w[0] == ".claude" || w[0] == "docs"))
    {
        return true;
    }
    // `persisted-output-dump` owns tool results.
    crate::rules::persisted_output_dump::is_persisted(&path.to_string_lossy())
}

/// How the remedy says to find the part of the file that is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finder {
    /// A code file LSP answers for. Shown only when `code-read-not-lsp` is
    /// observing, since that rule otherwise takes the Read.
    Lsp,
    Grep,
}

/// Whether `agent` is a reviewer: handed a brief, plan or diff to read whole,
/// so a whole Read is the job, not a lapse. Measured 2026-11-09: without this,
/// 332 of 775 would-refuse Reads (15.1 MB) were briefs reviewers are told to
/// read in full.
pub fn reviewer(agent: &crate::lsp::Agent) -> bool {
    match agent {
        crate::lsp::Agent::Named(kind) => {
            let kind = kind.rsplit_once(':').map_or(kind.as_str(), |(_, k)| k);
            kind == "implementation-review"
                || kind == "interface-review"
                || kind.starts_with("code-review-")
                || kind.starts_with("plan-review-")
                || kind.starts_with("relais-reviewer-")
                || kind == "relais-review"
        }
        crate::lsp::Agent::Main | crate::lsp::Agent::Unnamed => false,
    }
}

/// What the hook says when it refuses the first whole Read of this file by
/// this agent: the advice, then that the same Read repeated will pass.
pub fn phrase_refusal(shown: &str, bytes: u64, finder: Finder) -> (String, String) {
    let (reason, _) = phrase(shown, bytes, finder);
    (
        format!("{reason} Refused once, before it ran: an automatic check, not the person."),
        format!(
            "{}, then Read that window with `offset`/`limit`; or, if you need the whole \
             file, repeat this same Read unchanged: it will be allowed.",
            find_part(shown, finder)
        ),
    )
}

/// How the remedy says to find the part of the file that is needed.
fn find_part(shown: &str, finder: Finder) -> String {
    let quoted = format!("'{}'", shown.replace('\'', "'\\''"));
    match finder {
        Finder::Lsp => {
            "Find the part you need first with LSP (`documentSymbol`, `goToDefinition`)".to_string()
        }
        Finder::Grep => format!(
            "Find the part you need first with the Grep tool (or `grep -n '<anchor>' \
             {quoted} | head`)"
        ),
    }
}

/// What the hook says: the mechanism, then the way through.
pub fn phrase(shown: &str, bytes: u64, finder: Finder) -> (String, String) {
    let kb = bytes.div_ceil(1024);
    let find = find_part(shown, finder);
    (
        format!(
            "`{shown}` is {kb} KB on disk and is being Read with no window: up to 2,000 \
             lines of it land in the context, and every later turn of this session carries \
             them again. Subagents' whole Reads over 8 KB were 27% of every byte the \
             tools returned, measured."
        ),
        format!(
            "{find}, then Read that window with `offset`/`limit`. To read more on purpose, \
             pass `offset: 1` with a `limit` covering the lines you need: the whole file \
             only when that is the job."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewer_kinds_are_named_and_near_misses_are_not() {
        let named = |kind: &str| reviewer(&crate::lsp::Agent::Named(kind.to_string()));
        for kind in [
            "implementation-review",
            "interface-review",
            "code-review-backend",
            "plan-review-language",
            "relais:relais-reviewer-opus-medium",
            "relais-review",
        ] {
            assert!(named(kind), "{kind}");
        }
        for kind in [
            "code-reviewer",
            "general-purpose",
            "Explore",
            "relais:relais-worker-sonnet-medium",
        ] {
            assert!(!named(kind), "{kind}");
        }
        assert!(!reviewer(&crate::lsp::Agent::Main));
        assert!(!reviewer(&crate::lsp::Agent::Unnamed));
    }
    use std::path::PathBuf;

    /// A unique scratch directory per test, removed by the caller.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("amont-agent-rub-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn file(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(path, "x".repeat(bytes)).expect("file");
    }

    #[test]
    fn a_large_file_read_whole_applies() {
        let dir = scratch("large");
        let f = dir.join("a.rs");
        file(&f, 20_480);
        assert_eq!(applies(&f, "full"), Some(20_480));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_window_is_silent() {
        let dir = scratch("window");
        let f = dir.join("a.rs");
        file(&f, 20_480);
        assert_eq!(applies(&f, "0:200"), None);
        assert_eq!(applies(&f, "1:5000"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_threshold_is_exclusive() {
        let dir = scratch("threshold");
        let f = dir.join("a.rs");
        file(&f, 16_384);
        assert_eq!(applies(&f, "full"), None);
        file(&f, 16_385);
        let bytes = applies(&f, "full").expect("one byte over");
        assert_eq!(bytes, 16_385);
        assert!(phrase("a.rs", bytes, Finder::Grep).0.contains("17 KB"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_to_stat_is_silent() {
        let dir = scratch("nostat");
        assert_eq!(applies(&dir.join("missing.rs"), "full"), None);
        assert_eq!(applies(&dir, "full"), None);
        #[cfg(unix)]
        {
            let fifo = dir.join("pipe.rs");
            let made = std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo");
            assert!(made.success(), "mkfifo {}", fifo.display());
            assert_eq!(applies(&fifo, "full"), None);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exempt_paths_are_silent() {
        let dir = scratch("exempt");
        for rel in [
            ".claude/plans/x.md",
            "docs/plans/x.md",
            "x.diff",
            "X.PATCH",
            "x.png",
            "X.PNG",
            "tool-results/x.txt",
        ] {
            let f = dir.join(rel);
            file(&f, 20_480);
            assert_eq!(applies(&f, "full"), None, "{rel}");
        }
        let f = dir.join("myplans/x.md");
        file(&f, 20_480);
        assert!(applies(&f, "full").is_some(), "a substring is not enough");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_remedy_quotes_the_path() {
        let (_, remedy) = phrase("/a b/it's.rs", 20_480, Finder::Grep);
        assert!(remedy.contains(r#"'/a b/it'\''s.rs'"#), "{remedy}");
        let (_, lsp) = phrase("/a b/it's.rs", 20_480, Finder::Lsp);
        assert!(
            lsp.contains("documentSymbol") && !lsp.contains("grep"),
            "{lsp}"
        );
    }
}
