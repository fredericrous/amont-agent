//! How wide the terminal is, for the one place that wraps.
//!
//! Vendored from `amont-runtime::live`, whose doc comment already said it was
//! public "because `amont-agent` prints sampled commands and needs the same
//! answer". It now lives where its only caller does.

/// `$COLUMNS` when it is exported and sane, else a conservative 100 — the
/// lines this wraps are short and an ioctl is not worth its portability.
pub fn term_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse::<usize>().ok())
        .filter(|w| *w >= 20)
        .unwrap_or(100)
}

#[cfg(test)]
mod tests {
    use super::term_width;

    /// One test, not five: `COLUMNS` is process-wide, so separate tests
    /// would race each other inside the one test binary.
    #[test]
    fn columns_is_honoured_when_sane_and_ignored_when_not() {
        let restore = std::env::var("COLUMNS").ok();

        std::env::remove_var("COLUMNS");
        assert_eq!(term_width(), 100, "unset falls back");

        for (set, want) in [
            ("80", 80),
            ("20", 20),  // the floor itself is sane
            ("10", 100), // below the floor falls back
            ("", 100),   // unparseable falls back
            ("nonsense", 100),
            ("-5", 100), // negative does not parse as usize
        ] {
            std::env::set_var("COLUMNS", set);
            assert_eq!(term_width(), want, "COLUMNS={set:?}");
        }

        match restore {
            Some(v) => std::env::set_var("COLUMNS", v),
            None => std::env::remove_var("COLUMNS"),
        }
    }
}
