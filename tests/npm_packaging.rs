//! The npm packaging describes the same six targets the release builds.
//!
//! Four files have to agree about that list — `release.yaml`'s build matrix,
//! `scripts/npm-pack.sh`'s table, `npm/amont-agent/bin/native.js`'s resolution map, and
//! `npm/amont-agent/package.json.in`'s `optionalDependencies` — and nothing compiles
//! any of them together. Every way they can disagree fails at a moment nothing
//! can be taken back:
//!
//!   * a target built but not packed → `amont-agent` resolves to nothing on that
//!     platform, and the person finds out when a session opens;
//!   * a target packed but not built → `npm-pack.sh` dies mid-release, after
//!     the GitHub release has already published;
//!   * a package laid out but not declared → npm never installs it, and the
//!     wrapper reports "no native binary" on a platform that has one;
//!   * a package declared but not laid out → `npm publish` rejects
//!     `amont-agent` for a dependency version that does not exist;
//!   * a package packed but not named in the publish LOOP → the root package
//!     publishes against a dependency nobody uploaded, and npm versions are
//!     immutable, so that is a version bump to fix.
//!
//! So the lists are read off disk and compared. A Rust test is a strange
//! home for it and it is the only place that runs on every commit.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    // One crate at the repo root, so CARGO_MANIFEST_DIR is the root.
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read(rel: &str) -> String {
    let p = root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// Rust targets named in `release.yaml`'s build matrix.
fn release_targets() -> Vec<String> {
    read(".github/workflows/release.yaml")
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- target: ").map(str::to_string))
        .collect()
}

/// `(rust-target, npm-package)` pairs from `npm-pack.sh`'s table.
fn packed_pairs() -> Vec<(String, String)> {
    read("scripts/npm-pack.sh")
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('"') && l.contains("|amont-agent-"))
        .filter_map(|l| {
            let row = l.trim_start_matches('"').trim_end_matches('"');
            let mut it = row.split('|');
            Some((it.next()?.to_string(), it.next()?.to_string()))
        })
        .collect()
}

/// Package names in `amont-agent`'s `optionalDependencies`.
fn declared_packages() -> Vec<String> {
    let text = read("npm/amont-agent/package.json.in");
    let (_, after) = text
        .split_once("\"optionalDependencies\"")
        .expect("optionalDependencies block");
    after
        .lines()
        .take_while(|l| !l.contains('}'))
        .filter_map(|l| {
            let l = l.trim();
            l.strip_prefix('"')?
                .split_once('"')
                .map(|(name, _)| name.to_string())
        })
        .collect()
}

/// Package names the JS wrapper knows how to resolve.
fn wrapper_packages() -> Vec<String> {
    let text = read("npm/amont-agent/bin/native.js");
    let (_, after) = text.split_once("const PACKAGES").expect("PACKAGES map");
    after
        .lines()
        .take_while(|l| !l.starts_with("};"))
        .flat_map(|l| {
            l.match_indices("\"amont-agent-")
                .filter_map(|(i, _)| {
                    let rest = &l[i + 1..];
                    rest.split_once('"').map(|(name, _)| name.to_string())
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

#[test]
fn every_released_target_is_packed_for_npm() {
    let built = sorted(release_targets());
    let packed = sorted(packed_pairs().into_iter().map(|(t, _)| t).collect());
    assert_eq!(
        built, packed,
        "release.yaml builds one set of targets and npm-pack.sh packs another"
    );
}

#[test]
fn the_packed_packages_are_exactly_the_declared_ones() {
    let packed = sorted(packed_pairs().into_iter().map(|(_, p)| p).collect());
    let declared = sorted(declared_packages());
    assert_eq!(
        packed, declared,
        "npm-pack.sh lays out one set of packages and amont declares another \
         as optionalDependencies — npm rejects a version that does not exist"
    );
}

#[test]
fn the_wrapper_can_resolve_every_package_that_is_published() {
    let declared = sorted(declared_packages());
    let known = sorted(wrapper_packages());
    assert_eq!(
        declared, known,
        "bin/native.js resolves a different set than npm installs — a platform \
         with a binary would be told it has none"
    );
}

/// Package-name SUFFIXES named in `release.yaml`'s npm publish loop.
fn published_suffixes() -> Vec<String> {
    let text = read(".github/workflows/release.yaml");
    let (_, after) = text
        .split_once("for t in ")
        .expect("the explicit publish list in publish-npm");
    let line = after.lines().next().expect("the loop line");
    line.split_once("; do")
        .map(|(names, _)| names)
        .unwrap_or(line)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// The publish loop names every package, in a list rather than a glob.
///
/// The bug this exists to prevent, in full, because it is not obvious and it
/// is permanent: amont's release workflow can write
///
///     for dir in dist/npm/amont-*; do publish "$dir"; done
///     publish dist/npm/amont
///
/// because its root package's directory is `amont` and that glob cannot match
/// it. Here the root package IS `amont-agent`, so the same glob matches the
/// root as well as the six platforms — and `amont-agent` sorts BEFORE
/// `amont-agent-darwin-arm64`, so the root would publish FIRST, against six
/// `optionalDependencies` that do not exist yet, and then publish again.
///
/// npm versions are immutable. That is not a re-run, it is a version bump.
#[test]
fn the_publish_loop_names_every_packed_package_explicitly() {
    let packed: Vec<String> = sorted(packed_pairs().into_iter().map(|(_, p)| p).collect());
    let expected: Vec<String> = packed
        .iter()
        .map(|p| {
            p.strip_prefix("amont-agent-")
                .unwrap_or_else(|| panic!("{p} is not an amont-agent-* package"))
                .to_string()
        })
        .collect();
    assert_eq!(
        sorted(expected),
        sorted(published_suffixes()),
        "release.yaml's publish loop and npm-pack.sh's table name different \
         platforms — one of them publishes a package that does not exist, or \
         skips one that does"
    );
}

/// And it must stay a list. A glob over `dist/npm/amont-agent-*` would sweep
/// the root package in with the platforms; there is no glob that separates
/// them, which is exactly why the loop is explicit.
#[test]
fn the_publish_step_does_not_glob_the_package_directory() {
    let text = read(".github/workflows/release.yaml");
    let publish_step = text
        .split_once("for t in ")
        .map(|(before, _)| before)
        .unwrap_or(&text);
    assert!(
        !publish_step.contains("dist/npm/amont-agent-*"),
        "a glob over dist/npm/amont-agent-* also matches the ROOT package \
         amont-agent, and sorts it first — publish the six platforms by name"
    );
}
