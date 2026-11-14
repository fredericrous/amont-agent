//! Which shell the Bash tool actually runs — the one fact three rules share.
//!
//! Claude Code does not run the login shell. It picks bash or zsh, and on a
//! Mac whose login shell is fish it runs `/bin/zsh` and sets `SHELL` for the
//! tool's process accordingly (verified 2026-09-09: `dscl` says fish,
//! `ps -p $$` inside the tool says `/bin/zsh`). The distinction matters
//! because the three shape rules that read this are about zsh's defaults —
//! `NOMATCH` and `EQUALS` — which bash does not share: bash passes an
//! unmatched glob through as text and prints `===` as written. A rule that
//! advised a bash user about a zsh failure would be wrong every time.
//!
//! So this is read in `confirm`, never in `examine`: the shape is the same
//! everywhere, the consequence is not, and the backtester's replay stays a
//! count of the shape.

use crate::rules::Dialect;

/// `Ok` when the tool's shell aborts on an unmatched glob and expands a
/// leading `=`, i.e. zsh. The `Err` names why the guard stays silent.
pub fn zsh() -> Result<(), &'static str> {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let name = shell.rsplit(['/', '\\']).next().unwrap_or("");
    match name {
        "zsh" => Ok(()),
        "bash" | "sh" | "dash" | "ksh" | "mksh" | "ash" => {
            Err("the tool's shell is bash, which passes an unmatched glob through as text")
        }
        // fish is never the tool's shell: Claude Code falls back from it, to
        // zsh on macOS and to bash elsewhere. An empty `SHELL` is the same
        // question with less information, answered the same way.
        "fish" | "" => {
            if cfg!(target_os = "macos") {
                Ok(())
            } else {
                Err("the tool's shell is not known to be zsh")
            }
        }
        _ => Err("the tool's shell is not known to be zsh"),
    }
}

/// The tool's shell as a [`Dialect`], for the judgement layer.
///
/// Read once by the hook and handed to rules as input, so that `examine`
/// stays pure and the backtester can replay a command with the dialect it
/// ran in (or with `Unknown`, which is all a transcript can say).
pub fn dialect() -> Dialect {
    dialect_of(
        &std::env::var("SHELL").unwrap_or_default(),
        cfg!(target_os = "macos"),
    )
}

/// [`dialect`] without the world. Only `bash` and `zsh` are named: `sh`,
/// `dash` and `ksh` differ from bash in the very places a dialect-aware
/// analysis cares about, so calling them bash would be a guess.
pub fn dialect_of(shell: &str, macos: bool) -> Dialect {
    match shell.rsplit(['/', '\\']).next().unwrap_or("") {
        "zsh" => Dialect::Zsh,
        "bash" => Dialect::Bash,
        // fish is never the tool's shell: Claude Code falls back from it to
        // zsh on macOS (see `zsh`, above), and an empty `SHELL` is the same
        // question with less information.
        "fish" | "" if macos => Dialect::Zsh,
        _ => Dialect::Unknown,
    }
}

/// `*`, `?`, or a `[…]` class — the characters zsh expands, and the only
/// three, since brace expansion produces words rather than matching files.
pub fn has_glob(text: &str) -> bool {
    if text.contains('*') || text.contains('?') {
        return true;
    }
    matches!((text.find('['), text.rfind(']')), (Some(open), Some(close)) if open < close)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_path_names_its_dialect() {
        assert_eq!(dialect_of("/bin/zsh", false), Dialect::Zsh);
        assert_eq!(dialect_of("/usr/local/bin/bash", false), Dialect::Bash);
        assert_eq!(dialect_of("C:\\\\Git\\\\bin\\\\bash", false), Dialect::Bash);
    }

    /// fish is never the tool's shell; on macOS Claude Code runs zsh instead.
    #[test]
    fn fish_and_empty_are_zsh_on_macos_only() {
        assert_eq!(dialect_of("/usr/local/bin/fish", true), Dialect::Zsh);
        assert_eq!(dialect_of("", true), Dialect::Zsh);
        assert_eq!(dialect_of("/usr/local/bin/fish", false), Dialect::Unknown);
        assert_eq!(dialect_of("", false), Dialect::Unknown);
    }

    /// POSIX shells are not bash, and calling them bash would be a guess.
    #[test]
    fn other_shells_are_unknown() {
        for sh in ["/bin/sh", "/bin/dash", "/bin/ksh", "/bin/mksh", "/bin/ash"] {
            assert_eq!(dialect_of(sh, false), Dialect::Unknown, "{sh}");
        }
    }
}
