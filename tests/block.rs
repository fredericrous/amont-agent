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
    assert!(String::from_utf8_lossy(&no_verb.stderr).contains("usage: greenpiste block sha"));

    for args in [&["--help"][..], &["sha", "--help"][..], &["-h"][..]] {
        let out = block(args);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(said.contains("usage: greenpiste block sha"), "{said}");
        for line in said.lines() {
            assert!(line.chars().count() <= 80, "{line}");
        }
    }
}

/// A registry in a temp journal directory; `CLAUDE_CONFIG_DIR` points at it,
/// so the real journal is never read or written.
struct Registry {
    config: PathBuf,
}

const ACTIVE: &str = "aaaaaaaaaaaa00000000000000000000000000000000000000000000000000aa";
const REVOKED_FP: &str = "bbbbbbbbbbbb11111111111111111111111111111111111111111111111111bb";

impl Registry {
    fn new(name: &str) -> Registry {
        let config = std::env::temp_dir().join(format!(
            "amont-agent-registry-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&config); // a leftover dir from an earlier run; absence is the goal
        std::fs::create_dir_all(&config).unwrap();
        Registry { config }
    }

    fn base(&self) -> PathBuf {
        self.config.join("amont-agent")
    }

    fn register(&self, fingerprint: &str, roles: &[&str], repo: &str, commit: &str) {
        self.register_from(fingerprint, roles, Some(repo), repo, commit);
    }

    fn register_from(
        &self,
        fingerprint: &str,
        roles: &[&str],
        source_repo: Option<&str>,
        repo: &str,
        commit: &str,
    ) {
        use amont_agent_process::block_registry::{
            record_copy, register_entry, EntryData, Generation,
        };
        register_entry(
            &self.base(),
            fingerprint,
            &Generation {
                rules_sha: "c".repeat(64),
                agent_digest: "d".repeat(64),
            },
            &EntryData {
                roles: roles.iter().map(|r| r.to_string()).collect(),
                agents: vec![],
                source_branch: "topic".into(),
                source_repo: source_repo.map(str::to_string),
                content_id: "e".repeat(64),
            },
        )
        .unwrap();
        record_copy(&self.base(), fingerprint, repo, commit, "topic").unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .arg("block")
            .args(args)
            .env("CLAUDE_CONFIG_DIR", &self.config)
            .output()
            .unwrap()
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.config); // test cleanup in Drop; nothing to report to
    }
}

fn out_text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn err_text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn list_on_an_empty_store_prints_nothing_and_json_an_empty_array() {
    let registry = Registry::new("list-empty");
    let out = registry.run(&["list"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    let json = registry.run(&["list", "--json"]);
    assert_eq!(json.status.code(), Some(0));
    assert_eq!(out_text(&json).trim(), "[]");
}

#[test]
fn list_shows_active_and_revoked_blocks_as_tsv_and_json() {
    let registry = Registry::new("list-two");
    registry.register(ACTIVE, &["backend", "lang:rust"], "o/alpha", "abc123");
    registry.register(REVOKED_FP, &["backend"], "o/beta", "def456");
    amont_agent_process::block_registry::revoke(&registry.base(), REVOKED_FP, "a defect").unwrap();

    let out = registry.run(&["list"]);
    assert_eq!(out.status.code(), Some(0), "{}", err_text(&out));
    assert_eq!(
        out_text(&out),
        "fp:aaaaaaaaaaaa\tactive\tbackend,lang:rust\t1\to/alpha\n\
         fp:bbbbbbbbbbbb\trevoked\tbackend\t1\to/beta\n"
    );

    let json = registry.run(&["list", "--json"]);
    assert_eq!(json.status.code(), Some(0));
    let rows: serde_json::Value = serde_json::from_str(&out_text(&json)).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["fingerprint"], ACTIVE);
    assert_eq!(rows[0]["short"], "fp:aaaaaaaaaaaa");
    assert_eq!(rows[0]["state"], "active");
    assert!(rows[0]["because"].is_null());
    assert_eq!(
        rows[0]["roles"],
        serde_json::json!(["backend", "lang:rust"])
    );
    assert_eq!(rows[0]["source_repo"], "o/alpha");
    assert_eq!(
        rows[0]["copies"],
        serde_json::json!([{"repo": "o/alpha", "branch": "topic", "commit": "abc123"}])
    );
    assert_eq!(rows[1]["state"], "revoked");
    assert_eq!(rows[1]["because"], "a defect");
}

#[test]
fn revoke_by_short_id_prefix_and_full_hex() {
    for (name, given) in [
        ("short", "fp:aaaaaaaaaaaa".to_string()),
        ("prefix", "fp:aaaa".to_string()),
        ("bare-prefix", "aaaaaaaaaaaa0".to_string()),
        ("full", ACTIVE.to_string()),
    ] {
        let registry = Registry::new(&format!("revoke-{name}"));
        registry.register(ACTIVE, &["backend"], "o/alpha", "abc123");
        registry.register(REVOKED_FP, &["backend"], "o/beta", "def456");
        let out = registry.run(&["revoke", &given, "--because", "found a defect"]);
        assert_eq!(out.status.code(), Some(0), "{given}: {}", err_text(&out));
        assert_eq!(
            out_text(&out),
            "o/alpha\ttopic\t-\no/alpha\ttopic\tabc123\n",
            "{given}"
        );
        assert!(err_text(&out).contains("fp:aaaaaaaaaaaa is revoked"));
        assert_eq!(
            amont_agent_process::block_registry::revoked(&registry.base(), ACTIVE).as_deref(),
            Some("found a defect")
        );
        assert_eq!(
            amont_agent_process::block_registry::revoked(&registry.base(), REVOKED_FP),
            None
        );
    }
}

#[test]
fn a_second_revoke_exits_0_and_prints_the_copies_again() {
    let registry = Registry::new("revoke-twice");
    registry.register(ACTIVE, &["backend"], "o/alpha", "abc123");
    for _ in 0..2 {
        let out = registry.run(&["revoke", "fp:aaaaaaaaaaaa", "--because=twice"]);
        assert_eq!(out.status.code(), Some(0), "{}", err_text(&out));
        assert_eq!(
            out_text(&out),
            "o/alpha\ttopic\t-\no/alpha\ttopic\tabc123\n"
        );
    }
}

#[test]
fn revoke_without_because_exits_2() {
    let registry = Registry::new("revoke-no-because");
    registry.register(ACTIVE, &["backend"], "o/alpha", "abc123");
    for args in [
        &["revoke", "fp:aaaaaaaaaaaa"][..],
        &["revoke", "fp:aaaaaaaaaaaa", "--because", "  "][..],
        &["revoke", "fp:aaaaaaaaaaaa", "--because"][..],
    ] {
        let out = registry.run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
    }
    assert_eq!(
        amont_agent_process::block_registry::revoked(&registry.base(), ACTIVE),
        None
    );
}

#[test]
fn an_unknown_or_ambiguous_fingerprint_exits_1_naming_it() {
    let registry = Registry::new("unknown");
    registry.register(ACTIVE, &["backend"], "o/alpha", "abc123");
    let other = format!("aaab{}", "2".repeat(60));
    registry.register(&other, &["backend"], "o/beta", "def456");

    for args in [
        &["revoke", "fp:ffff", "--because", "x"][..],
        &["revoke", "fp:aaa", "--because", "x"][..],
        &["reinstate", "fp:ffff"][..],
        &["reinstate", "fp:aaa"][..],
    ] {
        let out = registry.run(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let said = err_text(&out);
        let id = args[1].strip_prefix("fp:").unwrap();
        let expected = if id == "aaa" {
            "greenpiste: block: fp:aaa matches 2 blocks; give more hex".to_string()
        } else {
            format!("greenpiste: block: no entry fp:{id}; see greenpiste block list")
        };
        assert!(said.contains(&expected), "{args:?}: {said}");
    }
}

#[test]
fn reinstate_a_revoked_block_prints_a_success_line() {
    let registry = Registry::new("reinstate");
    registry.register(ACTIVE, &["backend"], "o/alpha", "abc123");
    let revoked = registry.run(&["revoke", "fp:aaaaaaaaaaaa", "--because", "mistake"]);
    assert_eq!(revoked.status.code(), Some(0));

    let out = registry.run(&["reinstate", "fp:aaaaaaaaaaaa"]);
    assert_eq!(out.status.code(), Some(0), "{}", err_text(&out));
    assert!(out.stdout.is_empty(), "{}", out_text(&out));
    assert!(
        err_text(&out).contains("greenpiste: block: fp:aaaaaaaaaaaa reinstated"),
        "{}",
        err_text(&out)
    );
    assert_eq!(
        amont_agent_process::block_registry::revoked(&registry.base(), ACTIVE),
        None
    );
}

#[test]
fn reinstate_a_block_that_was_not_revoked_says_so_on_stderr_and_exits_0() {
    let registry = Registry::new("reinstate-active");
    registry.register(ACTIVE, &["backend"], "o/alpha", "abc123");
    let out = registry.run(&["reinstate", "fp:aaaaaaaaaaaa"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(
        err_text(&out).contains("greenpiste: block: fp:aaaaaaaaaaaa was not revoked"),
        "{}",
        err_text(&out)
    );
}

#[test]
fn an_entry_without_a_source_repo_lists_a_dash_and_a_null() {
    let registry = Registry::new("list-no-source");
    registry.register_from(ACTIVE, &["backend"], None, "o/alpha", "abc123");
    let out = registry.run(&["list"]);
    assert_eq!(out_text(&out), "fp:aaaaaaaaaaaa\tactive\tbackend\t1\t-\n");
    let json = registry.run(&["list", "--json"]);
    let rows: serde_json::Value = serde_json::from_str(&out_text(&json)).unwrap();
    assert!(rows[0]["source_repo"].is_null());
    let revoked = registry.run(&["revoke", "fp:aaaaaaaaaaaa", "--because", "x"]);
    assert_eq!(out_text(&revoked), "-\ttopic\t-\no/alpha\ttopic\tabc123\n");
}

#[test]
fn help_is_a_verb_flag_never_a_flag_value() {
    let registry = Registry::new("help-position");
    registry.register(ACTIVE, &["backend"], "o/alpha", "abc123");
    for args in [
        &["--help"][..],
        &["-h"][..],
        &["revoke", "--help"][..],
        &["list", "-h"][..],
        &["sha", "--help"][..],
        &["reinstate", "-h"][..],
    ] {
        let out = registry.run(args);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(out_text(&out).starts_with("usage:"), "{args:?}");
    }
    let out = registry.run(&["revoke", "fp:aaaaaaaaaaaa", "--because", "-h"]);
    assert_eq!(out.status.code(), Some(0), "{}", err_text(&out));
    assert!(!out_text(&out).starts_with("usage:"));
    assert_eq!(
        amont_agent_process::block_registry::revoked(&registry.base(), ACTIVE).as_deref(),
        Some("-h")
    );
}

#[test]
fn sha_accepts_a_double_dash_and_refuses_an_empty_base() {
    let registry = Registry::new("sha-args");
    let out = registry.run(&["sha", "--base="]);
    assert_eq!(out.status.code(), Some(2), "{}", err_text(&out));
    let out = registry.run(&["sha", "--base", ""]);
    assert_eq!(out.status.code(), Some(2));
    let out = registry.run(&["sha", "--", "extra"]);
    assert_eq!(out.status.code(), Some(2));
    // `--` alone ends the options and is not an unknown argument.
    let out = registry.run(&["sha", "--"]);
    assert!(
        !err_text(&out).contains("unknown argument"),
        "{}",
        err_text(&out)
    );
}

#[test]
fn a_tab_is_not_a_space_in_the_common_indentation() {
    let tabbed = of(&added(&["\tx", "\t\ty"])).unwrap();
    let spaced = of(&added(&[" x", "  y"])).unwrap();
    assert_ne!(tabbed, spaced);
    let tab_then_space = of(&added(&["\tx", " y"])).unwrap();
    let tab_then_tab = of(&added(&["\tx", "\ty"])).unwrap();
    assert_ne!(
        tab_then_space, tab_then_tab,
        "mixed indentation shares no byte prefix"
    );
    let one_more_tab = of(&added(&["\t\ta", "\t\t\tb"])).unwrap();
    let base = of(&added(&["\ta", "\t\tb"])).unwrap();
    assert_eq!(one_more_tab, base, "a block indented by one more tab");
}

#[test]
fn list_and_revoke_pick_the_same_source_across_generations() {
    use amont_agent_process::block_registry::{register_entry, EntryData, Generation};
    let registry = Registry::new("two-generations");
    for (digit, source_repo) in [("1", None), ("2", Some("o/second"))] {
        register_entry(
            &registry.base(),
            ACTIVE,
            &Generation {
                rules_sha: digit.repeat(64),
                agent_digest: "d".repeat(64),
            },
            &EntryData {
                roles: vec!["backend".into()],
                agents: vec![],
                source_branch: "topic".into(),
                source_repo: source_repo.map(str::to_string),
                content_id: "e".repeat(64),
            },
        )
        .unwrap();
    }
    let listed = registry.run(&["list"]);
    assert!(
        out_text(&listed).ends_with("\to/second\n"),
        "{}",
        out_text(&listed)
    );
    let revoked = registry.run(&["revoke", "fp:aaaaaaaaaaaa", "--because", "x"]);
    assert_eq!(out_text(&revoked), "o/second\ttopic\t-\n");
}
