//! The fingerprint of a block: what ADR-0067 compares to tell that a branch is
//! an exact copy of a reviewed one. The sha256 of the branch's `+` and `-`
//! lines with every context line, path and line number dropped, the common
//! leading indentation removed, and CR and trailing whitespace stripped.

use std::path::Path;

use std::time::Instant;

use greenpiste_guard::git;
use greenpiste_guard::sha256::{hex, sha256};

/// Length of the short form shown to people, after `fp:`.
const SHORT_LEN: usize = 12;

/// Pathspecs: the whole tree from any directory, without the plans.
const PATHSPECS: [&str; 3] = ["--", ":(top)", ":(top,exclude)docs/plans"];

/// Settings that would change `git diff` text, pinned over every config file.
const PINNED_CONFIG: [(&str, &str); 9] = [
    ("diff.algorithm", "myers"),
    ("diff.noprefix", "false"),
    ("diff.mnemonicPrefix", "false"),
    ("diff.indentHeuristic", "false"),
    ("diff.relative", "false"),
    ("diff.renames", "false"),
    ("diff.interHunkContext", "0"),
    ("diff.orderFile", "/dev/null"),
    ("core.quotepath", "true"),
];

/// A block's fingerprint: 64 lowercase hex characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// The full 64 hex characters.
    pub fn full(&self) -> &str {
        &self.0
    }

    /// `fp:` and the first 12 hex characters.
    pub fn short(&self) -> String {
        format!("fp:{}", &self.0[..SHORT_LEN])
    }
}

/// Why the diff could not be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FpError {
    /// `git diff` ran past its time budget.
    TimedOut,
    /// `git diff` exited non-zero; `stderr` is what it said.
    Git { stderr: String },
}

impl std::fmt::Display for FpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FpError::TimedOut => f.write_str("`git diff` ran past its time budget"),
            FpError::Git { stderr } if stderr.is_empty() => {
                f.write_str("`git diff` exited non-zero without a message")
            }
            FpError::Git { stderr } => write!(f, "`git diff` said: {stderr}"),
        }
    }
}

impl std::error::Error for FpError {}

/// The isolated `git diff` of `merge_base..head` in `repo`, as bytes.
pub fn diff_of(
    repo: &Path,
    merge_base: &str,
    head: &str,
    deadline: Instant,
) -> Result<Vec<u8>, FpError> {
    let count = PINNED_CONFIG.len().to_string();
    let mut environment: Vec<(String, String)> = vec![
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("LC_ALL".into(), "C".into()),
        ("GIT_CONFIG_COUNT".into(), count),
    ];
    for (index, (key, value)) in PINNED_CONFIG.iter().enumerate() {
        environment.push((format!("GIT_CONFIG_KEY_{index}"), (*key).into()));
        environment.push((format!("GIT_CONFIG_VALUE_{index}"), (*value).into()));
    }
    let environment: Vec<(&str, &str)> = environment
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let mut args = vec![
        "diff",
        "--binary",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-renames",
        "--no-indent-heuristic",
        "--no-relative",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "-U0",
        "--diff-algorithm=myers",
        merge_base,
        head,
    ];
    args.extend(PATHSPECS);
    git::bytes_bounded_detail(repo, &args, None, &environment, deadline).map_err(|failure| {
        match failure {
            git::BoundedFailure::TimedOut => FpError::TimedOut,
            git::BoundedFailure::Exited { stderr } => FpError::Git { stderr },
        }
    })
}

/// The fingerprint of the branch `merge_base..head`. `Ok(None)` means a binary
/// file is in the diff and nothing else; a git failure is an error.
pub fn fingerprint(
    repo: &Path,
    merge_base: &str,
    head: &str,
    deadline: Instant,
) -> Result<Option<Fingerprint>, FpError> {
    Ok(fingerprint_of_diff(&diff_of(
        repo, merge_base, head, deadline,
    )?))
}

/// The fingerprint of `git diff` output, on bytes. `None` when the diff has a
/// binary file.
pub fn fingerprint_of_diff(diff: &[u8]) -> Option<Fingerprint> {
    let lines = match scan(diff) {
        Scan::Binary(_) => return None,
        Scan::Lines(lines) => lines,
    };
    Some(Fingerprint(hex(&sha256(&normalised(&lines)))))
}

/// The path of the first binary file in `diff`, for the notice.
pub fn binary_path(diff: &[u8]) -> Option<String> {
    match scan(diff) {
        Scan::Binary(path) => Some(path),
        Scan::Lines(_) => None,
    }
}

enum Scan<'a> {
    Binary(String),
    Lines(Vec<&'a [u8]>),
}

enum State {
    Headers { path: String },
    Hunk { old_left: usize, new_left: usize },
}

/// Walks the diff by its hunk lengths, so a removed line `-- x` is content and
/// not a `---` header. Returns the `+` and `-` lines, sign included.
fn scan(diff: &[u8]) -> Scan<'_> {
    let mut lines: Vec<&[u8]> = Vec::new();
    let mut state = State::Headers {
        path: String::new(),
    };
    let mut pieces: Vec<&[u8]> = diff.split(|b| *b == b'\n').collect();
    if pieces.last().is_some_and(|last| last.is_empty()) {
        pieces.pop();
    }
    for line in pieces {
        state = match state {
            State::Hunk { old_left, new_left } if old_left > 0 || new_left > 0 => {
                match line.first() {
                    Some(b'-') if old_left > 0 => {
                        lines.push(line);
                        State::Hunk {
                            old_left: old_left - 1,
                            new_left,
                        }
                    }
                    Some(b'+') if new_left > 0 => {
                        lines.push(line);
                        State::Hunk {
                            old_left,
                            new_left: new_left - 1,
                        }
                    }
                    Some(b' ') | None if old_left > 0 && new_left > 0 => State::Hunk {
                        old_left: old_left - 1,
                        new_left: new_left - 1,
                    },
                    // `\ No newline at end of file`.
                    _ => State::Hunk { old_left, new_left },
                }
            }
            State::Hunk { .. } => match header_line(line, String::new()) {
                Ok(next) => next,
                Err(path) => return Scan::Binary(path),
            },
            State::Headers { path } => match header_line(line, path) {
                Ok(next) => next,
                Err(path) => return Scan::Binary(path),
            },
        };
    }
    Scan::Lines(lines)
}

/// One line outside a hunk. `Err` carries the path of a binary file.
fn header_line(line: &[u8], path: String) -> Result<State, String> {
    if let Some(names) = line.strip_prefix(b"diff --git ") {
        let names = String::from_utf8_lossy(names);
        let new_path = names.rsplit_once(" b/").map_or(&*names, |(_, p)| p);
        return Ok(State::Headers {
            path: new_path.trim_end_matches('\r').to_string(),
        });
    }
    if line.starts_with(b"GIT binary patch") || line.starts_with(b"Binary files ") {
        return Err(path);
    }
    if let Some(counts) = line.strip_prefix(b"@@ -") {
        if let Some((old_left, new_left)) = hunk_lengths(counts) {
            return Ok(State::Hunk { old_left, new_left });
        }
    }
    Ok(State::Headers { path })
}

/// The two lengths of `a[,b] +c[,d] @@ ...` (what follows `@@ -`); an absent
/// length is 1.
fn hunk_lengths(counts: &[u8]) -> Option<(usize, usize)> {
    let text = std::str::from_utf8(counts).ok()?;
    let (old, rest) = text.split_once(" +")?;
    let new = rest.split(' ').next()?;
    let length = |range: &str| -> Option<usize> {
        match range.split_once(',') {
            Some((_, n)) => n.trim().parse().ok(),
            None => range.trim().parse::<usize>().ok().map(|_| 1),
        }
    };
    Some((length(old)?, length(new)?))
}

/// The bytes that get hashed: lines stripped of CR and trailing blanks, the
/// longest common leading-whitespace byte prefix removed after the sign (a tab is not a space), joined by `\n`.
fn normalised(lines: &[&[u8]]) -> Vec<u8> {
    let stripped: Vec<&[u8]> = lines
        .iter()
        .map(|line| {
            let end = line
                .iter()
                .rposition(|b| !matches!(b, b' ' | b'\t' | b'\r'))
                .map_or(1, |i| i + 1);
            &line[..end.max(1)]
        })
        .collect();
    let indent_of = |line: &[u8]| {
        line[1..]
            .iter()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count()
    };
    let common_prefix: &[u8] = stripped
        .iter()
        .filter(|line| line.len() > 1)
        .map(|line| &line[1..1 + indent_of(line)])
        .reduce(|shared, indent| {
            let length = shared
                .iter()
                .zip(indent)
                .take_while(|(left, right)| left == right)
                .count();
            &shared[..length]
        })
        .unwrap_or(&[]);
    let common = common_prefix.len();
    let mut out: Vec<u8> = Vec::new();
    for (index, line) in stripped.iter().enumerate() {
        if index > 0 {
            out.push(b'\n');
        }
        out.push(line[0]);
        if line.len() > 1 {
            out.extend_from_slice(&line[1 + common..]);
        }
    }
    out
}
