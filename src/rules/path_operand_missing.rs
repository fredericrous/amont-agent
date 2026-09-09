//! `path-operand-missing` — a read of a path that is not there.
//!
//! ```sh
//! grep -rn "useLinkTarget" app e2e docs          # e2e was renamed last week
//! wc -l app/lib/bandRoute.ts app/lib/grid.ts     # one of them moved
//! cat node_modules/@duro-app/ui/dist/Icon.d.ts   # not installed here
//! ```
//!
//! A missing file is the loudest failure there is at a terminal. Under the
//! Bash tool it is not. `grep` in that shell is Claude Code's own function
//! over ugrep, where a path that isn't there is a `warning:` in mid-stream
//! rather than a fatal error: matches from the other operands print normally,
//! the warning scrolls off, and a search that read nothing looks exactly like
//! a search that found nothing. The coreutils are the same one diagnostic
//! line per missing operand, then carry on — and in a chain the exit status
//! belongs to the last clause anyway. Measured over 32,710 real calls: 206
//! reads of a path that did not exist, 71% reported as success, and in two
//! of every three the path was never mentioned again in the next three
//! calls. The empty result was taken as the answer.
//!
//! ## What is a path
//!
//! An operand of a program that only reads its operands. A word with a `/`,
//! a source-file extension, or a leading dot is a path wherever it sits; any
//! other operand counts only when nothing before it was a flag that might
//! have consumed it (`head -n 20 file`, `grep -A 3 pat file`). For `grep`,
//! `sed`, `awk` and `jq` the first operand is the program, not a file,
//! unless `-e`/`-f` carried it. Anything with a `$`, a glob character or a
//! substitution is unknowable and skipped.
//!
//! ## What is deliberate
//!
//! A clause that sends stderr to `/dev/null`, one followed by `||`, one
//! guarded by `[ -f … ]` or `test -e …` earlier in the same command, and a
//! path an earlier clause may have just created (`mkdir`, `touch`, `cp`,
//! `git clone`, a `>` redirect) — those are existence probes and sequences,
//! and the rule leaves them alone. Nothing inside `ssh`, `kubectl exec` or
//! `docker exec` is looked at: a remote path is not this filesystem's to
//! check.

use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Connector, Parsed, Simple, Word};
use std::path::{Path, PathBuf};

pub const RULE: Rule = Rule {
    id: "path-operand-missing",
    default_stance: Stance::Advise,
    evidence: Evidence {
        // The shape rate — a reader with a literal path operand. After
        // `confirm` (does the path exist?) the transcripts put it at 6.3 per
        // 1000, flat over six weeks and 95 sessions.
        per_1000: 292.6,
        measured: "2026-09-09",
        trend: Trend::Flat(6),
    },
    examine,
    confirm: Some(confirm),
};

/// Programs that only read their operands, so a missing one is a missed
/// read and never a file about to be created.
const READERS: &[&str] = &[
    "cat",
    "head",
    "tail",
    "wc",
    "nl",
    "cut",
    "sort",
    "column",
    "file",
    "du",
    "stat",
    "ls",
    "find",
    "grep",
    "egrep",
    "fgrep",
    "sed",
    "awk",
    "jq",
    "shasum",
    "sha256sum",
    "md5sum",
    "base64",
];

/// Programs whose FIRST operand is a program, not a file, unless one of
/// these flags carried it instead.
const PROGRAM_FIRST: &[(&str, &[&str])] = &[
    ("grep", &["-e", "--regexp", "-f", "--file"]),
    ("egrep", &["-e", "-f"]),
    ("fgrep", &["-e", "-f"]),
    ("sed", &["-e", "--expression", "-f", "--file"]),
    ("awk", &["-f"]),
    ("jq", &["-f", "--from-file"]),
];

/// Flags that take the NEXT word as their value, per program. A bare word
/// after any other flag is an operand: `sed -n 115,200p file` keeps its
/// script, `grep -rn pat app e2e` keeps its pattern.
const VALUE_FLAGS: &[(&str, &[&str])] = &[
    ("grep", &["-A", "-B", "-C", "-e", "-f", "-m", "-d", "-D"]),
    ("egrep", &["-A", "-B", "-C", "-e", "-f", "-m"]),
    ("fgrep", &["-A", "-B", "-C", "-e", "-f", "-m"]),
    ("sed", &["-e", "-f", "-l"]),
    ("awk", &["-f", "-v", "-F"]),
    (
        "jq",
        &[
            "-f",
            "--arg",
            "--argjson",
            "--slurpfile",
            "--rawfile",
            "--indent",
        ],
    ),
    ("head", &["-n", "-c"]),
    ("tail", &["-n", "-c", "-f"]),
    ("cut", &["-d", "-f", "-c", "-b"]),
    ("sort", &["-k", "-t", "-o", "-S", "-T"]),
    ("stat", &["-c", "-f", "--format", "--printf"]),
    ("column", &["-s", "-c", "-N", "-o"]),
    ("du", &["-d", "-B", "--max-depth"]),
    ("file", &["-f", "-m"]),
    ("nl", &["-b", "-n", "-s", "-w", "-v", "-i"]),
    ("base64", &["-w"]),
    ("shasum", &["-a"]),
];

fn takes_value(program: &str, flag: &str) -> bool {
    VALUE_FLAGS
        .iter()
        .find(|(p, _)| *p == program)
        .is_some_and(|(_, flags)| flags.contains(&flag))
}

/// Programs an earlier clause may have used to bring the path into being.
const CREATORS: &[&str] = &[
    "mkdir", "touch", "cp", "mv", "ln", "rsync", "scp", "tar", "unzip", "curl", "wget", "tee",
    "install", "git", "npm", "pnpm", "yarn", "bun", "cargo", "make", "go", "uv", "pip", "pip3",
    "python", "python3", "node", "brew", "docker", "kubectl", "helm",
];

const EXTENSIONS: &[&str] = &[
    ".ts",
    ".tsx",
    ".js",
    ".mjs",
    ".cjs",
    ".jsx",
    ".json",
    ".yaml",
    ".yml",
    ".toml",
    ".md",
    ".rs",
    ".go",
    ".py",
    ".sh",
    ".fish",
    ".css",
    ".scss",
    ".html",
    ".sql",
    ".txt",
    ".lock",
    ".xml",
    ".env",
    ".conf",
    ".ini",
    ".cfg",
    ".csv",
    ".log",
    ".tf",
    ".hcl",
    ".proto",
    ".java",
    ".kt",
    ".swift",
    ".rb",
    ".php",
    ".c",
    ".h",
    ".cpp",
    ".hpp",
    ".zig",
    ".lua",
    ".vue",
    ".svelte",
    ".d.ts",
    ".pem",
    ".key",
    ".crt",
    ".plist",
    ".service",
    ".dockerfile",
    ".mk",
];

/// Looks like a path wherever it appears.
fn path_shaped(text: &str) -> bool {
    if text.len() < 2 || text == ".." {
        return false;
    }
    text.contains('/')
        || text.starts_with('.')
        || EXTENSIONS.iter().any(|e| text.ends_with(e))
        || matches!(
            text,
            "Makefile" | "Dockerfile" | "Justfile" | "LICENSE" | "README"
        )
}

/// A word whose value this rule cannot know.
fn unknowable(w: &Word) -> bool {
    w.expanded
        || w.text.is_empty()
        || matches!(w.text.as_str(), "-" | "." | "..")
        || w.text
            .chars()
            .any(|c| matches!(c, '$' | '`' | '*' | '?' | '[' | '{' | '~' | '\n'))
}

/// The path operands of one clause, in the order written.
fn paths(cmd: &Simple) -> Vec<&Word> {
    let Some(program) = cmd.program() else {
        return Vec::new();
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    if !READERS.contains(&program) {
        return Vec::new();
    }
    let args = cmd.args();
    let mut out: Vec<&Word> = Vec::new();
    let mut after_ddash = false;
    for (i, w) in args.iter().enumerate() {
        if !w.quoted && w.text == "--" {
            after_ddash = true;
            continue;
        }
        if !after_ddash && !w.quoted && w.text.starts_with('-') {
            // `find` reads paths until its first primary; everything after
            // `-name` is expression, not files.
            if program == "find" {
                break;
            }
            continue;
        }
        // A bare word right after a value-taking flag is that flag's value
        // until proven a path: `head -n 20`, `grep -A 3`, `cut -d ,`.
        let after_flag =
            !after_ddash && i > 0 && !args[i - 1].quoted && takes_value(program, &args[i - 1].text);
        if after_flag && !path_shaped(&w.text) {
            continue;
        }
        out.push(w);
    }
    if let Some((_, carriers)) = PROGRAM_FIRST.iter().find(|(p, _)| *p == program) {
        let carried = carriers.iter().any(|f| {
            cmd.has_flag(f)
                || f.strip_prefix('-')
                    .filter(|s| s.len() == 1)
                    .and_then(|s| s.chars().next())
                    .is_some_and(|c| cmd.has_short(c))
                || args
                    .iter()
                    .any(|w| !w.quoted && w.text.starts_with(&format!("{f}=")))
        });
        if !carried && !out.is_empty() {
            out.remove(0);
        }
    }
    // Only now: a script full of `$` and `{` is still the script, and must be
    // dropped as the program before the files are judged.
    out.retain(|w| !unknowable(w));
    out
}

/// Is this clause a deliberate probe, or does something before it excuse a
/// path that is not there yet?
fn excused(parsed: &Parsed, idx: usize) -> bool {
    let clauses = parsed.clauses();
    let cmd = &clauses[idx];
    if cmd.next == Some(Connector::OrOr) {
        return true;
    }
    if cmd
        .redirects
        .iter()
        .any(|(op, target)| op.starts_with('2') && target.text == "/dev/null")
    {
        return true;
    }
    for earlier in &clauses[..idx] {
        let Some(program) = earlier.program() else {
            continue;
        };
        if matches!(program, "[" | "test")
            && earlier
                .args()
                .iter()
                .any(|w| !w.quoted && matches!(w.text.as_str(), "-f" | "-e" | "-d" | "-s" | "-r"))
        {
            return true;
        }
        if CREATORS.contains(&program) || earlier.redirects.iter().any(|(op, _)| op.contains('>')) {
            return true;
        }
    }
    false
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    for (idx, cmd) in parsed.clauses().iter().enumerate() {
        let found = paths(cmd);
        let Some(first) = found.first() else {
            continue;
        };
        if excused(parsed, idx) {
            continue;
        }
        let program = cmd.program().unwrap_or("the command");
        // The reason is written before `confirm` looks, so it names every
        // path it will look at rather than guessing which one is missing.
        let missing = match found.len() {
            1 => format!("`{}` does not exist", first.text),
            n if n <= 4 => format!(
                "one of {} does not exist",
                found
                    .iter()
                    .map(|w| format!("`{}`", w.text))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            n => format!(
                "one of the {n} paths named, `{}` first, does not exist",
                first.text
            ),
        };
        let reason = if matches!(program, "grep" | "egrep" | "fgrep") {
            format!(
                "`{program}` in this shell is Claude Code's function over ugrep, where a path \
                 that is not there is a `warning:` in mid-stream, not a fatal error: matches \
                 from the other operands print normally and a search that read nothing looks \
                 like a search that found nothing. Here {missing}."
            )
        } else {
            format!(
                "{} — and `{program}` prints one diagnostic line per missing operand and \
                 carries on with the rest, so a partial read reports the chain's last exit \
                 status, not the missing file.",
                missing[..1].to_uppercase() + &missing[1..]
            )
        };
        return Some(Finding {
            reason,
            remedy: format!(
                "Check the path first — `ls {}` or `git ls-files '…'` — or run the read on its \
                 own so its exit status is the call's. If the file may be absent, say so: \
                 `[ -f {} ] && …`.",
                parent_of(&first.text),
                first.text
            ),
            span: tight(cmd),
        });
    }
    None
}

/// The clause's own text: from its first word to the end of its last word or
/// redirect target, without the whitespace and connector `at..end` carries.
fn tight(cmd: &Simple) -> std::ops::Range<usize> {
    let start = cmd.words.first().map_or(cmd.at, |w| w.at);
    let end = cmd
        .words
        .iter()
        .chain(cmd.redirects.iter().map(|(_, w)| w))
        .map(|w| w.at + w.raw.len())
        .max()
        .unwrap_or(cmd.end);
    start..end
}

fn parent_of(text: &str) -> &str {
    match text.rfind('/') {
        Some(0) => "/",
        Some(i) => &text[..i],
        None => ".",
    }
}

fn resolve(cwd: &Path, text: &str) -> PathBuf {
    if text.starts_with('/') {
        PathBuf::from(text)
    } else {
        cwd.join(text)
    }
}

fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    let Some(cmd) = ctx
        .parsed
        .clauses()
        .iter()
        .find(|c| tight(c).start == f.span.start)
    else {
        return Confirmed::No("the clause could not be found again");
    };
    let cwd = ctx.cwd_at(f.span.start);
    if !cwd.is_dir() {
        return Confirmed::No("the directory the command moves to does not exist");
    }
    let found = paths(cmd);
    if found.is_empty() {
        return Confirmed::No("the command no longer matches");
    }
    if found.iter().any(|w| !resolve(&cwd, &w.text).exists()) {
        Confirmed::Yes
    } else {
        Confirmed::No("every path named exists")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    fn named(command: &str) -> Vec<String> {
        lex(command)
            .clauses()
            .iter()
            .flat_map(|c| paths(c).into_iter().map(|w| w.text.clone()))
            .collect()
    }

    #[test]
    fn a_reader_with_a_path_operand_fires() {
        assert!(fires(
            "cat node_modules/@duro-app/ui/dist/components/Icon.d.ts | head -30"
        ));
        assert!(fires(
            "wc -l app/components/StormBoard.tsx app/lib/bandRoute.ts"
        ));
        assert!(fires(
            "cd /x && grep -rn \"useLinkTarget\" app e2e docs --include=\"*.ts\""
        ));
        assert!(fires(
            "head -3 kubernetes/homelab/platform-foundation/crds/istio-base-crds.yaml"
        ));
        assert!(fires("find .forgejo .github -type f | sort"));
        assert!(fires("sed -n 115,200p app/components/risk/RiskMatrix.tsx"));
        assert!(fires("grep -n 'no-raw-html-element' eslint.config.mjs"));
        assert!(fires("jq -r .id resp.json"));
        assert!(fires("stat -c '%s' Cargo.toml"));
    }

    #[test]
    fn the_paths_are_the_files_not_the_flags_or_the_program() {
        assert_eq!(named("head -n 20 file.ts"), vec!["file.ts"]);
        assert_eq!(named("grep -A 3 pat file.log"), vec!["file.log"]);
        assert_eq!(named("grep -rn pat app e2e"), vec!["app", "e2e"]);
        assert_eq!(named("grep -e pat file.log"), vec!["file.log"]);
        assert_eq!(named("cut -d , -f 1 data.csv"), vec!["data.csv"]);
        assert_eq!(named("find src -name '*.rs' -newer x/y"), vec!["src"]);
        assert_eq!(named("sed -n '1,5p' a.rs b.rs"), vec!["a.rs", "b.rs"]);
        assert_eq!(named("awk '{print $1}' data.txt"), vec!["data.txt"]);
        assert_eq!(named("grep -rn foo/bar ."), Vec::<String>::new());
        assert_eq!(named("ls"), Vec::<String>::new());
    }

    #[test]
    fn probes_sequences_and_remotes_are_silent() {
        assert!(!fires("ls node_modules/@x/grid/ 2>/dev/null; cat node_modules/@x/grid/package.json 2>/dev/null | head"));
        assert!(!fires("cat docs/adr.md || echo none"));
        assert!(!fires("[ -f docs/adr.md ] && cat docs/adr.md"));
        assert!(!fires("test -e out.json && jq . out.json"));
        assert!(!fires("mkdir -p docs/design && cat docs/design/adr.md"));
        assert!(!fires("curl -sSo x.json https://e/x && jq . x.json"));
        assert!(!fires(
            "git worktree add ../wt -b f origin/main && cat ../wt/Cargo.toml"
        ));
        assert!(!fires("echo hi > out.txt; cat out.txt"));
        assert!(!fires(
            "ssh admin@192.168.1.42 'head -5 /share/Media2/report.txt'"
        ));
        assert!(!fires(
            "kubectl exec -n forgejo deploy/forgejo -- sh -c 'cat /data/app.ini | head'"
        ));
        assert!(!fires("docker exec web cat /etc/nginx/nginx.conf"));
        assert!(!fires("cat > /tmp/x/classify.sh <<'EOF'\nls\nEOF\n"));
        assert!(!fires("cat $DIR/file.ts"));
        assert!(!fires("cat ~/.netrc"));
        assert!(!fires("cat app/lib/*.ts"));
        assert!(!fires("git log --oneline -8 && git tag | head"));
        assert!(!fires("grep -c foo"));
    }

    #[test]
    fn the_span_is_the_clause() {
        let src = "cd /x && wc -l a.rs b.rs | sort";
        let f = examine(&lex(src)).unwrap();
        assert_eq!(&src[f.span], "wc -l a.rs b.rs");
    }
}
