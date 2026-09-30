//! The implementation review (ADR-0022, `work.implementation-review`):
//! before the push of a branch that carries a plan, one independent
//! reviewer has read the diff against the plan and the active rules, and
//! its verdict is bound to the tree it read.
//!
//! A review is bound to the **canonical tree id** of the commit it read:
//! the sha256 of `git ls-tree -r -z --full-tree <rev>`'s entries — mode,
//! type, object id and path, in git's own order — with every entry under
//! `docs/plans/` left out. So recording the review in the plan never makes
//! it stale, a later code change does, and the id is 64 hex whatever the
//! repository's object format. It is computed read-only: no index, no
//! object written, nothing to clean up if the hook is killed.
//!
//! The repository in a block is named by the basename of the parent of
//! `git rev-parse --git-common-dir`, so a worktree checked out as
//! `amont-agent-wt-x` still says `amont-agent`, and `tree-sha` and the
//! rule cannot disagree on the name.
//!
//! ## What the hook reads
//!
//! A push is judged by `confirm` (the rule is `rules::implementation_review`):
//! the pushed commit, the base it grew from (the remote's default branch,
//! never the branch's own tracking ref — once a branch is published its
//! plan commit is on the remote too, and a later push compared to the
//! tracking ref would show no `docs/plans/` change and skip the rule for
//! good), whether `docs/plans/` changed since that base, the canonical tree,
//! and then a binding: a completed `implementation-review` agent in this
//! session's transcript whose block names this repository and tree, or a
//! pass remembered under `<state>/implementation-review/by-tree/`.
//!
//! The verdict is read, not assumed: a review that said `rework` and got
//! no later delta on the same tree is not a pass. Only the hook writes a
//! pass file — from a transcript binding, or from the person's `Overrule`
//! on a marked question — and a Bash, Write or Edit under that directory
//! is refused by the hook itself, outside any stance, because under `deny`
//! a pass file is a trust boundary.

use std::path::{Component, Path, PathBuf};

use crate::git;
use crate::journal;
use crate::plan_review::{completed_agents, hex, sha256, Completed};
use crate::rules::{Confirmed, Context, Stance};
use crate::shell::Simple;

pub const RULE_ID: &str = "implementation-review";
/// The agent type the reviewer is launched as; a prefix, like the panel's.
const AGENT: &str = "implementation-review";
/// Opens a review block in the reviewer's prompt.
pub const BLOCK_OPEN: &str = "<<<TREE ";
const BLOCK_CLOSE: &str = ">>>";
/// The directory a review does not bind to: the plan's own record.
const PLANS_DIR: &[u8] = b"docs/plans/";
const PLANS_PATH: &str = "docs/plans";
/// The marker a person's overrule question carries.
const MARKER: &str = "[implementation-review ";
const OVERRULE: &str = "Overrule";

// ---------------------------------------------------------------- the tree

/// The canonical tree id of `rev` in `repo`: 64 lowercase hex.
pub fn canonical_tree(repo: &Path, rev: &str) -> Result<String, String> {
    let spec = format!("{rev}^{{tree}}");
    let entries = git::ls_tree_entries(repo, &spec)
        .map_err(|e| format!("git could not be run in {}: {e}", repo.display()))?
        .ok_or_else(|| format!("`{rev}` does not name a tree in {}", repo.display()))?;
    let mut bytes: Vec<u8> = Vec::new();
    for entry in entries {
        // `<mode> <type> <oid>\t<path>`; the path is everything after the
        // first tab, and a path is never one that starts inside the plans.
        let path = entry
            .iter()
            .position(|c| *c == b'\t')
            .map(|i| &entry[i + 1..])
            .unwrap_or(&entry[..]);
        if path.starts_with(PLANS_DIR) {
            continue;
        }
        bytes.extend_from_slice(&entry);
        bytes.push(0);
    }
    Ok(hex(&sha256(&bytes)))
}

/// The repository's name for a block: the basename of the directory that
/// holds its common `.git`, whichever worktree `repo` is.
pub fn repo_name(repo: &Path) -> Option<String> {
    let common = git::stdout_in(repo, &["rev-parse", "--git-common-dir"])?;
    if common.is_empty() {
        return None;
    }
    let common = PathBuf::from(common);
    let common = if common.is_absolute() {
        common
    } else {
        repo.join(common)
    };
    let common = std::fs::canonicalize(&common).unwrap_or(common);
    common
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
}

/// The review block a reviewer's prompt carries.
pub fn block(repo: &str, sha: &str) -> String {
    format!("{BLOCK_OPEN}repo={repo} sha={sha}{BLOCK_CLOSE}")
}

/// The `(repo, sha)` of every review block in a prompt. A sha that is not
/// 64 hex is not a binding.
pub fn parse_blocks(prompt: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = prompt;
    while let Some(at) = rest.find(BLOCK_OPEN) {
        rest = &rest[at + BLOCK_OPEN.len()..];
        let Some(end) = rest.find(BLOCK_CLOSE) else {
            break;
        };
        let inner = &rest[..end];
        rest = &rest[end..];
        let Some(body) = inner.strip_prefix("repo=") else {
            continue;
        };
        let Some(split) = body.rfind(" sha=") else {
            continue;
        };
        let repo = body[..split].trim().to_string();
        let sha = body[split + 5..].trim().to_ascii_lowercase();
        if repo.is_empty() || !is_sha256(&sha) {
            continue;
        }
        out.push((repo, sha));
    }
    out
}

pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

// ---------------------------------------------------------------- the verdict

/// What a reviewer's result said, read from its first `Verdict:` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    ApproveWithChanges,
    Rework,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Approve => "approve",
            Verdict::ApproveWithChanges => "approve-with-changes",
            Verdict::Rework => "rework",
        }
    }
}

/// The verdict a result text states, or `None` when it states none the
/// contract knows — which is not a pass.
pub fn verdict_of(result: &str) -> Option<Verdict> {
    let line = result.lines().find(|l| l.contains("Verdict:"))?;
    let after = line.split_once("Verdict:")?.1;
    let word: String = after
        .trim()
        .trim_start_matches(['*', '`', '_', ' '])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase();
    match word.as_str() {
        "approve-with-changes" => Some(Verdict::ApproveWithChanges),
        "approve" | "approved" => Some(Verdict::Approve),
        "rework" => Some(Verdict::Rework),
        _ => None,
    }
}

/// The reviews this session completed, in launch order: what each bound
/// and what it said.
pub struct Review {
    pub repo: String,
    pub sha: String,
    pub verdict: Option<Verdict>,
}

pub fn reviews(transcript: &Path) -> std::io::Result<Vec<Review>> {
    let file = std::fs::File::open(transcript)?;
    let reader = std::io::BufReader::new(file);
    Ok(completed_agents(reader, AGENT)
        .into_iter()
        .flat_map(|c: Completed| {
            let verdict = verdict_of(&c.result);
            parse_blocks(&c.prompt)
                .into_iter()
                .map(move |(repo, sha)| Review { repo, sha, verdict })
        })
        .collect())
}

// ---------------------------------------------------------------- the store

fn store() -> Option<PathBuf> {
    Some(journal::dir()?.join(RULE_ID))
}

fn private_dir(dir: &Path) -> Option<()> {
    std::fs::create_dir_all(dir).ok()?;
    journal::private(dir, 0o700);
    Some(())
}

fn pass_file(repo: &str, sha: &str) -> Option<PathBuf> {
    if repo.contains('/') || repo.contains("..") || !is_sha256(sha) {
        return None;
    }
    Some(
        store()?
            .join("by-tree")
            .join(repo)
            .join(format!("{sha}.json")),
    )
}

/// The verdict a remembered pass carries (`approve`, `approve-with-changes`
/// or `overruled`), if this repository and tree have one.
pub fn remembered(repo: &str, sha: &str) -> Option<String> {
    let f = pass_file(repo, sha)?;
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f).ok()?).ok()?;
    v.get("verdict")?.as_str().map(str::to_string)
}

/// Written only by the hook: from a transcript binding whose verdict is a
/// pass, or from the person's `Overrule` of a `rework`.
fn remember(repo: &str, sha: &str, verdict: &str, how: &str) -> Option<()> {
    let f = pass_file(repo, sha)?;
    private_dir(f.parent()?)?;
    let body = serde_json::json!({
        "repo": repo,
        "tree": sha,
        "verdict": verdict,
        "agent": AGENT,
        "how": how,
    })
    .to_string();
    crate::atomic::write_atomic(&f, &body).ok()?;
    journal::private(&f, 0o600);
    Some(())
}

// ---------------------------------------------------------------- the push

/// What the hook found out about a push.
pub enum Judged {
    /// The rule has nothing to say: the reason is the journal's
    /// `unconfirmed` reason.
    Decline(&'static str),
    /// The rule speaks: the reason to say, and the journal excerpt.
    Say { reason: String, excerpt: String },
}

/// Judge the push in `cmd`, whose clause runs in `cwd`.
pub fn judge_push(ctx: &Context, cmd: &Simple, stance: Stance) -> Judged {
    use crate::push_target::{self, Kind, Push};
    let cwd = ctx.cwd_at(cmd.at);
    let (repo, targets) = match push_target::resolve(&cwd, cmd) {
        Push::Resolved { repo, targets } => (repo, targets),
        Push::DryRun => return Judged::Decline("dry-run"),
        Push::Unresolvable { shape, .. } => {
            // Under `deny` an unreadable shape is held, so `--all` is not the
            // way around the gate; otherwise it is journalled and passes.
            return if stance == Stance::Deny {
                unknown(None, &format!("the push cannot be read: {shape}"))
            } else {
                Judged::Decline("unresolvable")
            };
        }
    };
    let Some(target) = targets.iter().find(|t| t.kind == Kind::Branch) else {
        return Judged::Decline("no-branch");
    };
    let pushed = target.src.as_str();
    let Some(base) = default_base(&repo, &target.remote) else {
        return unknown(
            None,
            &format!(
                "no default branch of `{}` is known here (no {0}/HEAD, {0}/main or {0}/master)",
                target.remote
            ),
        );
    };
    let Some(merge_base) = git::stdout_in(&repo, &["merge-base", pushed, &base]) else {
        return unknown(
            None,
            &format!("`git merge-base {} {base}` failed", short(pushed)),
        );
    };
    let Some(changed) = git::stdout_in(
        &repo,
        &["diff", "--name-only", &merge_base, pushed, "--", PLANS_PATH],
    ) else {
        return unknown(None, "`git diff` against the base failed");
    };
    if changed.trim().is_empty() {
        return Judged::Decline("no-plan");
    }
    let tree = match canonical_tree(&repo, pushed) {
        Ok(t) => t,
        Err(why) => return unknown(None, &why),
    };
    let Some(name) = repo_name(&repo) else {
        return unknown(Some(&tree), "the repository has no name");
    };
    match remembered(&name, &tree).as_deref() {
        Some("overruled") => return Judged::Decline("overruled"),
        Some(_) => return Judged::Decline("reviewed"),
        None => {}
    }
    let Some(transcript) = ctx.transcript else {
        return unknown(Some(&tree), "the payload carried no transcript_path");
    };
    let reviews = match reviews(transcript) {
        Ok(r) => r,
        Err(e) => {
            return unknown(
                Some(&tree),
                &format!(
                    "the transcript {} could not be read: {e}",
                    transcript.display()
                ),
            )
        }
    };
    let mine: Vec<&Review> = reviews.iter().filter(|r| r.repo == name).collect();
    if let Some(latest) = mine.iter().rev().find(|r| r.sha == tree) {
        return match latest.verdict {
            Some(Verdict::Rework) | None => Judged::Say {
                reason: format!(
                    "the implementation review of {name} tree {} said {}, and no delta followed.",
                    short(&tree),
                    latest
                        .verdict
                        .map(Verdict::as_str)
                        .unwrap_or("no verdict this guard can read")
                ),
                excerpt: format!(
                    "tree={} review=rework verdict={}",
                    short(&tree),
                    latest.verdict.map(Verdict::as_str).unwrap_or("-")
                ),
            },
            Some(v) => {
                remember(&name, &tree, v.as_str(), "transcript");
                Judged::Decline("reviewed")
            }
        };
    }
    if let Some(older) = mine.last() {
        return Judged::Say {
            reason: format!(
                "the implementation review of {name} read tree {}; this push carries tree {} (stale).",
                short(&older.sha),
                short(&tree)
            ),
            excerpt: format!("tree={} review=stale verdict=-", short(&tree)),
        };
    }
    Judged::Say {
        reason: format!(
            "no implementation review of {name} tree {} is in this session.",
            short(&tree)
        ),
        excerpt: format!("tree={} review=missing verdict=-", short(&tree)),
    }
}

fn unknown(tree: Option<&str>, why: &str) -> Judged {
    Judged::Say {
        reason: format!("whether this push was reviewed cannot be told: {why}"),
        excerpt: format!(
            "tree={} review=unknown verdict=-",
            tree.map(short).unwrap_or("-")
        ),
    }
}

/// The remote's default branch as last fetched: `<remote>/HEAD`, else
/// `<remote>/main`, else `<remote>/master`. Never `HEAD`: with the pushed
/// commit as its own base every push would show no plan.
fn default_base(repo: &Path, remote: &str) -> Option<String> {
    [
        format!("refs/remotes/{remote}/HEAD"),
        format!("refs/remotes/{remote}/main"),
        format!("refs/remotes/{remote}/master"),
    ]
    .into_iter()
    .find(|r| {
        git::stdout_in(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{r}^{{commit}}"),
            ],
        )
        .is_some_and(|s| !s.is_empty())
    })
}

/// The rule's `confirm`, for a push: what to say, or why not.
pub fn confirm_push(ctx: &Context, cmd: &Simple, stance: Stance) -> Confirmed {
    match judge_push(ctx, cmd, stance) {
        Judged::Decline(why) => Confirmed::No(why),
        Judged::Say { reason, excerpt } => Confirmed::YesSaying {
            floor: None,
            reason,
            excerpt,
        },
    }
}

// ---------------------------------------------------------------- the guards

/// Whether a path is inside the pass-file store, compared lexically (`.`
/// and `..` folded, no symlink followed) and, where the store exists, by
/// its canonical form too. Only the hook writes there.
pub fn guards_path(path: &Path) -> bool {
    let Some(store) = store() else {
        return false;
    };
    let here = lexical(path);
    if here.starts_with(lexical(&store)) {
        return true;
    }
    if let Ok(real) = std::fs::canonicalize(&store) {
        if here.starts_with(&real) {
            return true;
        }
        // The path's own existing ancestor, resolved.
        if let Some(parent) = here.parent() {
            if let Ok(real_parent) = std::fs::canonicalize(parent) {
                return real_parent.starts_with(&real);
            }
        }
    }
    false
}

/// A path with `.` and `..` folded lexically.
pub fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Programs whose operand naming the store is taken as a write. A reader
/// (`ls`, `cat`, `find`, `grep`, `stat`, …) is not on this list, so looking
/// at the record is allowed; the list is the Bash half of a best-effort
/// guard, since a parser cannot see inside `python -c` beyond its words.
const WRITERS: &[&str] = &[
    "rm", "mv", "cp", "tee", "touch", "install", "truncate", "ln", "chmod", "chown", "mkdir",
    "rmdir", "dd", "sed", "perl", "ruby", "node", "deno", "python", "python3", "sh", "bash", "zsh",
    "dash", "fish", "sudo", "xargs", "rsync", "tar", "unzip", "chezmoi", "git",
];

/// The first clause of a command that would WRITE into the store: a redirect
/// into it (`> …/by-tree/…`), or a program on [`WRITERS`] with a word naming
/// it. A read — `ls`, `cat`, `find` of the pass files — passes: the record is
/// there to be looked at. Returns the clause's span.
pub fn store_clause(parsed: &crate::shell::Parsed) -> Option<std::ops::Range<usize>> {
    parsed
        .clauses()
        .iter()
        .find(|cmd| {
            let redirected = cmd
                .redirects
                .iter()
                .any(|(op, w)| op.contains('>') && names_store(&w.text));
            let writer = cmd
                .program()
                .map(|p| p.rsplit('/').next().unwrap_or(p))
                .is_some_and(|p| WRITERS.contains(&p) || p.starts_with("python"));
            redirected || (writer && cmd.words.iter().any(|w| names_store(&w.text)))
        })
        .map(|cmd| cmd.at..cmd.end)
}

/// Whether a shell word names the store, spelled with either separator:
/// Windows writes the same directory with backslashes.
pub fn names_store(word: &str) -> bool {
    let word = word.replace('\\', "/");
    word.contains(&format!("amont-agent/{RULE_ID}/"))
        || word.contains(&format!("amont-agent/{RULE_ID}")) && word.ends_with(RULE_ID)
}

pub const GUARD_REASON: &str =
    "pass files under amont-agent/implementation-review/ are written by the hook alone: from a completed review in this session's transcript, or from the person's Overrule.";
pub const GUARD_REMEDY: &str =
    "Run the review (worktree-task F4b) or ask the person; never write the record by hand.";

// ---------------------------------------------------------------- the person

/// The `(repo, sha)` a marked question names:
/// `[implementation-review <repo>@<sha64>]`, split at the last `@`.
pub fn marker(question: &str) -> Option<(String, String)> {
    let start = question.find(MARKER)? + MARKER.len();
    let end = start + question[start..].find(']')?;
    let inner = question[start..end].trim();
    let at = inner.rfind('@')?;
    let repo = inner[..at].trim().to_string();
    let sha = inner[at + 1..].trim().to_ascii_lowercase();
    (!repo.is_empty() && is_sha256(&sha)).then_some((repo, sha))
}

fn asks_dir() -> Option<PathBuf> {
    Some(store()?.join("asks"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Before an `AskUserQuestion` runs: remember a marked question, and whether
/// it arrived pre-answered, so the answer can be trusted or not.
pub fn on_pre_ask(ask: &crate::payload::Ask) {
    let marked: Vec<(String, String)> = ask.questions.iter().filter_map(|q| marker(q)).collect();
    if marked.is_empty() || ask.tool_use_id.is_empty() || ask.tool_use_id.contains('/') {
        return;
    }
    let Some(d) = asks_dir() else { return };
    if private_dir(&d).is_none() {
        return;
    }
    let names: Vec<String> = marked.iter().map(|(r, s)| format!("{r}@{s}")).collect();
    let _ = crate::atomic::write_atomic(
        &d.join(&ask.tool_use_id),
        &format!(
            "{}\t{}\t{}\t{}\n",
            ask.session.replace(['\t', '\n'], " "),
            ask.prefilled,
            names.join(","),
            now_ms()
        ),
    );
    if ask.prefilled {
        note("prefilled", &ask.session, &marked[0].0, &names.join(","));
    }
}

/// After an `AskUserQuestion`: an `Overrule` of a `rework` this session's
/// transcript shows is remembered as a pass; anything else changes nothing.
/// Returns what the session must hear when an overrule bound nothing.
pub fn on_post_ask(ask: &crate::payload::Ask) -> Option<String> {
    if !ask.questions.iter().any(|q| marker(q).is_some()) {
        return None;
    }
    let recorded = asks_dir()
        .map(|d| d.join(&ask.tool_use_id))
        .filter(|_| !ask.tool_use_id.is_empty() && !ask.tool_use_id.contains('/'))
        .and_then(|p| {
            let t = std::fs::read_to_string(&p).ok();
            let _ = std::fs::remove_file(&p);
            t
        });
    let Some(recorded) = recorded else {
        note(
            "unrecorded",
            &ask.session,
            "-",
            "marked question with no PreToolUse record",
        );
        return None;
    };
    let f: Vec<&str> = recorded.trim_end().split('\t').collect();
    if f.len() != 4 || f[0] != ask.session || f[1] != "false" {
        return None;
    }
    let mut said: Vec<String> = Vec::new();
    for question in &ask.questions {
        let Some((repo, sha)) = marker(question) else {
            continue;
        };
        let label = ask
            .answers
            .iter()
            .find(|(q, _)| q == question)
            .map(|(_, l)| l.as_str())
            .unwrap_or("");
        if label != OVERRULE {
            if !label.is_empty() {
                note(
                    "answered",
                    &ask.session,
                    &repo,
                    &format!("tree={} answer={}", short(&sha), label.to_ascii_lowercase()),
                );
            }
            continue;
        }
        let Some(transcript) = ask.transcript.as_deref() else {
            note(
                "unknown",
                &ask.session,
                &repo,
                &format!("tree={} overrule without a transcript", short(&sha)),
            );
            said.push(format!(
                "The answer Overrule bound NOTHING for {repo} tree {}: the question's event carried no transcript, so the rework it overrules cannot be seen.",
                short(&sha)
            ));
            continue;
        };
        let reworked = reviews(transcript)
            .map(|rs| {
                rs.iter()
                    .rev()
                    .find(|r| r.repo == repo && r.sha == sha)
                    .is_some_and(|r| matches!(r.verdict, Some(Verdict::Rework) | None))
            })
            .unwrap_or(false);
        if !reworked {
            note(
                "unbound",
                &ask.session,
                &repo,
                &format!("tree={} overrule with no rework to overrule", short(&sha)),
            );
            said.push(format!(
                "The answer Overrule bound NOTHING for {repo} tree {}: no completed implementation review of that tree said rework in this session.",
                short(&sha)
            ));
            continue;
        }
        remember(&repo, &sha, "overruled", "person");
        note(
            "overruled",
            &ask.session,
            &repo,
            &format!("tree={} review=overruled verdict=rework", short(&sha)),
        );
    }
    (!said.is_empty()).then(|| said.join("\n\n"))
}

/// One journal line under the rule's id, for what happens away from a push.
fn note(outcome: &str, session: &str, repo: &str, excerpt: &str) {
    journal::record(&journal::Entry {
        rule: RULE_ID,
        stance: "-",
        outcome,
        session,
        repo,
        mode: "-",
        excerpt,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_round_trips_and_a_short_sha_does_not_bind() {
        let sha = "a".repeat(64);
        let b = block("amont-agent", &sha);
        assert_eq!(b, format!("<<<TREE repo=amont-agent sha={sha}>>>"));
        assert_eq!(
            parse_blocks(&format!("Round 1.\n{b}\nbrief")),
            vec![("amont-agent".to_string(), sha.clone())]
        );
        assert!(parse_blocks("<<<TREE repo=x sha=abc123>>>").is_empty());
        assert!(parse_blocks("<<<TREE sha=abc>>>").is_empty());
        assert!(parse_blocks(&format!("<<<TREE repo= sha={sha}>>>")).is_empty());
    }

    #[test]
    fn the_sha_is_lowercased() {
        let sha = "A".repeat(64);
        let got = parse_blocks(&block("r", &sha));
        assert_eq!(got[0].1, "a".repeat(64));
    }

    #[test]
    fn the_verdict_is_the_first_verdict_line() {
        assert_eq!(
            verdict_of("Verdict: approve\nFindings:"),
            Some(Verdict::Approve)
        );
        assert_eq!(
            verdict_of("**Verdict: approve-with-changes**\n"),
            Some(Verdict::ApproveWithChanges)
        );
        assert_eq!(
            verdict_of("preamble\nVerdict: rework"),
            Some(Verdict::Rework)
        );
        assert_eq!(verdict_of("no verdict here"), None);
        assert_eq!(verdict_of("Verdict: maybe"), None);
    }

    #[test]
    fn the_marker_splits_at_the_last_at_and_wants_64_hex() {
        let sha = "b".repeat(64);
        assert_eq!(
            marker(&format!("[implementation-review my@repo@{sha}] Overrule?")),
            Some(("my@repo".to_string(), sha.clone()))
        );
        assert_eq!(marker("[implementation-review repo@abc] x"), None);
        assert_eq!(marker(&format!("[implementation-review @{sha}]")), None);
        assert_eq!(marker("[preview a1] x"), None);
    }

    #[test]
    fn lexical_folds_dot_and_dotdot() {
        assert_eq!(lexical(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
    }

    #[test]
    fn a_read_of_the_store_passes_and_a_write_does_not() {
        use crate::shell::lex;
        let store = "~/.claude/amont-agent/implementation-review/by-tree/r/x.json";
        for read in [
            format!("ls {store}"),
            format!("cat {store} | head"),
            "find ~/.claude/amont-agent/implementation-review -type f".to_string(),
            format!("grep verdict {store}"),
        ] {
            assert!(store_clause(&lex(&read)).is_none(), "{read}");
        }
        for write in [
            format!("echo '{{}}' > {store}"),
            format!("printf x >> {store}"),
            format!("rm -f {store}"),
            format!("cp /tmp/x {store}"),
            format!("python3 -c 'open(\"{store}\",\"w\")'"),
            format!("tee {store} < /dev/null"),
            format!("cat x | sudo tee {store}"),
        ] {
            assert!(store_clause(&lex(&write)).is_some(), "{write}");
        }
    }

    #[test]
    fn a_word_names_the_store_by_its_directory() {
        assert!(names_store(
            "/Users/me/.claude/amont-agent/implementation-review/by-tree/r/x.json"
        ));
        assert!(names_store("~/.claude/amont-agent/implementation-review"));
        assert!(names_store(
            "C:\\Users\\me\\.claude\\amont-agent\\implementation-review\\by-tree\\r\\x.json"
        ));
        assert!(!names_store("~/.claude/amont-agent/journal.log"));
        assert!(!names_store("implementation-review"));
    }
}
