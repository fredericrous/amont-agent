//! `equals-separator` — a bare word that begins with `=`.
//!
//! ```sh
//! sed -n '1,40p' a.rs; echo ===; sed -n '1,40p' b.rs
//! until [ "$st" == "completed" ]; do sleep 10; done
//! ```
//!
//! zsh's `EQUALS` option replaces a word beginning with `=` by the path of
//! the command named after it: `=ls` is `/bin/ls`. So `echo ===` asks for a
//! command called `==`, `[ "$a" == "b" ]` asks for one called `=`, and the
//! lookup fails — `== not found` — with a twist bash never taught anyone:
//! the failure aborts the WHOLE command list. Every clause after the
//! separator is lost, and a wait loop written with `==` dies on its first
//! test. Inside `[[ … ]]` the same `==` is fine, which is why the rule steps
//! around that clause.
//!
//! Loud, mostly: 91% of the real firings came back `is_error`. That is why it
//! ships observing — the repeat rate on the next call (19% when measured) is
//! the number that decides whether it earns `advise`.

use crate::rules::tool_shell;
use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "equals-separator",
    default_stance: Stance::Observe,
    evidence: Evidence {
        // 78 zsh diagnostics in 32,555 calls (0.0 · 1.7 · 2.1 · 1.5 · 4.6 ·
        // 1.4 per week), all but a handful the `echo ===` idiom between two
        // file reads.
        per_1000: 2.5,
        measured: "2026-09-09",
        trend: Trend::Flat(5),
    },
    examine,
    confirm: Some(confirm),
};

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.clauses() {
        // `[[ … ]]` is parsed by the shell, not expanded, so a `==` inside it
        // is the comparison it looks like. (`(( a == b ))` is the same, but the
        // lexer strips grouping, so it cannot be told apart here — and it did
        // not occur once in 32,555 real calls.)
        if cmd.words.iter().any(|w| !w.quoted && w.text == "[[") {
            continue;
        }
        for w in &cmd.words {
            // A lone `=` is literal; `EQUALS` needs a name to look up.
            if w.quoted || w.text.len() < 2 || !w.text.starts_with('=') {
                continue;
            }
            let named = &w.text[1..];
            return Some(Finding {
                reason: format!(
                    "zsh replaces a word beginning with `=` by the path of the command named \
                     after it, so `{}` looks up a command called `{named}` and fails — \
                     `{named} not found` — and the failure aborts the whole command list: every \
                     clause after it is skipped.",
                    w.text
                ),
                remedy: String::from(
                    "Quote the separator (`echo '==='`) or use characters that are not \
                     operators (`---`, `###`); inside `[ ]` and `test` compare with a single \
                     `=`, or use `[[ $a == b ]]`.",
                ),
                span: w.at..w.at + w.raw.len(),
            });
        }
    }
    None
}

fn confirm(_ctx: &Context, _f: &Finding) -> Confirmed {
    match tool_shell::zsh() {
        Ok(()) => Confirmed::Yes,
        Err(why) => Confirmed::No(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn a_bare_equals_word_fires() {
        assert!(fires("sed -n '1,4p' a.rs; echo ===; sed -n '1,4p' b.rs"));
        assert!(fires("head -8 a.rs; echo ======; head -6 b.rs"));
        assert!(fires("cat a | head -100 && echo ===== && cat b"));
        assert!(fires(
            "grep -n sub a.ts | head; echo ==SESSION; grep -n sub b.ts"
        ));
        assert!(fires(
            "echo ====DOC-CONSTS; sed -n 44,80p app/lib/landscapeDoc.ts"
        ));
        assert!(fires("[ \"$a\" == \"1\" ] && echo y"));
        assert!(fires(
            "until [ \"$st\" == \"completed\" ]; do sleep 5; done"
        ));
        assert!(fires("test x == y && echo same"));
    }

    #[test]
    fn a_quoted_or_bracketed_equals_is_silent() {
        assert!(!fires(
            "echo \"=== checkout ===\"; cat app/routes/billing.ts | head"
        ));
        assert!(!fires(
            "echo '=== api shape ==='; sed -n '1,50p' app/routes/api.ts"
        ));
        assert!(!fires("echo --- separator ---; ls"));
        assert!(!fires("[[ $a == 1 ]] && echo y"));
        assert!(!fires("if [ \"$a\" = \"1\" ]; then echo y; fi"));
        assert!(!fires("git log --oneline HEAD^..HEAD"));
        assert!(!fires("curl -s 'https://example.com/x?a==b' | head"));
        assert!(!fires("python3 -c \"print('===')\""));
        assert!(!fires(
            "grep -n \"kind === \" app/lib/dsl/compile.ts | head"
        ));
        assert!(!fires("kubectl delete pod x --wait=false"));
    }

    #[test]
    fn the_span_is_the_word() {
        let src = "cat a; echo ===; cat b";
        let f = examine(&lex(src)).unwrap();
        assert_eq!(&src[f.span], "===");
    }
}
