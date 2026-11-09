//! `chain-with-and` — a step that can fail, joined by `;` or a newline to a
//! step that acts on what it was meant to produce.
//!
//! `python3 -c "…assert…"; cargo test` runs the tests on the unedited file
//! when the edit fails its own assert, and they pass. `git add -A ; git
//! commit` commits whatever the failed `add` left staged. `;` and a newline
//! lex to the same connector, so both are read here; `&&`, `||` and `&` never
//! match.
//!
//! The fallible side is a short list of edits, adds and builds. A clause the
//! lexer could not read, a clause that opens or continues a compound command
//! (`then`, `do`, `fi`, …) and anything nested in `$(…)` are neither side, so
//! the first command of a loop or `if` body is never the fallible one: a known
//! gap, accepted to keep bodies out of scope. `xargs` pipelines are opaque to
//! the lexer, so `… | xargs git add ; git commit` is not seen either.

use crate::rules::{Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::{Connector, Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "chain-with-and",
    prevents: "A failed edit, add or test followed by `;` into a commit, push or test run that acts on the wrong state",
    irreversible: false,
    default_stance: Stance::Observe,
    // A heuristic on the command's own shape must not deny.
    max_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 3.2,
        measured: "2026-11-09",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: None,
};

/// Words that open or continue a compound command, or run code this rule
/// does not read. A clause starting with one is neither side of a pair.
const COMPOUND_OPENERS: &[&str] = &[
    "if", "then", "elif", "else", "fi", "do", "done", "while", "until", "case", "esac", "function",
    "{", "}", "(", ")", "eval", "source", ".", "!",
];

fn is_compound(clause: &Simple) -> bool {
    clause
        .words
        .first()
        .is_some_and(|word| !word.quoted && COMPOUND_OPENERS.contains(&word.raw.as_str()))
}

/// The first operand that is not a toolchain selector like `+nightly`.
fn first_operand(clause: &Simple) -> Option<&str> {
    clause
        .operands()
        .into_iter()
        .map(|word| word.text.as_str())
        .find(|text| !text.starts_with('+'))
}

fn second_operand(clause: &Simple) -> Option<&str> {
    clause
        .operands()
        .into_iter()
        .map(|word| word.text.as_str())
        .filter(|text| !text.starts_with('+'))
        .nth(1)
}

fn is_python_edit(clause: &Simple) -> bool {
    matches!(clause.program(), Some("python" | "python3"))
        && (clause.has_short('c') || clause.heredoc || !clause.operands().is_empty())
}

fn is_in_place_edit(clause: &Simple) -> bool {
    matches!(clause.program(), Some("sed" | "perl"))
        && (clause.has_short('i') || clause.has_flag("--in-place"))
}

fn is_formatter(clause: &Simple) -> bool {
    clause.program() == Some("cargo") && first_operand(clause) == Some("fmt")
}

/// A step that rewrites files: its failure leaves them as they were.
fn is_edit(clause: &Simple) -> bool {
    is_python_edit(clause) || is_in_place_edit(clause) || is_formatter(clause)
}

fn is_runner(clause: &Simple) -> bool {
    match clause.program() {
        Some("make") => true,
        Some("pytest") => true,
        Some("cargo") => matches!(
            first_operand(clause),
            Some("test" | "build" | "check" | "clippy" | "nextest")
        ),
        Some("go") => matches!(first_operand(clause), Some("test" | "build" | "vet")),
        Some("npm" | "pnpm" | "yarn") => matches!(
            first_operand(clause),
            Some("test" | "build" | "lint" | "typecheck")
        ),
        _ => false,
    }
}

fn is_git_add(clause: &Simple) -> bool {
    clause.program() == Some("git") && clause.subcommand() == Some("add")
}

fn can_fail(clause: &Simple) -> bool {
    is_edit(clause) || is_git_add(clause) || is_runner(clause)
}

fn is_test_run(clause: &Simple) -> bool {
    match clause.program() {
        Some("pytest") => true,
        Some("cargo") => matches!(first_operand(clause), Some("test" | "nextest")),
        Some("go") => first_operand(clause) == Some("test"),
        Some("npm" | "pnpm" | "yarn") => first_operand(clause) == Some("test"),
        Some("make") => first_operand(clause) == Some("test"),
        _ => false,
    }
}

fn is_consequential(clause: &Simple) -> bool {
    match (clause.program(), clause.subcommand()) {
        (Some("git"), Some("commit" | "tag" | "push")) => true,
        (Some("amont"), Some("land")) => true,
        (Some("kubectl"), Some("apply" | "delete")) => !clause.is_dry_run(),
        (Some("gh"), _) => {
            first_operand(clause) == Some("pr") && second_operand(clause) == Some("merge")
        }
        _ => false,
    }
}

/// What a `set` clause does to errexit: `set -e`, `set -euo pipefail` and
/// `set -o errexit` turn it on (a failed step already stops the script, so
/// `;` is as safe as `&&`), `set +e` and `set +o errexit` turn it off, and
/// anything else leaves it alone.
fn errexit_after(clause: &Simple) -> Option<bool> {
    if clause.program() != Some("set") {
        return None;
    }
    let words: Vec<&str> = clause
        .args()
        .iter()
        .map(|word| word.text.as_str())
        .collect();
    if words.windows(2).any(|pair| pair == ["+o", "errexit"])
        || words
            .iter()
            .any(|word| word.starts_with('+') && word.contains('e'))
    {
        return Some(false);
    }
    if clause.has_short('e') || words.windows(2).any(|pair| pair == ["-o", "errexit"]) {
        return Some(true);
    }
    None
}

fn readable(clause: &Simple) -> bool {
    clause.opaque.is_none() && !is_compound(clause)
}

/// `git commit` rather than `git`, so the message names the step.
fn step_name(clause: &Simple, fallback: &str) -> String {
    match (clause.program(), clause.subcommand()) {
        (Some(program), Some(sub)) => format!("{program} {sub}"),
        (Some(program), None) => program.to_string(),
        _ => fallback.to_string(),
    }
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    let clauses = parsed.clauses();
    for (index, pair) in clauses.windows(2).enumerate() {
        let (fallible, consequential) = (&pair[0], &pair[1]);
        if fallible.next != Some(Connector::Semi)
            || !readable(fallible)
            || !readable(consequential)
            || fallible.nested.is_some()
            || consequential.nested.is_some()
            || !can_fail(fallible)
            || !(is_consequential(consequential)
                || (is_edit(fallible) && is_test_run(consequential)))
        {
            continue;
        }
        let stops_on_failure = clauses[..index]
            .iter()
            .filter(|earlier| earlier.nested.is_none())
            .filter_map(errexit_after)
            .next_back()
            .unwrap_or(false);
        if stops_on_failure {
            continue;
        }
        let first = step_name(fallible, "the step");
        let second = step_name(consequential, "the next step");
        return Some(Finding {
            reason: format!(
                "`;` or a newline runs `{second}` even if `{first}` failed, so it acts on the \
                 state the failed step left: a test passes on an unedited file, a commit takes \
                 half the files."
            ),
            remedy: "Join dependent steps with `&&`, or run them as separate calls and check \
                     each exit status."
                .to_string(),
            span: fallible.at..consequential.end,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn an_add_then_commit_fires() {
        assert!(fires("git add -A ; git commit -m x"));
    }

    #[test]
    fn a_python_heredoc_then_a_test_run_fires() {
        assert!(fires(
            "python3 - <<'EOF'\nassert False\nEOF\ncargo test -p a"
        ));
        assert!(fires("python3 -c 'assert 0'; cargo test"));
    }

    #[test]
    fn a_newline_pair_fires() {
        assert!(fires("git add -A\ngit commit -m x"));
    }

    #[test]
    fn a_nested_push_before_a_trailing_semicolon_does_not() {
        assert!(!fires("echo $(git push); make;"));
    }

    #[test]
    fn a_pair_inside_a_substitution_does_not() {
        assert!(!fires("echo $(git add -A; git commit -m x)"));
    }

    #[test]
    fn errexit_makes_the_semicolon_safe() {
        assert!(!fires("set -e; sed -i s/a/b/ f; git commit -m x"));
        assert!(!fires(
            "set -euo pipefail; sed -i s/a/b/ f; git commit -m x"
        ));
        assert!(fires("set +e; sed -i s/a/b/ f; git commit -m x"));
        assert!(fires("set +o errexit; sed -i s/a/b/ f; git commit -m x"));
        assert!(fires("set -e; set +e; git add -A; git commit -m x"));
        assert!(!fires("set +e; set -e; git add -A; git commit -m x"));
    }

    #[test]
    fn a_compound_body_is_not_the_fallible_side() {
        assert!(!fires("if x; then make; git push; fi"));
        let finding = examine(&lex("then sed -i s/a/b/ f; make; git push; fi"))
            .expect("make then git push fires");
        assert!(finding.reason.contains("`git push`") && finding.reason.contains("`make`"));
    }

    #[test]
    fn a_test_run_before_a_test_run_does_not() {
        assert!(!fires("cargo test -p a ; cargo test -p b"));
    }

    #[test]
    fn a_pipeline_ending_in_tail_does_not() {
        assert!(!fires("python x.py | tail -3 ; git commit"));
    }

    #[test]
    fn the_xargs_incident_is_opaque_and_does_not() {
        assert!(!fires(
            "git ls-files --deleted -z | xargs -0 git add -A ; git commit -m x"
        ));
    }

    #[test]
    fn a_harmless_first_step_or_a_joined_pair_does_not() {
        assert!(!fires("echo x ; git commit -m y"));
        assert!(!fires("cd d ; git push"));
        assert!(!fires("git status ; git commit -m y"));
        assert!(!fires("git add -A && git commit -m x"));
        assert!(!fires("git add -A || git commit -m x"));
    }
}
