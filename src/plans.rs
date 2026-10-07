//! The in-progress plans of the repository a session opens in (ADR-0022).
//!
//! A new or compacted session is told "continue phase 3" and, without this,
//! has no file that says what phase 3 is. One line per `active` plan — its
//! title and its first unchecked phase — and one per pointer file, naming the
//! canonical plan and the phases this repository carries. Nothing else from
//! `docs/plans/` is loaded: an old plan is history, and history loaded into
//! every session competes with the instructions that are current
//! (`work.plans-are-not-context`).
//!
//! `aval check` does not read `docs/plans/`, so this is also the one place a
//! malformed `status` is noticed.

use std::path::Path;

const STATUSES: &[&str] = &["active", "done", "abandoned", "imported"];
/// A session notice is not a report. Past this, say how many were left out.
const MAX_LINES: usize = 5;

pub fn notice(cwd: &Path) -> Option<String> {
    let repo = crate::push_target::toplevel(cwd)?;
    let lines = scan(&repo.join("docs/plans"));
    if lines.is_empty() {
        return None;
    }
    let shown = lines.len().min(MAX_LINES);
    let mut out = format!(
        "amont-agent/plans: {} in docs/plans/ (read one on demand; they are history, not instructions):",
        if lines.len() == 1 { "1 open plan".to_string() } else { format!("{} open plans", lines.len()) }
    );
    for l in &lines[..shown] {
        out.push_str("\n- ");
        out.push_str(l);
    }
    if lines.len() > shown {
        out.push_str(&format!("\n- … and {} more", lines.len() - shown));
    }
    Some(out)
}

fn scan(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .filter(|p| p.file_name().is_some_and(|n| n != "README.md"))
        .collect();
    files.sort();
    files
        .iter()
        .filter_map(|p| {
            let text = std::fs::read_to_string(p).ok()?;
            let name = p.file_name()?.to_string_lossy().into_owned();
            line_for(&name, &text)
        })
        .collect()
}

/// The notice line for one plan file, or `None` when it is closed.
pub fn line_for(name: &str, text: &str) -> Option<String> {
    let (front, body) = split_front_matter(text);
    let field = |k: &str| field(front, k);
    let status = field("status");
    match status.as_deref() {
        None | Some("") => return Some(format!("{name}: no `status` in its front matter")),
        Some(s) if !STATUSES.contains(&s) => {
            return Some(format!(
                "{name}: unknown status `{s}` (expected one of {})",
                STATUSES.join(", ")
            ))
        }
        Some("active") => {}
        Some(_) => return None,
    }
    if let Some(canonical) = field("canonical").filter(|c| !c.is_empty()) {
        let phases = field("phases").unwrap_or_default();
        return Some(format!("{name} → {canonical} — this repo: phases {phases}"));
    }
    let title = body
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .unwrap_or(name)
        .trim()
        .to_string();
    let next = body
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("- [ ] "))
        .map(|t| t.trim().to_string());
    Some(match next {
        Some(n) => format!("{title} ({name}) — next: {n}"),
        None => format!("{title} ({name})"),
    })
}

/// One front-matter field, its trailing `# comment` dropped.
pub(crate) fn field(front: &str, k: &str) -> Option<String> {
    front.lines().find_map(|l| {
        let (key, v) = l.split_once(':')?;
        (key.trim() == k).then(|| v.split('#').next().unwrap_or("").trim().to_string())
    })
}

pub(crate) fn split_front_matter(text: &str) -> (&str, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return ("", text);
    };
    match rest.find("\n---\n") {
        Some(end) => (&rest[..end], &rest[end + 5..]),
        None => ("", text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_active_plan_names_its_next_phase() {
        let text = "---\nstatus: active\n---\n# Settings toggle\n\n- [x] Phase 1 — model\n- [ ] Phase 2 — route\n";
        assert_eq!(
            line_for("2026-09-28-settings.md", text).as_deref(),
            Some("Settings toggle (2026-09-28-settings.md) — next: Phase 2 — route")
        );
    }

    #[test]
    fn a_closed_plan_is_silent() {
        for s in ["done", "abandoned", "imported"] {
            assert_eq!(
                line_for("x.md", &format!("---\nstatus: {s}\n---\n# X\n")),
                None,
                "{s}"
            );
        }
    }

    #[test]
    fn a_pointer_names_its_canonical_plan() {
        let text = "---\ncanonical: decisions:docs/plans/p.md\nphases: [2]\nstatus: active   # this slice\n---\nPart of …\n";
        assert_eq!(
            line_for("p.md", text).as_deref(),
            Some("p.md → decisions:docs/plans/p.md — this repo: phases [2]")
        );
    }

    #[test]
    fn a_malformed_status_is_reported() {
        assert!(line_for("x.md", "---\nstatus: wip\n---\n")
            .unwrap()
            .contains("unknown status `wip`"));
        assert!(line_for("x.md", "# no front matter\n")
            .unwrap()
            .contains("no `status`"));
    }
}
