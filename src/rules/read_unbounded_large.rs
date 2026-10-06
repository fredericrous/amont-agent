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

/// Below this the advice costs more than the read. The Read median is 3.4 KB
/// and p90 is 14.5 KB; it is also tokenjuice's pass-through threshold.
const LARGE: u64 = 16 * 1024;

/// `Some(bytes)` when this Read has no window, is not exempt, and names a
/// regular file over [`LARGE`]. A failed stat is silence.
pub fn applies(path: &Path, window: &str) -> Option<u64> {
    if window != "full" || exempt(path) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    (meta.is_file() && meta.len() > LARGE).then_some(meta.len())
}

fn exempt(path: &Path) -> bool {
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

/// What the hook says: the mechanism, then the way through.
pub fn phrase(shown: &str, bytes: u64) -> (String, String) {
    let kb = bytes.div_ceil(1024);
    let quoted = format!("'{}'", shown.replace('\'', "'\\''"));
    (
        format!(
            "`{shown}` is {kb} KB on disk and is being Read with no window: up to 2,000 \
             lines of it land in the context, and every later turn of this session carries \
             them again. Unbounded Reads over 16 KB were 37% of the Read tool's bytes \
             measured."
        ),
        format!(
            "Find the part you need first with the Grep tool (or `grep -n '<anchor>' \
             {quoted} | head`), then Read that window with `offset`/`limit`. To read more \
             on purpose, pass `offset: 1` with a `limit` covering the lines you need: the \
             whole file only when that is the job."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(phrase("a.rs", bytes).0.contains("17 KB"));
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
        let (_, remedy) = phrase("/a b/it's.rs", 20_480);
        assert!(remedy.contains(r#"'/a b/it'\''s.rs'"#), "{remedy}");
    }
}
