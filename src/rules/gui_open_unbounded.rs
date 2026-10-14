//! `gui-open-unbounded` — a macOS app launch sent where no clock reaches it.
//!
//! On 2026-10-14, in a happier N1 session, a background Bash call ran
//! `xcrun simctl boot <udid> 2>/dev/null; open -a Simulator; yarn ios:dev … >
//! build.log 2>&1`. `open -a Simulator` never returned — one hung 63 minutes,
//! a second 10 — so nothing after it in the chain ran, the task's output stayed
//! empty and no completion notice came. Killing the `open` let the chain go on
//! at once. The cause is not known and this rule does not claim one.
//!
//! ## What was measured (2026-10-14, macOS Intel, the Claude Code Bash tool
//! running zsh 5.9, each trial under an external 15–20 s watchdog that killed
//! only its own child)
//!
//! - `open -a Simulator`, `open -g -a Simulator`, `open -j -a Simulator`,
//!   `open -a Finder`, `open -g -a Finder`, `open -j -a Finder`,
//!   `open -b com.apple.finder` and `open -g <directory>` all hung until
//!   killed, in the foreground; `open -a Simulator` and `open -g -j -a
//!   Simulator` hung the same way with `run_in_background: true`.
//! - `open -R <path>` returned in 0.18 s with status 0.
//! - Other sessions' `open <file>.html` processes were hung at the same time.
//!
//! So here a plain launch, `-g` and `-j` all hang alike, and the hang was not
//! specific to the Simulator: no flag is treated as a deadline, and a flag that
//! returned in one trial is not exempted. Only `timeout` is, and `-R`, which
//! launches nothing. Opening a document or a URL (`open README.md`) goes through
//! the same call and was seen to hang too (`open -g <directory>`); the rule
//! leaves it alone because the person asked for app launches only — a recall
//! cost, said here rather than hidden.
//!
//! ## Scope
//!
//! Whether the call runs in the background is a sibling field of the payload;
//! `examine` sees only the command, so it judges every clause and `confirm`
//! declines when the call ran in the foreground, where the Bash tool's own
//! timeout ends it (the same split as `unbounded-background-push`). Each clause
//! is judged on its own: `timeout 15 open -a Simulator; open -a Safari` still
//! fires on the second launch.
//!
//! The remedy is `timeout 15 open -a <App>` as a call of its own, or no GUI at
//! all: `xcrun simctl boot`, `simctl io <udid> screenshot` and `simctl openurl`
//! work headless.

use crate::rules::{Confirmed, Context, Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "gui-open-unbounded",
    // Advises: the launch is legitimate — only unbounded.
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-10-14",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: Some(confirm),
};

fn examine(parsed: &Parsed) -> Option<Finding> {
    let cmd = parsed
        .judgeable()
        .find(|c| c.program() == Some("open") && !c.wrapped_by("timeout") && launches_an_app(c))?;
    Some(Finding {
        reason: "`open` of an app can block with nothing to end it: on 2026-10-14 `open -a \
                 Simulator` was measured to hang twice (63 and 10 minutes), `-g` and `-j` did \
                 not help, and in a background call the chain after it never ran, the output \
                 stayed empty and no completion notice came."
            .to_string(),
        remedy: "Run `timeout 15 open -a <App>` as a Bash call of its own, or skip the GUI: \
                 `xcrun simctl boot`, `simctl io <udid> screenshot` and `simctl openurl` work \
                 headless."
            .to_string(),
        span: cmd.at..cmd.end,
    })
}

/// `open -a <App>`, `open -b <bundle-id>` or an `<x>.app` operand, and not
/// `-R`, which only reveals in Finder (it returned in the trials).
fn launches_an_app(cmd: &Simple) -> bool {
    let mut args = cmd.args().iter();
    while let Some(w) = args.next() {
        if w.quoted {
            // An `.app` path given in quotes is still an operand.
            if is_app_path(&w.text) {
                return true;
            }
            continue;
        }
        let t = w.text.as_str();
        match t {
            // What follows belongs to the launched app.
            "--args" => return false,
            "--reveal" => return false,
            _ if t.starts_with("--") => {}
            _ if t.starts_with('-') && t.len() > 1 => {
                let cluster = &t[1..];
                if cluster.contains('R') {
                    return false;
                }
                // The last letter of a cluster takes the next word as its value.
                match cluster.chars().last() {
                    Some('a') | Some('b') => return true,
                    Some('f') | Some('t') | Some('e') | Some('F') | Some('s') => {
                        args.next();
                    }
                    _ => {}
                }
            }
            _ if is_app_path(t) => return true,
            _ => {}
        }
    }
    false
}

fn is_app_path(word: &str) -> bool {
    word.trim_end_matches('/').ends_with(".app")
}

fn confirm(ctx: &Context, _f: &Finding) -> Confirmed {
    if !ctx.background {
        return Confirmed::No("in the foreground the tool's own timeout bounds the launch");
    }
    Confirmed::Yes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn an_app_launch_is_examined() {
        assert!(fires("open -a Simulator"));
        assert!(fires("open -g -a Simulator"));
        assert!(fires("open -ja Simulator"));
        assert!(fires("open -a \"Google Chrome\" https://example.com"));
        assert!(fires("open /Applications/Xcode.app"));
        assert!(fires("open -b com.apple.iphonesimulator"));
        assert!(fires("cd x && open -a Simulator; yarn ios"));
    }

    #[test]
    fn a_document_a_url_or_a_reveal_is_not() {
        assert!(!fires("open README.md"));
        assert!(!fires("open https://example.com"));
        assert!(!fires("open ."));
        assert!(!fires("open -R /Applications/Xcode.app"));
        assert!(!fires("echo open -a Simulator"));
        assert!(!fires("git commit -m \"open -a Simulator hangs\""));
    }

    #[test]
    fn timeout_bounds_its_own_clause_only() {
        assert!(!fires("timeout 15 open -a Simulator"));
        assert!(!fires("timeout -k 5 15 open -a Simulator && echo ok"));
        assert!(fires("timeout 15 open -a Simulator; open -a Safari"));
        assert!(fires("timeout 15 open -a Simulator && open -g -a Safari"));
    }

    #[test]
    fn the_apps_own_arguments_are_not_ours() {
        assert!(!fires("open ./notes.txt --args -a"));
    }
}
