//! `amont-agent block sha` and the block fingerprint (decisions ADR-0067),
//! against the real diffs of the 2026-11-08 fleet change in
//! `tests/fixtures/block/` and a throwaway repository.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use amont_agent_process::block_fingerprint::{fingerprint, fingerprint_of_diff};

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/block")
        .join(format!("{name}.diff"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn of_fixture(name: &str) -> String {
    fingerprint_of_diff(&fixture(name))
        .unwrap_or_else(|| panic!("{name} has a binary file"))
        .full()
        .to_string()
}

fn of(diff: &str) -> Option<String> {
    fingerprint_of_diff(diff.as_bytes()).map(|f| f.full().to_string())
}

/// A one-file diff adding `lines`, each already carrying its indentation.
fn added(lines: &[&str]) -> String {
    let body: String = lines.iter().map(|l| format!("+{l}\n")).collect();
    format!(
        "diff --git a/ci.yaml b/ci.yaml\nindex 1111111..2222222 100644\n--- a/ci.yaml\n+++ b/ci.yaml\n@@ -10,0 +11,{} @@ jobs:\n{body}",
        lines.len()
    )
}

const BLOCK: [&str; 5] = [
    "  - name: fetch",
    "    run: |",
    "      gh release download v1 --repo o/r",
    "      # keep going",
    "      tar xzf a.tgz",
];

#[test]
fn copies_of_the_adr_lint_pilot_share_its_fingerprint() {
    let pilot = of_fixture("adr-lint-53");
    assert_eq!(of_fixture("attest-41"), pilot);
    assert_eq!(of_fixture("decisions-170"), pilot);
}

#[test]
fn copies_of_the_kb_vision_pilot_share_its_fingerprint() {
    let pilot = of_fixture("kb-vision-59");
    assert_eq!(of_fixture("sre-agent-65"), pilot);
    assert_ne!(pilot, of_fixture("adr-lint-53"));
}

#[test]
fn new_shapes_match_neither_pilot() {
    let pilots = [of_fixture("adr-lint-53"), of_fixture("kb-vision-59")];
    for name in ["amont-440", "duro-operator-22", "customer-vision-37"] {
        assert!(!pilots.contains(&of_fixture(name)), "{name}");
    }
}

#[test]
fn crlf_another_indent_and_trailing_blanks_still_match() {
    let base = of(&added(&BLOCK)).unwrap();
    let crlf = added(&BLOCK).replace('\n', "\r\n");
    assert_eq!(of(&crlf).unwrap(), base, "CRLF");
    let deeper: Vec<String> = BLOCK.iter().map(|l| format!("      {l}")).collect();
    let deeper: Vec<&str> = deeper.iter().map(String::as_str).collect();
    assert_eq!(of(&added(&deeper)).unwrap(), base, "another indent");
    let trailing: Vec<String> = BLOCK.iter().map(|l| format!("{l}  \t")).collect();
    let trailing: Vec<&str> = trailing.iter().map(String::as_str).collect();
    assert_eq!(of(&added(&trailing)).unwrap(), base, "trailing blanks");
}

#[test]
fn a_pin_changed_as_context_does_not_count() {
    let with_pin = |pin: &str| {
        format!(
            "diff --git a/ci.yaml b/ci.yaml\n--- a/ci.yaml\n+++ b/ci.yaml\n@@ -3,3 +3,4 @@ env:\n   AVAL_VERSION: {pin}\n+  step: one\n   other: x\n"
        )
    };
    assert_eq!(
        of(&with_pin("1.2.0")).unwrap(),
        of(&with_pin("1.3.0")).unwrap()
    );
}

#[test]
fn near_misses_do_not_match() {
    let base = of(&added(&BLOCK)).unwrap();
    let mut extra: Vec<&str> = BLOCK.to_vec();
    extra.push("      echo done");
    assert_ne!(of(&added(&extra)).unwrap(), base, "an extra line");

    let mut blank: Vec<&str> = BLOCK.to_vec();
    blank.insert(3, "");
    assert_ne!(of(&added(&blank)).unwrap(), base, "an extra blank line");

    let mut swapped: Vec<&str> = BLOCK.to_vec();
    swapped.swap(3, 4);
    assert_ne!(of(&added(&swapped)).unwrap(), base, "a swapped pair");

    let mut comment: Vec<&str> = BLOCK.to_vec();
    comment[3] = "      # keep going, please";
    assert_ne!(of(&added(&comment)).unwrap(), base, "a changed comment");

    let mut pin: Vec<&str> = BLOCK.to_vec();
    pin[2] = "      gh release download v2 --repo o/r";
    assert_ne!(
        of(&added(&pin)).unwrap(),
        base,
        "a pin changed in the change"
    );

    let mut moved: Vec<&str> = BLOCK.to_vec();
    moved[4] = "        tar xzf a.tgz";
    assert_ne!(
        of(&added(&moved)).unwrap(),
        base,
        "a line at another indent"
    );
}

#[test]
fn one_differing_non_utf8_byte_changes_the_hash() {
    let diff = |last: u8| {
        let mut bytes =
            b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,0 +1,2 @@\n+one\n+bytes ".to_vec();
        bytes.push(last);
        bytes.push(b'\n');
        bytes
    };
    let first = fingerprint_of_diff(&diff(0xff)).unwrap();
    let second = fingerprint_of_diff(&diff(0xfe)).unwrap();
    assert_ne!(first, second);
}

#[test]
fn a_removed_line_starting_with_dashes_is_content_not_a_header() {
    let diff =
        |text: &str| format!("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-{text}\n+kept\n");
    let with_dashes = of(&diff("-- x")).unwrap();
    assert_ne!(with_dashes, of(&diff("x")).unwrap());
    assert_ne!(with_dashes, of(&diff("-- y")).unwrap());
    let second_file = "diff --git a/g b/g\n--- a/g\n+++ b/g\n@@ -1,0 +1 @@\n+tail\n";
    let both = of(&format!("{}{second_file}", diff("-- x"))).unwrap();
    assert_ne!(both, with_dashes, "the next file is still read");
}

#[test]
fn a_missing_newline_at_end_of_file_is_handled() {
    let plain = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,0 +1,2 @@\n+one\n+two\n";
    let marked = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,0 +1,2 @@\n+one\n+two\n\\ No newline at end of file\n";
    assert_eq!(of(plain), of(marked));
    assert_eq!(of(plain), of(plain.trim_end_matches('\n')));
}

#[test]
fn a_binary_file_has_no_fingerprint() {
    let diff = "diff --git a/x.bin b/x.bin\nnew file mode 100644\nindex 0000000..1111111\nGIT binary patch\nliteral 1\nIcmZ\n\n";
    assert_eq!(of(diff), None);
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Scratch {
    root: PathBuf,
    repo: PathBuf,
}

impl Scratch {
    /// A repository on `main` with two files, and a branch `topic` changing
    /// one in a subdirectory and one at the root.
    fn new(name: &str) -> Scratch {
        let root =
            std::env::temp_dir().join(format!("amont-agent-block-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root); // a leftover scratch dir from an earlier run; absence is the goal
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        git(&repo, &["init", "-q", "-b", "main", "--template="]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["config", "user.name", "t"]);
        let old: String = (0..30).map(|n| format!("    line {n}\n")).collect();
        std::fs::write(repo.join("sub/a.txt"), &old).unwrap();
        std::fs::write(repo.join("root.txt"), "alpha\nbeta\ngamma\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "base"]);
        git(&repo, &["checkout", "-qb", "topic"]);
        let new: String = (0..30)
            .map(|n| {
                if n % 7 == 3 {
                    format!("    changed {n}\n    and more {n}\n")
                } else {
                    format!("    line {n}\n")
                }
            })
            .collect();
        std::fs::write(repo.join("sub/a.txt"), new).unwrap();
        std::fs::write(repo.join("root.txt"), "alpha\nbeta\ngamma\ndelta é\n").unwrap();
        git(&repo, &["commit", "-qam", "change"]);
        Scratch { root, repo }
    }

    fn sha(&self, cwd: &Path, global: Option<&Path>) -> Output {
        Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .args(["block", "sha", "-C"])
            .arg(cwd)
            .args(["--base", "main"])
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &self.root)
            .env(
                "GIT_CONFIG_GLOBAL",
                global.unwrap_or(Path::new("/dev/null")),
            )
            .output()
            .unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root); // test cleanup in Drop; nothing to report to
    }
}

const HOSTILE: [(&str, &str); 5] = [
    ("diff.algorithm", "patience"),
    ("diff.noprefix", "true"),
    ("diff.indentHeuristic", "false"),
    ("diff.relative", "true"),
    ("core.quotepath", "false"),
];

fn stdout_of(out: &Output) -> String {
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn the_fingerprint_ignores_the_persons_git_settings_and_the_subdirectory() {
    let scratch = Scratch::new("hostile");
    let plain = stdout_of(&scratch.sha(&scratch.repo, None));
    let (full, short) = plain.trim_end().split_once('\t').expect("two columns");
    assert_eq!(full.len(), 64);
    assert!(full.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(short, format!("fp:{}", &full[..12]));

    let library = fingerprint(
        &scratch.repo,
        "main",
        "HEAD",
        std::time::Instant::now() + std::time::Duration::from_secs(20),
    )
    .unwrap()
    .unwrap();
    assert_eq!(library.full(), full);

    for (key, value) in HOSTILE {
        git(&scratch.repo, &["config", key, value]);
    }
    assert_eq!(
        stdout_of(&scratch.sha(&scratch.repo, None)),
        plain,
        "repo config"
    );
    assert_eq!(
        fingerprint(
            &scratch.repo,
            "main",
            "HEAD",
            std::time::Instant::now() + std::time::Duration::from_secs(20)
        )
        .unwrap()
        .unwrap(),
        library
    );
    assert_eq!(
        stdout_of(&scratch.sha(&scratch.repo.join("sub"), None)),
        plain,
        "a subdirectory"
    );

    for (key, _) in HOSTILE {
        git(&scratch.repo, &["config", "--unset", key]);
    }
    let global = scratch.root.join("gitconfig");
    let text: String = HOSTILE
        .iter()
        .map(|(key, value)| {
            let (section, name) = key.split_once('.').unwrap();
            format!("[{section}]\n\t{name} = {value}\n")
        })
        .collect();
    std::fs::write(&global, text).unwrap();
    assert_eq!(
        stdout_of(&scratch.sha(&scratch.repo, Some(&global))),
        plain,
        "global config"
    );
    assert_eq!(
        stdout_of(&scratch.sha(&scratch.repo.join("sub"), Some(&global))),
        plain,
        "global config from a subdirectory"
    );
}

#[test]
fn a_binary_file_prints_nothing_and_exits_1() {
    let scratch = Scratch::new("binary");
    std::fs::write(scratch.repo.join("x.bin"), [0u8, 159, 146, 150, 0]).unwrap();
    git(&scratch.repo, &["add", "x.bin"]);
    git(&scratch.repo, &["commit", "-qm", "binary"]);
    let out = scratch.sha(&scratch.repo, None);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("binary file (x.bin)"), "{said}");
}

fn block(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_amont-agent"))
        .arg("block")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn usage_errors_exit_2_and_help_exits_0() {
    for args in [&[][..], &["frobnicate"][..], &["sha", "--bogus"][..]] {
        let out = block(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
    }
    let no_verb = block(&[]);
    assert!(String::from_utf8_lossy(&no_verb.stderr).contains("usage: amont-agent block sha"));

    for args in [&["--help"][..], &["sha", "--help"][..], &["-h"][..]] {
        let out = block(args);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(said.contains("usage: amont-agent block sha"), "{said}");
        for line in said.lines() {
            assert!(line.chars().count() <= 80, "{line}");
        }
    }
}
