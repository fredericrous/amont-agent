//! A file dumped into the tool result — the shape three rules share.
//!
//! `cat FILE`, `sed -n '1,400p' FILE`, `head -200 FILE`, `tail -n 300 FILE`:
//! a reader whose output goes nowhere but the tool result. A reader that
//! feeds a pipe (`cat f | grep x`) or a redirect (`cat a > b`) is not a
//! dump; the bytes never reach the model.

use crate::shell::{Connector, Parsed, Simple};

#[derive(Debug, Clone)]
pub struct Dump {
    /// The path as written.
    pub path: String,
    /// How much of the file: `Whole`, or a line count when the command
    /// bounds it (`sed -n 'A,Bp'`, `head -N`).
    pub extent: Extent,
    /// The window the command delivers, in the session record's spelling:
    /// `full`, `start:count` (0-based), `tail:N`, or
    /// [`crate::session_state::INEXACT`] when it cannot be stated exactly.
    pub window: String,
    pub at: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extent {
    Whole,
    Lines(u64),
    /// `head -c N`, `tail -c N`: a byte-bounded projection.
    Bytes(u64),
}

const DUMPERS: &[&str] = &["cat", "head", "tail", "sed", "bat", "more", "less", "nl"];

fn path_like(text: &str) -> bool {
    text.len() > 1
        && !text.starts_with('-')
        && !text
            .chars()
            .any(|c| matches!(c, '$' | '`' | '*' | '?' | '[' | '{' | '\n'))
}

/// The line count of `sed -n 'A,Bp'` / `sed -n Ap`, or `None` when the
/// script is anything else.
fn sed_extent(script: &str) -> Option<Extent> {
    let body = script.strip_suffix('p')?;
    if body == "$" || body.contains('$') {
        return Some(Extent::Whole);
    }
    let (a, b) = match body.split_once(',') {
        Some((a, b)) => (a, b),
        None => (body, body),
    };
    let a: u64 = a.trim().parse().ok()?;
    let b: u64 = b.trim().parse().ok()?;
    Some(Extent::Lines(b.saturating_sub(a) + 1))
}

/// The exact window of `sed -n 'A,Bp'`, or inexact.
fn sed_window(script: &str) -> String {
    let inexact = || crate::session_state::INEXACT.to_string();
    let Some(body) = script.strip_suffix('p') else {
        return inexact();
    };
    let (a, b) = match body.split_once(',') {
        Some((a, b)) => (a, b),
        None => (body, body),
    };
    match (a.trim().parse::<u64>(), b.trim().parse::<u64>()) {
        (Ok(a), Ok(b)) if a >= 1 && b >= a => format!("{}:{}", a - 1, b - a + 1),
        _ => inexact(),
    }
}

/// A flag that cannot change which lines are shown.
fn harmless_flag(program: &str, flag: &str) -> bool {
    match program {
        "sed" => matches!(flag, "-s" | "-E" | "-r"),
        "cat" | "nl" | "more" | "less" => !flag.starts_with("--") && flag.len() > 1,
        _ => false,
    }
}

fn detect(cmd: &Simple) -> Vec<Dump> {
    let Some(program) = cmd.program() else {
        return Vec::new();
    };
    if !DUMPERS.contains(&program) {
        return Vec::new();
    }
    // Output must reach the tool result: no pipe onward, no redirect of
    // stdout, no heredoc feeding it.
    if cmd.next.is_some_and(|c| c.is_pipe())
        || cmd.heredoc
        || cmd
            .redirects
            .iter()
            .any(|(op, _)| op.starts_with('>') || op == "1>" || op.contains('<'))
    {
        return Vec::new();
    }
    let args = cmd.args();
    let mut extent = Extent::Whole;
    let mut window = "full".to_string();
    let mut inexact = false;
    let mut quiet = program != "sed";
    let mut files: Vec<&str> = Vec::new();
    let mut script_seen = program != "sed";
    let mut skip = false;
    for (i, w) in args.iter().enumerate() {
        if skip {
            skip = false;
            continue;
        }
        let t = w.text.as_str();
        if !w.quoted && t.starts_with('-') {
            let digits = t.trim_start_matches('-');
            if program != "sed" && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
            {
                // `head -200`, `tail -300`.
                extent = Extent::Lines(digits.parse().unwrap_or(0));
                window = match (program, digits.parse::<u64>()) {
                    ("head", Ok(n)) => format!("0:{n}"),
                    ("tail", Ok(n)) => format!("tail:{n}"),
                    _ => crate::session_state::INEXACT.to_string(),
                };
            } else if matches!(t, "-n" | "-c") && matches!(program, "head" | "tail") {
                // `head -n 200`, `tail -c 4000`.
                if let Some(v) = args.get(i + 1) {
                    window = match (program, t, v.text.parse::<u64>()) {
                        ("head", "-n", Ok(n)) => format!("0:{n}"),
                        ("tail", "-n", Ok(n)) if !v.text.starts_with('+') => format!("tail:{n}"),
                        _ => crate::session_state::INEXACT.to_string(),
                    };
                    extent = match v.text.trim_start_matches('+').parse::<u64>() {
                        Ok(n) if t == "-n" => Extent::Lines(n),
                        Ok(n) => Extent::Bytes(n),
                        Err(_) => Extent::Whole,
                    };
                }
                skip = true;
            } else if matches!(t, "-e" | "--expression") && program == "sed" {
                // The script came by flag; `sed -n -e '1,40p' f`.
                let Some(v) = args.get(i + 1) else {
                    return Vec::new();
                };
                let Some(e) = sed_extent(&v.text) else {
                    return Vec::new();
                };
                extent = e;
                window = sed_window(&v.text);
                script_seen = true;
                skip = true;
            } else if program == "sed" && matches!(t, "-n" | "--quiet" | "--silent") {
                quiet = true;
            } else if !harmless_flag(program, t) {
                // A flag this does not understand may change what is shown.
                inexact = true;
            }
            continue;
        }
        if !script_seen {
            // sed's first operand is its script. Only a print range is a
            // dump; a substitution is an edit, and its output is a diff of
            // the author's making.
            let Some(e) = sed_extent(t) else {
                return Vec::new();
            };
            extent = e;
            window = sed_window(t);
            script_seen = true;
            continue;
        }
        if w.quoted || w.expanded || !path_like(t) {
            continue;
        }
        files.push(t);
    }
    if inexact || !quiet {
        window = crate::session_state::INEXACT.to_string();
    }
    files
        .into_iter()
        .map(|p| Dump {
            path: p.to_string(),
            extent,
            window: window.clone(),
            at: cmd.at,
            end: cmd.end,
        })
        .collect()
}

/// Whether the simple command at byte `at` of `src` certainly runs: not on the
/// right of `&&` or `||`, not in an `if`, `case`, loop, function or subshell,
/// not in the background, and not after a clause that may exit the shell.
/// Anything the parser does not model is not certain.
pub fn runs_unconditionally(src: &str, at: usize) -> bool {
    use crate::analysis::ir::Cmd;
    fn may_exit(text: &str) -> bool {
        text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|w| matches!(w, "exit" | "return" | "set" | "exec" | "trap" | "kill"))
    }
    fn find(cmd: &Cmd, src: &str, at: usize, ok: bool) -> Option<bool> {
        let span = cmd.span();
        if !(span.start <= at && at < span.end) {
            return None;
        }
        match cmd {
            Cmd::Simple(_) => Some(ok),
            Cmd::Seq { items, .. } => {
                let mut exits = false;
                for item in items {
                    let sp = item.cmd.span();
                    if sp.start <= at && at < sp.end {
                        return find(&item.cmd, src, at, ok && !exits && !item.background);
                    }
                    exits |= src.get(sp).is_none_or(may_exit);
                }
                Some(false)
            }
            Cmd::AndOr { first, rest, .. } => {
                if let Some(r) = find(first, src, at, ok) {
                    return Some(r);
                }
                rest.iter()
                    .find_map(|(_, c)| find(c, src, at, false))
                    .or(Some(false))
            }
            Cmd::Pipeline { cmds, .. } => cmds
                .iter()
                .find_map(|c| find(c, src, at, ok))
                .or(Some(false)),
            Cmd::Group { body, .. } => find(body, src, at, ok).or(Some(false)),
            _ => Some(false),
        }
    }
    match crate::analysis::frontend::parse(src) {
        Ok(cmd) => find(&cmd, src, at, true).unwrap_or(false),
        Err(_) => false,
    }
}

/// Every dump in the command, in order.
pub fn dumps(parsed: &Parsed) -> Vec<Dump> {
    let mut out = Vec::new();
    for cmd in parsed.judgeable() {
        // `cat f; …` where the clause before pipes INTO cat is a pipeline
        // sink, not a dump of `f`.
        if cmd.prev == Some(Connector::Pipe) {
            continue;
        }
        // `X=$(cat f)`, `echo "$(cat f)"`: the output goes to the shell, not
        // to the tool result. Not a dump — and not a read the session should
        // remember, either.
        if cmd.nested.is_some() {
            continue;
        }
        out.extend(detect(cmd));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn of(command: &str) -> Vec<(String, Extent)> {
        dumps(&lex(command))
            .into_iter()
            .map(|d| (d.path, d.extent))
            .collect()
    }

    #[test]
    fn a_reader_into_the_result_is_a_dump() {
        assert_eq!(
            of("cat src/main.rs"),
            vec![("src/main.rs".into(), Extent::Whole)]
        );
        assert_eq!(
            of("cat -n src/main.rs"),
            vec![("src/main.rs".into(), Extent::Whole)]
        );
        assert_eq!(
            of("cd /x && cat CLAUDE.md"),
            vec![("CLAUDE.md".into(), Extent::Whole)]
        );
        assert_eq!(
            of("sed -n '1,400p' app/lib/grid.ts"),
            vec![("app/lib/grid.ts".into(), Extent::Lines(400))]
        );
        assert_eq!(
            of("sed -n 115,200p app/x.tsx"),
            vec![("app/x.tsx".into(), Extent::Lines(86))]
        );
        assert_eq!(
            of("head -200 big.log"),
            vec![("big.log".into(), Extent::Lines(200))]
        );
        assert_eq!(
            of("head -n 30 f.rs"),
            vec![("f.rs".into(), Extent::Lines(30))]
        );
        assert_eq!(
            of("tail -n 300 f.log"),
            vec![("f.log".into(), Extent::Lines(300))]
        );
        assert_eq!(of("cat a.rs b.rs").len(), 2);
    }

    #[test]
    fn a_reader_that_feeds_something_else_is_not() {
        assert!(of("cat f | grep x").is_empty());
        assert!(of("cat a > b").is_empty());
        assert!(of("cat > notes.md <<'EOF'\nhi\nEOF\n").is_empty());
        assert!(of("echo x | cat").is_empty());
        assert!(of("sed -i 's/a/b/' f.rs").is_empty());
        assert!(of("sed 's/a/b/' f.rs").is_empty());
        assert!(of("cat $F").is_empty());
        assert!(of("cat *.rs").is_empty());
        assert!(of("git log | head -5").is_empty());
        assert!(of("ls").is_empty());
        // Into the shell, not into the result.
        assert!(of("X=$(cat notes.md)").is_empty());
        assert!(of("echo \"$(cat notes.md)\"").is_empty());
        assert!(of("diff <(cat a.txt) <(cat b.txt)").is_empty());
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use crate::shell::lex;

    fn win(command: &str) -> String {
        dumps(&lex(command)).remove(0).window
    }

    #[test]
    fn a_dump_states_its_window_exactly_or_not_at_all() {
        assert_eq!(win("cat f.txt"), "full");
        assert_eq!(win("head -20 f.txt"), "0:20");
        assert_eq!(win("head -n 20 f.txt"), "0:20");
        assert_eq!(win("tail -20 f.txt"), "tail:20");
        assert_eq!(win("tail -n 20 f.txt"), "tail:20");
        assert_eq!(win("sed -n '101,120p' f.txt"), "100:20");
        assert_eq!(win("sed -n 5p f.txt"), "4:1");
        assert_eq!(win("tail -n +5 f.txt"), "?");
        assert_eq!(win("head -c 100 f.txt"), "?");
        assert_eq!(win("head -n5 f.txt"), "?");
        assert_eq!(win("sed -n '5,$p' f.txt"), "?");
    }

    #[test]
    fn only_an_unconditional_clause_is_certain() {
        let at = |c: &str| c.find("cat").unwrap();
        for (c, want) in [
            ("cat f", true),
            ("cd /x; cat f", true),
            ("true || cat f", false),
            ("true && cat f", false),
            ("if true; then cat f; fi", false),
            ("for i in 1; do cat f; done", false),
            ("(cat f)", false),
            ("exit 1; cat f", false),
            ("cat f &", false),
        ] {
            assert_eq!(runs_unconditionally(c, at(c)), want, "{c}");
        }
    }
}
