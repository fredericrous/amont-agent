//! `sed-in-place` — `sed -i` spelled for the other sed.
//!
//! `-i` takes its backup suffix two incompatible ways. GNU sed wants it glued
//! on (`-i.bak`, or bare `-i` for none) and reads a separate `''` as the
//! SCRIPT — so `sed -i '' 's/a/b/' file` fails with `can't read s/a/b/: No
//! such file or directory`. BSD sed wants it as the next argument and
//! refuses bare `-i` followed by an expression. A command written for one
//! fails on the other, and a Mac with GNU sed first on `PATH` is both.
//!
//! Measured over forty-two sessions: 47 uses of the BSD spelling, 40 of them
//! failed. The failure is loud, which is the crate's usual reason NOT to
//! guard — except that this one is not correcting itself: the same spelling
//! was retried for months, because the error names a missing file rather
//! than the flag. This rule exists to name the flag.
//!
//! ## `confirm` asks which sed this is
//!
//! `examine` fires on either spelling; `confirm` runs `sed --version` in the
//! command's directory and stays silent when the spelling matches the sed
//! that will run it.

use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "sed-in-place",
    default_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 2.4,
        measured: "2026-09-05",
        trend: Trend::Flat(8),
    },
    examine,
    confirm: Some(confirm),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    /// `-i ''` — a separate, empty suffix argument. BSD.
    Bsd,
    /// bare `-i` with no suffix. GNU.
    Gnu,
}

fn detect(cmd: &Simple) -> Option<Spelling> {
    if cmd.program() != Some("sed") {
        return None;
    }
    let words = &cmd.words;
    let idx = words.iter().position(|w| !w.quoted && w.text == "-i")?;
    match words.get(idx + 1) {
        Some(next) if next.quoted && next.text.is_empty() => Some(Spelling::Bsd),
        Some(_) => Some(Spelling::Gnu),
        None => None,
    }
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.clauses() {
        let Some(spelling) = detect(cmd) else {
            continue;
        };
        let reason = match spelling {
            Spelling::Bsd => {
                "`sed -i ''` is the BSD spelling: GNU sed takes the empty string as the \
                 SCRIPT and then reads the real script as a filename (`can't read s/…/: \
                 No such file or directory`)."
            }
            Spelling::Gnu => {
                "bare `sed -i` is the GNU spelling: BSD sed takes the next argument as \
                 the backup suffix and then has no script to run."
            }
        };
        return Some(Finding {
            reason: reason.to_string(),
            remedy: "Use a spelling both accept: `sed -i.bak '…' file && rm file.bak`, \
                     `perl -pi -e '…' file`, or a short Python edit that asserts the \
                     replacement landed."
                .to_string(),
            span: cmd.at..cmd.end,
        });
    }
    None
}

fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    let Some(spelling) = ctx
        .parsed
        .clauses()
        .iter()
        .find(|c| c.at == f.span.start)
        .and_then(detect)
    else {
        return Confirmed::No("the command no longer matches");
    };
    let cwd = ctx.cwd_at(f.span.start);
    let gnu = std::process::Command::new("sed")
        .arg("--version")
        .current_dir(if cwd.is_dir() { cwd.as_path() } else { ctx.cwd })
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("GNU"))
        .unwrap_or(false);
    match (spelling, gnu) {
        (Spelling::Bsd, true) => Confirmed::Yes,
        (Spelling::Gnu, false) => Confirmed::Yes,
        _ => Confirmed::No("this sed accepts that spelling"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn spelling(command: &str) -> Option<Spelling> {
        lex(command).clauses().iter().find_map(detect)
    }

    #[test]
    fn both_spellings_are_seen() {
        assert_eq!(spelling("sed -i '' 's/a/b/' f.txt"), Some(Spelling::Bsd));
        assert_eq!(
            spelling("cd /x && sed -i 's/a/b/' f.txt"),
            Some(Spelling::Gnu)
        );
        assert_eq!(
            spelling("sed -E -i \"\" -e 's/a/b/' f"),
            Some(Spelling::Bsd)
        );
    }

    #[test]
    fn a_suffix_is_portable_and_reading_is_not_editing() {
        assert_eq!(spelling("sed -i.bak 's/a/b/' f.txt"), None);
        assert_eq!(spelling("sed -n '1,20p' f.txt"), None);
        assert_eq!(spelling("sed 's/a/b/' f.txt > g.txt"), None);
        assert_eq!(spelling("grep -rn \"sed -i ''\" docs/"), None);
        assert_eq!(spelling("sed -i"), None);
    }
}
