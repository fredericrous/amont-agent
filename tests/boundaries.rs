//! The guard and the work process are two components (plan
//! `docs/plans/2026-10-19-guard-and-work-process-components.md`): a guard
//! module never names a work-process module or the composition root, and a
//! work-process module never names the composition root. Read from the
//! source, every `crate::<module>` path, in `use` lines and inline; comments
//! and `#[cfg(test)] mod tests` blocks are not code the binary runs.
//!
//! holds-until: the Phase 2 workspace split, where the crate graph enforces
//! the same edges and this test goes away with it.

use std::path::{Path, PathBuf};

/// The work process: its verdicts hold or release a tool call (ADR-0039).
const PROCESS: &[&str] = &[
    "attestations",
    "content_id",
    "guide",
    "implementation_review",
    "implementation_review_rule",
    "plan_apply",
    "plan_phases",
    "plan_review",
    "plan_text",
    "plan_words",
    "plans",
    "preview",
    "provenance",
    "push_preview",
    "review_outcomes",
    "shipped",
];

/// The composition root: it wires the guard and the process together.
const ROOT: &[&str] = &["hook", "main", "rule_table"];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("src is readable").flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The module a source file is: its top-level name under `src/`.
fn module_of(src: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(src).unwrap();
    let first = rel
        .components()
        .next()
        .unwrap()
        .as_os_str()
        .to_string_lossy();
    first.trim_end_matches(".rs").to_string()
}

/// The code of a file: comments and the trailing `#[cfg(test)] mod tests`
/// block removed.
fn code(text: &str) -> String {
    let cut = text.find("#[cfg(test)]\nmod tests").unwrap_or(text.len());
    text[..cut]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .map(|l| match l.find(" // ") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every module `text` names as `crate::<module>`.
fn named(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("crate::") {
        let after = &rest[i + "crate::".len()..];
        let ident = |s: &str| -> String {
            s.trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect()
        };
        if let Some(group) = after.strip_prefix('{') {
            // `use crate::{a, b::c};`: the first segment of each item.
            let end = group.find('}').unwrap_or(group.len());
            out.extend(group[..end].split(',').map(ident).filter(|n| !n.is_empty()));
        } else {
            let name = ident(after);
            if !name.is_empty() {
                out.push(name);
            }
        }
        rest = after;
    }
    out
}

/// The edges that cross a boundary the wrong way, as `module -> named`.
fn violations(src: &Path) -> Vec<String> {
    let mut files = Vec::new();
    sources(src, &mut files);
    assert!(!files.is_empty(), "no sources under {}", src.display());
    let mut out = Vec::new();
    for f in files {
        let module = module_of(src, &f);
        if ROOT.contains(&module.as_str()) {
            continue;
        }
        let process = PROCESS.contains(&module.as_str());
        let text = std::fs::read_to_string(&f).unwrap();
        for name in named(&code(&text)) {
            let wrong =
                ROOT.contains(&name.as_str()) || (!process && PROCESS.contains(&name.as_str()));
            if wrong {
                out.push(format!(
                    "{} -> {name}",
                    f.strip_prefix(src).unwrap().display()
                ));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn the_guard_never_names_the_work_process_or_the_root() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let v = violations(&src);
    assert!(
        v.is_empty(),
        "edges against the boundary:\n{}",
        v.join("\n")
    );
}

#[test]
fn a_planted_edge_is_found_in_a_use_line_and_inline() {
    let dir = std::env::temp_dir().join(format!("amont-agent-boundaries-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("rules")).unwrap();
    std::fs::write(dir.join("doctor.rs"), "use crate::preview;\nfn f() {}\n").unwrap();
    std::fs::write(
        dir.join("ui.rs"),
        "use crate::{json, plan_review::Fence};\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("rules").join("x.rs"),
        "fn g() { let _ = crate::preview::outcome(); }\n// crate::plan_review in a comment\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("preview.rs"),
        "fn h() { crate::hook::run(); }\n#[cfg(test)]\nmod tests {\n    use crate::rule_table::RULES;\n}\n",
    )
    .unwrap();
    let v = violations(&dir);
    let _ = std::fs::remove_dir_all(&dir); // test cleanup: a leftover is harmless
    assert_eq!(
        v,
        vec![
            "doctor.rs -> preview".to_string(),
            "preview.rs -> hook".to_string(),
            "rules/x.rs -> preview".to_string(),
            "ui.rs -> plan_review".to_string(),
        ]
    );
}
