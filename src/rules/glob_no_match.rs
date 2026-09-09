//! `glob-no-match` — an unquoted glob operand that matches nothing.
//!
//! ```sh
//! grep -rn "external_id" scripts/seed-dev.ts app/lib/db/migrations/*.sql 2>/dev/null | head
//! cat .claude/*.md 2>/dev/null | head -150
//! ```
//!
//! Under zsh a glob that matches nothing is not an empty argument list, it is
//! an error — `no matches found: app/lib/db/migrations/*.sql` — raised by the
//! shell before the command starts, so the command never starts. Two things
//! make that silent rather than loud. The message goes to stderr from the
//! shell itself, so the `2>/dev/null` the author added (69% of real cases)
//! does nothing to it, and the exit status is the LAST clause's: in a `;` or
//! `|` chain the next clause runs and the tool reports success. Measured over
//! 32,555 calls: 212 of these, 86% reported as success, and in 170 of them
//! the same clause also named files that do exist — whose grep never ran
//! either. 85% of the time the next call was unrelated: the empty result was
//! taken as the answer.
//!
//! The shape alone is not the mistake; `ls *.py` usually works. So `examine`
//! fires on the shape and `confirm` expands the pattern against the directory
//! the clause runs in, and stays silent when anything matches. A program
//! whose whole job is the existence question — `ls .amont*`, `echo *.sql` —
//! with nothing but patterns to look at is left alone: "no matches" is its
//! answer.

use crate::rules::tool_shell;
use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple, Word};
use std::path::{Path, PathBuf};

pub const RULE: Rule = Rule {
    id: "glob-no-match",
    default_stance: Stance::Advise,
    evidence: Evidence {
        // The shape rate: every unquoted glob operand the backtester can see.
        // The FAILURE rate from the shell's own diagnostics is 6.5 per 1000
        // (8.3 · 7.9 · 5.8 · 5.8 · 7.5 · 3.8), and that is what `confirm`
        // narrows this to at run time.
        per_1000: 40.8,
        measured: "2026-09-09",
        trend: Trend::Flat(5),
    },
    examine,
    confirm: Some(confirm),
};

/// Programs for which a pattern that matches nothing is the answer, not an
/// accident — when every operand is a pattern.
const ASKS_WHETHER: &[&str] = &[
    "ls", "stat", "test", "[", "echo", "printf", "file", "du", "tree", "exa", "eza",
];

/// The unquoted, un-substituted glob operands of a clause.
fn globs(cmd: &Simple) -> Vec<&Word> {
    cmd.operands()
        .into_iter()
        .filter(|w| {
            !w.quoted && !w.expanded && !w.text.contains('$') && tool_shell::has_glob(&w.text)
        })
        .collect()
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.judgeable() {
        let Some(program) = cmd.program() else {
            continue;
        };
        // A pattern the shell MATCHES AGAINST rather than expands: `case … in
        // completed*)` and `[[ $f == *.rs ]]`.
        if program == "case" || program == "[[" {
            continue;
        }
        let patterns = globs(cmd);
        let Some(first) = patterns.first() else {
            continue;
        };
        if ASKS_WHETHER.contains(&program) && patterns.len() == cmd.operands().len() {
            continue;
        }
        return Some(Finding {
            reason: format!(
                "`{}` is expanded by zsh before `{program}` runs, and a pattern that matches \
                 nothing is an error there — `no matches found` — raised before the command \
                 starts, so nothing in this clause runs. `2>/dev/null` cannot hide it, and a \
                 later clause carries the exit status: the call reports success and returns \
                 nothing.",
                first.text
            ),
            remedy: format!(
                "Quote the pattern and let the program walk the tree (`--include='{}'`, `find \
                 -name '…'`, `git ls-files '…'`), name the files you know exist in their own \
                 clause, or prefix the command with `setopt nullglob;` when an empty match is \
                 acceptable.",
                first.text.rsplit('/').next().unwrap_or(&first.text)
            ),
            span: cmd.at..cmd.end,
        });
    }
    None
}

fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    if let Err(why) = tool_shell::zsh() {
        return Confirmed::No(why);
    }
    let cwd = ctx.cwd_at(f.span.start);
    if !cwd.is_dir() {
        return Confirmed::No("the directory the command moves to does not exist");
    }
    let Some(cmd) = ctx.parsed.clauses().iter().find(|c| c.at == f.span.start) else {
        return Confirmed::No("the clause could not be found again");
    };
    // An earlier pipeline we could not read may be what fills this directory —
    // `bash -c 'npm run build' && ls dist/*.js`. Expanding the pattern now
    // would call it empty because the thing that populates it is a command we
    // did not understand.
    if ctx
        .parsed
        .clauses()
        .iter()
        .any(|c| c.at < cmd.at && c.opaque.is_some())
    {
        return Confirmed::No("an earlier clause could not be read");
    }
    let mut budget = Budget(4_000);
    for w in globs(cmd) {
        match expand(&cwd, &w.text, &mut budget) {
            Some(false) => return Confirmed::Yes,
            Some(true) => continue,
            None => return Confirmed::No("the pattern is too wide to expand safely"),
        }
    }
    Confirmed::No("every pattern matches something")
}

/// Directory entries this confirm may still read. A guard that walks a
/// `node_modules` to answer a question is slower than the mistake it
/// prevents, so the walk gives up — toward silence — past the budget.
struct Budget(usize);

/// Does `pattern`, resolved against `cwd`, match at least one path?
/// `None` when the budget ran out.
fn expand(cwd: &Path, pattern: &str, budget: &mut Budget) -> Option<bool> {
    let (base, rest) = if let Some(rest) = pattern.strip_prefix('/') {
        (PathBuf::from("/"), rest)
    } else if pattern == "~" || pattern.starts_with("~/") {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        (
            home,
            pattern.strip_prefix('~').unwrap().trim_start_matches('/'),
        )
    } else {
        (cwd.to_path_buf(), pattern)
    };
    let wants_dir = rest.ends_with('/');
    let segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    walk(&base, &segments, wants_dir, budget)
}

fn walk(base: &Path, segments: &[&str], wants_dir: bool, budget: &mut Budget) -> Option<bool> {
    let Some((seg, rest)) = segments.split_first() else {
        return Some(if wants_dir {
            base.is_dir()
        } else {
            base.exists()
        });
    };
    if !tool_shell::has_glob(seg) {
        return walk(&base.join(seg), rest, wants_dir, budget);
    }
    if *seg == "**" {
        if walk(base, rest, wants_dir, budget)? {
            return Some(true);
        }
        for entry in children(base, budget)? {
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if entry.path().is_dir() && !hidden && walk(&entry.path(), segments, wants_dir, budget)?
            {
                return Some(true);
            }
        }
        return Some(false);
    }
    for entry in children(base, budget)? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // zsh does not match a leading dot unless the pattern spells it.
        if name.starts_with('.') && !seg.starts_with('.') {
            continue;
        }
        if matches(seg.as_bytes(), name.as_bytes()) && walk(&entry.path(), rest, wants_dir, budget)?
        {
            return Some(true);
        }
    }
    Some(false)
}

fn children(dir: &Path, budget: &mut Budget) -> Option<Vec<std::fs::DirEntry>> {
    let mut out = Vec::new();
    // A directory that does not exist has no children; only the budget
    // says `None`.
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Some(out);
    };
    for entry in entries.flatten() {
        if budget.0 == 0 {
            return None;
        }
        budget.0 -= 1;
        out.push(entry);
    }
    Some(out)
}

/// One path segment against one pattern segment: `*`, `?`, `[…]` with ranges
/// and `!`/`^` negation, and `\` escaping the next byte.
fn matches(pat: &[u8], name: &[u8]) -> bool {
    match pat.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => (0..=name.len()).any(|i| matches(rest, &name[i..])),
        Some((b'?', rest)) => !name.is_empty() && matches(rest, &name[1..]),
        Some((b'[', rest)) => {
            let Some(close) = rest.iter().position(|&b| b == b']') else {
                return name.first() == Some(&b'[') && matches(rest, &name[1..]);
            };
            let (class, after) = (&rest[..close], &rest[close + 1..]);
            let Some((&c, tail)) = name.split_first() else {
                return false;
            };
            class_matches(class, c) && matches(after, tail)
        }
        Some((b'\\', rest)) if !rest.is_empty() => {
            name.first() == Some(&rest[0]) && matches(&rest[1..], &name[1..])
        }
        Some((&p, rest)) => name.first() == Some(&p) && matches(rest, &name[1..]),
    }
}

fn class_matches(class: &[u8], c: u8) -> bool {
    let (negate, body) = match class.first() {
        Some(b'!') | Some(b'^') => (true, &class[1..]),
        _ => (false, class),
    };
    let mut i = 0;
    let mut hit = false;
    while i < body.len() {
        if i + 2 < body.len() && body[i + 1] == b'-' {
            hit |= body[i] <= c && c <= body[i + 2];
            i += 3;
        } else {
            hit |= body[i] == c;
            i += 1;
        }
    }
    hit != negate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn an_unquoted_glob_operand_fires() {
        assert!(fires(
            "grep -rn x scripts/seed.ts app/lib/db/migrations/*.sql 2>/dev/null | head"
        ));
        assert!(fires("cat .claude/*.md 2>/dev/null | head -150"));
        assert!(fires(
            "rg -n -A12 'kustomize:' .forgejo/workflows/*.yml .github/workflows/*.yml"
        ));
        assert!(fires("head -30 .github/workflows/*.yml | head -50"));
        assert!(fires(
            "grep -l x build/client/assets/*.css build/server/assets/*.css"
        ));
        assert!(fires("for f in src/*.rs; do head -3 $f; done"));
        assert!(fires("ls -la app/ node_modules/*.x"));
    }

    #[test]
    fn a_quoted_pattern_or_an_existence_question_is_silent() {
        assert!(!fires("ls .amont* 2>/dev/null"));
        assert!(!fires("ls -d /Users/x/Developer/*-wt-forgejo"));
        assert!(!fires("ls kubernetes/homelab/apps/argo*"));
        assert!(!fires("echo *.sql"));
        assert!(!fires("git ls-files '*.md' | head -20"));
        assert!(!fires("find . -name '*.go' | xargs grep -l TODO"));
        assert!(!fires(
            "rg -n --glob '*.yml' 'runs-on' .forgejo/workflows | head"
        ));
        assert!(!fires("echo 'app/lib/db/migrations/*.sql'"));
        assert!(!fires("bash -c 'cat .claude/*.md' 2>/dev/null | head"));
        assert!(!fires("grep -rn x app/ --include=*.ts | head"));
        assert!(!fires("cat $DIR/*.rs"));
        assert!(!fires("git tag -l 'v1.*' | tail -3"));
        assert!(!fires(
            "case \"$st\" in completed*) break;; *) sleep 5;; esac"
        ));
        assert!(!fires("[[ $f == *.rs ]] && echo rust"));
    }

    #[test]
    fn the_matcher_reads_zsh_patterns() {
        assert!(matches(b"*.rs", b"main.rs"));
        assert!(!matches(b"*.rs", b"main.ts"));
        assert!(matches(b"Menu*.js", b"MenuItem.js"));
        assert!(matches(b"?.txt", b"a.txt"));
        assert!(!matches(b"?.txt", b"ab.txt"));
        assert!(matches(b"[a-c]*.md", b"beta.md"));
        assert!(!matches(b"[!a-c]*.md", b"beta.md"));
        assert!(matches(b"type-presets.css.*", b"type-presets.css.d.ts"));
    }

    #[test]
    fn expansion_walks_the_tree_and_respects_dotfiles() {
        let dir = std::env::temp_dir().join(format!("amont-agent-glob-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src/inner")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "").unwrap();
        std::fs::write(dir.join("src/inner/lib.rs"), "").unwrap();
        std::fs::write(dir.join(".hidden.md"), "").unwrap();
        let mut b = Budget(1000);
        assert_eq!(expand(&dir, "src/*.rs", &mut b), Some(true));
        assert_eq!(expand(&dir, "src/*.ts", &mut b), Some(false));
        assert_eq!(expand(&dir, "src/**/lib.rs", &mut b), Some(true));
        assert_eq!(expand(&dir, "*.md", &mut b), Some(false));
        assert_eq!(expand(&dir, ".*.md", &mut b), Some(true));
        assert_eq!(expand(&dir, "src/inner*/", &mut b), Some(true));
        assert_eq!(expand(&dir, "nope/*.rs", &mut b), Some(false));
        assert_eq!(expand(&dir, "src/*", &mut Budget(0)), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
