//! A plan's text as reviews bind to it: the canonical body the panel's sha
//! is taken over, its fences, and what a repository's `package.json` says
//! about having an interface. Shared by the plan-review panel, preview
//! approvals and `plan-apply`, so none of them reaches into another for it.

use amont_agent_guard::sha256::{hex, sha256};

pub(crate) const REVIEW_SECTION: &str = "Review panel";
const FULL_REVIEWS: &str = "Full reviews";

#[cfg(test)]
thread_local! {
    /// Set by a test to make [`body_sha`] panic, as a parser bug would.
    pub(crate) static PARSER_PANICS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The plan as reviewed: CRLF → LF; YAML front matter (a `---` first line
/// through the next `---`) and the blank lines after it removed; the
/// `## Review panel` section, the `## Full reviews …` section to the end,
/// and a machine comment on the last non-empty line removed; trailing space
/// and tab trimmed per line; one final newline. Headings inside code fences
/// are text. Idempotent.
///
/// Front matter is skipped because a plan gains it when it lands in a
/// repository (`docs/plans/`, ADR-0022). The landed copy then hashes like
/// the approved one the reviewers read, so landing can be checked on the
/// file it writes.
pub fn canonical(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let comment = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .filter(|&i| is_machine_comment(lines[i]));
    let mut start = front_matter_end(&lines);
    while start < lines.len() && lines[start].trim().is_empty() {
        start += 1;
    }
    let mut out: Vec<&str> = Vec::new();
    let mut fence = Fence::default();
    let mut dropping = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        if Some(i) == comment {
            continue;
        }
        if let Some(title) = fence.h2(line) {
            if title.starts_with(FULL_REVIEWS) {
                break;
            }
            dropping = title == REVIEW_SECTION;
        }
        if !dropping {
            out.push(line.trim_end_matches([' ', '\t']));
        }
    }
    let mut body = out.join("\n");
    body.truncate(body.trim_end_matches(['\n', ' ', '\t']).len());
    body.push('\n');
    body
}

/// Whether the byte at `offset` of `text` lies in a region [`canonical`]
/// strips: front matter, `## Review panel`, `## Full reviews` and after, or
/// the machine comment. An edit there does not change the plan's sha.
pub(crate) fn outside_body(text: &str, offset: usize) -> bool {
    let lines: Vec<&str> = text.split('\n').collect();
    let comment = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .filter(|&i| is_machine_comment(lines[i]));
    let front = front_matter_end(&lines);
    let mut fence = Fence::default();
    let (mut dropping, mut reviews) = (false, false);
    let mut start = 0;
    for (i, line) in lines.iter().enumerate() {
        let end = start + line.len() + 1;
        if i >= front {
            if let Some(title) = fence.h2(line) {
                reviews |= title.starts_with(FULL_REVIEWS);
                dropping = title == REVIEW_SECTION;
            }
        }
        if offset < end {
            return i < front || Some(i) == comment || reviews || dropping;
        }
        start = end;
    }
    false
}

/// The index of the first line after YAML front matter, or 0 when there is
/// none: a `---` first line with no closing `---` is text, not front matter.
fn front_matter_end(lines: &[&str]) -> usize {
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return 0;
    }
    lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| l.trim_end() == "---")
        .map_or(0, |(i, _)| i + 1)
}

/// sha256 of what the canonical body says, read as CommonMark
/// ([`crate::plan_words::words`]), 64 lowercase hex: a formatter that only
/// moves blank lines, list markers or table padding keeps it.
pub fn body_sha(text: &str) -> String {
    #[cfg(test)]
    if PARSER_PANICS.with(|p| p.get()) {
        panic!("injected parser panic");
    }
    hex(&sha256(
        crate::plan_words::words(&canonical(text)).as_bytes(),
    ))
}

// LEGACY: the byte sha every review was bound to before 2.25. Accepted
// alongside `body_sha` until the journal shows no `match=legacy` for 14 days.
/// sha256 of the canonical body's bytes, 64 lowercase hex.
pub fn legacy_sha(text: &str) -> String {
    hex(&sha256(canonical(text).as_bytes()))
}

/// Both shas of a plan.
pub struct Shas {
    pub sha: String,
    pub legacy: String,
}

/// [`body_sha`] and [`legacy_sha`], with a panic in the Markdown parser
/// caught here: the hook's own guard turns a panic into silence, which for
/// this rule would let a plan through unreviewed.
pub fn shas(text: &str) -> Result<Shas, ShaError> {
    guarded(|| body_sha(text)).map(|sha| Shas {
        sha,
        legacy: legacy_sha(text),
    })
}

pub(crate) fn guarded(f: impl FnOnce() -> String) -> Result<String, ShaError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|_| ShaError::ParserPanicked)
}

/// Why a plan's sha could not be computed.
#[derive(Debug, PartialEq)]
pub enum ShaError {
    ParserPanicked,
}

impl std::fmt::Display for ShaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShaError::ParserPanicked => {
                f.write_str("the plan's sha cannot be computed (the Markdown parser panicked)")
            }
        }
    }
}

pub(crate) fn is_machine_comment(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("<!-- panel:") && t.ends_with("-->")
}

/// Tracks fenced code so that a `## Review panel` inside an example is text.
#[derive(Default)]
pub(crate) struct Fence {
    pub(crate) open: Option<(char, usize)>,
}

impl Fence {
    /// The line's text when it is prose outside any fence, indented at most
    /// 3; updates the fence state. A fence line, a line inside code, and an
    /// indented line are `None`.
    pub(crate) fn prose<'a>(&mut self, line: &'a str) -> Option<&'a str> {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let t = line.trim_start_matches(' ');
        if indent <= 3 {
            for c in ['`', '~'] {
                let run = t.len() - t.trim_start_matches(c).len();
                if run >= 3 {
                    match self.open {
                        None => self.open = Some((c, run)),
                        Some((o, n)) if o == c && run >= n && t[run..].trim().is_empty() => {
                            self.open = None
                        }
                        _ => {}
                    }
                    return None;
                }
            }
        }
        if self.open.is_some() || indent > 3 {
            return None;
        }
        Some(t)
    }

    /// The title of an ATX H2 outside any fence; updates the fence state.
    pub(crate) fn h2<'a>(&mut self, line: &'a str) -> Option<&'a str> {
        let title = self.prose(line)?.strip_prefix("## ")?;
        Some(title.trim().trim_end_matches('#').trim_end())
    }

    /// The title of an ATX heading of any level outside any fence; updates
    /// the fence state.
    pub(crate) fn heading<'a>(&mut self, line: &'a str) -> Option<&'a str> {
        let t = self.prose(line)?;
        let hashes = t.len() - t.trim_start_matches('#').len();
        if hashes == 0 || hashes > 6 {
            return None;
        }
        let rest = t[hashes..].strip_prefix(' ')?;
        Some(rest.trim().trim_end_matches('#').trim_end())
    }
}

/// Packages whose presence says "this renders something".
const UI_PACKAGES: &[&str] = &[
    "react",
    "react-dom",
    "react-native",
    "react-strict-dom",
    "@duro-app/ui",
];

/// A `package.json` that looks like an interface: a `dev` script, or a UI
/// package among its `dependencies` or its non-optional `peerDependencies`.
///
/// `devDependencies` and optional peers do not count: a CLI that reads the
/// design system's metadata (duro-design-system's `packages/cli`) carries
/// `@duro-app/ui` exactly there, and renders nothing. Unparseable counts.
pub fn package_is_ui(text: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return true;
    };
    let dev = v
        .get("scripts")
        .and_then(|s| s.get("dev"))
        .and_then(|d| d.as_str())
        .is_some_and(|s| !s.trim().is_empty());
    if dev {
        return true;
    }
    let has = |section: &str, name: &str| {
        v.get(section)
            .and_then(|d| d.as_object())
            .is_some_and(|d| d.contains_key(name))
    };
    let optional = |name: &str| {
        v.get("peerDependenciesMeta")
            .and_then(|m| m.get(name))
            .and_then(|m| m.get("optional"))
            .and_then(|o| o.as_bool())
            .unwrap_or(false)
    };
    UI_PACKAGES
        .iter()
        .any(|name| has("dependencies", name) || (has("peerDependencies", name) && !optional(name)))
}
