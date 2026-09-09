//! `stdin-hang` — a command that will read standard input, with nothing on it.
//!
//! ```sh
//! cat > notes.md            # waits for input that never comes
//! python3                   # a REPL, in a tool with no keyboard
//! tee /tmp/out.log          # not the sink of any pipe
//! ```
//!
//! Under the Bash tool, standard input is open and never closes. A command
//! that reads it — `cat` with no file, `sort`, `wc`, a bare interpreter — does
//! not fail: it blocks, silently, until the tool's ten-minute clock moves it
//! to the background, where it blocks some more. Nothing names the cause. The
//! author's own report from the night this rule was written: *"the background
//! command is hung on a stray `cat >` line of mine that waits on stdin"* — ten
//! minutes lost to one line, and the diagnosis came from `ps`, not from any
//! error.
//!
//! That is the crate's admission test met exactly: the failure is SILENT, so
//! no correcting loop can form. A person at a terminal sees the cursor sit
//! there and presses Ctrl-D; the model sees nothing at all.
//!
//! ## What counts as a source
//!
//! The clause is fed if it is the sink of a pipe, if it carries any `<`
//! redirect — `< file`, a `<<'EOF'` heredoc, a `<<<` here-string — or a
//! process substitution `<(…)`. Everything else is the terminal, and the
//! terminal is empty.
//!
//! ## What counts as a reader
//!
//! Only programs whose stdin-reading is a matter of argument SHAPE, so the
//! answer needs no knowledge of the world: the filters that read stdin when
//! given no file operand, the one that always does (`tee`), and the
//! interpreters that open a REPL when given nothing to run. `grep`, `sed`,
//! `awk` and `jq` take their program as the first operand, so "no file" means
//! "exactly one operand" unless a flag carried the program instead. `read`
//! is deliberately absent: `while read line; do …; done < file` feeds the
//! loop, not the `read`, and the lexer attaches that redirect to `done`.

use crate::rules::{Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "stdin-hang",
    default_stance: Stance::Observe,
    evidence: Evidence {
        // One firing in 32,401 real commands — the `cat > file` that cost the
        // author ten minutes the night this was written — and no false
        // positive once version flags and here-strings were understood.
        per_1000: 0.03,
        measured: "2026-09-08",
        trend: Trend::Rare,
    },
    examine,
    confirm: None,
};

/// Filters that read stdin when given no file operand.
const FILTERS: &[&str] = &[
    "cat",
    "sort",
    "uniq",
    "wc",
    "head",
    "tail",
    "tr",
    "cut",
    "paste",
    "column",
    "nl",
    "tac",
    "rev",
    "fold",
    "base64",
    "shasum",
    "sha256sum",
    "sha1sum",
    "md5sum",
    "md5",
    "hexdump",
    "xxd",
    "od",
    "strings",
];

/// Programs that read stdin whatever their operands are. (`xargs` belongs
/// here too, but the lexer peels it as a wrapper — `xargs rm` has program
/// `rm` — and a bare `xargs` with no pipe is rare enough not to fight for.)
const ALWAYS: &[&str] = &["tee"];

/// Flags that make any program print and exit instead of reading. `node -v`
/// and `python --version` were a third of the first measurement's firings.
const EXITS_EARLY: &[&str] = &["--version", "-V", "-v", "--help", "-h"];

/// Interpreters and shells that open a REPL, or read a script from stdin, when
/// handed nothing to run. `-c code`, `-m module`, `-e code` and a script path
/// all leave an operand behind; a bare `-` (stdin) is dropped by `operands()`
/// as a flag, which is the right answer here.
const INTERPRETERS: &[&str] = &[
    "python", "python3", "node", "ruby", "perl", "php", "bash", "sh", "zsh", "fish", "bc", "psql",
    "sqlite3", "mysql",
];

/// Programs whose FIRST operand is the program to run, so the file list starts
/// at the second — unless one of the listed flags carried the program instead,
/// in which case every operand is a file.
const PROGRAM_FIRST: &[(&str, &[&str])] = &[
    ("grep", &["-e", "--regexp", "-f", "--file"]),
    ("egrep", &["-e", "-f"]),
    ("fgrep", &["-e", "-f"]),
    ("sed", &["-e", "--expression", "-f", "--file"]),
    ("awk", &["-f"]),
    ("gawk", &["-f"]),
    ("jq", &["-f", "--from-file"]),
];

/// Flags that give one of the PROGRAM_FIRST programs its input some other way,
/// or make it fail loudly instead of waiting: recursive grep walks the tree,
/// `sed -i` with no file is an error, `jq -n` reads nothing.
const SELF_FED: &[(&str, &[&str])] = &[
    (
        "grep",
        &["-r", "-R", "--recursive", "--dereference-recursive"],
    ),
    ("egrep", &["-r", "-R"]),
    ("fgrep", &["-r", "-R"]),
    ("sed", &["-i", "--in-place"]),
    ("jq", &["-n", "--null-input"]),
];

fn has_stdin_source(cmd: &Simple) -> bool {
    if cmd.prev.is_some_and(|c| c.is_pipe()) {
        return true;
    }
    if cmd.redirects.iter().any(|(op, _)| op.contains('<')) {
        return true;
    }
    // A heredoc's operator, tag and body are all consumed by the lexer; the
    // clause keeps only this flag. A process substitution `<(…)` is lexed as
    // a `<` redirect, so it is already covered above.
    cmd.heredoc
}

/// Does this program read stdin, given these arguments?
fn reads_stdin(cmd: &Simple) -> bool {
    let Some(program) = cmd.program() else {
        return false;
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    let operands = cmd.operands();

    if cmd
        .args()
        .iter()
        .any(|w| !w.quoted && EXITS_EARLY.contains(&w.text.as_str()))
    {
        return false;
    }
    if ALWAYS.contains(&program) {
        return true;
    }
    if FILTERS.contains(&program) || INTERPRETERS.contains(&program) {
        return operands.is_empty();
    }
    if let Some((_, program_flags)) = PROGRAM_FIRST.iter().find(|(p, _)| *p == program) {
        if let Some((_, fed)) = SELF_FED.iter().find(|(p, _)| *p == program) {
            if fed.iter().any(|f| has_flag_prefix(cmd, f)) {
                return false;
            }
        }
        return program_first_files(cmd, program_flags) == Some(0);
    }
    false
}

/// How many FILE operands a program-first command has, or `None` when it has
/// no program at all (bare `grep` fails loudly rather than waiting).
///
/// The program is the first operand unless a flag in `program_flags` carried
/// it — as the next word (`-e pat`), glued (`-epat`, `--regexp=pat`) or bundled
/// at the end of a short cluster (`-ie pat`). A flag's next-word value is not a
/// file, so it is skipped.
fn program_first_files(cmd: &Simple, program_flags: &[&str]) -> Option<usize> {
    let letters: Vec<char> = program_flags
        .iter()
        .filter_map(|f| {
            f.strip_prefix('-')
                .filter(|r| r.len() == 1)
                .and_then(|r| r.chars().next())
        })
        .collect();
    let longs: Vec<&str> = program_flags
        .iter()
        .filter(|f| f.starts_with("--"))
        .copied()
        .collect();

    let mut operands = 0usize;
    let mut program_in_flag = false;
    let mut after_ddash = false;
    let mut skip_next = false;
    for w in cmd.args() {
        if skip_next {
            skip_next = false;
            continue;
        }
        let t = w.text.as_str();
        if after_ddash || w.quoted || !t.starts_with('-') || t == "-" {
            operands += 1;
            continue;
        }
        if t == "--" {
            after_ddash = true;
            continue;
        }
        if let Some(long) = t.strip_prefix("--") {
            let name = long.split_once('=').map_or(long, |(n, _)| n);
            if longs.iter().any(|l| &l[2..] == name) {
                program_in_flag = true;
                skip_next = !long.contains('=');
            }
            continue;
        }
        // Short cluster: `-e`, `-ie`, `-epat`. A program letter ENDING the
        // cluster takes the next word; one in the middle is glued to the rest.
        let cluster = &t[1..];
        if let Some(pos) = cluster.chars().position(|c| letters.contains(&c)) {
            program_in_flag = true;
            skip_next = pos + 1 == cluster.chars().count();
        }
    }
    if program_in_flag {
        Some(operands)
    } else if operands == 0 {
        None
    } else {
        Some(operands - 1)
    }
}

/// `-e`, `-e=…`, and short flags bundled as `-ie` all count.
fn has_flag_prefix(cmd: &Simple, flag: &str) -> bool {
    if cmd.has_flag(flag) {
        return true;
    }
    if let Some(c) = flag
        .strip_prefix('-')
        .filter(|s| s.len() == 1)
        .and_then(|s| s.chars().next())
    {
        return cmd.has_short(c);
    }
    cmd.words
        .iter()
        .any(|w| !w.quoted && w.text.starts_with(&format!("{flag}=")))
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.clauses() {
        if has_stdin_source(cmd) || !reads_stdin(cmd) {
            continue;
        }
        let program = cmd.program().unwrap_or("the command");
        return Some(Finding {
            reason: format!(
                "`{program}` will read standard input, and nothing feeds it: no pipe, no \
                 `<` redirect, no heredoc. Under the Bash tool stdin stays open forever, so \
                 the command blocks silently until the tool's timeout moves it to the \
                 background — where it keeps blocking."
            ),
            remedy: format!(
                "Give `{program}` its input — a file operand, `< file`, a `<<'EOF'` heredoc, \
                 or put it at the end of a pipe — or, when it should read nothing, add \
                 `< /dev/null`. To write a file, use the Write tool or a heredoc instead of \
                 `cat > file`."
            ),
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

    #[test]
    fn a_reader_with_nothing_on_stdin_fires() {
        assert!(fires("cat > notes.md"));
        assert!(fires("cd /x && cat >> log.txt"));
        assert!(fires("cat"));
        assert!(fires("cat -"));
        assert!(fires("sort | uniq -c"));
        assert!(fires("tee /tmp/out.log"));
        assert!(fires("sudo tee /etc/hosts"));
        assert!(fires("python3"));
        assert!(fires("python3 -"));
        assert!(fires("node"));
        assert!(fires("bash"));
        assert!(fires("grep -i error"));
        assert!(fires("grep -e error"));
        assert!(fires("sed -n '1,5p'"));
        assert!(fires("awk '{print $1}'"));
        assert!(fires("jq -r '.id'"));
        assert!(fires("wc -l"));
        assert!(fires("/usr/bin/base64"));
    }

    #[test]
    fn a_fed_reader_is_silent() {
        assert!(!fires("echo hi | cat"));
        assert!(!fires("cat file.txt"));
        assert!(!fires("cat -n file.txt > out.txt"));
        assert!(!fires("cat < in.txt > out.txt"));
        assert!(!fires("cat > notes.md <<'EOF'\nhello\nEOF\n"));
        assert!(!fires("python3 - <<'EOF'\nprint(1)\nEOF\n"));
        assert!(!fires("python3 <<< 'print(1)'"));
        assert!(!fires("python3 -c 'print(1)'"));
        assert!(!fires("python3 -m json.tool f.json"));
        assert!(!fires("python3 script.py"));
        assert!(!fires("node -e 'console.log(1)'"));
        assert!(!fires("bash -c 'echo hi'"));
        assert!(!fires("bash run.sh"));
        assert!(!fires("ls | sort | uniq -c | sort -rn | head"));
        assert!(!fires("find . -name '*.go' | xargs grep -l TODO"));
        assert!(!fires("echo x | sudo tee /etc/hosts > /dev/null"));
        assert!(!fires("diff <(sort a) <(sort b)"));
        assert!(!fires("cat < /dev/null"));
    }

    #[test]
    fn program_first_readers_know_where_their_files_start() {
        assert!(!fires("grep -rn error src/"));
        assert!(!fires("grep -r error"));
        assert!(!fires("grep error file.log"));
        assert!(!fires("grep -e error file.log"));
        assert!(!fires("grep"));
        assert!(!fires("sed -n '1,5p' file.txt"));
        assert!(!fires("sed -i 's/a/b/'"));
        assert!(!fires("sed -i 's/a/b/' file.txt"));
        assert!(!fires("sed -e 's/a/b/' file.txt"));
        assert!(!fires("awk '{print $1}' file.txt"));
        assert!(!fires("awk -f prog.awk data.txt"));
        assert!(!fires("jq -r '.id' resp.json"));
        assert!(!fires("jq -n '{a:1}'"));
        assert!(!fires("curl -s https://x | jq -r '.id'"));
    }

    #[test]
    fn a_reader_inside_a_string_is_text() {
        assert!(!fires("echo 'cat > file'"));
        assert!(!fires("grep -rn \"tee /tmp\" docs/"));
    }
}
