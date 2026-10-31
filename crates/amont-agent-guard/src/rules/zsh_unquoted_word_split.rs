//! `zsh-unquoted-word-split` — an unquoted `$var` used as a word list after
//! the same call assigned `var` a space-separated literal.
//!
//! ```sh
//! ids="1 2 3"; for id in $ids; do echo $id; done
//! pr="a/b 385"; set -- $pr
//! ```
//!
//! bash splits an unquoted expansion on `IFS`; zsh does not, so the loop runs
//! once over the whole string and `$1` holds all of it. `unsplit-expansion`
//! reads the idiom by shape alone; this rule fires only where the call itself
//! shows the value is several words, so the mistake is certain rather than
//! likely. It ships `observe` and cannot be raised: it journals, never speaks.

use crate::rules::tool_shell;
use crate::rules::unsplit_expansion::{scalar_param, OPENERS};
use crate::rules::{Confirmed, Context, Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Resolved, Simple, Word};

pub const RULE: Rule = Rule {
    id: "zsh-unquoted-word-split",
    prevents: "An unquoted variable assigned a space-separated literal and then used as a word list, which zsh does not split",
    irreversible: false,
    default_stance: Stance::Observe,
    max_stance: Stance::Observe,
    evidence: Evidence {
        // Not yet measured: the sibling `unsplit-expansion` fires 1.4 per
        // thousand on the shape alone, and this is a subset of it.
        per_1000: 0.0,
        measured: "2026-10-31",
        trend: Trend::Rare,
    },
    examine: Examine::Legacy(examine),
    confirm: Some(confirm),
};

/// The operands of `set [--]` or `for x in`, when the clause is one.
fn word_list<'a>(words: &'a [&'a Word]) -> &'a [&'a Word] {
    let mut i = 0;
    while i < words.len() && !words[i].quoted && OPENERS.contains(&words[i].text.as_str()) {
        i += 1;
    }
    let Some(head) = words.get(i).filter(|w| !w.quoted) else {
        return &[];
    };
    match head.text.as_str() {
        "set" => {
            let skip_dashes = words
                .get(i + 1)
                .is_some_and(|w| !w.quoted && w.text == "--");
            &words[i + 1 + usize::from(skip_dashes)..]
        }
        "for"
            if words
                .get(i + 2)
                .is_some_and(|w| !w.quoted && w.text == "in") =>
        {
            &words[i + 3..]
        }
        _ => &[],
    }
}

fn holds_several_words(parsed: &Parsed, cmd: &Simple, w: &Word) -> bool {
    matches!(
        parsed.word_value(cmd.at, w),
        Resolved::Text(value) if value.split_whitespace().count() > 1
    )
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.judgeable() {
        let words: Vec<&Word> = cmd.words.iter().collect();
        let Some(w) = word_list(&words)
            .iter()
            .find(|w| scalar_param(w) && holds_several_words(parsed, cmd, w))
        else {
            continue;
        };
        return Some(Finding {
            reason: format!(
                "`{}` was assigned several space-separated words earlier in this call, but zsh \
                 does not split an unquoted expansion (SH_WORD_SPLIT is off): the loop or the \
                 positionals get the whole string as one word.",
                w.text
            ),
            remedy: String::from(
                "Use an array (`ids=(1 2 3)` and `\"${ids[@]}\"`), force the split with \
                 `${=var}`, or run the loop under `bash`.",
            ),
            span: w.at..w.at + w.raw.len(),
        });
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
    fn a_word_list_variable_assigned_several_words_fires() {
        assert!(fires("ids=\"1 2 3\"; for id in $ids; do echo $id; done"));
        assert!(fires("pr=\"a/b 385\"; set -- $pr"));
    }

    #[test]
    fn arrays_quotes_single_words_and_unknown_values_are_silent() {
        assert!(!fires(
            "ids=(1 2 3); for id in \"${ids[@]}\"; do echo $id; done"
        ));
        assert!(!fires("for f in $HOME; do :; done"));
        assert!(!fires("ids=\"1 2 3\"; for id in \"$ids\"; do :; done"));
        assert!(!fires("one=x; for id in $one; do :; done"));
        assert!(!fires("for id in $ids; do :; done"));
    }

    #[test]
    fn the_rule_can_only_observe() {
        assert_eq!(RULE.default_stance, Stance::Observe);
        assert_eq!(RULE.max_stance, Stance::Observe);
    }
}
