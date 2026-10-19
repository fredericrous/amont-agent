//! The content id (attest SPEC, "Content id"; decisions ADR-0034): what an
//! implementation review and a preview approval bind to. Shared by both, so
//! neither reaches into the other for it.

use std::path::Path;

use amont_agent_guard::git;
use amont_agent_guard::sha256::{hex, sha256};

/// Paths a content id leaves out: the plan's own record.
const PLANS_DIR: &[u8] = b"docs/plans/";

/// The content id of `rev` in `repo` (attest SPEC, "Content id"): what an
/// implementation review and a preview approval bind to (ADR-0034). Not
/// attest's canonical tree: it ignores whether a plan exists. 64 lowercase
/// hex.
pub fn content_id(repo: &Path, rev: &str) -> Result<String, String> {
    let spec = format!("{rev}^{{tree}}");
    let entries = git::ls_tree_entries(repo, &spec)
        .map_err(|e| format!("git could not be run in {}: {e}", repo.display()))?
        .ok_or_else(|| format!("`{rev}` does not name a tree in {}", repo.display()))?;
    Ok(content_id_of(&entries))
}

/// The content id of raw `git ls-tree -r -z --full-tree` records, in git's
/// order: the sha256 of every record whose path does not start with
/// `docs/plans/`, each followed by one NUL.
pub fn content_id_of(entries: &[Vec<u8>]) -> String {
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
        bytes.extend_from_slice(entry);
        bytes.push(0);
    }
    hex(&sha256(&bytes))
}
