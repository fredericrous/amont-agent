//! `glob-in-flag-value` — a glob inside a flag's value, unquoted.
//!
//! ```sh
//! grep -rn "value-streams" app/ --include=*.tsx --include=*.ts
//! kubectl get pods -o custom-columns=NAME:.metadata.name,READY:.status.containerStatuses[*].ready
//! ```
//!
//! The shell expands `--include=*.ts` before grep ever sees it. Under zsh,
//! which is what the Bash tool runs on a Mac, a glob that matches nothing is
//! a hard error and the command never starts — `no matches found:
//! --include=*.ts` — while one that matches a file in the current directory
//! silently becomes `--include=app.ts`, and grep narrows to that one name
//! without a word. Either way the exit status belongs to whatever clause came
//! next: measured over 32,555 real calls, 95% of these came back reporting
//! success.
//!
//! Quoting is always the right spelling, so a fire on a glob that happened to
//! match is still a true positive.

use crate::rules::tool_shell;
use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "glob-in-flag-value",
    default_stance: Stance::Advise,
    evidence: Evidence {
        // 132 zsh diagnostics in 32,555 calls, flat across five weeks
        // (8.3 · 3.3 · 4.1 · 1.6 · 6.0 · 5.5) and 95% of them reported as
        // success by the tool. Advises from the start for the same reason
        // `stale-base` does: nothing fails, so nothing corrects.
        per_1000: 4.3,
        measured: "2026-09-09",
        trend: Trend::Flat(5),
    },
    examine,
    confirm: Some(confirm),
};

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.clauses() {
        for w in cmd.args() {
            // `rc=$?` is not a glob: the lexer blanks `$(…)` and `${…}` but a
            // bare `$?` or `$VAR` stays, and nothing here can expand it.
            if w.quoted || w.text.contains('$') {
                continue;
            }
            let Some(eq) = w.text.find('=') else {
                continue;
            };
            // `=foo` is `equals-separator`'s shape, not a flag value.
            if eq == 0 || !tool_shell::has_glob(&w.text[eq + 1..]) {
                continue;
            }
            let flag = &w.text[..eq];
            return Some(Finding {
                reason: format!(
                    "`{}` is expanded by the shell before the program sees it. Under zsh a \
                     glob that matches nothing is a hard error — `no matches found` — and the \
                     command never runs, while one that matches a file in the current \
                     directory silently becomes that one filename. Either way the exit status \
                     belongs to whatever clause came next.",
                    w.text
                ),
                remedy: format!(
                    "Quote the value so the glob reaches the program that understands it: \
                     `{flag}='{}'`.",
                    &w.text[eq + 1..]
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
    fn an_unquoted_glob_in_a_flag_value_fires() {
        assert!(fires(
            "grep -rn x app/ --include=*.tsx --include=*.ts | head -50"
        ));
        assert!(fires(
            "grep -rln --include=*.meta.ts \"\" packages/ui/src | wc -l"
        ));
        assert!(fires(
            "kubectl get pods -o custom-columns=NAME:.metadata.name,READY:.status.containerStatuses[*].ready"
        ));
        assert!(fires(
            "kubectl get pods -o jsonpath={.items[*].metadata.name}"
        ));
        assert!(fires("grep -rn x --include=*.rs crates/*/tests/ | head"));
    }

    #[test]
    fn a_quoted_value_or_a_plain_one_is_silent() {
        assert!(!fires(
            "grep -rn foo app/ --include='*.tsx' --include='*.ts' | head"
        ));
        assert!(!fires("grep -rn bar src/ --include=\"*.rs\" | head"));
        assert!(!fires(
            "grep -rn baz . --exclude-dir=node_modules --include=Makefile"
        ));
        assert!(!fires(
            "kubectl get pods -o jsonpath='{.items[*].metadata.name}'"
        ));
        assert!(!fires("rg -n --glob '*.ts' useLinkTarget app/ | head -20"));
        assert!(!fires("kubectl delete pod x --wait=false"));
        assert!(!fires("git log --format=%h -3"));
        assert!(!fires("bash -c 'grep -rn foo . --include=*.rs' | head"));
        assert!(!fires("python3 -c \"print('--include=*.ts')\""));
        assert!(!fires("echo ===; ls"));
        assert!(!fires("cmd; rc=$?; echo rc=$?"));
        assert!(!fires("grep -rn x --include=$EXT . | head"));
    }

    #[test]
    fn the_span_is_the_word() {
        let f = examine(&lex("grep -rn x app/ --include=*.ts | head")).unwrap();
        assert_eq!(
            &"grep -rn x app/ --include=*.ts | head"[f.span],
            "--include=*.ts"
        );
    }
}
