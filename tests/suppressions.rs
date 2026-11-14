//! `amont-agent suppressions --staged` through the real binary, in scratch
//! git repositories with a scratch global config (never the real one).
//!
//! Unix only: the fixtures use `#!/bin/sh` hooks and a `git` shim.
#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_amont-agent");
const GIT_ENV: [&str; 12] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_PREFIX",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "AMONT_AGENT_OFF",
    "AMONT_INDEX_FILE",
    "AMONT_INDEX_GIT_DIR",
    "GREENGATE_INDEX_FILE",
    "GREENGATE_INDEX_GIT_DIR",
];
const BARE: &str = "x = 1  # noqa: E501\n";

struct Repo {
    dir: PathBuf,
    config: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir); // scratch space; a leftover is harmless
    }
}

fn scratch() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "amont-agent-suppressions-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn real_git() -> String {
    let out = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

impl Repo {
    fn new() -> Repo {
        Repo::named(OsStr::new("work"))
    }

    fn named(directory_name: &OsStr) -> Repo {
        let dir = scratch();
        let config = dir.join("global.gitconfig");
        fs::write(&config, "").unwrap();
        let repo = Repo {
            dir: dir.join(directory_name),
            config,
        };
        fs::create_dir_all(&repo.dir).unwrap();
        repo.git(&["init", "-q"]);
        repo
    }

    fn command(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.current_dir(&self.dir)
            .env("GIT_CONFIG_GLOBAL", &self.config)
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com");
        for key in GIT_ENV {
            cmd.env_remove(key);
        }
        cmd
    }

    fn git_output(&self, args: &[&str]) -> Output {
        self.command("git").args(args).output().unwrap()
    }

    fn git(&self, args: &[&str]) -> String {
        let out = self.git_output(args);
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    fn write(&self, path: &str, text: &str) {
        let full = self.dir.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }

    fn stage(&self, path: &str, text: &str) {
        self.write(path, text);
        self.git(&["add", "--", path]);
    }

    fn commit(&self, message: &str) {
        self.git(&["commit", "-q", "-m", message]);
    }

    fn gate(&self) -> Output {
        self.command(BIN)
            .args(["suppressions", "--staged"])
            .output()
            .unwrap()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn install_hook(&self) {
        let hook = self.dir.join(".git/hooks/pre-commit");
        fs::write(
            &hook,
            format!("#!/bin/sh\nexec '{BIN}' suppressions --staged\n"),
        )
        .unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A `git` that logs its arguments to `log` and runs the real one.
    fn passthrough_shim(&self) -> (PathBuf, PathBuf) {
        self.shim_intercepting("")
    }

    /// A `git` that logs its arguments and answers `missing` to every name of
    /// `cat-file --batch-check`.
    fn missing_names_shim(&self) -> (PathBuf, PathBuf) {
        self.shim_intercepting(
            "  *--batch-check*) while read l; do echo \"$l missing\"; done; exit 0;;\n",
        )
    }

    /// A `git` that logs its arguments and exits 0 at once on `cat-file`,
    /// reading none of the names it is sent.
    fn deaf_cat_file_shim(&self) -> (PathBuf, PathBuf) {
        self.shim_intercepting("  *cat-file*) exit 0;;\n")
    }

    /// A `git` shim whose `case` holds `arms`; anything they do not match
    /// runs the real git.
    fn shim_intercepting(&self, arms: &str) -> (PathBuf, PathBuf) {
        let bin = self.dir.parent().unwrap().join("shim");
        fs::create_dir_all(&bin).unwrap();
        let log = bin.join("calls.log");
        let script = format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in\n{arms}esac\nexec '{}' \"$@\"\n",
            log.display(),
            real_git()
        );
        let path = bin.join("git");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        // Run it once so a parallel fork never meets it half-written (ETXTBSY).
        let _ = Command::new(&path).arg("--version").output(); // a priming run; its answer is not read
        (bin, log)
    }

    /// The gate with the shim directory first on `PATH`.
    fn gate_with_shim(&self, bin: &Path) -> Output {
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        self.command(BIN)
            .env("PATH", path)
            .args(["suppressions", "--staged"])
            .output()
            .unwrap()
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap()
}

#[test]
fn an_added_bare_marker_blocks_with_its_fix() {
    let repo = Repo::new();
    repo.stage("a.py", BARE);
    let out = repo.gate();
    assert_eq!(code(&out), 1, "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("a.py:1: "), "{stdout}");
    assert!(stdout.contains("add -- <why>"), "{stdout}");
    let stderr = text(&out.stderr);
    assert!(stderr.contains("fix the cause, or stop and tell the person"));
    assert!(stderr.contains("for the person only (an agent's attempt is refused):"));
    assert!(stderr.contains("git -c amont.agent.suppressions=allow commit"));
    assert!(!stdout.contains("for the person only"), "{stdout}");
}

#[test]
fn a_reasoned_marker_is_kept() {
    let repo = Repo::new();
    repo.stage("a.py", "x = 1  # noqa: E501 -- a long URL\n");
    let out = repo.gate();
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout).trim(),
        "suppressions: 1 files, 0 added (1 kept with a reason)"
    );
}

#[test]
fn a_marker_moved_between_files_adds_nothing() {
    let repo = Repo::new();
    repo.stage("a.py", BARE);
    repo.commit("base");
    repo.stage("a.py", "x = 1\n");
    repo.stage("b.py", BARE);
    let moved = repo.gate();
    assert_eq!(
        code(&moved),
        0,
        "{}{}",
        text(&moved.stdout),
        text(&moved.stderr)
    );

    repo.git(&["rm", "-q", "-f", "a.py"]);
    let deleted = repo.gate();
    assert_eq!(
        code(&deleted),
        0,
        "{}{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
}

#[test]
fn a_rename_into_an_examined_kind_is_judged_by_its_new_name() {
    let repo = Repo::new();
    repo.stage("x.txt", BARE);
    repo.commit("base");
    repo.git(&["mv", "x.txt", "x.py"]);
    let out = repo.gate();
    assert_eq!(code(&out), 1, "{}{}", text(&out.stdout), text(&out.stderr));
    assert!(text(&out.stdout).contains("x.py:1: "));

    let back = Repo::new();
    back.stage("y.py", BARE);
    back.commit("base");
    back.git(&["mv", "y.py", "y.txt"]);
    assert_eq!(code(&back.gate()), 0);
}

#[test]
fn an_unborn_head_has_an_empty_before() {
    let repo = Repo::new();
    repo.stage("a.py", BARE);
    assert_eq!(code(&repo.gate()), 1);
    repo.stage("a.py", "x = 1\n");
    assert_eq!(code(&repo.gate()), 0);
}

#[test]
fn the_staged_text_is_judged_not_the_working_tree() {
    let repo = Repo::new();
    repo.stage("a.py", BARE);
    repo.write("a.py", "x = 1\n");
    assert_eq!(code(&repo.gate()), 1);

    let other = Repo::new();
    other.stage("a.py", "x = 1\n");
    other.write("a.py", BARE);
    assert_eq!(code(&other.gate()), 0);
}

#[test]
fn commit_dash_a_and_commit_path_are_checked_through_the_hook() {
    let repo = Repo::new();
    repo.stage("a.py", "x = 1\n");
    repo.stage("b.py", "y = 1\n");
    repo.commit("base");
    repo.install_hook();
    let base = repo.head();

    repo.write("a.py", BARE);
    let all = repo.git_output(&["commit", "-q", "-a", "-m", "all"]);
    assert!(!all.status.success(), "commit -a was not blocked");
    assert!(
        text(&all.stderr).contains("a.py:1: "),
        "{}",
        text(&all.stderr)
    );
    assert_eq!(repo.head(), base);

    let only = repo.git_output(&["commit", "-q", "-m", "only", "a.py"]);
    assert!(!only.status.success(), "commit <path> was not blocked");
    assert!(text(&only.stderr).contains("a.py:1: "));
    assert_eq!(repo.head(), base);

    repo.write("a.py", "x = 1  # noqa: E501 -- why\n");
    let reasoned = repo.git_output(&["commit", "-q", "-a", "-m", "reasoned"]);
    assert!(reasoned.status.success(), "{}", text(&reasoned.stderr));
    assert_ne!(repo.head(), base);
}

#[test]
fn a_name_git_reports_missing_is_exit_two() {
    let repo = Repo::new();
    repo.stage("a.py", "x = 1\n");
    repo.commit("base");
    repo.stage("a.py", "x = 2\n");
    let (bin, _) = repo.missing_names_shim();
    let out = repo.gate_with_shim(&bin);
    assert_eq!(code(&out), 2, "{}{}", text(&out.stdout), text(&out.stderr));
    assert!(
        text(&out.stderr).contains("missing"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn under_observe_a_scan_error_prints_its_reason_and_exits_zero() {
    let repo = Repo::new();
    repo.stage("a.py", "x = 1\n");
    repo.commit("base");
    repo.stage("a.py", "x = 2\n");
    repo.git(&[
        "config",
        "--file",
        repo.config.to_str().unwrap(),
        "amont.agent.lint-suppression-added.stance",
        "observe",
    ]);
    let (bin, _) = repo.missing_names_shim();
    let out = repo.gate_with_shim(&bin);
    assert_eq!(code(&out), 0, "{}{}", text(&out.stdout), text(&out.stderr));
    assert!(
        text(&out.stderr).contains("missing"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(text(&out.stdout), "");
}

#[test]
fn a_git_that_exits_at_once_on_cat_file_is_exit_two_not_a_signal() {
    let repo = Repo::new();
    for n in 0..700 {
        repo.write(&format!("d/{}_{n}.py", "n".repeat(100)), "x = 1\n");
    }
    repo.git(&["add", "."]);
    let (bin, _) = repo.deaf_cat_file_shim();
    let out = repo.gate_with_shim(&bin);
    assert_eq!(code(&out), 2, "{}{}", text(&out.stdout), text(&out.stderr));
    assert!(
        text(&out.stderr).contains("incomplete"),
        "{}",
        text(&out.stderr)
    );
}

fn padded(first: &str, bytes: usize) -> String {
    let mut body = String::from(first);
    let filler = "const filler = 1;\n";
    while body.len() < bytes {
        body.push_str(filler);
    }
    body.truncate(bytes);
    body
}

#[test]
fn a_fifty_mebibyte_staged_script_is_not_read() {
    let repo = Repo::new();
    repo.stage(
        "big.js",
        &padded("// eslint-disable-next-line no-undef\n", 50 * 1024 * 1024),
    );
    let (bin, log) = repo.passthrough_shim();
    let out = repo.gate_with_shim(&bin);
    assert_eq!(code(&out), 0, "{}{}", text(&out.stdout), text(&out.stderr));
    assert!(
        text(&out.stderr).contains("big.js"),
        "{}",
        text(&out.stderr)
    );
    assert!(text(&out.stdout)
        .trim()
        .ends_with(", 1 skipped (over 4 MiB)"));
    let calls = fs::read_to_string(log).unwrap();
    assert!(
        !calls.lines().any(|l| l.trim() == "cat-file --batch"),
        "content was read:\n{calls}"
    );
}

fn stage_many(repo: &Repo, files: usize) {
    for i in 0..files {
        repo.write(
            &format!("f{i}.js"),
            &padded(&format!("// file {i}\n"), 4 * 1024 * 1024),
        );
    }
    repo.git(&["add", "."]);
}

#[test]
fn the_cumulative_budget_is_checked_before_any_content_read() {
    let repo = Repo::new();
    stage_many(&repo, 16);
    let within = repo.gate();
    assert_eq!(code(&within), 0, "{}", text(&within.stderr));

    stage_many(&repo, 17);
    let (bin, log) = repo.passthrough_shim();
    let over = repo.gate_with_shim(&bin);
    assert_eq!(code(&over), 2, "{}", text(&over.stderr));
    assert!(
        text(&over.stderr).contains("scan budget exceeded (64 MiB); scan incomplete"),
        "{}",
        text(&over.stderr)
    );
    let calls = fs::read_to_string(log).unwrap();
    assert!(calls.contains("--batch-check"));
    assert!(
        !calls.lines().any(|l| l.trim() == "cat-file --batch"),
        "content was read:\n{calls}"
    );
}

#[test]
fn every_blocking_hit_is_printed() {
    let repo = Repo::new();
    let lines: String = (1..=5)
        .map(|n| format!("x{n} = 1  # noqa: E50{n}\n"))
        .collect();
    repo.stage("a.py", &lines);
    let out = repo.gate();
    assert_eq!(code(&out), 1);
    let stdout = text(&out.stdout);
    for n in 1..=5 {
        assert!(stdout.contains(&format!("a.py:{n}: ")), "{stdout}");
    }
}

#[test]
fn only_a_command_scoped_allow_passes() {
    let repo = Repo::new();
    repo.stage("a.py", "x = 1\n");
    repo.commit("base");
    repo.install_hook();
    repo.git(&["config", "--local", "amont.agent.suppressions", "allow"]);
    repo.write("a.py", BARE);
    repo.git(&["add", "a.py"]);
    let base = repo.head();

    let plain = repo.git_output(&["commit", "-q", "-m", "plain"]);
    assert!(!plain.status.success(), "a repo-config allow must not pass");
    assert!(
        text(&plain.stderr).contains("amont.agent.suppressions=allow in local config is ignored"),
        "{}",
        text(&plain.stderr)
    );
    assert_eq!(repo.head(), base);

    let scoped = repo.git_output(&[
        "-c",
        "amont.agent.suppressions=allow",
        "commit",
        "-q",
        "-m",
        "ok",
    ]);
    assert!(
        scoped.status.success(),
        "{}{}",
        text(&scoped.stdout),
        text(&scoped.stderr)
    );
    assert_ne!(repo.head(), base);
}

#[test]
fn an_extensionless_shell_script_is_judged_by_its_shebang() {
    let repo = Repo::new();
    repo.stage(
        "bin/run",
        "#!/bin/sh\n# shellcheck disable=SC2086\necho $1\n",
    );
    let out = repo.gate();
    assert_eq!(code(&out), 1, "{}{}", text(&out.stdout), text(&out.stderr));
    assert!(text(&out.stdout).contains("bin/run:2: "));

    let reasoned = Repo::new();
    reasoned.stage(
        "bin/run",
        "#!/bin/sh\n# shellcheck disable=SC2086 # word splitting is wanted\necho $1\n",
    );
    assert_eq!(code(&reasoned.gate()), 0);
}

#[test]
fn the_stance_decides_the_exit_status() {
    let repo = Repo::new();
    repo.stage("a.py", BARE);
    let key = "amont.agent.lint-suppression-added.stance";

    assert_eq!(code(&repo.gate()), 1, "advise is the default");

    repo.git(&[
        "config",
        "--file",
        repo.config.to_str().unwrap(),
        key,
        "observe",
    ]);
    let observe = repo.gate();
    assert_eq!(code(&observe), 0);
    assert!(
        text(&observe.stdout).contains("a.py:1: "),
        "observe still prints"
    );

    repo.git(&[
        "config",
        "--file",
        repo.config.to_str().unwrap(),
        key,
        "advise",
    ]);
    assert_eq!(code(&repo.gate()), 1);

    repo.git(&[
        "config",
        "--file",
        repo.config.to_str().unwrap(),
        key,
        "deny",
    ]);
    assert_eq!(code(&repo.gate()), 1);
}

#[test]
fn a_usage_error_is_exit_two() {
    let repo = Repo::new();
    let out = repo.command(BIN).arg("suppressions").output().unwrap();
    assert_eq!(code(&out), 2);
    let misspelled = repo
        .command(BIN)
        .args(["suppressions", "--stagd"])
        .output()
        .unwrap();
    assert_eq!(code(&misspelled), 2);
    assert!(
        text(&misspelled.stderr).contains("unknown argument '--stagd'"),
        "{}",
        text(&misspelled.stderr)
    );
    let help = repo
        .command(BIN)
        .args(["suppressions", "--help"])
        .output()
        .unwrap();
    assert_eq!(code(&help), 0);
}

#[test]
fn a_repeated_staged_flag_behaves_as_one() {
    let repo = Repo::new();
    repo.stage("a.py", "x = 1  # noqa: E501 -- a long URL\n");
    let clean = repo
        .command(BIN)
        .args(["suppressions", "--staged", "--staged"])
        .output()
        .unwrap();
    assert_eq!(code(&clean), 0, "{}", text(&clean.stderr));
    repo.stage("b.py", BARE);
    let blocked = repo
        .command(BIN)
        .args(["suppressions", "--staged", "--staged", "--staged"])
        .output()
        .unwrap();
    assert_eq!(code(&blocked), 1, "{}", text(&blocked.stderr));
    assert!(
        text(&blocked.stdout).contains("b.py:1: "),
        "{}",
        text(&blocked.stdout)
    );
}

#[test]
fn the_pack_declares_the_one_gate() {
    let pack =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("greengate.pack")).unwrap();
    let lines: Vec<&str> = pack
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .collect();
    assert_eq!(
        lines,
        [format!(
            "pre-commit  suppressions  *  block  {} suppressions --staged",
            amont_agent_guard::names::ALIAS
        )]
    );
}

/// A repository whose ordinary index and HEAD are clean, and an alternate
/// index (absolute path) staging a bare suppression in `a.py`.
fn repo_with_alternate_index() -> (Repo, PathBuf, String) {
    with_alternate_index(Repo::new())
}

fn with_alternate_index(repo: Repo) -> (Repo, PathBuf, String) {
    repo.stage("a.py", "x = 1\n");
    repo.commit("base");
    let alternate = repo.dir.parent().unwrap().join("alternate.index");
    fs::copy(repo.dir.join(".git/index"), &alternate).unwrap();
    repo.write("a.py", BARE);
    let added = repo
        .command("git")
        .args(["add", "--", "a.py"])
        .env("GIT_INDEX_FILE", &alternate)
        .output()
        .unwrap();
    assert!(added.status.success(), "{}", text(&added.stderr));
    repo.git(&["checkout", "-q", "--", "a.py"]);
    let head = repo.head();
    (repo, alternate, head)
}

#[test]
fn amont_index_file_selects_the_alternate_index_over_git_index_file() {
    let (repo, alternate, head) = repo_with_alternate_index();
    let ordinary = repo.dir.join(".git/index");
    assert_eq!(code(&repo.gate()), 0, "ordinary index must be clean");

    let out = repo
        .command(BIN)
        .args(["suppressions", "--staged"])
        .env("AMONT_INDEX_FILE", &alternate)
        .env("AMONT_INDEX_GIT_DIR", repo.dir.join(".git"))
        .env("GIT_INDEX_FILE", &ordinary)
        .output()
        .unwrap();
    assert_eq!(code(&out), 1, "{}{}", text(&out.stdout), text(&out.stderr));
    assert!(
        text(&out.stdout).contains("a.py:1: "),
        "{}",
        text(&out.stderr)
    );

    assert_eq!(code(&repo.gate()), 0, "ordinary index still clean");
    assert_eq!(repo.head(), head);
}

#[test]
fn greengate_index_file_selects_the_alternate_index_and_wins_over_amont() {
    let (repo, alternate, _) = repo_with_alternate_index();
    let ordinary = repo.dir.join(".git/index");

    let out = repo
        .command(BIN)
        .args(["suppressions", "--staged"])
        .env("GREENGATE_INDEX_FILE", &alternate)
        .env("GREENGATE_INDEX_GIT_DIR", repo.dir.join(".git"))
        .env("GIT_INDEX_FILE", &ordinary)
        .output()
        .unwrap();
    assert_eq!(code(&out), 1, "{}{}", text(&out.stdout), text(&out.stderr));
    assert!(
        text(&out.stdout).contains("a.py:1: "),
        "{}",
        text(&out.stderr)
    );

    // The new pair wins: an old pair naming the clean index is not read.
    let both = repo
        .command(BIN)
        .args(["suppressions", "--staged"])
        .env("GREENGATE_INDEX_FILE", &alternate)
        .env("GREENGATE_INDEX_GIT_DIR", repo.dir.join(".git"))
        .env("AMONT_INDEX_FILE", &ordinary)
        .env("AMONT_INDEX_GIT_DIR", repo.dir.join(".git"))
        .env("GIT_INDEX_FILE", &ordinary)
        .output()
        .unwrap();
    assert_eq!(
        code(&both),
        1,
        "{}{}",
        text(&both.stdout),
        text(&both.stderr)
    );
}

#[test]
fn unset_and_empty_amont_index_file_keep_the_git_index_file_fallback() {
    let (repo, alternate, _) = repo_with_alternate_index();
    let unset = repo
        .command(BIN)
        .args(["suppressions", "--staged"])
        .env("GIT_INDEX_FILE", &alternate)
        .output()
        .unwrap();
    assert_eq!(code(&unset), 1, "{}", text(&unset.stderr));
    assert!(text(&unset.stdout).contains("a.py:1: "));

    let empty = repo
        .command(BIN)
        .args(["suppressions", "--staged"])
        .env("AMONT_INDEX_FILE", "")
        .env("GIT_INDEX_FILE", &alternate)
        .output()
        .unwrap();
    assert_eq!(code(&empty), 1, "{}", text(&empty.stderr));
    assert!(text(&empty.stdout).contains("a.py:1: "));

    let ordinary_empty = repo
        .command(BIN)
        .args(["suppressions", "--staged"])
        .env("AMONT_INDEX_FILE", "")
        .output()
        .unwrap();
    assert_eq!(code(&ordinary_empty), 0);
}

#[test]
fn an_empty_amont_index_file_keeps_the_real_hook_commit_paths_working() {
    let repo = Repo::new();
    repo.stage("a.py", "x = 1\n");
    repo.commit("base");
    repo.install_hook();
    let base = repo.head();
    repo.write("a.py", BARE);
    let all = repo
        .command("git")
        .args(["commit", "-q", "-a", "-m", "all"])
        .env("AMONT_INDEX_FILE", "")
        .output()
        .unwrap();
    assert!(!all.status.success(), "commit -a was not blocked");
    assert!(
        text(&all.stderr).contains("a.py:1: "),
        "{}",
        text(&all.stderr)
    );
    let only = repo
        .command("git")
        .args(["commit", "-q", "-m", "only", "a.py"])
        .env("AMONT_INDEX_FILE", "")
        .output()
        .unwrap();
    assert!(!only.status.success(), "commit <path> was not blocked");
    assert_eq!(repo.head(), base);
}

#[test]
fn an_invalid_amont_index_file_fails_closed_without_scanning_the_ordinary_index() {
    let (repo, _, head) = repo_with_alternate_index();
    let missing = repo.dir.parent().unwrap().join("no-such.index");
    for value in [missing.as_os_str(), std::ffi::OsStr::new("relative.index")] {
        let out = repo
            .command(BIN)
            .args(["suppressions", "--staged"])
            .env("AMONT_INDEX_FILE", value)
            .env("AMONT_INDEX_GIT_DIR", repo.dir.join(".git"))
            .output()
            .unwrap();
        assert_eq!(code(&out), 2, "{}{}", text(&out.stdout), text(&out.stderr));
        assert!(
            text(&out.stderr).contains("AMONT_INDEX_FILE"),
            "{}",
            text(&out.stderr)
        );
    }
    assert_eq!(repo.head(), head);
}

#[test]
fn a_command_scoped_allow_passes_even_with_an_invalid_amont_index_file() {
    let (repo, _, head) = repo_with_alternate_index();
    let missing = repo.dir.parent().unwrap().join("no-such.index");
    let scoped = repo
        .command(BIN)
        .args(["suppressions", "--staged"])
        .env("AMONT_INDEX_FILE", &missing)
        .env("AMONT_INDEX_GIT_DIR", repo.dir.join(".git"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "amont.agent.suppressions")
        .env("GIT_CONFIG_VALUE_0", "allow")
        .output()
        .unwrap();
    assert_eq!(
        code(&scoped),
        0,
        "{}{}",
        text(&scoped.stdout),
        text(&scoped.stderr)
    );

    repo.git(&["config", "--local", "amont.agent.suppressions", "allow"]);
    let persistent = repo
        .command(BIN)
        .args(["suppressions", "--staged"])
        .env("AMONT_INDEX_FILE", &missing)
        .env("AMONT_INDEX_GIT_DIR", repo.dir.join(".git"))
        .output()
        .unwrap();
    assert_eq!(code(&persistent), 2, "{}", text(&persistent.stderr));
    assert_eq!(repo.head(), head);
}

/// The gate with an index hint and the owner it names; `GIT_INDEX_FILE` is
/// left to the caller.
fn hinted_gate(repo: &Repo, index: &OsStr, owner: &OsStr) -> Command {
    let mut cmd = repo.command(BIN);
    cmd.args(["suppressions", "--staged"])
        .env("AMONT_INDEX_FILE", index)
        .env("AMONT_INDEX_GIT_DIR", owner);
    cmd
}

/// A copy of the repository's current ordinary index, kept beside it.
fn index_copy(repo: &Repo, name: &str) -> PathBuf {
    let copy = repo.dir.parent().unwrap().join(name);
    fs::copy(repo.dir.join(".git/index"), &copy).unwrap();
    copy
}

fn ignored_hint_notes(out: &Output) -> usize {
    text(&out.stderr)
        .matches("AMONT_INDEX_FILE ignored")
        .count()
}

/// A committed `a.py` whose ordinary index then stages a bare suppression;
/// the clean index copy was taken before that.
fn repo_staging_bare_with_clean_copy() -> (Repo, PathBuf) {
    let repo = Repo::new();
    repo.stage("a.py", "x = 1\n");
    repo.commit("base");
    let clean = index_copy(&repo, "clean.index");
    repo.stage("a.py", BARE);
    (repo, clean)
}

#[test]
fn a_foreign_clean_hint_cannot_hide_this_repositorys_bare_suppression() {
    let (repo, clean) = repo_staging_bare_with_clean_copy();
    let (foreign, _, _) = repo_with_alternate_index();
    let out = hinted_gate(
        &repo,
        clean.as_os_str(),
        foreign.dir.join(".git").as_os_str(),
    )
    .output()
    .unwrap();
    assert_eq!(code(&out), 1, "{}{}", text(&out.stdout), text(&out.stderr));
    assert!(text(&out.stdout).contains("a.py:1: "));
    assert_eq!(ignored_hint_notes(&out), 1, "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).matches("a.py:1: ").count(), 1);
}

#[test]
fn a_clean_local_fixture_passes_under_a_foreign_bare_hint() {
    let repo = Repo::new();
    repo.stage("a.py", "x = 1\n");
    repo.commit("base");
    repo.stage("a.py", "x = 2\n");
    let (foreign, bare_alternate, _) = repo_with_alternate_index();
    let out = hinted_gate(
        &repo,
        bare_alternate.as_os_str(),
        foreign.dir.join(".git").as_os_str(),
    )
    .output()
    .unwrap();
    assert_eq!(code(&out), 0, "{}{}", text(&out.stdout), text(&out.stderr));
    assert_eq!(ignored_hint_notes(&out), 1);
    assert_eq!(text(&out.stdout).matches("a.py:1: ").count(), 0);
}

#[test]
fn an_unbound_or_mismatched_hint_keeps_the_direct_git_index_file_fallback() {
    let (repo, alternate, _) = repo_with_alternate_index();
    let clean = index_copy(&repo, "clean.index");
    let missing = repo.dir.parent().unwrap().join("no-such.git");
    let (foreign, _, _) = repo_with_alternate_index();
    let foreign_git_dir = foreign.dir.join(".git");
    let owners: [Option<&OsStr>; 5] = [
        None,
        Some(OsStr::new("")),
        Some(OsStr::new("relative/.git")),
        Some(missing.as_os_str()),
        Some(foreign_git_dir.as_os_str()),
    ];
    for owner in owners {
        let mut cmd = repo.command(BIN);
        cmd.args(["suppressions", "--staged"])
            .env("AMONT_INDEX_FILE", &clean)
            .env("GIT_INDEX_FILE", &alternate);
        if let Some(owner) = owner {
            cmd.env("AMONT_INDEX_GIT_DIR", owner);
        }
        let out = cmd.output().unwrap();
        assert_eq!(
            code(&out),
            1,
            "owner {owner:?}: {}{}",
            text(&out.stdout),
            text(&out.stderr)
        );
        assert!(text(&out.stdout).contains("a.py:1: "), "owner {owner:?}");
        assert_eq!(ignored_hint_notes(&out), 1, "owner {owner:?}");
        assert!(!text(&out.stdout).contains("AMONT_INDEX_FILE ignored"));
    }
}

#[test]
fn a_symlink_alias_of_the_owner_git_dir_is_the_same_identity() {
    let (repo, alternate, _) = repo_with_alternate_index();
    let alias = repo.dir.parent().unwrap().join("git-dir-alias");
    std::os::unix::fs::symlink(repo.dir.join(".git"), &alias).unwrap();
    let out = hinted_gate(&repo, alternate.as_os_str(), alias.as_os_str())
        .output()
        .unwrap();
    assert_eq!(code(&out), 1, "{}{}", text(&out.stdout), text(&out.stderr));
    assert_eq!(ignored_hint_notes(&out), 0);
}

#[test]
fn primary_and_linked_worktrees_sharing_a_common_dir_are_different_identities() {
    let repo = Repo::new();
    repo.stage("a.py", "x = 1\n");
    repo.commit("base");
    let clean = index_copy(&repo, "clean.index");
    let linked_dir = repo.dir.parent().unwrap().join("linked");
    repo.git(&["worktree", "add", "-q", linked_dir.to_str().unwrap()]);
    let linked = Repo {
        dir: linked_dir.clone(),
        config: repo.config.clone(),
    };
    linked.stage("a.py", BARE);
    let primary_git_dir = repo.dir.join(".git");
    let linked_git_dir = PathBuf::from(linked.git(&["rev-parse", "--absolute-git-dir"]).trim());
    assert_ne!(primary_git_dir.canonicalize().unwrap(), linked_git_dir);

    let primary_owner = hinted_gate(&linked, clean.as_os_str(), primary_git_dir.as_os_str())
        .output()
        .unwrap();
    assert_eq!(
        code(&primary_owner),
        1,
        "{}{}",
        text(&primary_owner.stdout),
        text(&primary_owner.stderr)
    );
    assert_eq!(ignored_hint_notes(&primary_owner), 1);

    let own_owner = hinted_gate(&linked, clean.as_os_str(), linked_git_dir.as_os_str())
        .output()
        .unwrap();
    assert_eq!(
        code(&own_owner),
        0,
        "{}{}",
        text(&own_owner.stdout),
        text(&own_owner.stderr)
    );
    assert_eq!(ignored_hint_notes(&own_owner), 0);
    std::mem::forget(linked); // the primary repository's scratch directory owns the cleanup
}

#[test]
fn a_nested_fixture_commit_under_the_inherited_outer_pair_is_judged_on_its_own_index() {
    let (outer, outer_alternate, _) = repo_with_alternate_index();
    let fixture = Repo::new();
    fixture.stage("a.py", "x = 1\n");
    fixture.commit("base");
    fixture.install_hook();
    let base = fixture.head();

    fixture.write("a.py", BARE);
    let mut blocked = fixture.command("git");
    blocked
        .args(["commit", "-q", "-a", "-m", "bare"])
        .env("AMONT_INDEX_FILE", &outer_alternate)
        .env("AMONT_INDEX_GIT_DIR", outer.dir.join(".git"));
    let blocked = blocked.output().unwrap();
    assert!(!blocked.status.success(), "{}", text(&blocked.stderr));
    assert!(text(&blocked.stderr).contains("a.py:1: "));
    assert_eq!(fixture.head(), base);

    fixture.write("a.py", "x = 2\n");
    let mut allowed = fixture.command("git");
    allowed
        .args(["commit", "-q", "-a", "-m", "clean"])
        .env("AMONT_INDEX_FILE", &outer_alternate)
        .env("AMONT_INDEX_GIT_DIR", outer.dir.join(".git"));
    let allowed = allowed.output().unwrap();
    assert!(allowed.status.success(), "{}", text(&allowed.stderr));
    assert_ne!(fixture.head(), base);
    assert_eq!(
        text(
            &fixture
                .command("git")
                .args(["rev-parse", "HEAD^"])
                .output()
                .unwrap()
                .stdout
        )
        .trim(),
        base
    );
}

#[test]
fn a_selected_index_lock_is_only_read() {
    let (repo, alternate, _) = repo_with_alternate_index();
    let lock = repo.dir.parent().unwrap().join("index.lock");
    fs::copy(&alternate, &lock).unwrap();
    let before = fs::read(&lock).unwrap();
    let out = hinted_gate(&repo, lock.as_os_str(), repo.dir.join(".git").as_os_str())
        .output()
        .unwrap();
    assert_eq!(code(&out), 1, "{}{}", text(&out.stdout), text(&out.stderr));
    assert_eq!(fs::read(&lock).unwrap(), before);
    assert!(lock.exists());
}

/// APFS refuses a name that is not UTF-8, so only Linux can hold this path.
#[cfg(target_os = "linux")]
#[test]
fn a_matching_owner_in_a_non_utf8_repository_path_is_honoured() {
    use std::os::unix::ffi::OsStrExt;
    let (repo, alternate, _) =
        with_alternate_index(Repo::named(OsStr::from_bytes(b"work-\xff-repo")));
    let owner = repo.dir.join(".git");
    assert!(owner.to_str().is_none());
    let out = hinted_gate(&repo, alternate.as_os_str(), owner.as_os_str())
        .output()
        .unwrap();
    assert_eq!(code(&out), 1, "{}{}", text(&out.stdout), text(&out.stderr));
    assert_eq!(ignored_hint_notes(&out), 0);
    assert!(!text(&out.stderr).contains('\u{fffd}'));
}

#[test]
fn a_failed_identity_query_is_exit_two_unless_scoped_allow_or_observe() {
    let (repo, alternate, _) = repo_with_alternate_index();
    let (bin, _) = repo.shim_intercepting("  *--absolute-git-dir*) echo boom >&2; exit 1;;\n");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let owner = repo.dir.join(".git");
    let plain = hinted_gate(&repo, alternate.as_os_str(), owner.as_os_str())
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(
        code(&plain),
        2,
        "{}{}",
        text(&plain.stdout),
        text(&plain.stderr)
    );
    assert!(
        text(&plain.stderr).contains("rev-parse"),
        "{}",
        text(&plain.stderr)
    );

    let scoped = hinted_gate(&repo, alternate.as_os_str(), owner.as_os_str())
        .env("PATH", &path)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "amont.agent.suppressions")
        .env("GIT_CONFIG_VALUE_0", "allow")
        .output()
        .unwrap();
    assert_eq!(
        code(&scoped),
        0,
        "{}{}",
        text(&scoped.stdout),
        text(&scoped.stderr)
    );

    repo.git(&[
        "config",
        "--file",
        repo.config.to_str().unwrap(),
        "amont.agent.lint-suppression-added.stance",
        "observe",
    ]);
    let observed = hinted_gate(&repo, alternate.as_os_str(), owner.as_os_str())
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(
        code(&observed),
        0,
        "{}{}",
        text(&observed.stdout),
        text(&observed.stderr)
    );
}
