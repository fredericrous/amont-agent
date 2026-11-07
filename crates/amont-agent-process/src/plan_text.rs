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

pub(crate) fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, ShaError> {
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

/// The sections every slice shares (ADR-0061 `work.review-slices`). Every
/// other section is outside every slice, and backend, which binds the whole
/// body, reviews it.
pub const SHARED_SECTIONS: &[&str] = &["Goal", "Non-goals", "Behaviour", "Code review"];

/// The slice rule's version, hashed into every slice sha: a change to the
/// rule bumps it, so an old slice sha turns stale, never falsely current.
pub const SLICE_RULE: &str = "slice/1";

/// The label of the shared part of a slice in [`Slice::spans`].
pub const SHARED: &str = "shared";

/// A role's slice of a plan: the text it is bound to, the phases it keeps,
/// its sha, and one sha per span (`shared` and each kept phase), so a
/// refusal can say which span moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slice {
    pub text: String,
    pub kept: Vec<String>,
    pub sha: String,
    pub spans: std::collections::BTreeMap<String, String>,
}

/// A link reference definition's label, when the prose line is one
/// (`[x]: dest`). Footnotes (`[^x]: …`) are definitions too: the parser
/// does not enable footnotes, so it reads them as link definitions.
fn definition_label(prose: &str) -> Option<String> {
    let rest = prose.strip_prefix('[')?;
    let end = rest.find("]:")?;
    let label = &rest[..end];
    if label.trim().is_empty() || label.contains('[') || label.contains(']') {
        return None;
    }
    Some(normalised(label))
}

/// A reference label as CommonMark matches it: case folded, whitespace runs
/// collapsed to one space, trimmed.
fn normalised(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The slice of a plan's canonical body that `keep` selects: the lines
/// before the first `##`, the [`SHARED_SECTIONS`], the `## Phases` text
/// before its first item, every `🧑` decision, and each phase whose paths
/// `keep` accepts. A reference definition that kept text uses, and that
/// sits outside the slice, is carried with it (the first one of its label,
/// as CommonMark resolves it), so a link keeps its destination.
pub fn slice(text: &str, keep: impl Fn(&[String]) -> bool) -> Result<Slice, ShaError> {
    guarded(|| slice_of(text, &keep))
}

fn slice_of(text: &str, keep: &impl Fn(&[String]) -> bool) -> Slice {
    let body = canonical(text);
    let lines: Vec<&str> = body.lines().collect();
    let phase_spans = crate::file_types::phase_spans(&body);
    // Which phase each Phases line belongs to, and whether it is kept.
    let mut owner: Vec<Option<usize>> = vec![None; lines.len()];
    for (k, s) in phase_spans.iter().enumerate() {
        for i in s.lines.clone() {
            if i < owner.len() {
                owner[i] = Some(k);
            }
        }
    }
    let kept_phase = |k: usize| {
        let s = &phase_spans[k];
        s.label.is_empty() || keep(&s.paths)
    };
    let mut fence = Fence::default();
    let mut section: Option<String> = None;
    let mut shared: Vec<&str> = Vec::new();
    let mut by_phase: Vec<Vec<&str>> = vec![Vec::new(); phase_spans.len()];
    let mut in_slice = vec![false; lines.len()];
    // label → (first definition's lines, whether it lies in the slice)
    let mut definitions: Vec<(String, Vec<&str>, bool)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let was_open = fence.open.is_some();
        let prose = fence.prose(line);
        if let Some(t) = prose {
            if let Some(title) = t.strip_prefix("## ") {
                section = Some(title.trim().trim_end_matches('#').trim_end().to_string());
            }
        }
        let keep_line = match section.as_deref() {
            None => true,
            Some(s) if SHARED_SECTIONS.contains(&s) => true,
            Some("Phases") => match owner[i] {
                None => true,
                Some(k) => kept_phase(k),
            },
            Some(_) => false,
        };
        let definition = if was_open {
            None
        } else {
            prose.and_then(definition_label)
        };
        // A definition and its indented continuation lines.
        let mut end = i + 1;
        if definition.is_some() {
            while end < lines.len()
                && !lines[end].trim().is_empty()
                && lines[end].starts_with([' ', '\t'])
            {
                end += 1;
            }
        }
        for j in i..end {
            in_slice[j] = keep_line;
            if keep_line {
                match (section.as_deref(), owner[j]) {
                    (Some("Phases"), Some(k)) if !phase_spans[k].label.is_empty() => {
                        by_phase[k].push(lines[j])
                    }
                    _ => shared.push(lines[j]),
                }
            }
            if j > i {
                // Continuation lines update the fence like any line.
                fence.prose(lines[j]);
            }
        }
        if let Some(label) = definition {
            if !definitions.iter().any(|(l, _, _)| *l == label) {
                definitions.push((label, lines[i..end].to_vec(), keep_line));
            }
        }
        i = end;
    }
    let kept_text = {
        let mut all: Vec<&str> = Vec::new();
        for (j, l) in lines.iter().enumerate() {
            if in_slice[j] {
                all.push(l);
            }
        }
        normalised(&all.join("\n"))
    };
    // Definitions used by the slice but outside it go first: the first
    // definition of a label wins, and a definition makes no text.
    let mut carried: Vec<&str> = Vec::new();
    // holds-until: a `[label]` inside a code span counts as a use, which can
    // only carry one definition too many (a slice that moves on an edit it
    // did not need to), never one too few.
    for (label, def, inside) in &definitions {
        if !*inside && kept_text.contains(&format!("[{label}]")) {
            carried.extend(def.iter().copied());
        }
    }
    let mut text_lines: Vec<&str> = Vec::new();
    if !carried.is_empty() {
        text_lines.extend(carried.iter().copied());
        text_lines.push("");
    }
    for (j, l) in lines.iter().enumerate() {
        if in_slice[j] {
            text_lines.push(l);
        }
    }
    let mut slice_text = text_lines.join("\n");
    slice_text.push('\n');
    let mut spans = std::collections::BTreeMap::new();
    let mut shared_text: Vec<&str> = carried.clone();
    shared_text.extend(shared.iter().copied());
    spans.insert(SHARED.to_string(), versioned_sha(&shared_text.join("\n")));
    let mut kept = Vec::new();
    for (k, s) in phase_spans.iter().enumerate() {
        if !s.label.is_empty() && kept_phase(k) {
            // Two items with one label (`Phase 2` twice) stay two spans.
            let mut label = s.label.clone();
            if spans.contains_key(&label) {
                label = format!("{label} (item {})", k + 1);
            }
            kept.push(label.clone());
            spans.insert(label, versioned_sha(&by_phase[k].join("\n")));
        }
    }
    Slice {
        sha: versioned_sha(&slice_text),
        text: slice_text,
        kept,
        spans,
    }
}

/// sha256 of [`SLICE_RULE`] and what the text says, read as CommonMark.
fn versioned_sha(text: &str) -> String {
    hex(&sha256(
        format!("{SLICE_RULE}\n{}", crate::plan_words::words(text)).as_bytes(),
    ))
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

#[cfg(test)]
mod slice_tests {
    use super::*;

    const PLAN: &str = "# T\n\n## Goal\nG.\n\n## Context\nC [spec].\n\n[spec]: https://a.example/one\n[other]: https://a.example/unused\n\n## Behaviour\nB.\n\n## Phases\nPreamble.\n- [ ] Phase 1 — `src/a.rs`: rust work [spec].\n  more rust.\n- 🧑 decide: x.\n- [ ] Phase 2 — `web/b.ts`: ts work.\n\n## Decision log\n- one.\n\n## Verification\n- Phase 2 row.\n";

    fn rust(paths: &[String]) -> bool {
        paths.iter().any(|p| p.ends_with(".rs"))
    }

    fn sha(text: &str) -> String {
        slice(text, rust).unwrap().sha
    }

    #[test]
    fn a_slice_keeps_shared_sections_its_phases_and_decisions() {
        let s = slice(PLAN, rust).unwrap();
        assert_eq!(s.kept, vec!["Phase 1".to_string()]);
        assert!(s.text.contains("rust work") && s.text.contains("more rust"));
        assert!(s.text.contains("🧑 decide") && s.text.contains("Preamble."));
        assert!(s.text.contains("G.") && s.text.contains("B."));
        assert!(!s.text.contains("ts work") && !s.text.contains("Decision log"));
        assert!(!s.text.contains("C [spec]"));
        assert_eq!(
            s.spans.keys().cloned().collect::<Vec<_>>(),
            vec!["Phase 1".to_string(), SHARED.to_string()]
        );
    }

    #[test]
    fn an_edit_outside_the_slice_keeps_its_sha() {
        let base = sha(PLAN);
        for (a, b) in [
            ("ts work", "ts work, changed"),
            ("- one.", "- one.\n- two."),
            ("Phase 2 row.", "Phase 2 row, changed."),
            ("C [spec].", "C [spec], changed."),
        ] {
            assert_eq!(sha(&PLAN.replace(a, b)), base, "{a}");
        }
    }

    #[test]
    fn an_edit_inside_the_slice_moves_its_sha_and_names_the_span() {
        let s = slice(PLAN, rust).unwrap();
        for (a, b, span) in [
            ("more rust", "more rust, changed", "Phase 1"),
            ("G.", "G, changed.", SHARED),
            ("B.", "B, changed.", SHARED),
        ] {
            let t = slice(&PLAN.replace(a, b), rust).unwrap();
            assert_ne!(t.sha, s.sha, "{a}");
            assert_ne!(t.spans[span], s.spans[span], "{a}");
        }
    }

    #[test]
    fn a_reformat_that_keeps_the_body_sha_keeps_the_slice_sha() {
        let loose = PLAN.replace(
            "rust work [spec].\n  more rust.",
            "rust work [spec]. more\n  rust.",
        );
        assert_eq!(body_sha(&loose), body_sha(PLAN));
        assert_eq!(sha(&loose), sha(PLAN));
    }

    #[test]
    fn a_used_definition_outside_the_slice_travels_with_it() {
        let base = sha(PLAN);
        assert_ne!(sha(&PLAN.replace("a.example/one", "a.example/two")), base);
        assert_eq!(sha(&PLAN.replace("a.example/unused", "a.example/x")), base);
        let spaced = PLAN
            .replace("rust work [spec].", "rust work [Spec  Thing].")
            .replace(
                "[spec]: https://a.example/one",
                "[spec   thing]: https://a.example/one",
            );
        assert_ne!(
            sha(&spaced.replace("a.example/one", "a.example/two")),
            sha(&spaced)
        );
        let foot = "# T\n\n## Context\n[^n]: the note\n\n## Phases\n- Phase 1 — `a.rs` [^n].\n";
        let s = slice(foot, rust).unwrap();
        assert!(s.text.contains("[^n]: the note"));
        assert_ne!(sha(&foot.replace("the note", "a note")), s.sha);
    }

    #[test]
    fn the_rule_version_keeps_a_whole_slice_apart_from_the_body() {
        // Only shared sections and Phases: the slice holding every phase is
        // the whole body, so only the rule's version tells the shas apart.
        let whole = "# T\n\n## Goal\nG.\n\n## Phases\n- Phase 1 — `a.rs`\n";
        let all = slice(whole, |_: &[String]| true).unwrap();
        assert_eq!(all.text, canonical(whole));
        assert_ne!(all.sha, body_sha(whole));
    }
}
