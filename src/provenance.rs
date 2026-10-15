//! The unsigned provenance record of a pushed commit (`amont-provenance-v1`,
//! attest SPEC; decisions ADR-0036). When the implementation-review hook
//! passes a push that carries a plan, it files what it knew about the push
//! under `<config dir>/amont-agent/provenance/<repo>/<pushed sha>`. Only the
//! hook writes there; a signer reads it later.

use std::path::{Path, PathBuf};

use crate::git;

const MAGIC: &str = "amont-provenance-v1";

/// How the implementation review bound to the pushed tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Review {
    Reviewed,
    ByPatch,
    Overruled,
}

impl Review {
    fn word(self) -> &'static str {
        match self {
            Review::Reviewed => "reviewed",
            Review::ByPatch => "by-patch",
            Review::Overruled => "overruled",
        }
    }
}

/// What the preview rule knew about the push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preview {
    Approved,
    Evidence,
    NotRequired,
}

impl Preview {
    fn word(self) -> &'static str {
        match self {
            Preview::Approved => "approved",
            Preview::Evidence => "evidence",
            Preview::NotRequired => "not-required",
        }
    }
}

/// Everything a record states.
pub struct Record<'a> {
    pub commit: &'a str,
    pub tree: &'a str,
    pub canon: &'a str,
    pub patch: Option<&'a str>,
    pub plan_path: &'a str,
    pub plan_sha: &'a str,
    pub review: Review,
    pub review_canon: &'a str,
    pub preview: Preview,
    pub version: &'a str,
}

/// The payload: one datum per line, trailing newline.
pub fn payload(r: &Record) -> String {
    let mut out = format!(
        "{MAGIC}\ncommit {}\ntree {}\ncanon {}\n",
        r.commit, r.tree, r.canon
    );
    if let Some(p) = r.patch {
        out.push_str(&format!("patch {p}\n"));
    }
    out.push_str(&format!(
        "plan {} {}\nreview {} {}\npreview {}\nproducer amont-agent {}\n",
        r.plan_path,
        r.plan_sha,
        r.review.word(),
        r.review_canon,
        r.preview.word(),
        r.version
    ));
    out
}

/// The record's file, or `None` when the repository name or the sha could
/// climb out of the directory.
pub fn record_file(repo: &str, commit: &str) -> Option<PathBuf> {
    if repo.is_empty()
        || repo.contains('/')
        || repo.contains('\\')
        || repo.contains("..")
        || commit.is_empty()
        || !commit.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(
        crate::journal::dir()?
            .join("provenance")
            .join(repo)
            .join(commit),
    )
}

/// The plan file changed on the branch, read from the pushed tree: its path
/// and body sha. `plans` are the non-empty, trimmed paths changed under
/// `docs/plans`.
pub fn plan_of(repo: &Path, commit: &str, plans: &[String]) -> Result<(String, String), String> {
    let path = plans
        .iter()
        .find(|p| {
            p.ends_with(".md")
                && git::succeeds_in(repo, &["cat-file", "-e", &format!("{commit}:{p}")])
        })
        .ok_or("no plan file under docs/plans/ exists in the pushed tree")?;
    let bytes = git::bytes_in(repo, &["show", &format!("{commit}:{path}")], None)
        .map_err(|e| format!("git could not be run: {e}"))?
        .ok_or("git could not read the plan")?;
    let shas = crate::plan_review::shas(&String::from_utf8_lossy(&bytes))
        .map_err(|e| format!("the plan has no body sha: {e}"))?;
    Ok((path.to_string(), shas.sha))
}

/// Everything about the push the hook knows, before the review is bound.
pub struct Push<'a> {
    pub repo: &'a Path,
    pub name: &'a str,
    pub pushed: &'a str,
    pub canon: &'a str,
    pub patch: Option<&'a str>,
    pub plans: &'a [String],
    /// The branch target the push publishes: the preview rule reads it.
    pub target: &'a crate::push_target::Target,
}

/// What `write` did with the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filed {
    /// The record was written to this file.
    Written(PathBuf),
    /// The record of this repository and commit already exists and was kept.
    Kept(PathBuf),
}

/// Write the record once per commit: a second push of the same commit finds
/// the file and leaves it, so the first pass's origin (by-patch, say) stays.
/// `Ok` says whether it wrote or kept the file, `Err` why it could not be.
pub fn write(
    p: &Push,
    review: Review,
    review_canon: &str,
    preview: Preview,
) -> Result<Filed, String> {
    let commit = git::stdout_in(
        p.repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{}^{{commit}}", p.pushed),
        ],
    )
    .ok_or("the pushed commit cannot be resolved")?;
    // The path needs the full sha, so resolving it is the one git call before
    // the check: a kept record never reads the tree or the plan again.
    let file = record_file(p.name, &commit).ok_or("no provenance directory is known")?;
    if file.exists() {
        return Ok(Filed::Kept(file));
    }
    let tree = git::stdout_in(
        p.repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{commit}^{{tree}}"),
        ],
    )
    .ok_or("the pushed tree cannot be resolved")?;
    let (plan_path, plan_sha) = plan_of(p.repo, &commit, p.plans)?;
    let body = payload(&Record {
        commit: &commit,
        tree: &tree,
        canon: p.canon,
        patch: p.patch,
        plan_path: &plan_path,
        plan_sha: &plan_sha,
        review,
        review_canon,
        preview,
        version: env!("CARGO_PKG_VERSION"),
    });
    let dir = file.parent().ok_or("no provenance directory is known")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    crate::journal::private(dir, 0o700);
    crate::atomic::write_atomic(&file, &body).map_err(|e| format!("{}: {e}", file.display()))?;
    crate::journal::private(&file, 0o600);
    Ok(Filed::Written(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(patch: Option<&'static str>) -> Record<'static> {
        Record {
            commit: "c0",
            tree: "t0",
            canon: "k0",
            patch,
            plan_path: "docs/plans/p.md",
            plan_sha: "s0",
            review: Review::ByPatch,
            review_canon: "k1",
            preview: Preview::NotRequired,
            version: "1.2.3",
        }
    }

    #[test]
    fn payload_lines_are_in_order() {
        assert_eq!(
            payload(&rec(Some("p0"))),
            "amont-provenance-v1\ncommit c0\ntree t0\ncanon k0\npatch p0\nplan docs/plans/p.md s0\nreview by-patch k1\npreview not-required\nproducer amont-agent 1.2.3\n"
        );
    }

    #[test]
    fn payload_omits_patch_without_one() {
        assert!(!payload(&rec(None)).lines().any(|l| l.starts_with("patch ")));
    }

    #[test]
    fn record_file_refuses_escapes() {
        assert!(record_file("../x", "ab").is_none());
        assert!(record_file("r", "../ab").is_none());
        assert!(record_file("r/s", "ab").is_none());
    }
}
