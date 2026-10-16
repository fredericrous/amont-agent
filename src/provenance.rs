//! The unsigned provenance record of a pushed commit (`amont-provenance-v1`,
//! attest SPEC; decisions ADR-0036). When the implementation-review hook
//! passes a push that carries a plan, it files what it knew about the push
//! under `<config dir>/amont-agent/provenance/<repo>/<pushed sha>`. Only the
//! hook writes there; a signer reads it later.
//!
//! It records only what the hook witnessed: the plan, the review and the
//! preview. It writes no `canon` line: attest's canonical tree is a tree fact
//! the signer computes and inserts itself, and a payload carrying one is
//! refused there.

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
    pub patch: Option<&'a str>,
    pub plan_path: &'a str,
    pub plan_sha: &'a str,
    pub review: Review,
    /// The content id the review read (attest SPEC, "Content id").
    pub review_content: &'a str,
    pub preview: Preview,
    pub version: &'a str,
}

/// The payload: one datum per line, trailing newline.
pub fn payload(r: &Record) -> String {
    let mut out = format!("{MAGIC}\ncommit {}\ntree {}\n", r.commit, r.tree);
    if let Some(p) = r.patch {
        out.push_str(&format!("patch {p}\n"));
    }
    out.push_str(&format!(
        "plan {} {}\nreview {} {}\npreview {}\nproducer amont-agent {}\n",
        r.plan_path,
        r.plan_sha,
        r.review.word(),
        r.review_content,
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

/// Whether a plan file is a pointer: a non-empty `canonical:` in its front
/// matter, as the preview gate reads it.
fn is_pointer(text: &str) -> bool {
    let (front, _) = crate::plans::split_front_matter(text);
    crate::plans::field(front, "canonical").is_some_and(|c| !c.is_empty())
}

/// The plan file changed on the branch, read from the pushed tree: its path
/// and body sha. `plans` are the non-empty, trimmed paths changed under
/// `docs/plans`. A `.reviews.md` sidecar is never the plan; of several plan
/// files the one that is not a pointer wins.
pub fn plan_of(repo: &Path, commit: &str, plans: &[String]) -> Result<(String, String), String> {
    let mut found: Vec<(&String, String)> = Vec::new();
    for p in plans
        .iter()
        .filter(|p| p.ends_with(".md") && !p.ends_with(".reviews.md"))
    {
        if let Some(bytes) = git::bytes_in(repo, &["show", &format!("{commit}:{p}")], None)
            .map_err(|e| format!("git could not be run: {e}"))?
        {
            found.push((p, String::from_utf8_lossy(&bytes).into_owned()));
        }
    }
    let (path, text) = found
        .iter()
        .find(|(_, t)| !is_pointer(t))
        .or_else(|| found.first())
        .ok_or("no plan file under docs/plans/ exists in the pushed tree")?;
    let shas =
        crate::plan_review::shas(text).map_err(|e| format!("the plan has no body sha: {e}"))?;
    Ok((path.to_string(), shas.sha))
}

/// Everything about the push the hook knows, before the review is bound.
pub struct Push<'a> {
    pub repo: &'a Path,
    pub name: &'a str,
    pub pushed: &'a str,
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
    review_content: &str,
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
        patch: p.patch,
        plan_path: &plan_path,
        plan_sha: &plan_sha,
        review,
        review_content,
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
            patch,
            plan_path: "docs/plans/p.md",
            plan_sha: "s0",
            review: Review::ByPatch,
            review_content: "k1",
            preview: Preview::NotRequired,
            version: "1.2.3",
        }
    }

    #[test]
    fn payload_lines_are_in_order() {
        assert_eq!(
            payload(&rec(Some("p0"))),
            "amont-provenance-v1\ncommit c0\ntree t0\npatch p0\nplan docs/plans/p.md s0\nreview by-patch k1\npreview not-required\nproducer amont-agent 1.2.3\n"
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

    fn git_in(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn a_reviews_sidecar_is_never_the_plan() {
        let dir = std::env::temp_dir().join(format!("amont-plan-of-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs/plans")).unwrap();
        git_in(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(
            dir.join("docs/plans/a.md"),
            "---\nstatus: active\n---\n# A\n\nbody\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("docs/plans/a.reviews.md"),
            "---\nstatus: active\n---\n# R\n\nreviews\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("docs/plans/b.md"),
            "---\ncanonical: x/b.md\n---\n# B\n",
        )
        .unwrap();
        git_in(&dir, &["add", "-A"]);
        git_in(&dir, &["commit", "-q", "-m", "x"]);
        let plans: Vec<String> = [
            "docs/plans/a.reviews.md",
            "docs/plans/b.md",
            "docs/plans/a.md",
        ]
        .map(String::from)
        .to_vec();
        let (path, _) = plan_of(&dir, "HEAD", &plans).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(path, "docs/plans/a.md");
    }
}
