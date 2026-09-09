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

/// `*`, `?`, or a `[…]` class — the characters zsh expands, and the only
/// three, since brace expansion produces words rather than matching files.
pub fn has_glob(text: &str) -> bool {
    if text.contains('*') || text.contains('?') {
        return true;
    }
    matches!((text.find('['), text.rfind(']')), (Some(open), Some(close)) if open < close)
}
