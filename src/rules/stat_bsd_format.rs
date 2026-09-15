//! `stat-bsd-format` — `stat -f '%…'` spelled for the other stat.
//!
//! ```sh
//! echo "mtime: $(stat -f '%Sm' "$LOG")"
//! ```
//!
//! BSD stat takes its format with `-f`; GNU stat takes it with `-c` and
//! reads `-f` as `--file-system`. So on a Mac with GNU coreutils first on
//! `PATH`, `stat -f '%Sm' file` treats the format as a FILENAME, fails to
//! find it, and then prints the filesystem report for the real file — six
//! lines, exit 1, and inside `$( )` the caller substitutes an apfs block
//! count where a timestamp was wanted. Eleven uses in 32,710 calls; three of
//! the four failures were written exactly that way. Rare, and the last
//! spelling on this machine that succeeds into the wrong answer.
//!
//! `examine` fires on either spelling; `confirm` runs `stat --version` in the
//! command's directory and stays silent when the spelling matches the stat
//! that will run it — the mirror of `sed-in-place`.

use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Connector, Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "stat-bsd-format",
    default_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 0.34,
        measured: "2026-09-09",
        trend: Trend::Rare,
    },
    examine,
    confirm: Some(confirm),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    /// `-f '%…'`: the format flag on BSD, `--file-system` on GNU.
    Bsd,
    /// `-c '%…'` or `--format=…`: GNU.
    Gnu,
}

fn detect(cmd: &Simple) -> Option<Spelling> {
    if cmd.program() != Some("stat") {
        return None;
    }
    let args = cmd.args();
    for (i, w) in args.iter().enumerate() {
        // `--format='%s'` is a partly quoted word; the flag half is still the
        // flag.
        if w.text.starts_with("--format") || w.text.starts_with("--printf") {
            return Some(Spelling::Gnu);
        }
        if w.quoted {
            continue;
        }
        let next_is_format = args.get(i + 1).is_some_and(|n| n.text.contains('%'));
        match w.text.as_str() {
            "-f" if next_is_format => return Some(Spelling::Bsd),
            "-c" if next_is_format => return Some(Spelling::Gnu),
            _ => {}
        }
    }
    None
}

/// `stat -c %s f || stat -f %z f` — one spelling, then the other — is the
/// portable form, not the mistake: whichever stat runs, one of the two is
/// its own. Found in the transcripts the day substitutions became readable,
/// as `"$(stat -c %s "$f" 2>/dev/null || stat -f %z "$f")"`; a deny there
/// would refuse the one spelling that works everywhere.
fn has_fallback(clauses: &[Simple], i: usize, spelling: Spelling) -> bool {
    let other = |j: usize| {
        clauses
            .get(j)
            .and_then(detect)
            .is_some_and(|s| s != spelling)
    };
    (clauses[i].next == Some(Connector::OrOr) && other(i + 1))
        || (clauses[i].prev == Some(Connector::OrOr) && i > 0 && other(i - 1))
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    let clauses = parsed.clauses();
    for (i, cmd) in clauses.iter().enumerate() {
        if cmd.opaque.is_some() {
            continue;
        }
        let Some(spelling) = detect(cmd) else {
            continue;
        };
        if has_fallback(clauses, i, spelling) {
            continue;
        }
        let (reason, remedy) = match spelling {
            Spelling::Bsd => (
                "`stat -f` is the BSD format flag, but on GNU coreutils `-f` means \
                 `--file-system`: the format string is taken as a filename, and the command \
                 then prints the filesystem report for the real file. It exits 1 but still \
                 writes six lines to stdout, so a `$(stat -f …)` substitutes an apfs block \
                 count where a timestamp was wanted.",
                "Use the GNU spelling — `stat -c '%y %n'`, `-c '%s'` for size, `-c '%a'` for \
                 mode — or a portable read: `date -r <file>` for mtime, `wc -c < <file>` for \
                 size.",
            ),
            Spelling::Gnu => (
                "`stat -c` is the GNU format flag; BSD stat has no `-c` and refuses the \
                 command outright, so nothing is read.",
                "Use the BSD spelling — `stat -f '%Sm %N'`, `-f '%z'` for size — or a portable \
                 read: `date -r <file>` for mtime, `wc -c < <file>` for size.",
            ),
        };
        return Some(Finding {
            reason: reason.to_string(),
            remedy: remedy.to_string(),
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
    let gnu = std::process::Command::new("stat")
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
        _ => Confirmed::No("this stat accepts that spelling"),
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
        assert_eq!(spelling("stat -f '%Sm' $L"), Some(Spelling::Bsd));
        assert_eq!(
            spelling("stat -f \"%z bytes\" ~/x.zip"),
            Some(Spelling::Bsd)
        );
        assert_eq!(
            spelling("stat -f '%Sm %N' -t '%Y-%m-%d' x.log"),
            Some(Spelling::Bsd)
        );
        // Inside `$( )`: the header's own example, read since the lexer
        // started reading substitutions. It was `None` before that.
        assert_eq!(
            spelling("echo \"m: $(stat -f '%Sm' x)\""),
            Some(Spelling::Bsd)
        );
    }

    /// One spelling tried, then the other: portable, and silent — bare or
    /// inside a substitution. A fallback to something that is not a stat is
    /// no fallback.
    #[test]
    fn a_fallback_pair_is_the_portable_form() {
        let fires = |c: &str| examine(&lex(c)).is_some();
        assert!(!fires("stat -c %s f 2>/dev/null || stat -f %z f"));
        assert!(!fires("stat -f '%Sm' f 2>/dev/null || stat -c '%y' f"));
        assert!(!fires(
            "printf '%10d %s\\n' \"$(stat -c %s \"$f\" 2>/dev/null || stat -f %z \"$f\")\" \"$f\""
        ));
        assert!(fires("stat -c %s f || echo 0"));
        assert!(fires("stat -f '%Sm' f && echo ok"));
        assert!(fires("stat -c %s f; stat -f %z f"));
        assert_eq!(spelling("stat -c '%s %n' a.tsx"), Some(Spelling::Gnu));
        assert_eq!(
            spelling("stat --format='%s' /tmp/sw.js"),
            Some(Spelling::Gnu)
        );
    }

    #[test]
    fn a_filesystem_query_and_a_remote_are_not_a_format() {
        assert_eq!(spelling("stat -f /var/lib/garage"), None);
        assert_eq!(spelling("stat -L x"), None);
        assert_eq!(spelling("kubectl exec pod -- sh -c 'stat -f /var'"), None);
        assert_eq!(spelling("grep -rn 'stat -f' docs/ | head"), None);
        assert_eq!(spelling("df -h / | cat"), None);
    }
}
