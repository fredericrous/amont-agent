//! The implementation review's tree identity (ADR-0022,
//! `work.implementation-review`): what a review of a diff binds to.
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

use std::path::{Path, PathBuf};

use crate::git;
use crate::plan_review::{hex, sha256};

/// Opens a review block in the reviewer's prompt.
pub const BLOCK_OPEN: &str = "<<<TREE ";
const BLOCK_CLOSE: &str = ">>>";
/// The directory a review does not bind to: the plan's own record.
const PLANS_DIR: &[u8] = b"docs/plans/";

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
#[allow(dead_code)] // its reader is the rule, the next change
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

#[allow(dead_code)] // its reader is the rule, the next change
pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
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
}
