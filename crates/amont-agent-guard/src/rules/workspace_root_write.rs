//! `workspace-root-write` — an output file created directly in a workspace
//! root.
//!
//! ```sh
//! cd ~/Developer/Perso && make > sb-dev.log 2>&1   # the root is in no repository
//! cargo test 2>&1 | tee test.log                   # same
//! ```
//!
//! A workspace root is a directory that is in no git tree and holds several
//! repositories side by side. The Bash tool's cwd keeps drifting back to it,
//! and a redirect with a bare file name then leaves a scratch file there. Such
//! a file belongs to no repository: git never shows it, no sweep removes it,
//! and nothing says which task wrote it.
//!
//! ## What fires
//!
//! `examine` is the shape: a write redirect (`>`, `>>`, `>|`, `N>`, `&>`,
//! `&>>`, the zsh `>!`, `>&file` and `<>`) or an operand of `tee` whose target
//! is a word the analysis can read literally. `/dev/*`, fd duplications
//! (`2>&1`, `>&-`) and anything with a `$`, a glob or a substitution are
//! skipped. `confirm` is the fact: the directory the target lands in, after the
//! `cd`s before it, is in no git tree and at least two of its children hold a
//! `.git` (a file or a directory). A nested path, a path inside a repository
//! and the system temp directories never fire.
//!
//! ## What it says
//!
//! The refusal names the linked worktree this session touched last, when
//! `git check-ignore` says the file would be ignored there (an untracked log
//! would hold `worktree-sweep` forever), or else the session's scratch
//! directory, which `worktree-sweep --scratch` clears. The transcript is read
//! only after the root is established, from a bounded tail.
//!
//! ## Demotion
//!
//! The rule ships at `deny`. One command turns it into a record:
//! `git config --global amont.agent.workspace-root-write.stance observe`.

use crate::analysis::frontend;
use crate::analysis::ir::{Cmd, Part, Redirect, SimpleCmd, Span, Word};
use crate::rules::{
    resolve, Confirmed, Context, Evidence, Examine, Finding, Input, Rule, Stance, Trend,
};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const RULE: Rule = Rule {
    id: "workspace-root-write",
    default_stance: Stance::Deny,
    max_stance: Stance::Deny,
    evidence: Evidence {
        // The shape rate: a redirect or `tee` to a literal path, 9,856 of
        // 83,733 calls since 2026-09-07. The backtester never runs `confirm`,
        // so this is not the rate of writes into a root. That was replayed
        // through the built binary: of 3,357 commands recorded with
        // `~/Developer/Perso` as their cwd, 22 were refused, all scratch
        // files (`*.tmp.mjs` probes, `../../scratch-relay.log`). The replay
        // also found `$HOME` qualifying as a root, which is now excluded. The
        // measurement on disk: 86 `*.log`/`*.err` files in the root.
        per_1000: 117.7,
        measured: "2026-10-13",
        trend: Trend::Flat(6),
    },
    examine: Examine::Analysis(examine),
    confirm: Some(confirm),
};

/// How much of the end of the transcript is read.
const TAIL_BYTES: u64 = 2 * 1024 * 1024;

/// One file a command writes, as the analysis reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    /// The target word.
    span: Span,
    /// Its text, without a leading `~` when `home` is set.
    text: String,
    /// The word began with an unquoted `~`, to be resolved against `$HOME`.
    home: bool,
}

// ------------------------------------------------------------------ examine

fn examine(input: &Input) -> Option<Finding> {
    let tree = frontend::parse(input.src).ok()?;
    let first = targets(&tree).into_iter().next()?;
    Some(Finding {
        reason: format!(
            "`{}` is written by a redirect or `tee`; if the command runs in a workspace \
             root, a directory in no repository that holds several, the file belongs to \
             none of them: git never shows it and nothing sweeps it.",
            &input.src[first.span.clone()]
        ),
        remedy: "Write it inside the repository or worktree the task is about, or in the \
                 session's scratch directory, with an absolute path."
            .to_string(),
        span: first.span,
    })
}

/// Every write target in the command, in source order.
fn targets(cmd: &Cmd) -> Vec<Target> {
    let mut out = Vec::new();
    walk(cmd, &mut out);
    out.sort_by_key(|t| t.span.start);
    out
}

fn walk(cmd: &Cmd, out: &mut Vec<Target>) {
    match cmd {
        Cmd::Simple(s) => simple(s, out),
        Cmd::Seq { items, .. } => items.iter().for_each(|i| walk(&i.cmd, out)),
        Cmd::AndOr { first, rest, .. } => {
            walk(first, out);
            rest.iter().for_each(|(_, c)| walk(c, out));
        }
        Cmd::Pipeline { cmds, .. } => cmds.iter().for_each(|c| walk(c, out)),
        Cmd::If {
            arms, otherwise, ..
        } => {
            for (c, b) in arms {
                walk(c, out);
                walk(b, out);
            }
            if let Some(o) = otherwise {
                walk(o, out);
            }
        }
        Cmd::Case { word, arms, .. } => {
            substitutions(word, out);
            arms.iter().for_each(|a| walk(&a.body, out));
        }
        Cmd::For { items, body, .. } => {
            items.iter().flatten().for_each(|w| substitutions(w, out));
            walk(body, out);
        }
        Cmd::ArithFor { body, .. } => walk(body, out),
        Cmd::Loop { cond, body, .. } => {
            walk(cond, out);
            walk(body, out);
        }
        Cmd::Group { body, .. } | Cmd::Subshell { body, .. } => walk(body, out),
        Cmd::Redirected {
            body, redirects, ..
        } => {
            walk(body, out);
            for r in redirects {
                substitutions(&r.target, out);
                if let Some(t) = redirect_target(r, None) {
                    out.push(t);
                }
            }
        }
        // A definition runs nothing; the rest hold no redirect.
        Cmd::FuncDef { .. } | Cmd::Arith { .. } | Cmd::Cond { .. } | Cmd::Unsupported { .. } => {}
    }
}

/// Commands inside a word's `$(…)`, backticks and `<(…)`.
fn substitutions(word: &Word, out: &mut Vec<Target>) {
    fn parts(ps: &[Part], out: &mut Vec<Target>) {
        for p in ps {
            match p {
                Part::CmdSubst { body, .. } | Part::ProcSubst { body, .. } => walk(body, out),
                Part::DoubleQuoted(inner) => parts(inner, out),
                _ => {}
            }
        }
    }
    parts(&word.parts, out);
}

fn simple(s: &SimpleCmd, out: &mut Vec<Target>) {
    s.words.iter().for_each(|w| substitutions(w, out));
    s.assigns.iter().for_each(|a| substitutions(&a.value, out));
    for r in &s.redirects {
        substitutions(&r.target, out);
        if let Some(t) = redirect_target(r, Some(&s.words)) {
            out.push(t);
        }
    }
    out.extend(tee_operands(&s.words));
}

/// The operator without its fd prefix.
fn bare_op(op: &str) -> &str {
    op.trim_start_matches(|c: char| c.is_ascii_digit())
}

fn redirect_target(r: &Redirect, words: Option<&[Word]>) -> Option<Target> {
    let op = bare_op(&r.op);
    if !matches!(op, ">" | ">>" | ">|" | "&>" | "&>>" | ">&" | "<>") && !r.op.starts_with("&>") {
        return None;
    }
    // zsh's `>! file` and `>>! file`: the lexer reads `!` as the target and
    // the file as the next word of the command.
    let word = if matches!(op, ">" | ">>") && r.target.literal().as_deref() == Some("!") {
        words?.iter().find(|w| w.span.start >= r.target.span.end)?
    } else {
        &r.target
    };
    let t = known(word)?;
    if !t.home && t.text.starts_with("/dev/") {
        return None;
    }
    // `2>&1`, `>&2`, `>&-`: a descriptor, not a file.
    if op == ">&" && (t.text == "-" || t.text.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    Some(t)
}

fn tee_operands(words: &[Word]) -> Vec<Target> {
    let literal = |w: &Word| w.literal();
    let mut i = 0;
    if words.first().and_then(literal).is_some_and(|p| p == "sudo") {
        i = 1;
        while words
            .get(i)
            .and_then(literal)
            .is_some_and(|f| f.starts_with('-'))
        {
            i += 1;
        }
    }
    let is_tee = words
        .get(i)
        .and_then(literal)
        .is_some_and(|p| p.rsplit('/').next() == Some("tee"));
    if !is_tee {
        return Vec::new();
    }
    words[i + 1..]
        .iter()
        .filter(|w| !w.literal().is_some_and(|t| t.starts_with('-')))
        .filter_map(known)
        .filter(|t| t.home || !t.text.starts_with("/dev/"))
        .collect()
}

/// The word's value when it is the same on every run: literal and single
/// quoted text, no `$`, backtick, glob or `{`, and at most a leading unquoted
/// `~`. `~user` and a quoted `~` are unknown.
fn known(word: &Word) -> Option<Target> {
    let mut text = String::new();
    let mut home = false;
    for (i, p) in word.parts.iter().enumerate() {
        match p {
            Part::Tilde(t) if i == 0 && t == "~" => home = true,
            Part::Lit(s) | Part::SingleQuoted(s) => {
                if s.contains(['$', '`', '*', '?', '[', '{']) {
                    return None;
                }
                text.push_str(s);
            }
            _ => return None,
        }
    }
    if text.is_empty() && !home {
        return None;
    }
    if home && !(text.is_empty() || text.starts_with('/')) {
        return None;
    }
    Some(Target {
        span: word.span.clone(),
        text,
        home,
    })
}

// ------------------------------------------------------------------ confirm

fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    if ctx.command.is_empty() {
        return Confirmed::No("there is no command to read again");
    }
    let Ok(tree) = frontend::parse(ctx.command) else {
        return Confirmed::No("the command could not be read again");
    };
    // The finding's own target first, then any other write in the command.
    let mut all = targets(&tree);
    all.sort_by_key(|t| t.span != f.span);
    for t in &all {
        let Some(path) = target_path(ctx, t) else {
            continue;
        };
        let Some(dir) = path.parent().and_then(|d| d.canonicalize().ok()) else {
            continue;
        };
        if !is_workspace_root(&dir) {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        return said(ctx, t, &dir, &name);
    }
    Confirmed::No("no write lands directly in a workspace root")
}

/// Where the target lands: against the directory the clause containing it runs
/// in, `~` against `$HOME`.
fn target_path(ctx: &Context, t: &Target) -> Option<PathBuf> {
    if t.home {
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
        let rest = t.text.trim_start_matches('/');
        return Some(if rest.is_empty() {
            PathBuf::from(home)
        } else {
            PathBuf::from(home).join(rest)
        });
    }
    let at = clause_start(ctx, &t.span);
    if is_relative(t) && cd_unresolved(ctx, at) {
        return None;
    }
    Some(resolve(&ctx.cwd_at(at), &t.text))
}

fn is_relative(t: &Target) -> bool {
    !t.text.starts_with('/')
}

/// Does the last `cd` before `at` (in the same substitution scope) have a
/// destination that cannot be known statically: a variable, a substitution, a
/// glob, `-`? Then a relative target lands somewhere unknown.
fn cd_unresolved(ctx: &Context, at: usize) -> bool {
    let clauses = ctx.parsed.clauses();
    let asking = clauses.iter().find(|c| c.at == at).and_then(|c| c.nested);
    let mut unknown = false;
    for cmd in clauses {
        if cmd.at >= at {
            continue;
        }
        // A clause inside a loop or block may open with its keyword.
        let mut words = cmd.words.iter().skip_while(|w| {
            matches!(
                w.text.as_str(),
                "do" | "then" | "else" | "elif" | "if" | "while" | "until" | "{" | "(" | "!"
            )
        });
        if words.next().map(|w| w.text.as_str()) != Some("cd") {
            continue;
        }
        if cmd.nested.is_some() && cmd.nested != asking {
            continue;
        }
        unknown = match words.next() {
            None => false,
            Some(w) => {
                w.expanded
                    || w.text.trim().is_empty()
                    || w.text == "-"
                    || w.text.contains(['$', '`', '*', '?', '[', '{'])
            }
        };
    }
    unknown
}

/// The start of the legacy clause containing the span, which is what
/// [`Context::cwd_at`] matches on; the span's own start when none does.
fn clause_start(ctx: &Context, span: &Span) -> usize {
    ctx.parsed
        .clauses()
        .iter()
        .filter(|c| c.at <= span.start && span.end <= c.end)
        .map(|c| c.at)
        .max()
        .unwrap_or(span.start)
}

fn has_git(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(".git")).is_ok()
}

/// In no git tree, with at least two children that are.
fn is_workspace_root(dir: &Path) -> bool {
    if is_temp_dir(dir) || dir.ancestors().any(has_git) {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries
        .flatten()
        .filter(|e| has_git(&e.path()))
        .nth(1)
        .is_some()
}

/// The system's own scratch directories hold worktrees and are where the
/// remedy points. `$HOME` is excluded too: it often holds a clone or two, and
/// dotfiles (`>> ~/.netrc`) belong there on purpose.
fn is_temp_dir(dir: &Path) -> bool {
    let mut temps = vec![std::env::temp_dir(), PathBuf::from("/tmp")];
    for var in ["CLAUDE_CODE_TMPDIR", "HOME"] {
        if let Some(t) = std::env::var_os(var).filter(|t| !t.is_empty()) {
            temps.push(PathBuf::from(t));
        }
    }
    temps
        .iter()
        .filter_map(|t| t.canonicalize().ok())
        .any(|t| t == dir)
}

fn said(ctx: &Context, t: &Target, root: &Path, name: &str) -> Confirmed {
    // A count that could not be read is left out, never printed as zero.
    let loose = std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|ft| !ft.is_dir()))
                .count()
        })
        .map_or_else(|_| String::new(), |n| format!(" ({n} files there today)"));
    let reason = format!(
        "`{name}` would land in `{}`, a workspace root that is in no repository: git never \
         shows the file and nothing sweeps it{loose}.",
        tilde(root)
    );
    let worktree = ctx
        .transcript
        .and_then(|tr| latest_worktree(tr, TAIL_BYTES))
        .filter(|wt| ignores(wt, name));
    let scratch = ctx.transcript.and_then(scratch_dir);
    let scratch_part = match &scratch {
        Some(d) => format!(
            "`{}/{name}` for a scratch log (create the directory first)",
            d.display()
        ),
        None => format!(
            "`${{CLAUDE_CODE_TMPDIR:-/tmp}}/claude-<uid>/<project>/<session>/{name}` for a scratch log"
        ),
    };
    let remedy = match worktree {
        Some(wt) => format!(
            "This session works in the worktree `{}`: write `{}/{name}` (absolute, never `~`: \
             quoted, it does not expand), or {scratch_part}. If the file belongs in the root \
             on purpose, ask the person.",
            wt.display(),
            wt.display()
        ),
        None => format!(
            "Write {scratch_part}, an absolute path that `worktree-sweep --scratch` clears. \
             If the file belongs in the root on purpose, ask the person."
        ),
    };
    Confirmed::YesSaying {
        floor: None,
        ceiling: None,
        reason,
        excerpt: crate::backtest::excerpt(ctx.command, t.span.start, t.span.end),
        remedy: Some(remedy),
    }
}

/// `path` with `$HOME` written as `~`.
fn tilde(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    for h in home
        .iter()
        .flat_map(|h| [Some(h.clone()), h.canonicalize().ok()])
    {
        if let Some(rest) = h.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
            return if rest.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    path.display().to_string()
}

/// Would `git` ignore a file of this name at the top of the worktree? Only
/// then does a log there leave the worktree clean for `worktree-sweep`.
fn ignores(worktree: &Path, name: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["check-ignore", "-q", "--", name])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `${CLAUDE_CODE_TMPDIR:-/tmp}/claude-<uid>/<project slug>/<session id>`:
/// the slug is the directory the transcript sits in, the session id its stem,
/// the uid the transcript's owner.
fn scratch_dir(transcript: &Path) -> Option<PathBuf> {
    let session = transcript.file_stem()?;
    let slug = transcript.parent()?.file_name()?;
    let uid = owner_uid(transcript)?;
    let base = std::env::var_os("CLAUDE_CODE_TMPDIR")
        .filter(|t| !t.is_empty())
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    Some(base.join(format!("claude-{uid}")).join(slug).join(session))
}

/// The uid that owns `path`, which names the `claude-<uid>` directory.
#[cfg(unix)]
fn owner_uid(path: &Path) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(std::fs::metadata(path).ok()?.uid())
}

/// Windows has no uid in the scratch path: the remedy falls back to the
/// generic scratch pattern.
#[cfg(not(unix))]
fn owner_uid(_path: &Path) -> Option<u32> {
    None
}

// --------------------------------------------------------------- transcript

/// What the git tree around a path is.
#[derive(Debug, PartialEq, Eq)]
enum Tree {
    /// No `.git` above it.
    None,
    /// A primary checkout or a submodule: nothing sweeps it.
    Other,
    /// A linked worktree, by its top directory.
    Linked(PathBuf),
}

fn tree_of(path: &Path) -> Tree {
    for dir in path.ancestors() {
        let git = dir.join(".git");
        let Ok(meta) = std::fs::symlink_metadata(&git) else {
            continue;
        };
        if !meta.is_file() {
            return Tree::Other;
        }
        let linked = std::fs::read_to_string(&git).is_ok_and(|text| {
            text.lines()
                .next()
                .and_then(|l| l.strip_prefix("gitdir:"))
                .map(|g| Path::new(g.trim()))
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|n| n == "worktrees")
        });
        return if linked {
            Tree::Linked(dir.to_path_buf())
        } else {
            Tree::Other
        };
    }
    Tree::None
}

/// The linked worktree of the latest path in the transcript's last `max`
/// bytes that lies in a git tree at all. A primary checkout or a submodule
/// touched last means the session has moved on: no worktree.
fn latest_worktree(transcript: &Path, max: u64) -> Option<PathBuf> {
    let mut file = std::fs::File::open(transcript).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        // The cut may fall inside a line.
        lines.remove(0);
    }
    for line in lines.iter().rev() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let mut paths = Vec::new();
        tool_paths(&value, &mut paths);
        for p in paths.iter().rev() {
            match tree_of(p) {
                Tree::None => {}
                Tree::Other => return None,
                Tree::Linked(top) => return Some(top),
            }
        }
    }
    None
}

/// The paths tool calls in this JSON value name, in the order written.
fn tool_paths(v: &serde_json::Value, out: &mut Vec<PathBuf>) {
    match v {
        serde_json::Value::Array(items) => items.iter().for_each(|i| tool_paths(i, out)),
        serde_json::Value::Object(map) => {
            if map.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                let name = map.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if let Some(input) = map.get("input") {
                    input_paths(name, input, out);
                }
            } else {
                map.values().for_each(|i| tool_paths(i, out));
            }
        }
        _ => {}
    }
}

fn input_paths(tool: &str, input: &serde_json::Value, out: &mut Vec<PathBuf>) {
    if tool == "Bash" {
        if let Some(c) = input.get("command").and_then(|c| c.as_str()) {
            out.extend(bash_paths(c));
        }
        return;
    }
    if !matches!(
        tool,
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Grep" | "LSP"
    ) {
        return;
    }
    for key in ["file_path", "path", "notebook_path", "filePath"] {
        if let Some(p) = input.get(key).and_then(|p| p.as_str()) {
            if Path::new(p).is_absolute() {
                out.push(PathBuf::from(p));
            }
        }
    }
}

/// The absolute directories a Bash command names with `git -C <p>` or
/// `cd <p>`, in the order written.
// holds-until: a session that names its worktree only through a relative or
// quoted-with-spaces `cd`/`git -C` operand. A miss only degrades the remedy to
// the scratch directory; the shell parser would read every form.
fn bash_paths(command: &str) -> Vec<PathBuf> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let operand = match *w {
            "cd" => words.get(i + 1),
            "-C" if i > 0 && words[i - 1] == "git" => words.get(i + 1),
            _ => None,
        };
        let Some(p) = operand.map(|p| p.trim_matches(['"', '\''])) else {
            continue;
        };
        if p.starts_with('/') {
            out.push(PathBuf::from(p));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Dialect;
    use crate::shell::lex;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fires(command: &str) -> bool {
        let parsed = lex(command);
        examine(&Input::new(command, Dialect::Bash, &parsed)).is_some()
    }

    fn texts(command: &str) -> Vec<String> {
        let tree = frontend::parse(command).expect("parses");
        targets(&tree)
            .into_iter()
            .map(|t| command[t.span].to_string())
            .collect()
    }

    // ---- examine

    #[test]
    fn the_write_redirects_fire() {
        for c in [
            "make > x.log",
            "make >> x.log",
            "make >| x.log",
            "make 2> x.log",
            "make 2>> x.log",
            "make &> x.log",
            "make &>> x.log",
            "make >& x.log",
            "make >&x.log",
            "make >! x.log",
            "make >>! x.log",
            "make 1<> x.log",
            "make 2>&1 > x.log",
            "make > x.log 2>&1",
        ] {
            assert!(fires(c), "{c}");
        }
    }

    #[test]
    fn the_target_is_the_file_not_the_operator() {
        assert_eq!(texts("make &> x.log"), vec!["x.log"]);
        assert_eq!(texts("make >&x.log"), vec!["x.log"]);
        assert_eq!(texts("make >! x.log"), vec!["x.log"]);
        assert_eq!(texts("make 2>&1 > x.log"), vec!["x.log"]);
    }

    #[test]
    fn tee_operands_fire() {
        assert_eq!(texts("make | tee x.log"), vec!["x.log"]);
        assert_eq!(texts("make | tee -a x.log y.log"), vec!["x.log", "y.log"]);
        assert_eq!(texts("make | sudo tee x.log"), vec!["x.log"]);
        assert_eq!(texts("make | sudo -n tee -a x.log"), vec!["x.log"]);
        assert_eq!(texts("make | /usr/bin/tee x.log"), vec!["x.log"]);
        assert!(texts("make | tee").is_empty());
        assert!(texts("make | tee $LOG").is_empty());
    }

    #[test]
    fn a_heredoc_to_a_file_fires() {
        assert!(fires("cat <<EOF > x.log\nhello\nEOF\n"));
        assert!(fires("cat > x.log <<'EOF'\nhello\nEOF\n"));
        assert!(!fires("cat <<EOF\nhello\nEOF\n"));
    }

    #[test]
    fn a_compound_command_with_a_redirect_fires() {
        assert_eq!(texts("{ a; b; } > x.log"), vec!["x.log"]);
        assert_eq!(
            texts("for i in 1 2; do echo $i; done > x.log"),
            vec!["x.log"]
        );
        assert_eq!(texts("(a; b) >> x.log"), vec!["x.log"]);
        assert_eq!(texts("echo $(make > x.log)"), vec!["x.log"]);
    }

    #[test]
    fn a_nested_path_still_fires_as_a_shape() {
        assert!(fires("make > sub/x.log"));
    }

    #[test]
    fn a_home_path_fires_and_a_quoted_one_is_unknown() {
        assert!(fires("make > ~/Developer/Perso/x.log"));
        assert!(fires("make > ~"));
        assert!(!fires("make > \"~/x.log\""));
        assert!(!fires("make > ~user/x.log"));
    }

    #[test]
    fn descriptors_devices_and_unknown_words_are_skipped() {
        for c in [
            "make > /dev/null",
            "make 2>&1",
            "make >&2",
            "make 2>&-",
            "make >&-",
            "make > $LOG",
            "make > $DIR/x.log",
            "make > x.$$",
            "make > \"$DIR/x.log\"",
            "make > *.log",
            "make > $(mktemp)",
            "make > {a,b}.log",
            "make < in.txt",
            "make 2> /dev/stderr",
            "echo hi",
            "make | tee /dev/null",
        ] {
            assert!(!fires(c), "{c}");
        }
    }

    // ---- confirm

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wrw-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let dir = dir.canonicalize().expect("canonical");
        // Windows canonicalizes to a verbatim `\\?\C:\…` path, which
        // `git worktree add` rejects; the plain drive path names the same place.
        match dir.to_str().and_then(|d| d.strip_prefix(r"\\?\")) {
            Some(plain) => PathBuf::from(plain),
            None => dir,
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A root with repositories `a` (a directory `.git`) and `b` (a `.git`
    /// file, as a worktree or submodule has).
    fn root(tag: &str) -> PathBuf {
        let root = fixture(tag);
        std::fs::create_dir_all(root.join("a/.git")).unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::write(root.join("b/.git"), "gitdir: /elsewhere/.git/modules/b\n").unwrap();
        std::fs::write(root.join("loose.log"), "x").unwrap();
        root
    }

    fn verdict(command: &str, cwd: &Path, transcript: Option<&Path>) -> Confirmed {
        let parsed = lex(command);
        let finding = examine(&Input::new(command, Dialect::Bash, &parsed)).expect("shape");
        let ctx = Context {
            platform: None,
            cwd,
            parsed: &parsed,
            background: false,
            timeout_ms: None,
            tool_use_id: "",
            transcript,
            agent_transcript: None,
            command,
        };
        confirm(&ctx, &finding)
    }

    fn fires_in(command: &str, cwd: &Path) -> bool {
        matches!(verdict(command, cwd, None), Confirmed::YesSaying { .. })
    }

    fn said_of(c: Confirmed) -> (String, String) {
        match c {
            Confirmed::YesSaying { reason, remedy, .. } => (reason, remedy.expect("a remedy")),
            _ => panic!("expected a refusal"),
        }
    }

    #[test]
    fn a_root_with_two_repositories_fires() {
        let root = root("fires");
        assert!(fires_in("make > x.log", &root));
        assert!(fires_in("make 2>&1 | tee x.log", &root));
        // A Windows path in a shell word is backslash-escaped: Unix paths only.
        #[cfg(unix)]
        assert!(fires_in(&format!("make > {}/x.log", root.display()), &root));
        assert!(fires_in("make > ./x.log", &root));
        assert!(fires_in("make > a/../x.log", &root));
    }

    #[test]
    fn the_reason_counts_the_loose_files() {
        let root = root("count");
        let (reason, _) = said_of(verdict("make > sb-dev.log", &root, None));
        assert!(
            reason.starts_with("`sb-dev.log` would land in `"),
            "{reason}"
        );
        assert!(reason.contains("(1 files there today)"), "{reason}");
        assert!(reason.contains("nothing sweeps it"), "{reason}");
    }

    #[test]
    fn one_repository_is_not_a_root() {
        let root = fixture("one");
        std::fs::create_dir_all(root.join("a/.git")).unwrap();
        assert!(!fires_in("make > x.log", &root));
    }

    #[test]
    fn a_repository_cwd_passes() {
        let root = root("repo");
        assert!(!fires_in("make > x.log", &root.join("a")));
        assert!(!fires_in("make > x.log", &root.join("b")));
    }

    #[test]
    fn a_nested_path_passes() {
        let root = root("nested");
        assert!(!fires_in("make > a/x.log", &root));
        assert!(!fires_in("make > a/sub/x.log", &root));
        assert!(!fires_in("make > missing/x.log", &root));
    }

    #[test]
    fn a_cd_into_a_repository_first_passes() {
        let root = root("cd");
        assert!(!fires_in("cd a && make > x.log", &root));
        assert!(!fires_in("cd a && make 2>&1 | tee x.log", &root));
        assert!(!fires_in("cd a; make > x.log", &root));
        assert!(!fires_in("cd a && (make > x.log)", &root));
        // A `cd` back out lands in the root again.
        assert!(fires_in("cd a && cd .. && make > x.log", &root));
    }

    #[test]
    fn a_relative_write_after_an_unresolvable_cd_is_unconfirmed() {
        let root = root("unresolved");
        assert!(!fires_in(
            "for r in a b; do cd /ws/$r-wt-x && printf x >> .prettierignore; done",
            &root
        ));
        assert!(!fires_in("cd $D && make > x.log", &root));
        assert!(!fires_in("cd a* && make > x.log", &root));
        assert!(fires_in("make > x.log", &root));
        // A literal cd still resolves (`.`, not the root's path: a Windows
        // path is no shell word).
        assert!(fires_in("cd . && make > x.log", &root));
    }

    #[test]
    fn a_cd_inside_a_substitution_moves_that_subshell_only() {
        let root = root("subst");
        assert!(!fires_in("echo $(cd a && make > x.log)", &root));
        assert!(fires_in("echo $(cd a && make); make > x.log", &root));
    }

    // `HOME` and `/tmp` are Unix facts; Windows CI sets neither.
    #[cfg(unix)]
    #[test]
    fn a_home_target_is_resolved_against_home() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
        let parsed = lex("");
        let ctx = Context {
            platform: None,
            cwd: Path::new("/"),
            parsed: &parsed,
            background: false,
            timeout_ms: None,
            tool_use_id: "",
            transcript: None,
            agent_transcript: None,
            command: "",
        };
        let at = |text: &str| {
            target_path(
                &ctx,
                &Target {
                    span: 0..0,
                    text: text.to_string(),
                    home: true,
                },
            )
        };
        assert_eq!(
            at("/Developer/Perso/x.log"),
            Some(home.join("Developer/Perso/x.log"))
        );
        assert_eq!(at(""), Some(home));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_root_fires() {
        let root = root("sym");
        let link = fixture("link").join("ws");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert!(fires_in("make > x.log", &link));
        assert!(fires_in(&format!("make > {}/x.log", link.display()), &root));
    }

    // `HOME` and `/tmp` are Unix facts; Windows CI sets neither.
    #[cfg(unix)]
    #[test]
    fn a_temp_dir_with_two_repositories_never_fires() {
        let temp = std::env::temp_dir().canonicalize().unwrap();
        assert!(is_temp_dir(&temp));
        assert!(!is_workspace_root(&temp));
        assert!(!is_workspace_root(
            &PathBuf::from("/tmp").canonicalize().unwrap()
        ));
    }

    // `HOME` and `/tmp` are Unix facts; Windows CI sets neither.
    #[cfg(unix)]
    #[test]
    fn home_is_never_a_root() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap())
            .canonicalize()
            .unwrap();
        assert!(is_temp_dir(&home));
        assert!(!is_workspace_root(&home));
    }

    #[test]
    fn a_missing_command_or_a_missing_directory_passes() {
        let root = root("missing");
        let parsed = lex("make > x.log");
        let finding = examine(&Input::new("make > x.log", Dialect::Bash, &parsed)).unwrap();
        let ctx = Context {
            platform: None,
            cwd: &root,
            parsed: &parsed,
            background: false,
            timeout_ms: None,
            tool_use_id: "",
            transcript: None,
            agent_transcript: None,
            command: "",
        };
        assert!(matches!(confirm(&ctx, &finding), Confirmed::No(_)));
        assert!(!fires_in("make > x.log", &root.join("gone")));
    }

    // ---- the remedy

    fn line(tool: &str, key: &str, value: &str) -> String {
        format!(
            "{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"name\":\"{tool}\",\"input\":{{\"{key}\":{}}}}}]}}}}\n",
            serde_json::to_string(value).unwrap()
        )
    }

    fn transcript(dir: &Path, lines: &[String]) -> PathBuf {
        let slug = dir.join("-proj");
        std::fs::create_dir_all(&slug).unwrap();
        let path = slug.join("sess-1.jsonl");
        std::fs::write(&path, lines.concat()).unwrap();
        path
    }

    /// A primary checkout with two linked worktrees that ignore `*.log`
    /// (`w1`, `w2`) and one that does not (`w3`), beside a submodule-like
    /// directory `sub`.
    struct Trees {
        top: PathBuf,
        primary: PathBuf,
        w1: PathBuf,
        w2: PathBuf,
        w3: PathBuf,
        sub: PathBuf,
    }

    fn trees(tag: &str) -> Trees {
        let top = fixture(tag);
        let primary = top.join("primary");
        std::fs::create_dir_all(&primary).unwrap();
        git(&primary, &["init", "-q", "-b", "main"]);
        std::fs::write(primary.join(".gitignore"), "*.log\n").unwrap();
        git(&primary, &["add", ".gitignore"]);
        git(&primary, &["commit", "-q", "-m", "init"]);
        let w = |n: &str| {
            let p = top.join(n);
            git(
                &primary,
                &["worktree", "add", "-q", p.to_str().unwrap(), "-b", n],
            );
            p
        };
        let (w1, w2, w3) = (w("w1"), w("w2"), w("w3"));
        // A global excludes file may ignore logs too: this rule beats it.
        std::fs::write(w3.join(".gitignore"), "!*.log\n").unwrap();
        let sub = top.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(".git"), "gitdir: /x/primary/.git/modules/sub\n").unwrap();
        Trees {
            top,
            primary,
            w1,
            w2,
            w3,
            sub,
        }
    }

    fn rd(p: &Path) -> String {
        p.join("src/lib.rs").display().to_string()
    }

    #[test]
    fn the_latest_of_two_linked_worktrees_wins() {
        let t = trees("latest");
        let tr = transcript(
            &t.top,
            &[
                line("Edit", "file_path", &rd(&t.w1)),
                line("Read", "file_path", &rd(&t.w2)),
            ],
        );
        assert_eq!(latest_worktree(&tr, TAIL_BYTES), Some(t.w2.clone()));
    }

    // holds-until: `bash_paths` reads absolute Unix operands only (see its
    // comment); Windows `cd C:\…` forms end it.
    #[cfg(unix)]
    #[test]
    fn every_path_key_and_bash_form_counts() {
        let t = trees("forms");
        for (tool, key, value) in [
            ("Grep", "path", t.w1.display().to_string()),
            ("NotebookEdit", "notebook_path", rd(&t.w1)),
            ("LSP", "filePath", rd(&t.w1)),
            (
                "Bash",
                "command",
                format!("git -C {} status", t.w1.display()),
            ),
            (
                "Bash",
                "command",
                format!("cd \"{}\" && ls", t.w1.display()),
            ),
        ] {
            let tr = transcript(&t.top, &[line(tool, key, &value)]);
            assert_eq!(
                latest_worktree(&tr, TAIL_BYTES),
                Some(t.w1.clone()),
                "{tool}"
            );
        }
        // A tool that is not a file tool, and a relative path, name nothing.
        let tr = transcript(&t.top, &[line("WebFetch", "path", &rd(&t.w1))]);
        assert_eq!(latest_worktree(&tr, TAIL_BYTES), None);
        let tr = transcript(&t.top, &[line("Read", "file_path", "src/lib.rs")]);
        assert_eq!(latest_worktree(&tr, TAIL_BYTES), None);
    }

    #[test]
    fn a_primary_checkout_touched_last_is_skipped() {
        let t = trees("primary");
        let tr = transcript(
            &t.top,
            &[
                line("Edit", "file_path", &rd(&t.w1)),
                line("Read", "file_path", &rd(&t.primary)),
            ],
        );
        assert_eq!(latest_worktree(&tr, TAIL_BYTES), None);
        assert_eq!(tree_of(&t.primary), Tree::Other);
    }

    #[test]
    fn a_submodule_touched_last_is_skipped() {
        let t = trees("submodule");
        let tr = transcript(
            &t.top,
            &[
                line("Edit", "file_path", &rd(&t.w1)),
                line("Read", "file_path", &rd(&t.sub)),
            ],
        );
        assert_eq!(latest_worktree(&tr, TAIL_BYTES), None);
        assert_eq!(tree_of(&t.sub), Tree::Other);
    }

    #[test]
    fn a_path_in_no_tree_is_passed_over() {
        let t = trees("notree");
        let tr = transcript(
            &t.top,
            &[
                line("Edit", "file_path", &rd(&t.w1)),
                line("Read", "file_path", "/nonexistent-wrw/x.rs"),
            ],
        );
        assert_eq!(latest_worktree(&tr, TAIL_BYTES), Some(t.w1.clone()));
    }

    #[test]
    fn the_tail_drops_its_first_cut_line() {
        let t = trees("cut");
        let only = line("Edit", "file_path", &rd(&t.w1));
        let tr = transcript(&t.top, std::slice::from_ref(&only));
        assert_eq!(latest_worktree(&tr, TAIL_BYTES), Some(t.w1.clone()));
        // Cut inside the line: what is left of it must not be read.
        assert_eq!(latest_worktree(&tr, only.len() as u64 - 10), None);
        // With a whole line after it, the cut one is gone and the next stays.
        let both = [only.clone(), line("Edit", "file_path", &rd(&t.w2))];
        let tr = transcript(&t.top, &both);
        assert_eq!(
            latest_worktree(&tr, both[1].len() as u64 + 10),
            Some(t.w2.clone())
        );
    }

    #[test]
    fn an_unreadable_transcript_names_nothing() {
        assert_eq!(
            latest_worktree(Path::new("/nonexistent-wrw.jsonl"), TAIL_BYTES),
            None
        );
        let dir = fixture("garbage");
        let p = dir.join("t.jsonl");
        std::fs::write(&p, "not json\n{\n").unwrap();
        assert_eq!(latest_worktree(&p, TAIL_BYTES), None);
    }

    #[test]
    fn a_worktree_that_ignores_the_file_is_named() {
        let t = trees("named");
        let ws = root("named-root");
        let tr = transcript(&t.top, &[line("Edit", "file_path", &rd(&t.w1))]);
        let (_, remedy) = said_of(verdict("make > probe.log", &ws, Some(&tr)));
        let wt = t.w1.display();
        assert!(
            remedy.contains(&format!("This session works in the worktree `{wt}`")),
            "{remedy}"
        );
        assert!(
            remedy.contains(&format!("write `{wt}/probe.log`")),
            "{remedy}"
        );
        assert!(remedy.contains("or `"), "{remedy}");
        assert!(remedy.contains("/claude-"), "{remedy}");
        assert!(remedy.ends_with("If the file belongs in the root on purpose, ask the person."));
    }

    #[test]
    fn a_worktree_that_does_not_ignore_it_is_not_named() {
        let t = trees("unignored");
        let ws = root("unignored-root");
        let tr = transcript(&t.top, &[line("Edit", "file_path", &rd(&t.w3))]);
        let (_, remedy) = said_of(verdict("make > probe.log", &ws, Some(&tr)));
        assert!(!remedy.contains("works in the worktree"), "{remedy}");
        assert!(!remedy.contains(&t.w3.display().to_string()), "{remedy}");
        // The session's own directory needs the owner's uid, a Unix fact.
        #[cfg(unix)]
        assert!(remedy.contains("sess-1/probe.log"), "{remedy}");
        assert!(remedy.contains("worktree-sweep --scratch"), "{remedy}");
        assert!(remedy.ends_with("ask the person."), "{remedy}");
    }

    #[cfg(unix)]
    #[test]
    fn the_scratch_directory_is_built_from_the_transcript() {
        let t = trees("scratch");
        let tr = transcript(&t.top, &[]);
        let uid = owner_uid(&tr).unwrap();
        let d = scratch_dir(&tr).expect("scratch dir");
        assert!(d.ends_with(format!("claude-{uid}/-proj/sess-1")), "{d:?}");
    }

    #[test]
    fn without_a_transcript_the_remedy_still_names_a_scratch_place() {
        let ws = root("notranscript");
        let (_, remedy) = said_of(verdict("make > probe.log", &ws, None));
        assert!(remedy.contains("claude-<uid>"), "{remedy}");
        assert!(remedy.contains("probe.log"), "{remedy}");
        assert!(remedy.ends_with("ask the person."), "{remedy}");
    }

    #[test]
    fn the_rule_is_default_deny_with_a_confirm() {
        assert_eq!(RULE.default_stance, Stance::Deny);
        assert_eq!(RULE.max_stance, Stance::Deny);
        assert!(RULE.confirm.is_some());
    }
}
