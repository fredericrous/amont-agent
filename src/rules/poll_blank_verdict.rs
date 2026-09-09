//! `poll-blank-verdict` — a wait that reads its own failure as an answer.
//!
//! A polling loop asks something for a status and stops when the answer is no
//! longer the keep-going one:
//!
//! ```sh
//! s=$(curl -sS "$api/status" | jq -r .state)
//! if [ "$s" != "pending" ]; then echo "CI: $s"; exit 0; fi
//! ```
//!
//! When the lookup fails — a resolver not yet up after a laptop wakes, a
//! five-second network blip, an expired token — `$s` is the empty string, and
//! the empty string is not `pending`. The loop stops and reports a verdict it
//! never received. Measured live on 2026-09-06: a Mac woke from a four-hour
//! sleep, the loop resumed 3.6 seconds later before the resolver had settled,
//! `curl` printed `Could not resolve host`, and the loop announced a CI state
//! of "" as fact. Twenty-seven seconds later the same query resolved in 40 ms.
//!
//! This is the `pipe-to-tail` failure in a different costume: not "the exit
//! status came from the wrong command" but "the absence of an answer was
//! treated as one". Both make a failure look like a result, and neither leaves
//! anything behind for a correcting loop to notice — the wrong verdict is
//! reported once, confidently, and acted on.
//!
//! ## The shapes it fires on
//!
//! Only where the terminating comparison lets a blank through:
//!
//! | loop | test | stops when | verdict |
//! |---|---|---|---|
//! | `if` / `until` | `!=` | the value is anything else, blank included | **fires** |
//! | `while` | `=` | the value stops matching, blank included | **fires** |
//! | `if` / `until` | `=` | only on the value named — blank keeps waiting | silent |
//! | `while` | `!=` | only on the value named | silent |
//!
//! and only when the compared variable was assigned from a command
//! substitution, which is what makes blank reachable in the first place.

use crate::rules::{Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple, Word};

pub const RULE: Rule = Rule {
    id: "poll-blank-verdict",
    // Ships observing, like everything here: measured first, promoted on the
    // corpus rather than on how good the argument reads.
    default_stance: Stance::Observe,
    evidence: Evidence {
        // 33 firings in 30,486 Bash calls across five weeks, p95 2.4/1000 —
        // about seven a week. Every one of them reviewed into the corpus, and
        // every one the same defect; the `$rev != $start_rev` variants are it
        // too, since a blank is not equal to the starting revision either.
        per_1000: 2.4,
        measured: "2026-09-06",
        trend: Trend::Rare,
    },
    examine,
    confirm: None,
};

const LOOPS: &[&str] = &["until", "while", "for"];

/// `s=$(…)` — an assignment whose value came from a substitution, so a failed
/// command leaves it empty rather than unset.
fn assigned_from_substitution(word: &Word) -> Option<&str> {
    if !word.expanded {
        return None;
    }
    let name = word.text.split('=').next()?;
    if name.is_empty() || name.len() == word.text.len() {
        return None;
    }
    let ok = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit());
    ok.then_some(name)
}

fn is_test(cmd: &Simple) -> bool {
    cmd.words
        .iter()
        .any(|w| !w.quoted && (w.text == "[" || w.text == "[["))
}

/// Which keyword introduced this clause, if any.
fn introducer(cmd: &Simple) -> Option<&str> {
    let first = cmd.words.first()?;
    if first.quoted {
        return None;
    }
    match first.text.as_str() {
        "if" | "while" | "until" | "elif" => Some(first.text.as_str()),
        _ => None,
    }
}

/// The comparison operators that mean "stop on anything else".
fn escaping_operator(intro: &str) -> &'static [&'static str] {
    match intro {
        // `while [ "$s" = pending ]` keeps going only while it matches, so it
        // leaves the loop on a blank.
        "while" => &["=", "==", "-eq"],
        // `if`/`until`/`elif` with `!=` leaves on anything that is not the
        // named value — blank included.
        _ => &["!=", "-ne"],
    }
}

fn names_variable(words: &[Word], vars: &[&str]) -> bool {
    words.iter().any(|w| {
        let t = w.text.trim();
        vars.iter().any(|v| {
            t == format!("${v}") || t == format!("${{{v}}}") || t.contains(&format!("${v}"))
        })
    })
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    let clauses = parsed.clauses();

    // A wait, not just a loop: the same test outside a polling loop is an
    // ordinary conditional and none of this crate's business.
    let head = clauses
        .iter()
        .position(|c| c.opaque.is_none() && c.program().is_some_and(|p| LOOPS.contains(&p)))?;
    let polls = clauses.iter().skip(head).any(|c| {
        c.words
            .iter()
            .any(|w| !w.quoted && (w.text == "sleep" || w.text == "done"))
            && c.words.iter().any(|w| !w.quoted && w.text == "sleep")
    });
    if !polls {
        return None;
    }

    // Variables that a failed command can leave empty.
    let vars: Vec<&str> = clauses
        .iter()
        .flat_map(|c| c.words.iter())
        .filter_map(assigned_from_substitution)
        .collect();
    if vars.is_empty() {
        return None;
    }

    for cmd in clauses {
        if !is_test(cmd) {
            continue;
        }
        let Some(intro) = introducer(cmd) else {
            continue;
        };
        let escapes = escaping_operator(intro);
        if !cmd
            .words
            .iter()
            .any(|w| !w.quoted && escapes.contains(&w.text.as_str()))
        {
            continue;
        }
        if !names_variable(&cmd.words, &vars) {
            continue;
        }
        return Some(Finding {
            reason: "this loop stops on ANY value that is not the one it names, and a \
                     failed lookup leaves the variable empty — so a resolver blip, a \
                     dropped connection or an expired token ends the wait and reports \
                     the blank as the answer."
                .to_string(),
            remedy: "Name the terminal values instead: `case \"$s\" in success|failure|error) …` \
                     so anything else — including the empty string — keeps waiting. Add \
                     `--retry` to the lookup, and treat a much larger wall-clock gap than \
                     you slept for as a suspended machine rather than a result."
                .to_string(),
            span: cmd.at..cmd.end,
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

    /// The exact loop that produced a wrong CI verdict on 2026-09-06.
    #[test]
    fn the_incident_shape_fires() {
        assert!(fires(
            r#"for i in $(seq 1 200); do s=$(curl -sS https://x/status | jq -r .state); if [ "$s" != "pending" ]; then echo "CI: $s"; exit 0; fi; sleep 20; done"#
        ));
    }

    #[test]
    fn the_while_form_fires_too() {
        assert!(fires(
            r#"while [ "$state" = "pending" ]; do state=$(gh run view 1 --json status --jq .status); sleep 30; done"#
        ));
    }

    /// Naming the terminal values is the remedy, so it must be silent.
    #[test]
    fn naming_the_terminal_values_is_silent() {
        assert!(!fires(
            r#"for i in $(seq 1 200); do s=$(curl -sS https://x/status | jq -r .state); case "$s" in success|failure|error) echo "CI: $s"; exit 0 ;; esac; sleep 20; done"#
        ));
    }

    /// `until [ "$s" = done ]` leaves only on the value it names; a blank keeps
    /// waiting, which is the correct behaviour.
    #[test]
    fn waiting_for_a_named_value_is_silent() {
        assert!(!fires(
            r#"until [ "$s" = "completed" ]; do s=$(gh run view 1 --json status --jq .status); sleep 20; done"#
        ));
        assert!(!fires(
            r#"while [ "$s" != "completed" ]; do s=$(gh run view 1 --json status --jq .status); sleep 20; done"#
        ));
    }

    /// A literal the loop cannot get wrong is not this rule's business.
    #[test]
    fn a_constant_comparison_is_silent() {
        assert!(!fires(
            r#"for i in 1 2 3; do if [ "$i" != "2" ]; then echo "$i"; fi; sleep 1; done"#
        ));
    }

    /// The same test outside a wait is an ordinary conditional.
    #[test]
    fn a_conditional_without_a_wait_is_silent() {
        assert!(!fires(
            r#"s=$(git rev-parse HEAD); if [ "$s" != "$other" ]; then echo differ; fi"#
        ));
        assert!(!fires("git status"));
    }
}
