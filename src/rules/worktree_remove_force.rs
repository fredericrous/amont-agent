//! `worktree-remove-force` — deleting a worktree with whatever is in it.
//!
//! `git worktree remove` refuses when the worktree has uncommitted changes
//! or untracked files, and that refusal is the only warning anyone gets.
//! `--force` skips it: the directory goes, edits and all, and the command
//! prints nothing about what went. The habit forms because a finished
//! worktree usually has leftover build output that also trips the refusal,
//! so `--force` becomes the default spelling — and then one day the
//! worktree was not finished. Measured over forty-two sessions: 101 forced
//! removals.
//!
//! ## `confirm` looks inside first
//!
//! The shape fires on every `--force`; `confirm` asks git what the worktree
//! holds and stays silent when it is clean — which is most of the time, and
//! is exactly when `--force` was harmless.

use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "worktree-remove-force",
    default_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 5.2,
        measured: "2026-09-05",
        trend: Trend::Flat(8),
    },
    examine,
    confirm: Some(confirm),
};

fn detect(cmd: &Simple) -> Option<String> {
    if cmd.program() != Some("git") || cmd.subcommand() != Some("worktree") {
        return None;
    }
    let ops = cmd.operands();
    if ops.get(1).map(|w| w.text.as_str()) != Some("remove") {
        return None;
    }
    if !cmd.has_flag("--force") && !cmd.has_short('f') {
        return None;
    }
    // The path is the operand after `remove`.
    let path = ops.get(2)?;
    if path.expanded {
        return None;
    }
    Some(path.text.clone())
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    for cmd in parsed.clauses() {
        let Some(path) = detect(cmd) else { continue };
        return Some(Finding {
            reason: format!(
                "`git worktree remove --force` deletes `{path}` with whatever is \
                 uncommitted in it and prints nothing about what went; the refusal it \
                 skips is the only warning."
            ),
            remedy: format!(
                "Look first: `git -C {path} status --short`. Commit or move what matters, \
                 then remove without `--force` — it refuses only when something would be \
                 lost."
            ),
            span: cmd.at..cmd.end,
        });
    }
    None
}

fn confirm(ctx: &Context, f: &Finding) -> Confirmed {
    let Some(path) = ctx
        .parsed
        .clauses()
        .iter()
        .find(|c| c.at == f.span.start)
        .and_then(detect)
    else {
        return Confirmed::No("the command no longer matches");
    };
    let cwd = ctx.cwd_at(f.span.start);
    let target = if let Some(rest) = path.strip_prefix("~/") {
        match std::env::var_os("HOME") {
            Some(h) => std::path::PathBuf::from(h).join(rest),
            None => return Confirmed::No("cannot resolve ~"),
        }
    } else {
        cwd.join(&path)
    };
    if !target.is_dir() {
        return Confirmed::No("nothing at that path");
    }
    match crate::git::stdout_in(&target, &["status", "--porcelain", "--untracked-files=all"]) {
        Some(s) if !s.trim().is_empty() => Confirmed::Yes,
        Some(_) => Confirmed::No("the worktree is clean"),
        None => Confirmed::No("git could not read the worktree"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn path_of(command: &str) -> Option<String> {
        lex(command).clauses().iter().find_map(detect)
    }

    #[test]
    fn a_forced_removal_names_its_path() {
        assert_eq!(
            path_of(
                "git worktree remove /Users/me/Developer/x-wt-y --force 2>&1; git worktree list"
            ),
            Some("/Users/me/Developer/x-wt-y".to_string())
        );
        assert_eq!(
            path_of("cd ~/Developer/homelab && git worktree remove /tmp/hl-bump --force"),
            Some("/tmp/hl-bump".to_string())
        );
        assert_eq!(
            path_of("git worktree remove -f ../x"),
            Some("../x".to_string())
        );
    }

    #[test]
    fn a_plain_removal_keeps_gits_own_refusal() {
        assert_eq!(path_of("git worktree remove ../amont-wt-rehearse"), None);
        assert_eq!(path_of("git worktree prune"), None);
        assert_eq!(path_of("git worktree list"), None);
        assert_eq!(path_of("git worktree add --force ../x feat/y"), None);
    }
}
