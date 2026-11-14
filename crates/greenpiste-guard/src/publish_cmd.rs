//! Which shell commands publish: a version tag pushed, a pull request merged.
//!
//! Three rules read these shapes. `release-tag-push` and `forge-merge-by-hand`
//! judge the command itself, `gh-pr-merge-auto` judges one flag of it, and
//! `publish-without-skill` judges what came before it. The matchers live here
//! so that none of them imports another rule's private functions, and so that
//! the four cannot drift apart on what "a tag push" is.
//!
//! PURE, like every `examine`: words in, verdict out.

use std::ops::Range;

use crate::shell::{Parsed, Simple};

/// What a matched command publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A `v*` tag reaches a remote, which starts the release workflow.
    TagPush,
    /// A pull request merges, through `gh pr merge` or the forge's API.
    PrMerge,
}

impl Kind {
    /// The skill that carries this procedure.
    pub fn skill(self) -> &'static str {
        match self {
            Kind::TagPush => "tag-release",
            Kind::PrMerge => "merge-when-green",
        }
    }
}

/// One publishing clause.
#[derive(Debug, Clone)]
pub struct Publish {
    pub kind: Kind,
    /// The words that publish, for a reason or a journal line: the tag, or
    /// the merging program.
    pub what: String,
    /// Byte range of the match within the original command.
    pub span: Range<usize>,
}

/// `v` followed by a digit: `v1`, `v0.9.1`, `v2.0.0-rc1`, also as
/// `refs/tags/v…` and with a leading `+` (a forced refspec). Deliberately not
/// every tag — a docs or checkpoint tag publishes nothing.
pub fn is_version_tag(text: &str) -> bool {
    let text = text.strip_prefix('+').unwrap_or(text);
    let rest = text.strip_prefix("refs/tags/").unwrap_or(text);
    matches!(rest.strip_prefix('v'), Some(r) if r.starts_with(|c: char| c.is_ascii_digit()))
}

/// A `git push` that publishes a version tag: a named `v*` tag, `--tags`
/// (every tag there is), or `--mirror` (every ref there is). Not a dry run,
/// and not `--delete`: deleting a tag publishes nothing.
///
/// `--follow-tags` is left out on purpose: whether it carries a `v*` tag
/// depends on what is reachable and not yet on the remote, which the command
/// does not say, and matching it would catch ordinary branch pushes.
pub fn tag_push(cmd: &Simple) -> Option<(String, Range<usize>)> {
    if cmd.program() != Some("git") || cmd.subcommand() != Some("push") || cmd.is_dry_run() {
        return None;
    }
    if cmd.has_flag("--delete") || cmd.has_flag("-d") {
        return None;
    }
    if let Some(w) = cmd.args().iter().find(|w| is_version_tag(&w.text)) {
        return Some((w.text.clone(), w.at..cmd.end));
    }
    for flag in ["--tags", "--mirror"] {
        if cmd.has_flag(flag) {
            return Some((flag.to_string(), cmd.at..cmd.end));
        }
    }
    None
}

/// `gh pr merge …`, any flags.
pub fn gh_pr_merge(cmd: &Simple) -> bool {
    // operands()[0] is `pr`; the action follows.
    cmd.program() == Some("gh")
        && cmd.subcommand() == Some("pr")
        && cmd.operands().get(1).map(|w| w.text.as_str()) == Some("merge")
}

/// True for a URL path that merges a pull request on either forge:
/// `…/pulls/<digits>/merge`, with anything or nothing after it.
///
/// The digits matter. `/pulls/10` reads a pull request and `/pulls` lists
/// them; only the numbered `merge` child mutates, so requiring an index is
/// what keeps every read-only call out.
pub fn is_pr_merge_path(text: &str) -> bool {
    let mut rest = text;
    while let Some(i) = rest.find("/pulls/") {
        let after = &rest[i + "/pulls/".len()..];
        let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 {
            let tail = &after[digits..];
            if tail == "/merge" || tail.starts_with("/merge?") || tail.starts_with("/merge/") {
                return true;
            }
        }
        rest = after;
    }
    false
}

/// The HTTP clients a shell reaches for. `gh api` is matched separately in
/// [`api_merge`]: it is the same raw endpoint with authentication attached.
pub fn is_http_client(program: &str) -> bool {
    matches!(
        program,
        "curl" | "wget" | "http" | "https" | "httpie" | "xh"
    )
}

/// An HTTP client, or `gh api`, sent to a pull request's merge endpoint. The
/// program and the span from the URL to the end of the clause.
pub fn api_merge(cmd: &Simple) -> Option<(String, Range<usize>)> {
    let program = cmd.program()?;
    let gh_api = program == "gh" && cmd.subcommand() == Some("api");
    if !is_http_client(program) && !gh_api {
        return None;
    }
    // Every argument, quoted included: a URL in quotes is still a URL.
    let hit = cmd.args().iter().find(|w| is_pr_merge_path(&w.text))?;
    Some((program.to_string(), hit.at..cmd.end))
}

/// The first publishing clause of a command.
pub fn classify(parsed: &Parsed) -> Option<Publish> {
    for cmd in parsed.judgeable() {
        if let Some((what, span)) = tag_push(cmd) {
            return Some(Publish {
                kind: Kind::TagPush,
                what,
                span,
            });
        }
        if gh_pr_merge(cmd) {
            return Some(Publish {
                kind: Kind::PrMerge,
                what: "gh pr merge".to_string(),
                span: cmd.at..cmd.end,
            });
        }
        if let Some((what, span)) = api_merge(cmd) {
            return Some(Publish {
                kind: Kind::PrMerge,
                what,
                span,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(src: &str) -> Option<Kind> {
        classify(&crate::shell::lex(src)).map(|p| p.kind)
    }

    #[test]
    fn version_tags_only() {
        assert!(is_version_tag("v0.9.1"));
        assert!(is_version_tag("v2"));
        assert!(is_version_tag("refs/tags/v1.0.0-rc2"));
        assert!(is_version_tag("+refs/tags/v1.0.0"));
        assert!(is_version_tag("+v3.1.0"));
        assert!(!is_version_tag("verify"));
        assert!(!is_version_tag("main"));
        assert!(!is_version_tag("docs-snapshot"));
        assert!(!is_version_tag("feat/v2-rewrite"));
    }

    #[test]
    fn tag_pushes() {
        assert_eq!(kind("git push origin v1.2.3"), Some(Kind::TagPush));
        assert_eq!(
            kind("git push origin refs/tags/v1.2.3"),
            Some(Kind::TagPush)
        );
        assert_eq!(kind("git push --tags"), Some(Kind::TagPush));
        assert_eq!(kind("git push --mirror backup"), Some(Kind::TagPush));
        assert_eq!(
            kind("git push origin +refs/tags/v1.2.3"),
            Some(Kind::TagPush)
        );
        assert_eq!(kind("git push --dry-run origin v1.2.3"), None);
        assert_eq!(kind("git push --delete origin v1.2.3"), None);
        assert_eq!(kind("git push --follow-tags origin main"), None);
        assert_eq!(kind("git push -u origin feat/v2"), None);
    }

    #[test]
    fn merges() {
        assert_eq!(kind("gh pr merge 12 --squash"), Some(Kind::PrMerge));
        assert_eq!(kind("gh pr merge --auto 12"), Some(Kind::PrMerge));
        assert_eq!(kind("gh pr view 12"), None);
        assert_eq!(kind("gh pr create --body 'gh pr merge later'"), None);
        assert_eq!(
            kind("curl -X POST https://git.example/api/v1/repos/o/r/pulls/7/merge -d '{}'"),
            Some(Kind::PrMerge)
        );
        assert_eq!(
            kind("gh api -X PUT repos/o/r/pulls/7/merge"),
            Some(Kind::PrMerge)
        );
        assert_eq!(
            kind("curl https://git.example/api/v1/repos/o/r/pulls/7"),
            None
        );
    }

    #[test]
    fn the_first_publish_wins() {
        let p = classify(&crate::shell::lex("cd /r && git push origin v1.0.0")).unwrap();
        assert_eq!(p.what, "v1.0.0");
        assert_eq!(p.kind.skill(), "tag-release");
    }

    #[test]
    fn merge_paths() {
        assert!(is_pr_merge_path(
            "https://git.daddyshome.fr/api/v1/repos/fredericrous/sre-agent/pulls/10/merge"
        ));
        assert!(is_pr_merge_path(
            "https://api.github.com/repos/o/r/pulls/1234/merge?foo=1"
        ));
        assert!(!is_pr_merge_path(
            "https://git.daddyshome.fr/api/v1/repos/o/r/pulls/10"
        ));
        assert!(!is_pr_merge_path(
            "https://git.daddyshome.fr/api/v1/repos/o/r/pulls/merge"
        ));
        assert!(!is_pr_merge_path("https://example.com/merge"));
    }
}
