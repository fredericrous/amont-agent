//! The signs and the display guard.
//!
//! Vendored from `amont-runtime`, trimmed to what this crate calls.
//!
//! **Colour is the terminal's decision, not ours.** These are base ANSI codes
//! (`31`/`32`/`33`), not the 256-colour cube: a terminal theme remaps only
//! indices 0–15, so anything above renders identically whatever palette the
//! user chose, overriding a carefully themed terminal with numbers picked
//! years ago. `32` means "whatever this terminal calls green".
//!
//! `sanitize` is the load-bearing function here, and the reason this module
//! is vendored rather than reimplemented: everything this guard prints is a
//! command string it was handed by a model, or a branch name, or a line of a
//! transcript. All of it is untrusted text on its way to a terminal.

use std::sync::OnceLock;

/// Base ANSI, deliberately. See the module docs.
const GREEN: &str = "32";
const RED: &str = "31";
const YELLOW: &str = "33";

/// Does the caller want colour at all?
///
/// `NO_COLOR` per no-color.org: present and NON-EMPTY disables it, whatever
/// the value. `TERM=dumb` is a terminal that cannot render SGR.
///
/// Read once — this is a short-lived process and its environment does not
/// change underneath it.
pub fn colors_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let no_color = std::env::var_os("NO_COLOR")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        let dumb = std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false);
        !no_color && !dumb
    })
}

fn paint(text: &str, sgr: &str) -> String {
    if colors_enabled() {
        format!("\u{1b}[{sgr}m{text}\u{1b}[0m")
    } else {
        text.to_string()
    }
}

fn sign(glyph: &str, sgr: &str) -> String {
    format!("  {}", paint(glyph, sgr))
}

/// The glyph carries the meaning and the colour only reinforces it, so under
/// `NO_COLOR` — and for the ~8% of men with red-green colour vision
/// deficiency — `✓ ✗ !` stay distinguishable on their own.
pub fn valid_sign() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| sign("✓", GREEN))
}

pub fn error_sign() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| sign("✗", RED))
}

pub fn warning_sign() -> &'static str {
    static S: OnceLock<String> = OnceLock::new();
    S.get_or_init(|| sign("!", YELLOW))
}

/// Emphasise a fragment inside a message, in the terminal's own accent.
///
/// Sanitises what it is given, because most of what is highlighted came from
/// somewhere else: a config key the user typed, a rule id, a command. An
/// escape sequence inside would also break this function's own painting —
/// the reset it emits is no longer the last word — so this is as much about
/// the colouring being correct as about the text being safe.
pub fn highlight(text: &str) -> String {
    paint(&sanitize(text), YELLOW)
}

/// Render `text` so a terminal displays it rather than obeying it.
///
/// Every byte a terminal would act on becomes visible instead: C0 and C1
/// control characters, DEL, and the bidirectional overrides behind Trojan
/// Source. A tab keeps its width without keeping its behaviour.
///
/// This is a DISPLAY guard, not a charset policy — accented text, CJK and
/// emoji pass through untouched.
pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\t' => out.push(' '),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if ('\u{80}'..='\u{9f}').contains(&c) => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {
                out.push_str(&format!("\\u{{{:04x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every byte a terminal would act on, and nothing else.
    #[test]
    fn sanitize_escapes_what_a_terminal_would_obey() {
        // The one that hid a declaration from amont's trust prompt; here the
        // same sequence would hide half a command from the deny reason.
        assert_eq!(sanitize("a\u{1b}[8mb"), "a\\x1b[8mb");
        assert_eq!(sanitize("bell\u{7}"), "bell\\x07");
        assert_eq!(sanitize("cr\r"), "cr\\x0d");
        assert_eq!(sanitize("del\u{7f}"), "del\\x7f");
        // 8-bit CSI: no ESC in sight.
        assert_eq!(sanitize("csi\u{9b}"), "csi\\x9b");
        // Trojan Source.
        assert_eq!(sanitize("rtl\u{202e}"), "rtl\\u{202e}");
        assert_eq!(sanitize("iso\u{2066}"), "iso\\u{2066}");
        // A tab keeps its width without keeping its behaviour.
        assert_eq!(sanitize("a\tb"), "a b");
    }

    /// It is a display guard, not a charset policy.
    #[test]
    fn sanitize_leaves_ordinary_text_alone() {
        for s in ["plain ascii", "café", "日本語", "✓ ✗ !", "a/b-c_d.e", "🐛"] {
            assert_eq!(sanitize(s), s, "{s:?} should pass through");
        }
    }

    #[test]
    fn no_color_is_honoured_per_the_standard() {
        fn decide(no_color: Option<&str>, term: &str) -> bool {
            let nc = no_color.map(|v| !v.is_empty()).unwrap_or(false);
            !nc && term != "dumb"
        }
        assert!(decide(None, "xterm-256color"), "colour by default");
        assert!(!decide(Some("1"), "xterm-256color"), "any value disables");
        assert!(!decide(Some("0"), "xterm-256color"), "even \"0\" disables");
        assert!(decide(Some(""), "xterm-256color"), "empty does NOT disable");
        assert!(!decide(None, "dumb"), "TERM=dumb cannot render SGR");
    }
}
