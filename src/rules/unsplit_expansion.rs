//! `unsplit-expansion` — an unquoted `$var` where the command only makes
//! sense if the shell splits it into words.
//!
//! ```sh
//! for rp in "sre-agent 41" "stealth-fetch 17"; do set -- $rp; merge "$1" "$2"; done
//! files=$(git grep -l pattern); for f in $files; do sed -i 's/a/b/' "$f"; done
//! ```
//!
//! bash splits an unquoted parameter expansion on `IFS`; zsh does not
//! (`SH_WORD_SPLIT` is off), so `$rp` is one word whatever it holds. `set --
//! $rp` then sets `$1` to the whole string and `$2` to nothing, and `for f in
//! $files` runs once with every path glued into one name. Nothing errors:
//! the command runs with the wrong arguments and reports success — the six
//! merges of 2026-09-21 hit `curl …/repos/fredericrous/sre-agent 41/pulls//merge`
//! (URL rejected, six times) and the Prometheus query the same day fed an
//! empty window into `increase(…[0s])`.
//!
//! Only the two idioms whose sole purpose is splitting are read: `set [--]
//! $var` and `for x in $var`. A plain `cmd $var` is not — one word is usually
//! what it means. `$(…)` is exempt because zsh DOES split an unquoted command
//! substitution; `"$var"`, `$@`, `$*` and positionals are exempt because they
//! are either deliberate or already split. A `${var}` is blanked by the lexer
//! and so is not seen; the bare spelling is what the transcripts carry.
//!
//! Priced on 44,929 real calls (2026-08-10..09-21): 65 firings, 1.4 per
//! thousand and flat week over week (0.8 · 1.6 · 1.4 · 1.2 · 0.9), 8.1 the
//! week it was written. Every sampled hit is the idiom itself — `set -- $pair`
//! over a list of quoted pairs, `for x in $f` over a `grep -l`/`find`/`ls`
//! result — so precision is not the question; whether the guard changes the
//! habit is, and that is what `observe` measures first.

use crate::rules::tool_shell;
use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Word};

pub const RULE: Rule = Rule {
    id: "unsplit-expansion",
    default_stance: Stance::Observe,
    evidence: Evidence {
        // 65 firings in 44,929 calls; per full week 0.8 · 1.6 · 1.4 · 1.2 ·
        // 0.9 per thousand, no downward drift without a guard.
        per_1000: 1.4,
        measured: "2026-09-21",
        trend: Trend::Flat(5),
    },
    examine,
    confirm: Some(confirm),
};

/// Words that may open a clause without being its command.
const OPENERS: &[&str] = &[
    "do", "then", "else", "elif", "if", "while", "until", "!", "{", "(", "time",
];

/// `$name` and nothing else: a scalar parameter whose value we cannot know.
/// `$@`, `$*`, `$1`, `$?` and friends are not names, and a name with a suffix
/// (`$dir/x`) is a path, not a list.
fn scalar_param(w: &Word) -> bool {
    if w.quoted || w.expanded {
        return false;
    }
    let mut chars = w.text.chars();
    if chars.next() != Some('$') {
        return false;
    }
    let name: &str = &w.text[1..];
    let mut it = name.chars();
    match it.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    it.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn finding(w: &Word, idiom: &str) -> Finding {
    Finding {
        reason: format!(
            "zsh does not split an unquoted parameter expansion into words (SH_WORD_SPLIT \
             is off), so in `{idiom}` `{}` is ONE word whatever it holds: the positionals \
             or the loop see the whole string glued together, nothing errors, and the \
             command runs with the wrong arguments while reporting success.",
            w.text
        ),
        remedy: String::from(
            "Split on purpose: `read -r a b <<<\"$var\"` (or `while read -r a b; do …; \
             done <<'EOF'` for a list of pairs), iterate a command substitution — \
             `for f in $(cmd)` DOES split under zsh — or put the loop in a file and run \
             it with `bash`. Inline under zsh, `${=var}` forces the split.",
        ),
        span: w.at..w.at + w.raw.len(),
    }
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.judgeable() {
        let words: Vec<&Word> = cmd.words.iter().collect();
        let mut i = 0;
        while i < words.len() && !words[i].quoted && OPENERS.contains(&words[i].text.as_str()) {
            i += 1;
        }
        let Some(head) = words.get(i) else { continue };
        if head.quoted {
            continue;
        }
        match head.text.as_str() {
            // `set -- $var`, `set $var`: the first operand is the one that matters.
            "set" => {
                let mut j = i + 1;
                if words.get(j).is_some_and(|w| !w.quoted && w.text == "--") {
                    j += 1;
                }
                if let Some(w) = words.get(j).filter(|w| scalar_param(w)) {
                    return Some(finding(w, format!("set -- {}", w.text).as_str()));
                }
            }
            // `for x in $var …`: any bare scalar in the list.
            "for"
                if words
                    .get(i + 2)
                    .is_some_and(|w| !w.quoted && w.text == "in") =>
            {
                for w in &words[i + 3..] {
                    if scalar_param(w) {
                        return Some(finding(w, format!("for … in {}", w.text).as_str()));
                    }
                }
            }
            _ => {}
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
    fn set_and_for_over_a_bare_scalar_fire() {
        assert!(fires(
            "for rp in \"sre-agent 41\" \"stealth-fetch 17\"; do set -- $rp; echo \"$1 $2\"; done"
        ));
        assert!(fires(
            "files=$(git grep -l x); for f in $files; do sed -i 's/a/b/' \"$f\"; done"
        ));
        assert!(fires("set -- $args; echo $1"));
        assert!(fires("set $args; echo $1"));
        assert!(fires("for w in $windows; do echo $w; done"));
        assert!(fires("if true; then for x in $list; do echo $x; done; fi"));
    }

    #[test]
    fn deliberate_or_already_split_forms_are_silent() {
        assert!(!fires("set -- \"$@\"; echo $1"));
        assert!(!fires("set -- $@"));
        assert!(!fires("set -- $*"));
        assert!(!fires("set -e; set -o pipefail"));
        assert!(!fires("set -- one two three"));
        assert!(!fires("for f in $(git grep -l x); do echo \"$f\"; done"));
        assert!(!fires("for f in \"$@\"; do echo \"$f\"; done"));
        assert!(!fires("for i in 1 2 3; do echo $i; done"));
        assert!(!fires("for f in $dir/*.rs; do echo \"$f\"; done"));
        assert!(!fires("for f in \"$files\"; do echo \"$f\"; done"));
        assert!(!fires("echo $HOME; ls $dir"));
        assert!(!fires("for f in ${files}; do echo \"$f\"; done"));
        assert!(!fires("read -r a b <<<\"$rp\"; echo \"$a\""));
    }

    #[test]
    fn the_span_is_the_expansion() {
        let src = "for rp in \"a 1\" \"b 2\"; do set -- $rp; done";
        let f = examine(&lex(src)).unwrap();
        assert_eq!(&src[f.span], "$rp");
    }
}
