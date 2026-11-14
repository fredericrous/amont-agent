//! Which review seats a plan earns from the files its Phases name, and which
//! part of a diff a code-review profile reads (decisions ADR-0061,
//! `work.review-seats`, `work.review-slices`).
//!
//! One table answers both questions, so the panel and the brief can never
//! disagree about what a file is. Pure: paths in, classes out. The repository
//! listing that attributes a path to a repository comes from the caller.

use std::collections::BTreeSet;

/// Files whose name alone says what they are.
const KNOWN_FILES: [&str; 14] = [
    "Cargo.toml",
    "Cargo.lock",
    "go.mod",
    "go.sum",
    "package.json",
    "tsconfig.json",
    "pyproject.toml",
    "Dockerfile",
    "Makefile",
    "Chart.yaml",
    "kustomization.yaml",
    "kustomization.yml",
    "Kustomization",
    "CHANGELOG",
];

/// Extensions a backticked span may end in to count as a path.
const KNOWN_EXTENSIONS: [&str; 26] = [
    "rs", "go", "py", "pyi", "ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs", "css", "scss",
    "html", "vue", "svelte", "md", "mdx", "toml", "yaml", "yml", "json", "tf", "sh", "cases",
];

/// Rendered files: what a person sees (react, ui-design).
const UI_EXTENSIONS: [&str; 7] = [".tsx", ".jsx", ".css", ".scss", ".html", ".vue", ".svelte"];

/// YAML that is configuration of a tool, never a deployment: read as
/// nothing rather than as `ops`.
const NOT_OPS_YAML: [&str; 4] = [
    ".adr.yaml",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    ".pre-commit-config.yaml",
];

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn extension(path: &str) -> Option<&str> {
    let base = basename(path);
    base.rsplit_once('.')
        .map(|(_, e)| e)
        .filter(|_| !base.starts_with('.') || base.matches('.').count() > 1)
}

/// The path a backticked span names, if it names one: no space, not a
/// flag, a URL or an assignment, and either a directory path or a file this
/// table knows. A `:<line>` or `:<line>-<line>` suffix is dropped.
pub fn path_in_span(span: &str) -> Option<String> {
    let text = span.trim();
    if text.is_empty()
        || text.contains(char::is_whitespace)
        || text.starts_with('-')
        || text.contains("://")
        || text.contains(['=', '<', '>', '(', ')', '{', '}', '*', '$', '|', '\''])
    {
        return None;
    }
    let without_line = match text.split_once(':') {
        Some((path, line))
            if line
                .chars()
                .all(|ch| ch.is_ascii_digit() || ch == '-' || ch == ',') =>
        {
            path
        }
        Some(_) => return None,
        None => text,
    };
    let path = without_line
        .trim_end_matches(['.', ',', ';'])
        .trim_start_matches("./");
    if path.is_empty() || path.starts_with('/') || path.starts_with('~') || path.contains("..") {
        return None;
    }
    let known_file = KNOWN_FILES.contains(&basename(path));
    let known_ext = extension(path).is_some_and(|e| KNOWN_EXTENSIONS.contains(&e));
    let looks_like_dir = path.ends_with('/') && path.len() > 1;
    (known_file || known_ext || (looks_like_dir && path.contains('/'))).then(|| path.to_string())
}

/// The spans between backticks on one line, single backticks only.
fn spans(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('`') else { break };
        out.push(&after[..end]);
        rest = &after[end + 1..];
    }
    out
}

/// One phase of `## Phases`: its label (`Phase 2`) and the paths it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase {
    pub label: String,
    pub paths: Vec<String>,
}

/// The phases of a plan body and the paths each names. A phase is a
/// top-level list item under `## Phases`, labelled by its own `Phase <n>`
/// or else by its place (`phase 3`); its continuation lines belong to it. A
/// `🧑` decision is not a phase. Only Phases count: Context and evidence
/// cite files a plan does not change.
pub fn phases(body: &str) -> Vec<Phase> {
    phase_spans(body)
        .into_iter()
        .filter(|s| !s.label.is_empty())
        .map(|s| Phase {
            label: s.label,
            paths: s.paths,
        })
        .collect()
}

/// One top-level item of `## Phases` and the lines it covers, as indices
/// into `body.lines()`: from its own line to the next top-level item or the
/// end of the section, so its continuation lines and fenced examples are its
/// own. A `🧑` decision has an empty label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseSpan {
    pub label: String,
    pub paths: Vec<String>,
    pub lines: std::ops::Range<usize>,
}

/// Every top-level item of `## Phases`, `🧑` decisions included, with the
/// lines each covers. The section's text before its first item belongs to
/// no item. [`phases`] is this without the decisions.
pub fn phase_spans(body: &str) -> Vec<PhaseSpan> {
    let mut fence = crate::plan_text::Fence::default();
    let mut inside = false;
    let mut out: Vec<PhaseSpan> = Vec::new();
    let close = |out: &mut Vec<PhaseSpan>, at: usize| {
        if let Some(last) = out.last_mut() {
            if last.lines.end == usize::MAX {
                last.lines.end = at;
            }
        }
    };
    let mut count = 0;
    for (i, line) in body.lines().enumerate() {
        count = i + 1;
        if let Some(title) = fence.h2(line) {
            if inside {
                close(&mut out, i);
            }
            inside = title == "Phases";
            continue;
        }
        if !inside || fence.open.is_some() {
            // A fenced example belongs to its phase's lines, but the paths
            // it shows are an example's, not the phase's (as before).
            continue;
        }
        let top = line.starts_with("- ") || line.starts_with("* ");
        if top {
            close(&mut out, i);
            let after_marker = line[2..].trim_start();
            let item_text = ["[ ] ", "[x] ", "[X] "]
                .iter()
                .find_map(|checkbox| after_marker.strip_prefix(checkbox))
                .unwrap_or(after_marker)
                .trim_start();
            let label = if item_text.starts_with('🧑') {
                // A decision for the person ends the phase before it.
                String::new()
            } else if let Some(after) = item_text.strip_prefix("Phase ") {
                let number: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                format!("Phase {number}")
            } else {
                let place = out.iter().filter(|p| !p.label.is_empty()).count() + 1;
                format!("phase {place}")
            };
            out.push(PhaseSpan {
                label,
                paths: Vec::new(),
                lines: i..usize::MAX,
            });
        }
        if let Some(current) = out.last_mut() {
            for span in spans(line) {
                if let Some(p) = path_in_span(span) {
                    if !current.paths.contains(&p) {
                        current.paths.push(p);
                    }
                }
            }
        }
    }
    close(&mut out, count);
    out
}

/// Every path the plan's Phases name, in order, once each.
pub fn named_paths(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in phases(body).into_iter().flat_map(|p| p.paths) {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// The phases that name no path, for the advice (`Phase 5 names no file`).
pub fn phases_without_paths(body: &str) -> Vec<String> {
    phases(body)
        .into_iter()
        .filter(|p| p.paths.is_empty())
        .map(|p| p.label)
        .collect()
}

/// The language a path is written in, if any.
pub fn language(path: &str) -> Option<&'static str> {
    match basename(path) {
        "Cargo.toml" | "Cargo.lock" => return Some("rust"),
        "go.mod" | "go.sum" => return Some("go"),
        "pyproject.toml" => return Some("python"),
        "package.json" | "tsconfig.json" => return Some("typescript"),
        _ => {}
    }
    match extension(path)? {
        "rs" => Some("rust"),
        "go" => Some("go"),
        "py" | "pyi" => Some("python"),
        "ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "mjs" | "cjs" => Some("typescript"),
        _ => None,
    }
}

/// A file a person sees rendered, or a story of one.
pub fn is_ui(path: &str) -> bool {
    let base = basename(path);
    base.contains(".stories.") || UI_EXTENSIONS.iter().any(|e| base.ends_with(e))
}

/// A deployment or infrastructure file: Kubernetes, Helm, Flux, Terraform, a
/// container build, a CI workflow. Unresolved YAML counts, on purpose: a
/// `deployment.yaml` or `values.yaml` names nothing, and missing a platform
/// seat costs more than an extra one.
// holds-until: a manifest is recognised from its content (`kind:`) at the
// tree read the areas already make, or the ledger shows non-deploy YAML
// seating `platform` with no platform finding on 5 plans.
pub fn is_ops(path: &str) -> bool {
    let base = basename(path);
    if matches!(
        base,
        "kustomization.yaml" | "kustomization.yml" | "Kustomization" | "Chart.yaml" | "Dockerfile"
    ) || base.starts_with("Dockerfile.")
        || base.ends_with(".tf")
        || base.ends_with(".tf.json")
        || path.contains(".github/workflows/")
        || path.contains(".forgejo/workflows/")
    {
        return true;
    }
    (base.ends_with(".yaml") || base.ends_with(".yml")) && !NOT_OPS_YAML.contains(&base)
}

/// The review classes one path earns on its own: `lang:<x>`, `ui`, `ops`.
/// `cli` depends on the repository and is added by [`classes`].
pub fn path_classes(path: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(l) = language(path) {
        out.insert(format!("lang:{l}"));
    }
    if is_ui(path) {
        out.insert("ui".to_string());
    }
    if is_ops(path) {
        out.insert("ops".to_string());
    }
    out
}

/// The named paths each repository holds, in the order of `trees` (a name
/// and its file listing): the file itself, a file under it as a directory,
/// a file in its parent directory (a new file in an existing directory), or,
/// for a bare file name, a file of that name anywhere. A path no repository
/// holds goes to every repository, so an unattributed path is never dropped.
pub fn attribute(paths: &[String], trees: &[(&str, &[String])]) -> Vec<Vec<String>> {
    let mut out = vec![Vec::new(); trees.len()];
    for path in paths {
        let named = path.trim_end_matches('/');
        let dir = format!("{named}/");
        let parent = parent_of(named);
        let tail = (!named.contains('/')).then(|| format!("/{named}"));
        let holds = |files: &[String]| {
            files.iter().any(|f| {
                f == named
                    || f.starts_with(&dir)
                    || parent.as_deref().is_some_and(|d| f.starts_with(d))
                    || tail.as_deref().is_some_and(|t| f.ends_with(t))
            })
        };
        let holders: Vec<usize> = (0..trees.len()).filter(|&i| holds(trees[i].1)).collect();
        let to = if holders.is_empty() {
            (0..trees.len()).collect()
        } else {
            holders
        };
        for i in to {
            out[i].push(path.clone());
        }
    }
    out
}

/// `a/b/` for `a/b/c.rs`: a new file in an existing directory is held by the
/// repository that has the directory. `None` for a top-level file.
fn parent_of(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit_once('/').map(|(dir, _)| format!("{dir}/"))
}

/// The classes a set of paths earns in one repository: each path's own
/// classes, plus `cli` when any of them is code and the repository has a
/// command line (`repo_areas` holds `cli`).
pub fn classes(paths: &[String], repo_areas: &BTreeSet<String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for p in paths {
        out.extend(path_classes(p));
    }
    if repo_areas.contains("cli") && paths.iter().any(|p| language(p).is_some()) {
        out.insert("cli".to_string());
    }
    out
}

/// Whether a profile reads a path.
pub type Reads = Box<dyn Fn(&str) -> bool>;

/// What a code-review profile reads of a diff: `Some(test)` for a profile
/// whose files the table knows, `None` for one that reads the whole diff
/// (backend, architect, unix, tui).
pub fn profile_reads(profile: &str) -> Option<Reads> {
    if let Some(lang) = profile.strip_prefix("lang:") {
        let lang = lang.to_string();
        return Some(Box::new(move |p: &str| language(p) == Some(lang.as_str())));
    }
    match profile {
        "react" | "ui-design" => Some(Box::new(is_ui)),
        "platform" => Some(Box::new(is_ops)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_is_a_path_only_when_it_looks_like_one() {
        assert_eq!(
            path_in_span("crates/x/src/lib.rs:12"),
            Some("crates/x/src/lib.rs".into())
        );
        assert_eq!(
            path_in_span("plan_review.rs:1559-1561"),
            Some("plan_review.rs".into())
        );
        assert_eq!(path_in_span("Cargo.toml"), Some("Cargo.toml".into()));
        assert_eq!(path_in_span("docs/plans/"), Some("docs/plans/".into()));
        for not in [
            "classes_of",
            "seats=paths",
            "git diff --stat",
            "--no-renames",
            "https://x.org/a.md",
            "<repo>",
            "lang:rust",
            "~/.claude/plans/p.md",
            "/tmp/x.rs",
            "a.rs:foo",
        ] {
            assert_eq!(path_in_span(not), None, "{not}");
        }
    }

    #[test]
    fn only_phases_name_paths_and_each_phase_keeps_its_own() {
        let body = "## Context\n- cites `crates/other/src/x.rs`\n\n## Phases\n\
                    - [ ] Phase 1 — `src/a.rs` and\n  `docs/b.md`\n\
                    - [ ] 🧑 decision: pick `src/z.rs`?\n\
                    - [x] Phase 2 — release and install\n\n## Verification\n- `src/v.rs`\n";
        let ps = phases(body);
        assert_eq!(ps.len(), 2, "{ps:?}");
        assert_eq!(ps[0].label, "Phase 1");
        assert_eq!(ps[0].paths, vec!["src/a.rs", "docs/b.md"]);
        assert!(ps[1].paths.is_empty());
        assert_eq!(named_paths(body), vec!["src/a.rs", "docs/b.md"]);
        assert_eq!(phases_without_paths(body), vec!["Phase 2"]);
    }

    #[test]
    fn a_phase_without_its_number_is_labelled_by_its_place() {
        let body = "## Phases\n- [ ] build `src/a.rs`\n- [ ] release\n- x Phase 7 `x/y.rs`\n";
        assert_eq!(phases_without_paths(body), vec!["phase 2"]);
        // Only a checkbox is stripped: an item's own leading `x ` stays, so
        // `x Phase 7` is not read as Phase 7.
        assert_eq!(phases(body)[2].label, "phase 3");
    }

    #[test]
    fn spans_cover_each_phases_line_once_and_keep_fences_and_decisions() {
        let body = "# T\n## Phases\nIntro.\n- Phase 1 — `a.rs`\n  ```\n  - not an item `b.ts`\n  ```\n- 🧑 decide\n- Phase 2 — `c.ts`\n## After\n- not a phase\n";
        let s = phase_spans(body);
        let ranges: Vec<_> = s.iter().map(|p| p.lines.clone()).collect();
        assert_eq!(ranges, vec![3..7, 7..8, 8..9]);
        assert_eq!(s[0].paths, vec!["a.rs".to_string()]);
        assert_eq!(s[1].label, "");
        assert_eq!(phases(body).len(), 2);
    }

    #[test]
    fn a_fenced_example_inside_phases_names_nothing() {
        let body = "## Phases\n- [ ] Phase 1 — see below\n```\n- [ ] Phase 9 — `x/y.rs`\n```\n";
        assert!(named_paths(body).is_empty());
    }

    #[test]
    fn each_file_type_earns_its_class() {
        let c = |p: &str| path_classes(p).into_iter().collect::<Vec<_>>();
        assert_eq!(c("src/lib.rs"), vec!["lang:rust"]);
        assert_eq!(c("crates/x/Cargo.toml"), vec!["lang:rust"]);
        assert_eq!(c("web/App.tsx"), vec!["lang:typescript", "ui"]);
        assert_eq!(c("web/Button.stories.ts"), vec!["lang:typescript", "ui"]);
        assert_eq!(c("styles/a.css"), vec!["ui"]);
        assert_eq!(c("apps/web/deployment.yaml"), vec!["ops"]);
        assert_eq!(c("charts/x/values.yaml"), vec!["ops"]);
        assert_eq!(c("infra/main.tf.json"), vec!["ops"]);
        assert_eq!(c(".github/workflows/ci.yml"), vec!["ops"]);
        assert_eq!(c("Dockerfile"), vec!["ops"]);
        assert!(c(".adr.yaml").is_empty());
        assert!(c("docs/plan-review.md").is_empty());
        assert!(c("CHANGELOG.md").is_empty());
    }

    #[test]
    fn cli_comes_from_code_in_a_repository_with_a_command_line() {
        let cli = BTreeSet::from(["cli".to_string()]);
        let paths = vec!["src/dispatch.rs".to_string()];
        assert!(classes(&paths, &cli).contains("cli"));
        assert!(!classes(&paths, &BTreeSet::new()).contains("cli"));
        let docs = vec!["docs/x.md".to_string()];
        assert!(!classes(&docs, &cli).contains("cli"));
    }

    #[test]
    fn a_path_goes_to_the_repository_that_holds_it_or_to_every_one() {
        let app = vec!["src/main.rs".to_string(), "src/plan_review.rs".to_string()];
        let docs = vec!["adr/0001.md".to_string()];
        let trees = [("app", app.as_slice()), ("docs", docs.as_slice())];
        let paths: Vec<String> = [
            "src/main.rs",
            "src/new.rs",
            "plan_review.rs",
            "adr/0002.md",
            "adr/",
            "nowhere/x.go",
        ]
        .map(String::from)
        .to_vec();
        let got = attribute(&paths, &trees);
        assert_eq!(
            got[0],
            vec![
                "src/main.rs",
                "src/new.rs",
                "plan_review.rs",
                "nowhere/x.go"
            ]
        );
        assert_eq!(got[1], vec!["adr/0002.md", "adr/", "nowhere/x.go"]);
    }

    #[test]
    fn a_profile_reads_its_types_or_the_whole_diff() {
        let rust = profile_reads("lang:rust").expect("filtered");
        assert!(rust("crates/x/Cargo.toml") && rust("a.rs") && !rust("web/a.ts"));
        let platform = profile_reads("platform").expect("filtered");
        assert!(platform("k8s/deployment.yaml") && !platform("src/a.rs"));
        for whole in ["backend", "architect", "unix", "tui"] {
            assert!(profile_reads(whole).is_none(), "{whole}");
        }
    }
}
