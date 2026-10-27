//! `cd-carries-over` — a top-level `cd` or `pushd`, whose directory change
//! outlives the call.
//!
//! Claude Code's Bash tool carries a successful `cd` into the next call,
//! including the next call of the same parallel batch. A model that writes
//! `cd /repo && git status` believes the move is local to that command; it is
//! not, and every sibling call then runs somewhere it did not expect.
//!
//! A `cd` inside `( … )`, `$( … )`, a function body or a `bash -c` string
//! moves nothing outside it and is not matched. Neither is a `cd` into the
//! temp directories: the harness resets the cwd for a path outside the
//! working directories, so nothing carries.

use crate::rules::{Evidence, Examine, Finding, Input, Rule, Stance, Trend};

pub const RULE: Rule = Rule {
    id: "cd-carries-over",
    prevents: "A directory change that carries into the next Bash call, so a sibling call runs in a directory its author never chose",
    irreversible: false,
    default_stance: Stance::Observe,
    max_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 0.0,
        measured: "2026-10-27",
        trend: Trend::Rare,
    },
    examine: Examine::Analysis(examine),
    confirm: None,
};

/// Directories outside every working directory, where the harness resets the
/// cwd after the call.
const RESET_PREFIXES: &[&str] = &[
    "/tmp",
    "/private/tmp",
    "/var/folders",
    "/private/var/folders",
];

fn examine(input: &Input) -> Option<Finding> {
    for cmd in input.legacy.judgeable() {
        if cmd.nested.is_some() {
            continue;
        }
        let Some(program) = cmd.program() else {
            continue;
        };
        if program != "cd" && program != "pushd" {
            continue;
        }
        match cmd.operands().first() {
            // A bare `cd` goes to $HOME, which is not a working directory.
            None if program == "cd" => continue,
            Some(target) if is_reset_path(&target.text) => continue,
            _ => {}
        }
        // The clause's own start can sit before a grouping `(` the lexer
        // consumed, so the scope is read up to the program word itself.
        let program_at = cmd
            .words
            .iter()
            .find(|w| w.text == program)
            .map_or(cmd.at, |w| w.at);
        if is_scoped(input.src, program_at) {
            continue;
        }
        return Some(Finding {
            reason: format!(
                "`{program}` at the top level of a command changes the directory of the \
                 Bash tool's shell, and a successful change is carried into the next \
                 call, including the next call of the same parallel batch. On \
                 2026-10-27 a batch of `cd …/relais && pwd` followed by a sibling \
                 `pwd` printed `…/relais`, while a subshell `(cd … && pwd)` did not \
                 carry."
            ),
            remedy: "Scope the move: a subshell `(cd X && …)`, `git -C X …`, or absolute \
                     paths."
                .to_string(),
            span: cmd.at..cmd.end,
        });
    }
    None
}

fn is_reset_path(target: &str) -> bool {
    RESET_PREFIXES
        .iter()
        .any(|p| target == *p || target.starts_with(&format!("{p}/")))
}

/// Whether the command at byte `at` sits inside a `( … )` group or a function
/// body, where its directory change stays.
fn is_scoped(src: &str, at: usize) -> bool {
    let bytes = src.as_bytes();
    let end = at.min(bytes.len());
    let mut parens = 0usize;
    // One entry per open `{` at word start: true for a function body.
    let mut braces: Vec<bool> = Vec::new();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < end {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == b'\\' && q == b'"' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'\\' => i += 1,
                b'\'' | b'"' => quote = Some(c),
                b'(' => parens += 1,
                b')' => parens = parens.saturating_sub(1),
                b'{' if starts_word(bytes, i) && ends_word(bytes, i) => {
                    braces.push(opens_function(&src[..i]));
                }
                b'}' if starts_word(bytes, i) && ends_word(bytes, i) => {
                    braces.pop();
                }
                _ => {}
            },
        }
        i += 1;
    }
    parens > 0 || braces.contains(&true)
}

fn starts_word(bytes: &[u8], i: usize) -> bool {
    i == 0
        || matches!(
            bytes[i - 1],
            b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'('
        )
}

fn ends_word(bytes: &[u8], i: usize) -> bool {
    i + 1 >= bytes.len() || matches!(bytes[i + 1], b' ' | b'\t' | b'\n' | b';')
}

/// `name() {` or `function name {`: the text before the brace says which.
fn opens_function(before: &str) -> bool {
    let trimmed = before.trim_end();
    if trimmed.ends_with(')') {
        return true;
    }
    trimmed
        .rsplit(|c: char| c.is_whitespace() || c == ';')
        .nth(1)
        .is_some_and(|word| word == "function")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Dialect;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        let parsed = lex(command);
        let input = Input::new(command, Dialect::Zsh, &parsed);
        RULE.judge(&input).is_some()
    }

    #[test]
    fn a_top_level_cd_or_pushd_matches() {
        assert!(fires("cd /repo && git status"));
        assert!(fires("cd /repo"));
        assert!(fires("pushd /repo && make"));
        assert!(fires("echo hi; cd ~/work && ls"));
    }

    #[test]
    fn a_scoped_directory_change_does_not_match() {
        assert!(!fires("(cd /repo && git status)"));
        assert!(!fires("git -C /repo status"));
        assert!(!fires("echo $(cd /repo && pwd)"));
        assert!(!fires("bash -c 'cd /repo && make'"));
        assert!(!fires("f() { cd /repo; }; f"));
        assert!(!fires("function f { cd /repo; }; f"));
    }

    #[test]
    fn a_brace_group_is_not_a_subshell() {
        assert!(fires("{ cd /repo; git status; }"));
    }

    #[test]
    fn a_cd_after_a_closed_subshell_matches() {
        assert!(fires("(echo hi) && cd /repo"));
    }

    #[test]
    fn a_path_the_harness_resets_does_not_match() {
        assert!(!fires("cd /tmp && ls"));
        assert!(!fires("cd /private/tmp/x && ls"));
        assert!(fires("cd /tmpfoo && ls"));
    }

    #[test]
    fn the_advice_names_the_evidence() {
        let command = "cd /repo";
        let parsed = lex(command);
        let finding = RULE
            .judge(&Input::new(command, Dialect::Zsh, &parsed))
            .expect("fires");
        assert!(finding.reason.contains("2026-10-27"));
        assert!(finding.remedy.contains("(cd X && …)"));
        assert!(finding.remedy.contains("git -C X"));
    }
}
