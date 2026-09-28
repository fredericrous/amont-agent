//! What a `git push` would publish: which repository, which commits, to which
//! refs.
//!
//! A documented subset, on purpose. The only pushes the preview guard sees in
//! practice come from the `worktree-task` skill — `git push -u origin
//! <branch>` — so v1 reads an explicit remote plus explicit refspecs, with
//! `-C` and a leading `cd` honoured. Everything else is [`Push::Unresolvable`]
//! with the shape named, never an approximation of git's own defaults
//! (`push.default`, `remote.<r>.push`, `pushRemote`): a guard that guesses
//! which commit a push publishes can approve the wrong one. The journal's
//! list of unresolvable shapes is what decides the next ones to support.
//!
//! Two halves, like a rule: [`read`] is pure and runs on the words alone;
//! [`resolve`] asks git in the repository the push actually runs in.

use std::path::{Path, PathBuf};

use crate::shell::Simple;

/// The destination kind decides whether a ref is judged: a tag is excluded,
/// a branch is judged — including a tag's commit pushed onto a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Branch,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub remote: String,
    /// The commit being published.
    pub src: String,
    /// The full destination ref, `refs/heads/…` or `refs/tags/…`.
    pub dst: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Push {
    /// Read and resolved.
    Resolved { repo: PathBuf, targets: Vec<Target> },
    /// A dry run publishes nothing and is not judged at all.
    DryRun,
    /// A push this guard will not interpret, and why.
    Unresolvable {
        repo: Option<PathBuf>,
        shape: &'static str,
    },
}

/// The push as written: directories, remote, refspecs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    /// Each `git -C <dir>`, in order.
    pub dirs: Vec<String>,
    pub remote: String,
    pub refspecs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    Spec(Spec),
    DryRun,
    Unresolvable(&'static str),
}

/// Flags of `git push` that change nothing about WHAT is published.
const HARMLESS: &[&str] = &[
    "-u",
    "--set-upstream",
    "-f",
    "--force",
    "--force-if-includes",
    "-q",
    "--quiet",
    "-v",
    "--verbose",
    "--progress",
    "--no-progress",
    "--porcelain",
    "--no-verify",
    "--verify",
    "--atomic",
    "--no-atomic",
    "--signed",
    "--no-signed",
    "--ipv4",
    "--ipv6",
    "-4",
    "-6",
];

/// Flags that take a separate value and change nothing about what is
/// published.
const HARMLESS_VALUED: &[&str] = &["-o", "--push-option", "--receive-pack", "--exec", "--repo"];

/// Flags whose question this guard does not answer.
const UNRESOLVABLE: &[(&str, &str)] = &[
    ("--all", "--all pushes every branch"),
    ("--branches", "--branches pushes every branch"),
    ("--mirror", "--mirror pushes every ref"),
    ("--tags", "--tags pushes every tag"),
    ("--follow-tags", "--follow-tags adds tags"),
    ("--delete", "a delete publishes nothing"),
    ("-d", "a delete publishes nothing"),
    ("--prune", "--prune deletes remote refs"),
    ("--stdin", "refspecs come from stdin"),
];

/// Read the push out of a `git … push …` clause. Pure.
pub fn read(cmd: &Simple) -> Read {
    let Some(start) = cmd.program_index() else {
        return Read::Unresolvable("no program");
    };
    let words = &cmd.words[start + 1..];
    let mut dirs = Vec::new();
    let mut i = 0;
    // Global options before the subcommand. `-C` is honoured; anything else
    // that could move the repository is not guessed at.
    loop {
        let Some(w) = words.get(i) else {
            return Read::Unresolvable("no push subcommand");
        };
        if w.expanded {
            return Read::Unresolvable("a word comes from a substitution");
        }
        match w.text.as_str() {
            "push" if !w.quoted => break,
            "-C" => {
                let Some(d) = words.get(i + 1) else {
                    return Read::Unresolvable("-C without a directory");
                };
                if d.expanded {
                    return Read::Unresolvable("a word comes from a substitution");
                }
                dirs.push(d.text.clone());
                i += 2;
            }
            "-c" => i += 2,
            "--no-pager" | "--no-replace-objects" | "--literal-pathspecs" => i += 1,
            _ => return Read::Unresolvable("a git global option this guard does not read"),
        }
    }
    let mut operands: Vec<String> = Vec::new();
    let mut after_ddash = false;
    let mut j = i + 1;
    while let Some(w) = words.get(j) {
        j += 1;
        if w.expanded {
            return Read::Unresolvable("a word comes from a substitution");
        }
        let t = w.text.as_str();
        if after_ddash || w.quoted || !t.starts_with('-') || t == "-" {
            operands.push(t.to_string());
            continue;
        }
        if t == "--" {
            after_ddash = true;
            continue;
        }
        if t == "--dry-run" || t == "-n" {
            return Read::DryRun;
        }
        if let Some((_, why)) = UNRESOLVABLE.iter().find(|(f, _)| *f == t) {
            return Read::Unresolvable(why);
        }
        if HARMLESS.contains(&t) || t.starts_with("--force-with-lease") {
            continue;
        }
        if HARMLESS_VALUED.contains(&t) {
            j += 1;
            continue;
        }
        if HARMLESS_VALUED
            .iter()
            .any(|f| t.starts_with(&format!("{f}=")))
        {
            continue;
        }
        // A short cluster such as `-uf`: every letter must be harmless.
        if !t.starts_with("--") && t.len() > 2 {
            let letters = &t[1..];
            if letters.contains('n') {
                return Read::DryRun;
            }
            if letters.contains('d') {
                return Read::Unresolvable("a delete publishes nothing");
            }
            if letters.chars().all(|c| "ufqv46".contains(c)) {
                continue;
            }
        }
        return Read::Unresolvable("a push flag this guard does not read");
    }
    match operands.as_slice() {
        [] => Read::Unresolvable("no remote named; push.default decides"),
        [_] => Read::Unresolvable("no refspec named; push.default decides"),
        [remote, refspecs @ ..] => {
            if remote.contains('/') || remote.contains(':') {
                return Read::Unresolvable("the remote is a URL or path, not a configured name");
            }
            for r in refspecs {
                let bare = r.strip_prefix('+').unwrap_or(r);
                if bare.contains('*') {
                    return Read::Unresolvable("a glob refspec");
                }
                if bare.starts_with(':') || bare.is_empty() {
                    return Read::Unresolvable("a delete publishes nothing");
                }
            }
            Read::Spec(Spec {
                dirs,
                remote: remote.clone(),
                refspecs: refspecs.to_vec(),
            })
        }
    }
}

/// Resolve a read push against the repository it runs in. `cwd` is the
/// directory of the clause (`Context::cwd_at`).
pub fn resolve(cwd: &Path, cmd: &Simple) -> Push {
    let spec = match read(cmd) {
        Read::Spec(s) => s,
        Read::DryRun => return Push::DryRun,
        Read::Unresolvable(shape) => {
            return Push::Unresolvable {
                repo: toplevel(cwd),
                shape,
            }
        }
    };
    let dir = match directory(cwd, &spec) {
        Ok(d) => d,
        Err(shape) => return Push::Unresolvable { repo: None, shape },
    };
    let Some(repo) = toplevel(&dir) else {
        return Push::Unresolvable {
            repo: None,
            shape: "not a git repository",
        };
    };
    let mut targets = Vec::new();
    for r in &spec.refspecs {
        let bare = r.strip_prefix('+').unwrap_or(r);
        let (src, dst) = match bare.split_once(':') {
            Some((s, d)) => (s, Some(d)),
            None => (bare, None),
        };
        let Some(commit) = git(
            &repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{src}^{{commit}}"),
            ],
        ) else {
            return Push::Unresolvable {
                repo: Some(repo),
                shape: "the refspec source does not resolve to a commit",
            };
        };
        let Some(dst) = qualify(&repo, src, dst) else {
            return Push::Unresolvable {
                repo: Some(repo),
                shape: "the destination ref cannot be named without guessing",
            };
        };
        let kind = if dst.starts_with("refs/tags/") {
            Kind::Tag
        } else if dst.starts_with("refs/heads/") {
            Kind::Branch
        } else {
            return Push::Unresolvable {
                repo: Some(repo),
                shape: "the destination is neither a branch nor a tag",
            };
        };
        targets.push(Target {
            remote: spec.remote.clone(),
            src: commit,
            dst,
            kind,
        });
    }
    Push::Resolved { repo, targets }
}

/// The directory the push runs in: `cwd` moved by each `-C`.
fn directory(cwd: &Path, spec: &Spec) -> Result<PathBuf, &'static str> {
    let mut dir = cwd.to_path_buf();
    for d in &spec.dirs {
        dir = if let Some(rest) = d.strip_prefix("~/") {
            match std::env::var_os("HOME") {
                Some(h) => PathBuf::from(h).join(rest),
                None => return Err("-C names a home-relative path and HOME is unset"),
            }
        } else {
            dir.join(d)
        };
    }
    Ok(dir)
}

/// The repository a push runs in, without resolving what it publishes:
/// what the journal names it by. `cwd` is the directory of the clause.
pub fn repository(cwd: &Path, cmd: &Simple) -> Option<PathBuf> {
    match read(cmd) {
        Read::Spec(spec) => toplevel(&directory(cwd, &spec).ok()?),
        _ => toplevel(cwd),
    }
}

/// The full destination ref for `src[:dst]`, the way git names it — or
/// `None` where naming it would be a guess.
fn qualify(repo: &Path, src: &str, dst: Option<&str>) -> Option<String> {
    let is = |r: &str| git(repo, &["rev-parse", "--verify", "--quiet", r]).is_some();
    match dst {
        Some(d) if d.starts_with("refs/") => Some(d.to_string()),
        Some(d) => {
            // An unqualified destination takes the source's namespace.
            if src.starts_with("refs/tags/")
                || (!src.starts_with("refs/")
                    && is(&format!("refs/tags/{src}"))
                    && !is(&format!("refs/heads/{src}")))
            {
                Some(format!("refs/tags/{d}"))
            } else {
                Some(format!("refs/heads/{d}"))
            }
        }
        None if src == "HEAD" => {
            let branch = git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
            Some(format!("refs/heads/{branch}"))
        }
        None if src.starts_with("refs/") => Some(src.to_string()),
        None => {
            let heads = is(&format!("refs/heads/{src}"));
            let tags = is(&format!("refs/tags/{src}"));
            match (heads, tags) {
                (true, false) => Some(format!("refs/heads/{src}")),
                (false, true) => Some(format!("refs/tags/{src}")),
                // Both, or a bare SHA: git itself would refuse or guess.
                _ => None,
            }
        }
    }
}

pub fn toplevel(dir: &Path) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    git(dir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    crate::git::stdout_in(dir, args).filter(|s| !s.is_empty())
}

/// The `git … push` clause of a command, if it has exactly one. Pure.
pub fn find(parsed: &crate::shell::Parsed) -> Option<&Simple> {
    let mut pushes = parsed
        .judgeable()
        .filter(|c| c.program() == Some("git") && c.subcommand() == Some("push"));
    let first = pushes.next()?;
    Some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn read_of(command: &str) -> Read {
        let parsed = lex(command);
        let cmd = find(&parsed).expect("a push clause");
        read(cmd)
    }

    fn spec(remote: &str, refspecs: &[&str], dirs: &[&str]) -> Read {
        Read::Spec(Spec {
            dirs: dirs.iter().map(|s| s.to_string()).collect(),
            remote: remote.to_string(),
            refspecs: refspecs.iter().map(|s| s.to_string()).collect(),
        })
    }

    #[test]
    fn the_skills_own_push_is_read() {
        assert_eq!(
            read_of("git push -u origin feat/x"),
            spec("origin", &["feat/x"], &[])
        );
        assert_eq!(
            read_of("git push --set-upstream origin feat/x"),
            spec("origin", &["feat/x"], &[])
        );
        assert_eq!(
            read_of("git push origin HEAD:feat/x"),
            spec("origin", &["HEAD:feat/x"], &[])
        );
        assert_eq!(
            read_of("git push --force-with-lease=feat/x:abc origin +feat/x"),
            spec("origin", &["+feat/x"], &[])
        );
    }

    #[test]
    fn dash_c_is_honoured() {
        assert_eq!(
            read_of("git -C ../wt push -u origin feat/x"),
            spec("origin", &["feat/x"], &["../wt"])
        );
        assert_eq!(
            read_of("git -C /a -C b push origin other"),
            spec("origin", &["other"], &["/a", "b"])
        );
    }

    #[test]
    fn a_dry_run_is_not_judged() {
        assert_eq!(read_of("git push --dry-run origin main"), Read::DryRun);
        assert_eq!(read_of("git push -n origin main"), Read::DryRun);
        assert_eq!(read_of("git push -un origin main"), Read::DryRun);
    }

    #[test]
    fn the_shapes_v1_does_not_interpret_are_named() {
        for (command, shape) in [
            ("git push", "no remote named; push.default decides"),
            ("git push origin", "no refspec named; push.default decides"),
            ("git push --all origin", "--all pushes every branch"),
            ("git push --mirror backup", "--mirror pushes every ref"),
            ("git push --tags origin", "--tags pushes every tag"),
            ("git push origin --delete old", "a delete publishes nothing"),
            ("git push origin :old", "a delete publishes nothing"),
            (
                "git push origin 'refs/heads/*:refs/heads/*'",
                "a glob refspec",
            ),
            (
                "git push git@host:x.git main",
                "the remote is a URL or path, not a configured name",
            ),
            (
                "git push --frobnicate origin main",
                "a push flag this guard does not read",
            ),
            (
                "git push origin $(git branch --show-current)",
                "a word comes from a substitution",
            ),
        ] {
            assert_eq!(read_of(command), Read::Unresolvable(shape), "{command}");
        }
    }

    #[test]
    fn several_refspecs_are_read_in_order() {
        assert_eq!(
            read_of("git push origin feat/x v1.2.0"),
            spec("origin", &["feat/x", "v1.2.0"], &[])
        );
    }
}
