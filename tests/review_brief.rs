//! `amont-agent rules-sha` and `review-brief`, through the real binary
//! (decisions ADR-0043, ADR-0044).

use std::path::PathBuf;
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "amont-agent-review-brief-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => panic!("cannot clear {}: {e}", dir.display()),
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs the binary with the fake `aval` in `bin` first on PATH, so no test
/// reaches the machine's own `aval`.
fn run(args: &[&str], bin: &std::path::Path) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_amont-agent"));
    cmd.args(args);
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![bin.to_path_buf()];
    dirs.extend(std::env::split_paths(&inherited));
    cmd.env("PATH", std::env::join_paths(dirs).unwrap());
    // No test reaches the machine's own `adr-lint`: a fake in `bin`, or none.
    cmd.env("AMONT_AGENT_ADR_LINT", bin.join("adr-lint"));
    cmd.output().expect("amont-agent runs")
}

/// The fake is a `#!/bin/sh` script: Windows resolves `Command::new("aval")`
/// to `aval.exe` only, so a `.cmd` fake would never be found. Every test that
/// needs it is therefore gated `cfg(unix)`, and none runs the real `aval`.
#[cfg(unix)]
fn fake_aval(dir: &std::path::Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("aval");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn a_failing_aval_exits_3_with_nothing_on_stdout() {
    let bin = scratch("failing");
    fake_aval(&bin, "#!/bin/sh\nexit 1\n");
    for sub in ["rules-sha", "review-brief"] {
        let out = run(&[sub, "-C", "."], &bin);
        assert_eq!(out.status.code(), Some(3), "{sub}");
        assert!(out.stdout.is_empty(), "{sub}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(err.trim().lines().count(), 1, "{err}");
        assert!(err.contains("aval --json rules"), "{err}");
    }
}

#[cfg(unix)]
#[test]
fn aval_rules_that_are_not_json_exit_3_with_nothing_on_stdout() {
    let bin = scratch("not-json");
    fake_aval(&bin, "#!/bin/sh\nprintf 'not json'\n");
    let out = run(&["review-brief", "-C", "."], &bin);
    assert_eq!(out.status.code(), Some(3));
    assert!(out.stdout.is_empty());
}

#[cfg(unix)]
#[test]
fn rules_sha_prints_the_sha256_of_what_aval_printed() {
    let bin = scratch("sha");
    fake_aval(&bin, "#!/bin/sh\nprintf abc\n");
    let out = run(&["rules-sha"], &bin);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n"
    );
}

#[cfg(unix)]
#[test]
fn both_are_listed_and_answer_help() {
    let bin = scratch("help");
    fake_aval(&bin, "#!/bin/sh\nexit 1\n");
    let help = run(&["--help"], &bin);
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("rules-sha"), "{text}");
    assert!(text.contains("review-brief"), "{text}");
    for sub in ["rules-sha", "review-brief"] {
        assert_eq!(run(&[sub, "--help"], &bin).status.code(), Some(0), "{sub}");
    }
}

#[cfg(unix)]
#[test]
fn a_usage_error_exits_2() {
    let bin = scratch("usage");
    fake_aval(&bin, "#!/bin/sh\nexit 1\n");
    for sub in ["rules-sha", "review-brief"] {
        assert_eq!(run(&[sub, "--bogus"], &bin).status.code(), Some(2), "{sub}");
        assert_eq!(run(&[sub, "-C"], &bin).status.code(), Some(2), "{sub}");
        // A word whose second byte is inside a character is a usage error, not a panic.
        assert_eq!(run(&[sub, "aé"], &bin).status.code(), Some(2), "{sub}");
    }
    assert_eq!(
        run(&["rules-sha", "--plan", "x"], &bin).status.code(),
        Some(2)
    );
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

#[cfg(unix)]
#[test]
fn a_failed_git_diff_is_a_repository_error_with_nothing_on_stdout() {
    let bin = scratch("diff-fails");
    fake_aval(&bin, "#!/bin/sh\nprintf '{\"rules\":[]}'\n");
    let repo = scratch("diff-fails-repo");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "base"]);
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    git(&repo, &["commit", "-qam", "change"]);
    // Remove the changed file's new blob: commits and trees still read, a diff cannot.
    let blob = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "HEAD:a.txt"])
        .output()
        .unwrap();
    let blob = String::from_utf8(blob.stdout).unwrap();
    let blob = blob.trim();
    let object = repo.join(".git/objects").join(&blob[..2]).join(&blob[2..]);
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&object, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    std::fs::remove_file(&object).unwrap();

    let out = run(
        &[
            "review-brief",
            "-C",
            repo.to_str().unwrap(),
            "--base",
            "HEAD~1",
        ],
        &bin,
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("git diff"),
        "{out:?}"
    );
}

/// A scratch repository with a base commit and one change on top, for the
/// brief's success path and the flag shapes.
#[cfg(unix)]
fn two_commit_repo(name: &str) -> PathBuf {
    two_commit_repo_with(name, &[".adr.yaml"])
}

/// The same, with `extra` files in the base commit (`.adr.yaml` marks a
/// repository that has an aval corpus).
#[cfg(unix)]
fn two_commit_repo_with(name: &str, extra: &[&str]) -> PathBuf {
    let repo = scratch(name);
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    for file in extra {
        std::fs::write(repo.join(file), "x: 1\n").unwrap();
    }
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "base"]);
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    git(&repo, &["commit", "-qam", "change"]);
    repo
}

/// An `aval` that answers each command the brief runs with fixed text.
#[cfg(unix)]
const BRIEF_AVAL: &str = r#"#!/bin/sh
case "$*" in
  "--json rules --level constraint --governs code --full")
    printf '{"rules":[{"id":"names.reveal-intent","level":"constraint","statement":"A name says what it is.","body":"No placeholder names.","active":true}]}' ;;
  "rules --level constraint") printf 'constraint names.reveal-intent\n' ;;
  "--json relevant"*) printf '{"keys":[]}' ;;
  *) exit 1 ;;
esac
"#;

#[cfg(unix)]
#[test]
fn the_brief_binds_the_same_tree_and_rules_as_tree_sha_and_rules_sha() {
    let bin = scratch("brief-ok");
    fake_aval(&bin, BRIEF_AVAL);
    let repo = two_commit_repo("brief-ok-repo");
    let dir = repo.to_str().unwrap();
    let brief = run(&["review-brief", "-C", dir, "--base", "HEAD~1"], &bin);
    assert_eq!(brief.status.code(), Some(0), "{brief:?}");
    let brief = String::from_utf8(brief.stdout).unwrap();
    let rules_sha = run(&["rules-sha", "-C", dir], &bin);
    assert_eq!(rules_sha.status.code(), Some(0), "{rules_sha:?}");
    let rules_sha = String::from_utf8(rules_sha.stdout).unwrap();
    assert_eq!(rules_sha.trim().len(), 64, "{rules_sha}");
    let tree = run(&["tree-sha", "--block", "-C", dir], &bin);
    assert_eq!(tree.status.code(), Some(0), "{tree:?}");
    let tree = String::from_utf8(tree.stdout).unwrap();
    assert!(tree.trim().starts_with("<<<TREE "), "{tree}");
    assert!(
        brief.contains(&format!("<<<RULES sha={}>>>", rules_sha.trim())),
        "{brief}"
    );
    assert!(brief.contains(tree.trim()), "{brief}");
    assert!(brief.contains("No placeholder names."), "{brief}");
}

#[cfg(unix)]
#[test]
fn flags_take_the_posix_shapes() {
    let bin = scratch("posix");
    fake_aval(&bin, BRIEF_AVAL);
    let repo = two_commit_repo("posix-repo");
    let attached = format!("-C{}", repo.display());
    for args in [
        vec!["review-brief", attached.as_str(), "--base=HEAD~1"],
        vec!["review-brief", attached.as_str(), "--base=HEAD~1", "--"],
        vec!["rules-sha", attached.as_str()],
    ] {
        let out = run(&args, &bin);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {out:?}");
    }
    let operand = run(&["rules-sha", attached.as_str(), "--", "extra"], &bin);
    assert_eq!(operand.status.code(), Some(2), "{operand:?}");
}

#[cfg(unix)]
#[test]
fn an_aval_relevant_answer_without_keys_exits_3() {
    let bin = scratch("no-keys");
    fake_aval(
        &bin,
        &BRIEF_AVAL.replace(
            r#""--json relevant"*) printf '{"keys":[]}' ;;"#,
            r#""--json relevant"*) printf '{}' ;;"#,
        ),
    );
    let repo = two_commit_repo("no-keys-repo");
    let out = run(
        &[
            "review-brief",
            "-C",
            repo.to_str().unwrap(),
            "--base",
            "HEAD~1",
        ],
        &bin,
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("keys"),
        "{out:?}"
    );
}

const ID_ONE: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const ID_TWO: &str = "2222222222222222222222222222222222222222222222222222222222222222";

/// A fake `adr-lint` that records its arguments and prints `document`.
#[cfg(unix)]
fn fake_adr_lint(dir: &std::path::Path, document: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(dir.join("candidates.json"), document).unwrap();
    let path = dir.join("adr-lint");
    std::fs::write(
        &path,
        "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s' \"$*\" > \"$d/args\"\ncat \"$d/candidates.json\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
fn two_candidates() -> String {
    let row = |id: &str, line: u32| {
        format!(
            r#"{{"id":"{id}","rule":"names.reveal-intent","pattern_id":"placeholder","pattern":"a placeholder name","path":"src/a.rs","line":{line},"col":5,"text":"let data = 1;","future":true}}"#
        )
    };
    format!(
        r#"{{"format":"adr-lint.candidates/1","base":"b","head":"h","candidates":[{},{}],"overflow":{{"names.reveal-intent":7}}}}"#,
        row(ID_ONE, 3),
        row(ID_TWO, 9)
    )
}

#[cfg(unix)]
fn brief_text(bin: &std::path::Path, repo: &std::path::Path) -> (Output, String) {
    let out = run(
        &[
            "review-brief",
            "-C",
            repo.to_str().unwrap(),
            "--base",
            "HEAD~1",
        ],
        bin,
    );
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    (out, text)
}

#[cfg(unix)]
#[test]
fn the_brief_carries_adr_lints_candidates_right_after_the_rules_line() {
    let bin = scratch("cand-two");
    fake_aval(&bin, BRIEF_AVAL);
    let document = two_candidates();
    fake_adr_lint(&bin, &document);
    let repo = two_commit_repo("cand-two-repo");
    let (out, brief) = brief_text(&bin, &repo);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    let sha =
        amont_agent_guard::sha256::hex(&amont_agent_guard::sha256::sha256(document.as_bytes()));
    let lines: Vec<&str> = brief.lines().collect();
    let rules = lines
        .iter()
        .position(|l| l.starts_with("<<<RULES sha="))
        .expect("a rules line");
    assert_eq!(lines[rules + 1], format!("<<<CANDIDATES sha={sha} n=2>>>"));
    assert_eq!(
        lines[rules + 2],
        format!("{ID_ONE} names.reveal-intent placeholder src/a.rs:3")
    );
    assert_eq!(
        lines[rules + 3],
        format!("{ID_TWO} names.reveal-intent placeholder src/a.rs:9")
    );
    assert_eq!(lines[rules + 4], "<<<END CANDIDATES>>>");
    assert!(brief.contains("## Candidates"), "{brief}");
    assert!(brief.contains("a placeholder name"), "{brief}");
    assert!(brief.contains("let data = 1;"), "{brief}");
    assert!(brief.contains("names.reveal-intent: 7"), "{brief}");
    assert!(brief.contains("`Candidates:` block"), "{brief}");
    let base = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "HEAD~1"])
        .output()
        .unwrap();
    let base = String::from_utf8(base.stdout).unwrap();
    assert_eq!(
        std::fs::read_to_string(bin.join("args")).unwrap(),
        format!("candidates --base {} --json --max 40", base.trim())
    );
}

#[cfg(unix)]
#[test]
fn no_candidates_is_an_empty_block_and_no_ruling_is_needed() {
    let bin = scratch("cand-zero");
    fake_aval(&bin, BRIEF_AVAL);
    let document =
        r#"{"format":"adr-lint.candidates/1","base":"b","head":"h","candidates":[],"overflow":{}}"#;
    fake_adr_lint(&bin, document);
    let repo = two_commit_repo("cand-zero-repo");
    let (out, brief) = brief_text(&bin, &repo);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let sha =
        amont_agent_guard::sha256::hex(&amont_agent_guard::sha256::sha256(document.as_bytes()));
    assert!(
        brief.contains(&format!(
            "<<<CANDIDATES sha={sha} n=0>>>\n<<<END CANDIDATES>>>\n"
        )),
        "{brief}"
    );
    assert!(brief.contains("no ruling is needed"), "{brief}");
}

#[cfg(unix)]
#[test]
fn a_missing_adr_lint_leaves_the_brief_with_an_unavailable_marker() {
    let bin = scratch("cand-missing");
    fake_aval(&bin, BRIEF_AVAL);
    let repo = two_commit_repo("cand-missing-repo");
    let (out, brief) = brief_text(&bin, &repo);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(brief.contains("<<<CANDIDATES unavailable>>>"), "{brief}");
    assert!(brief.contains("## Candidates"), "{brief}");
    assert!(brief.contains("could not run"), "{brief}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.trim().lines().count(), 1, "{err}");
    assert!(err.contains("adr-lint"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_wrong_format_token_or_a_failing_adr_lint_is_unavailable_too() {
    let bin = scratch("cand-wrong");
    fake_aval(&bin, BRIEF_AVAL);
    fake_adr_lint(
        &bin,
        r#"{"format":"adr-lint.candidates/2","candidates":[]}"#,
    );
    let repo = two_commit_repo("cand-wrong-repo");
    let (out, brief) = brief_text(&bin, &repo);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(brief.contains("<<<CANDIDATES unavailable>>>"), "{brief}");
    assert!(brief.contains("candidates/2"), "{brief}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("candidates/2"),
        "{out:?}"
    );

    use std::os::unix::fs::PermissionsExt;
    let failing = bin.join("adr-lint");
    std::fs::write(&failing, "#!/bin/sh\necho broken >&2\nexit 4\n").unwrap();
    std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (out, brief) = brief_text(&bin, &repo);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(brief.contains("<<<CANDIDATES unavailable>>>"), "{brief}");
    assert!(brief.contains("broken"), "{brief}");
}

#[cfg(unix)]
#[test]
fn a_repository_without_a_corpus_gets_a_brief_without_rules() {
    let bin = scratch("no-corpus");
    fake_aval(&bin, "#!/bin/sh\nexit 1\n");
    fake_adr_lint(
        &bin,
        r#"{"format":"adr-lint.candidates/1","base":"b","head":"h","candidates":[],"overflow":{}}"#,
    );
    let repo = two_commit_repo_with("no-corpus-repo", &[]);
    let (out, brief) = brief_text(&bin, &repo);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(!brief.contains("<<<RULES"), "{brief}");
    assert!(brief.contains("# Code rules"), "{brief}");
    assert!(brief.contains("no aval corpus"), "{brief}");
    assert!(!brief.contains("# Constraint rules"), "{brief}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.trim().lines().count(), 1, "{err}");
    assert!(err.contains(".adr.yaml"), "{err}");
    let sha = run(&["rules-sha", "-C", repo.to_str().unwrap()], &bin);
    assert_eq!(sha.status.code(), Some(3), "{sha:?}");
}

#[cfg(unix)]
#[test]
fn an_aval_keys_row_without_a_string_key_is_an_error_naming_the_row() {
    let bin = scratch("keyless-row");
    fake_aval(
        &bin,
        &BRIEF_AVAL.replace(
            r#""--json relevant"*) printf '{"keys":[]}' ;;"#,
            r#""--json relevant"*) printf '{"keys":[{"key":"a.b"},{"score":1}]}' ;;"#,
        ),
    );
    let repo = two_commit_repo("keyless-row-repo");
    let (out, _) = brief_text(&bin, &repo);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("row 1") && err.contains("key"), "{err}");
}

/// Runs the binary with a PATH that holds only `dir` and a directory with a
/// symlink to `git`, so the machine's own `aval` is never found.
#[cfg(unix)]
fn run_without_aval(args: &[&str], dir: &std::path::Path, aval: Option<&str>) -> Output {
    let git_path = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let only = dir.join("only-git");
    std::fs::create_dir_all(&only).unwrap();
    let link = only.join("git");
    if !link.exists() {
        std::os::unix::fs::symlink(git_path.trim(), &link).unwrap();
    }
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_amont-agent"));
    cmd.args(args);
    cmd.env("PATH", std::env::join_paths([dir, &only]).unwrap());
    cmd.env("AMONT_AGENT_ADR_LINT", dir.join("adr-lint"));
    cmd.env_remove("AMONT_AGENT_AVAL");
    if let Some(program) = aval {
        cmd.env("AMONT_AGENT_AVAL", program);
    }
    cmd.output().expect("amont-agent runs")
}

#[cfg(unix)]
#[test]
fn without_aval_the_brief_says_the_rules_were_not_read_and_exits_0() {
    // No adr-lint either: its candidates are unavailable at once, so no stub
    // can outlast the 30 s deadline on a loaded machine.
    let bin = scratch("no-aval");
    let repo = two_commit_repo("no-aval-repo");
    let out = run_without_aval(
        &[
            "review-brief",
            "-C",
            repo.to_str().unwrap(),
            "--base",
            "HEAD~1",
        ],
        &bin,
        None,
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let brief = String::from_utf8_lossy(&out.stdout);
    assert!(!brief.contains("<<<RULES"), "{brief}");
    assert!(brief.contains("# Code rules"), "{brief}");
    assert!(
        brief.contains("aval is not installed, so its decided code rules were not read"),
        "{brief}"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    let aval_notices = err
        .lines()
        .filter(|line| line.contains("has .adr.yaml but aval is not installed"))
        .count();
    assert_eq!(aval_notices, 1, "{err}");
    assert_eq!(
        err.trim().lines().count(),
        2,
        "the aval and adr-lint notices: {err}"
    );
    let sha = run_without_aval(&["rules-sha", "-C", repo.to_str().unwrap()], &bin, None);
    assert_eq!(sha.status.code(), Some(3), "{sha:?}");
}

#[cfg(unix)]
#[test]
fn an_aval_that_points_nowhere_exits_3() {
    let bin = scratch("aval-nowhere");
    let repo = two_commit_repo("aval-nowhere-repo");
    let out = run_without_aval(
        &["review-brief", "-C", repo.to_str().unwrap()],
        &bin,
        Some("/nonexistent/aval"),
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
}

#[cfg(unix)]
#[test]
fn a_non_executable_aval_on_path_exits_3() {
    use std::os::unix::fs::PermissionsExt;
    let bin = scratch("aval-not-executable");
    let repo = two_commit_repo("aval-not-executable-repo");
    std::fs::write(bin.join("aval"), "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(bin.join("aval"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = run_without_aval(
        &[
            "review-brief",
            "-C",
            repo.to_str().unwrap(),
            "--base",
            "HEAD~1",
        ],
        &bin,
        None,
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
}

#[cfg(unix)]
#[test]
fn a_dangling_aval_symlink_on_path_exits_3() {
    let bin = scratch("aval-dangling");
    let repo = two_commit_repo("aval-dangling-repo");
    std::os::unix::fs::symlink("/nonexistent/target", bin.join("aval")).unwrap();
    let out = run_without_aval(
        &[
            "review-brief",
            "-C",
            repo.to_str().unwrap(),
            "--base",
            "HEAD~1",
        ],
        &bin,
        None,
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
}

#[cfg(unix)]
#[test]
fn an_aval_with_a_missing_interpreter_exits_3() {
    let bin = scratch("aval-bad-shebang");
    let repo = two_commit_repo("aval-bad-shebang-repo");
    fake_aval(&bin, "#!/nonexistent/sh\nexit 0\n");
    let out = run_without_aval(
        &[
            "review-brief",
            "-C",
            repo.to_str().unwrap(),
            "--base",
            "HEAD~1",
        ],
        &bin,
        None,
    );
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
}

/// An `aval` whose code rules carry `Reviewed by:` lines.
#[cfg(unix)]
const PROFILE_AVAL: &str = r#"#!/bin/sh
case "$*" in
  "--json rules --level constraint --governs code --full")
    printf '%s' '{"rules":[{"id":"cli.unix-only","level":"constraint","statement":"S1.","body":"UNIX-RULE-TEXT\n\n**Checked by:** judge.\n\n**Reviewed by:** unix, tui.","active":true},{"id":"errors.backend-only","level":"constraint","statement":"S2.","body":"BACKEND-RULE-TEXT\n\n**Checked by:** judge.\n\n**Reviewed by:** backend.","active":true},{"id":"types.mixed","level":"constraint","statement":"S3.","body":"MIXED-RULE-TEXT\n\n**Checked by:** exact (a lint), judge (the rest).\n\n**Reviewed by:** unix.","active":true},{"id":"errors.typed","level":"constraint","statement":"S4.","body":"LANGUAGE-RULE-TEXT\n\n**Checked by:** judge.\n\n**Reviewed by:** language.","active":true}]}' ;;
  "rules --level constraint") printf 'constraint x\n' ;;
  "--json relevant"*) printf '{"keys":[]}' ;;
  *) exit 1 ;;
esac
"#;

#[cfg(unix)]
fn profile_brief(name: &str, profile: &str, aval: &str) -> (Output, String) {
    let bin = scratch(name);
    fake_aval(&bin, aval);
    let repo = two_commit_repo(&format!("{name}-repo"));
    let out = run(
        &[
            "review-brief",
            "-C",
            repo.to_str().unwrap(),
            "--base",
            "HEAD~1",
            "--profile",
            profile,
        ],
        &bin,
    );
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    (out, text)
}

#[cfg(unix)]
#[test]
fn a_unix_profile_holds_the_judge_rules_unix_reviews() {
    let (out, brief) = profile_brief("profile-unix", "unix", PROFILE_AVAL);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(brief.contains("## Judge rules for unix"), "{brief}");
    assert!(brief.contains("Profile: unix\n"), "{brief}");
    let judge = &brief[brief.find("## Judge rules for unix").unwrap()..];
    let judge = &judge[..judge.find("# Constraint rules").unwrap()];
    assert!(judge.contains("UNIX-RULE-TEXT"), "{judge}");
    assert!(!judge.contains("BACKEND-RULE-TEXT"), "{judge}");
    assert!(!judge.contains("MIXED-RULE-TEXT"), "{judge}");
    assert!(!judge.contains("LANGUAGE-RULE-TEXT"), "{judge}");
}

#[cfg(unix)]
#[test]
fn a_lang_profile_holds_the_language_rules() {
    let (out, brief) = profile_brief("profile-lang", "lang:rust", PROFILE_AVAL);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(brief.contains("## Judge rules for lang:rust"), "{brief}");
    let judge = &brief[brief.find("## Judge rules for lang:rust").unwrap()..];
    assert!(judge.contains("LANGUAGE-RULE-TEXT"), "{judge}");
}

#[cfg(unix)]
#[test]
fn an_unknown_reviewed_by_name_exits_1_and_an_unknown_profile_exits_2() {
    let aval = PROFILE_AVAL.replace("**Reviewed by:** backend.", "**Reviewed by:** wizard.");
    let (out, _) = profile_brief("profile-bad-name", "unix", &aval);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("errors.backend-only") && err.contains("wizard"),
        "{err}"
    );
    let (out, _) = profile_brief("profile-unknown", "wizard", PROFILE_AVAL);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

#[cfg(unix)]
#[test]
fn the_plan_section_carries_the_declared_code_review() {
    let bin = scratch("declared");
    fake_aval(&bin, BRIEF_AVAL);
    let repo = two_commit_repo("declared-repo");
    let name = repo.file_name().unwrap().to_str().unwrap();
    let plan = repo.join("plan.md");
    std::fs::write(
        &plan,
        format!("# P\n\n## Code review\n\n- {name}: none — docs only\n"),
    )
    .unwrap();
    let args = |plan: &PathBuf| {
        run(
            &[
                "review-brief",
                "-C",
                repo.to_str().unwrap(),
                "--base",
                "HEAD~1",
                "--plan",
                plan.to_str().unwrap(),
            ],
            &bin,
        )
    };
    let out = args(&plan);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let brief = String::from_utf8_lossy(&out.stdout);
    assert!(
        brief.contains(&format!(
            "Declared code review for {name}: none — docs only"
        )),
        "{brief}"
    );
    std::fs::write(&plan, "# P\n").unwrap();
    let brief = String::from_utf8_lossy(&args(&plan).stdout).to_string();
    assert!(
        brief.contains(&format!("Declared code review for {name}: not declared")),
        "{brief}"
    );
}

#[cfg(unix)]
#[test]
fn a_declaration_that_does_not_parse_is_said_not_hidden() {
    let bin = scratch("declared-bad");
    fake_aval(&bin, BRIEF_AVAL);
    let repo = two_commit_repo("declared-bad-repo");
    let name = repo.file_name().unwrap().to_str().unwrap();
    let plan = repo.join("plan.md");
    std::fs::write(
        &plan,
        format!("# P\n\n## Code review\n\n- {name}: wizard\n"),
    )
    .unwrap();
    let out = run(
        &[
            "review-brief",
            "-C",
            repo.to_str().unwrap(),
            "--base",
            "HEAD~1",
            "--plan",
            plan.to_str().unwrap(),
        ],
        &bin,
    );
    let brief = String::from_utf8_lossy(&out.stdout);
    assert!(
        brief.contains(&format!("Declared code review for {name}: unreadable ("))
            && brief.contains("wizard"),
        "{brief}"
    );
}

#[cfg(unix)]
#[test]
fn a_profile_without_a_corpus_says_it_has_no_rules() {
    let bin = scratch("profile-nocorpus");
    fake_aval(&bin, BRIEF_AVAL);
    let repo = two_commit_repo_with("profile-nocorpus-repo", &[]);
    let out = run(
        &[
            "review-brief",
            "-C",
            repo.to_str().unwrap(),
            "--base",
            "HEAD~1",
            "--profile",
            "unix",
        ],
        &bin,
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let brief = String::from_utf8_lossy(&out.stdout);
    assert!(
        brief.contains("## Judge rules for unix\n\nNone: this repository has no aval corpus."),
        "{brief}"
    );
}
