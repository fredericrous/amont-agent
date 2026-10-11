//! `symbol-grep-not-lsp` — a text search for a symbol, over code LSP serves.
//!
//! ```sh
//! grep -n "fn lease_is_stale" -A25 crates/relais/src/admission/mod.rs
//! grep -rn "EMAIL_PX" ../ --include=*.ts -l
//! rg -t rust agent_types
//! ```
//!
//! Measured with `amont-agent backtest` (2026-10-11): 46.8 to 59.0 searches
//! of this shape per 1,000 Bash calls over the last four weeks, and 4.5 per
//! 1,000 tool calls through the Grep tool in the last one. A text search for
//! a symbol finds strings, comments and same-named symbols alike, and
//! returns lines without what they mean; `workspaceSymbol` and
//! `findReferences` return the definition and its uses.
//!
//! `examine` reads the shape: `grep`, `egrep`, `rg` or `git grep`, not fed
//! from a pipe, with an identifier-shaped pattern ([`symbol`]) and a code
//! target — a code file operand, an include or type naming code, or a
//! recursive search with no file operand that is not code. A pipe filter
//! (`cmd | grep foo`), a count (`-c`), an inverted match (`-v`), a
//! case-insensitive one (`-i`), patterns from a file (`-f`), several
//! patterns (`-e a -e b`) or a `$VAR` path are text work or unknowable, and
//! stay silent. `confirm` asks [`crate::lsp::serves`] about the target. The
//! Grep tool is judged by the hook with [`symbol`] and [`grep_tool_targets_code`].

use crate::rules::{Confirmed, Context, Evidence, Examine, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple, Word};

pub const RULE: Rule = Rule {
    id: "symbol-grep-not-lsp",
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        // `amont-agent backtest`, per 1,000 Bash calls, weeks from 09-07:
        // 25.4, 46.8, 55.9, 59.2, 54.4. Hand-labelled samples of 50 hits:
        // 34 real symbol lookups before the shape test (lowercase words,
        // `-i`, `$` paths), 44 after. The Grep tool (4.5 per 1,000 tool calls
        // in W41) is not replayed; fixtures check it.
        per_1000: 54.4,
        measured: "2026-10-11",
        trend: Trend::Flat(5),
    },
    examine: Examine::Legacy(examine),
    confirm: Some(confirm),
};

/// Comment markers: text, whatever their shape.
const STOP: &[&str] = &["TODO", "FIXME", "XXX", "HACK", "NOTE"];

/// Declaration keywords a symbol search may start with.
const KEYWORDS: &[&str] = &[
    "pub fn",
    "fn",
    "def",
    "func",
    "class",
    "struct",
    "impl",
    "interface",
    "type",
    "const",
    "let",
    "enum",
    "trait",
];

/// An identifier of three characters or more, optionally behind one
/// declaration keyword and inside word boundaries: `agent_types`,
/// `\bFileOp\b`, `fn lease_is_stale`, `def parse(`. Anything with regex
/// alternation, a class, a path or a space elsewhere is text.
///
/// Without a keyword the word must look like a symbol — an uppercase letter
/// or an underscore. In a hand-labelled sample of 50 hits (2026-10-11), the
/// lowercase words (`sandbox`, `derive`, `inline`, `intent`) were nearly all
/// searches for text; the camelCase, PascalCase and snake_case ones were
/// symbols.
pub fn symbol(pattern: &str) -> bool {
    let mut p = pattern.trim();
    for open in ["\\b", "\\<", "^"] {
        p = p.strip_prefix(open).unwrap_or(p);
    }
    for close in ["\\b", "\\>", "\\(", "("] {
        p = p.strip_suffix(close).unwrap_or(p);
    }
    let mut declared = false;
    for kw in KEYWORDS {
        if let Some(rest) = p.strip_prefix(kw).and_then(|r| r.strip_prefix(' ')) {
            p = rest;
            declared = true;
            break;
        }
    }
    let shaped = declared || p.contains('_') || p.chars().any(|c| c.is_ascii_uppercase());
    let mut chars = p.chars();
    let starts = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    starts
        && p.len() >= 3
        && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && shaped
        && !STOP.contains(&p)
}

/// Where a search looks, as far as the command says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A code file operand.
    File(String),
    /// A directory operand, or the cwd when `None`.
    Dir(Option<String>),
}

/// A symbol search, read from one clause.
#[derive(Debug)]
pub struct Search<'a> {
    pub pattern: &'a str,
    pub target: Target,
    /// `git -C <dir>`: where the search runs, before any operand.
    pub run_in: Option<&'a str>,
}

/// Flags that take the next word (or the rest of a short cluster) as a value.
fn takes_value(program: &str, flag: &str) -> bool {
    let grep: &[&str] = &[
        "-A",
        "-B",
        "-C",
        "-e",
        "-f",
        "-m",
        "-d",
        "-D",
        "--include",
        "--exclude",
        "--exclude-dir",
        "--max-count",
        "--regexp",
        "--file",
        "--context",
        "--after-context",
        "--before-context",
        "--color",
        "--colour",
        "--label",
    ];
    let rg: &[&str] = &[
        "-e",
        "-f",
        "-g",
        "-t",
        "-T",
        "-A",
        "-B",
        "-C",
        "-m",
        "-j",
        "-M",
        "--type",
        "--type-not",
        "--type-add",
        "--glob",
        "--iglob",
        "--max-count",
        "--regexp",
        "--file",
        "--context",
        "--after-context",
        "--before-context",
        "--threads",
        "--max-columns",
        "--color",
        "--colors",
        "--sort",
        "--sortr",
        "--max-depth",
        "--max-filesize",
    ];
    let git: &[&str] = &[
        "-A",
        "-B",
        "-C",
        "-e",
        "-f",
        "-m",
        "--max-count",
        "--context",
        "--after-context",
        "--before-context",
        "--max-depth",
        "--threads",
        "--color",
        "--open-files-in-pager",
    ];
    match program {
        "rg" => rg.contains(&flag),
        "git grep" => git.contains(&flag),
        _ => grep.contains(&flag),
    }
}

/// Short flags that may carry their value in the same word (`-A25`, `-trust`).
fn short_with_value(program: &str) -> &'static [char] {
    match program {
        "rg" => &['e', 'f', 'g', 't', 'T', 'A', 'B', 'C', 'm', 'j', 'M'],
        _ => &['A', 'B', 'C', 'e', 'f', 'm', 'd', 'D'],
    }
}

/// One clause read as a symbol search, or `None`.
pub fn search(cmd: &Simple) -> Option<Search<'_>> {
    if cmd.prev.is_some_and(|c| c.is_pipe()) || cmd.heredoc {
        return None;
    }
    let raw = cmd.program()?;
    let name = raw.rsplit('/').next().unwrap_or(raw);
    let args: &[Word] = cmd.args();
    let (program, args, run_in) = match name {
        "grep" | "egrep" => (name, args, None),
        "rg" => ("rg", args, None),
        "git" => {
            let mut run_in = None;
            let mut i = 0;
            while let Some(w) = args.get(i) {
                match w.text.as_str() {
                    "-C" => {
                        run_in = args.get(i + 1).map(|w| w.text.as_str());
                        i += 2;
                    }
                    "-c" | "--git-dir" | "--work-tree" | "--namespace" => i += 2,
                    "grep" => break,
                    t if t.starts_with('-') => i += 1,
                    _ => return None,
                }
            }
            ("git grep", args.get(i + 1..)?, run_in)
        }
        _ => return None,
    };

    let mut patterns: Vec<&str> = Vec::new();
    let mut selectors: Vec<&str> = Vec::new();
    let mut operands: Vec<&Word> = Vec::new();
    let mut recursive = program != "grep" && program != "egrep";
    let mut silenced = false;
    let mut after_ddash = false;
    let mut i = 0;
    while let Some(w) = args.get(i) {
        i += 1;
        let t = w.text.as_str();
        // A `$VAR` or `$(…)` names something this rule cannot see.
        if w.expanded || t.contains('$') {
            return None;
        }
        // A word whose quotes start after its leading dash is still a flag:
        // `--include='*.{ts,tsx}'`.
        let flag = t.starts_with('-') && t != "-" && (!w.quoted || w.raw.starts_with('-'));
        if after_ddash || !flag {
            operands.push(w);
            continue;
        }
        if t == "--" {
            after_ddash = true;
            continue;
        }
        if let Some(long) = t.strip_prefix("--") {
            let (flag, inline) = match long.split_once('=') {
                Some((f, v)) => (format!("--{f}"), Some(v)),
                None => (t.to_string(), None),
            };
            let value = if takes_value(program, &flag) {
                match inline {
                    Some(v) => Some(v),
                    None => {
                        i += 1;
                        args.get(i - 1).map(|w| w.text.as_str())
                    }
                }
            } else {
                None
            };
            match flag.as_str() {
                "--regexp" => patterns.extend(value),
                "--file" | "--count" | "--invert-match" | "--count-matches" | "--ignore-case" => {
                    silenced = true
                }
                "--include" | "--glob" | "--iglob" | "--type" => selectors.extend(value),
                "--recursive" | "--dereference-recursive" => recursive = true,
                _ => {}
            }
            continue;
        }
        // A short cluster: `-rn`, `-A25`, `-e pat`, `-trust`.
        let cluster = &t[1..];
        for (k, c) in cluster.char_indices() {
            if short_with_value(program).contains(&c) {
                let rest = &cluster[k + c.len_utf8()..];
                let value = if rest.is_empty() {
                    i += 1;
                    args.get(i - 1).map(|w| w.text.as_str())
                } else {
                    Some(rest)
                };
                match c {
                    'e' => patterns.extend(value),
                    'f' => silenced = true,
                    'g' | 't' if program == "rg" => selectors.extend(value),
                    _ => {}
                }
                break;
            }
            match c {
                'v' | 'c' | 'i' => silenced = true,
                'r' | 'R' => recursive = true,
                _ => {}
            }
        }
    }
    if silenced || patterns.len() > 1 {
        return None;
    }
    let mut operands = operands.into_iter();
    let pattern = match patterns.first() {
        Some(p) => *p,
        None => operands.next()?.text.as_str(),
    };
    if !symbol(pattern) {
        return None;
    }
    let paths: Vec<&str> = operands.map(|w| w.text.as_str()).collect();
    if let Some(code) = paths.iter().find(|p| crate::lsp::is_code(p)) {
        return Some(Search {
            pattern,
            target: Target::File((*code).to_string()),
            run_in,
        });
    }
    let dir = paths.first().map(|p| (*p).to_string());
    if !selectors.is_empty() {
        return selectors
            .iter()
            .any(|s| crate::lsp::names_code(s))
            .then_some(Search {
                pattern,
                target: Target::Dir(dir),
                run_in,
            });
    }
    // A plain grep with no path reads stdin; one with only file operands
    // that are not code searches text.
    if !recursive || paths.iter().any(|p| file_shaped(p)) {
        return None;
    }
    Some(Search {
        pattern,
        target: Target::Dir(dir),
        run_in,
    })
}

/// A last component with an extension: a file, not a directory.
fn file_shaped(path: &str) -> bool {
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    if name.chars().all(|c| c == '.') {
        return false;
    }
    name.get(1..).is_some_and(|rest| rest.contains('.'))
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    parsed.judgeable().find_map(|cmd| {
        let s = search(cmd)?;
        let (reason, remedy) = phrase(s.pattern);
        Some(Finding {
            reason,
            remedy,
            span: cmd.at..cmd.end,
        })
    })
}

/// The search the finding was made from, read again from the parse.
pub fn search_at(parsed: &Parsed, at: usize) -> Option<Search<'_>> {
    parsed.judgeable().find(|c| c.at == at).and_then(search)
}

fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    let Some(s) = search_at(ctx.parsed, f.span.start) else {
        return Confirmed::No("the command no longer matches");
    };
    let mut base = ctx.cwd_at(f.span.start);
    if let Some(dir) = s.run_in {
        base = base.join(expand_home(dir));
    }
    let target = match &s.target {
        Target::File(p) | Target::Dir(Some(p)) => base.join(expand_home(p)),
        Target::Dir(None) => base,
    };
    match crate::lsp::serves(&target) {
        Ok(()) => Confirmed::Yes,
        Err(why) => Confirmed::No(why),
    }
}

fn expand_home(p: &str) -> std::path::PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => std::path::PathBuf::from(home).join(rest),
        _ => std::path::PathBuf::from(p),
    }
}

/// What a Grep tool call's `path` names, as the filesystem says: a path's
/// spelling does not tell a file from a directory (`C:\repo\.`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrepScope {
    /// A regular file, by its file name.
    File(String),
    /// A directory, or the cwd when `path` is empty.
    Dir,
    /// Nothing is there: a mistyped path searches nothing.
    Missing,
}

/// A Grep tool call that searches code: a code file, or a directory with a
/// glob or type naming code, or with neither.
pub fn grep_tool_targets_code(scope: &GrepScope, glob: Option<&str>, kind: Option<&str>) -> bool {
    match scope {
        GrepScope::File(name) => crate::lsp::is_code(name),
        GrepScope::Missing => false,
        GrepScope::Dir => {
            let selectors: Vec<&str> = glob.into_iter().chain(kind).collect();
            selectors.is_empty() || selectors.iter().any(|s| crate::lsp::names_code(s))
        }
    }
}

/// What the hook says: the mechanism, then the way through.
pub fn phrase(pattern: &str) -> (String, String) {
    (
        format!(
            "`{pattern}` is a symbol: a text search finds strings, comments and same-named \
             symbols alike, and returns lines without what they mean."
        ),
        "Ask LSP: workspaceSymbol for the definition; findReferences or incomingCalls for \
         its uses. Keep grep for text LSP cannot see: strings, comments, config, logs."
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    fn target(command: &str) -> Target {
        let parsed = lex(command);
        let cmd = parsed.judgeable().next().expect("a clause");
        search(cmd).expect("a search").target
    }

    #[test]
    fn symbols_are_identifiers() {
        for p in [
            "agent_types",
            "\\bFileOp\\b",
            "fn lease_is_stale",
            "def parse(",
            "pub fn run",
            "EMAIL_PX",
        ] {
            assert!(symbol(p), "{p}");
        }
        for p in [
            "TODO",
            "FIXME",
            "sandbox",
            "derive",
            "ab",
            "foo\\|bar",
            "a.b",
            "fn .*",
            "some words",
            "^pub mod",
            "x-y",
            "[A-Z]+",
            "",
        ] {
            assert!(!symbol(p), "{p}");
        }
    }

    #[test]
    fn a_symbol_search_over_code_fires() {
        assert!(fires(
            "grep -n \"fn lease_is_stale\" -A25 crates/relais/src/admission/mod.rs"
        ));
        assert!(fires("grep -rn agent_types src/"));
        assert!(fires("grep -rn EMAIL_PX ../ --include=*.ts -l"));
        assert!(fires(
            "grep -rn EMAIL_PX ../ --include '*.ts' --exclude-dir node_modules"
        ));
        assert!(fires("rg agent_types"));
        assert!(fires("rg -t rust agent_types crates"));
        assert!(fires("rg -g '*.rs' FileOp"));
        assert!(fires("git grep -n FileOp"));
        assert!(fires("git -C ../relais grep -n FileOp -- crates"));
        assert!(fires("cd /x && grep -n parse_args src/main.go | head"));
        assert!(fires("grep -e FileOp -r src"));
        assert!(
            fires("grep -rn FileOp --include='*.{ts,tsx}' ."),
            "a quoted include"
        );
        assert!(fires("grep -rn FileOp .."), "a parent directory");
    }

    #[test]
    fn text_work_is_silent() {
        assert!(!fires("cargo test | grep FileOp"), "a pipe filter");
        assert!(!fires("grep FileOp"), "stdin");
        assert!(!fires("grep -c FileOp src/hook.rs"), "a count");
        assert!(!fires("grep -rv FileOp src"), "inverted");
        assert!(!fires("grep -rn -f pats.txt src"), "patterns from a file");
        assert!(!fires("grep -rn -e FileOp -e Bash src"), "several patterns");
        assert!(!fires("grep -rn TODO src"), "a comment marker");
        assert!(!fires("grep -n FileOp notes.md"), "not code");
        assert!(
            !fires("grep -rn FileOp docs/rules.md CHANGELOG.md"),
            "text files"
        );
        assert!(!fires("grep -rn 'foo\\|bar' src"), "alternation");
        assert!(!fires("rg -t md FileOp"), "a type that is not code");
        assert!(
            !fires("grep -rn FileOp . --include='*.md'"),
            "an include that is not code"
        );
        assert!(!fires("grep -n \"$X\" src/a.rs"), "unknowable");
        assert!(!fires("git log --grep FileOp"), "not git grep");
        assert!(!fires("grep -rn sandbox src"), "a plain lowercase word");
        assert!(!fires("grep -rni FileOp src"), "case-insensitive");
        assert!(
            !fires("grep -rln FileOp $R/src"),
            "a path behind a variable"
        );
    }

    #[test]
    fn the_target_is_the_code_file_or_the_directory() {
        assert_eq!(
            target("grep -n FileOp src/payload.rs"),
            Target::File("src/payload.rs".into())
        );
        assert_eq!(
            target("grep -rn FileOp src"),
            Target::Dir(Some("src".into()))
        );
        assert_eq!(target("rg FileOp"), Target::Dir(None));
        let parsed = lex("git -C ../relais grep FileOp");
        let s = search(parsed.judgeable().next().expect("clause")).expect("search");
        assert_eq!(s.run_in, Some("../relais"));
    }

    #[test]
    fn the_grep_tool_targets_code_by_file_glob_or_type() {
        let file = |n: &str| GrepScope::File(n.to_string());
        assert!(grep_tool_targets_code(&file("hook.rs"), None, None));
        assert!(!grep_tool_targets_code(&file("x.yaml"), None, None));
        assert!(grep_tool_targets_code(&GrepScope::Dir, None, None));
        assert!(grep_tool_targets_code(
            &GrepScope::Dir,
            Some("**/*.{ts,tsx}"),
            None
        ));
        assert!(grep_tool_targets_code(&GrepScope::Dir, None, Some("rust")));
        assert!(!grep_tool_targets_code(
            &GrepScope::Dir,
            Some("**/*macro*"),
            None
        ));
        assert!(!grep_tool_targets_code(&GrepScope::Dir, None, Some("yaml")));
        assert!(!grep_tool_targets_code(
            &GrepScope::Missing,
            None,
            Some("rust")
        ));
    }

    #[test]
    fn the_advice_stays_under_sixty_words() {
        let (reason, remedy) = phrase("lease_is_stale");
        let words = reason.split_whitespace().count() + remedy.split_whitespace().count();
        assert!(words <= 60, "{words} words");
    }
}
