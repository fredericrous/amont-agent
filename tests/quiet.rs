//! `amont-agent quiet`, driven through the real binary.
//!
//! The failure injections (`AMONT_AGENT_TEST_*`) exist only behind the
//! `test-hooks` cargo feature, so the tests that need them are compiled only
//! with it: `cargo test --features test-hooks --test quiet`.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_amont-agent");

/// The sha256 of `tests/fixtures/excerpt/MANIFEST.sha256`. amont carries a
/// copy of the fixtures and pins the same value; an edit to one copy only
/// fails that repository's test.
const MANIFEST_PIN: &str = "a00c12dd56c47d7368f00b6cc4853832cf600cfa04fd90882fcb16ba419d41fc";

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "amont-agent-quiet-{}-{}-{name}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn logs(home: &Path) -> PathBuf {
    home.join("amont-agent").join("logs")
}

fn active(home: &Path) -> PathBuf {
    home.join("active")
}

/// Entries as a watcher sees them: a dotfile is a writer's temp, renamed
/// away the next moment.
fn entries(dir: &Path) -> Vec<PathBuf> {
    files(dir, ".json")
        .into_iter()
        .filter(|p| {
            !p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect()
}

fn read_entry(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn plant(dir: &Path, name: &str, pid: u64, ended: Option<u64>) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let mut v = serde_json::json!({"v":1,"tool":"amont-agent","session":null,"check":"x",
        "repo":"/","log":"/x.log.tmp","pid":pid,"started":1});
    if let Some(e) = ended {
        v["exit"] = serde_json::json!(0);
        v["ended"] = serde_json::json!(e);
    }
    let p = dir.join(name);
    std::fs::write(&p, v.to_string()).unwrap();
    p
}

fn wait_for_entry(dir: &Path) -> PathBuf {
    for _ in 0..1200 {
        if let Some(p) = entries(dir).into_iter().next() {
            if std::fs::read_to_string(&p).is_ok_and(|t| t.contains("started")) {
                return p;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("no active entry in {}", dir.display());
}

fn quiet_cmd(home: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(BIN);
    c.arg("quiet")
        .args(args)
        .env("CLAUDE_CONFIG_DIR", home)
        .env("QUIET_ACTIVE_DIR", active(home))
        // A fixed session: an entry is written only inside one, and the
        // agent's own must never reach a test (or CI, which has none).
        .env("CLAUDE_CODE_SESSION_ID", "test-session")
        .env_remove("CI");
    c
}

fn quiet(home: &Path, args: &[&str]) -> Output {
    quiet_cmd(home, args).output().expect("run amont-agent")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    v.retain(|p| p.to_string_lossy().ends_with(ext));
    v.sort();
    v
}

fn full_log_path(stdout: &str) -> PathBuf {
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("full log: "))
        .unwrap_or_else(|| panic!("no `full log:` line in {stdout}"));
    PathBuf::from(line)
}

#[test]
fn a_failure_keeps_its_exit_code_and_a_private_log() {
    use std::os::unix::fs::PermissionsExt;
    let home = scratch("fail");
    let out = quiet(&home, &["--", "sh", "-c", "echo x; exit 3"]);
    assert_eq!(out.status.code(), Some(3));
    let stdout = text(&out.stdout);
    assert!(stdout.starts_with("exit 3 "), "{stdout}");
    let log = full_log_path(&stdout);
    assert!(std::fs::read_to_string(&log).unwrap().contains('x'));
    let mode = std::fs::metadata(&log).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let dir_mode = std::fs::metadata(logs(&home)).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);
    assert!(files(&logs(&home), ".tmp").is_empty());
}

#[test]
fn a_pass_is_one_line() {
    let home = scratch("pass");
    let out = quiet(&home, &["--", "sh", "-c", "echo hello; echo oops >&2"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = text(&out.stdout);
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(stdout.starts_with("ok "), "{stdout}");
    let path = stdout.split("log=").nth(1).unwrap().trim();
    let log = std::fs::read_to_string(path).unwrap();
    assert!(log.contains("hello") && log.contains("oops"));
    assert_eq!(text(&out.stderr), "", "a quick pass prints no running line");
}

#[test]
fn stdout_and_stderr_keep_their_order() {
    let home = scratch("order");
    let out = quiet(
        &home,
        &[
            "--",
            "sh",
            "-c",
            "echo one; echo two >&2; echo three; exit 1",
        ],
    );
    let log = std::fs::read_to_string(full_log_path(&text(&out.stdout))).unwrap();
    assert_eq!(log, "one\ntwo\nthree\n");
}

#[test]
fn a_signalled_child_is_128_plus_the_signal() {
    let home = scratch("term");
    let out = quiet(&home, &["--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.code(), Some(143));
}

#[test]
fn usage_errors_exit_2() {
    let home = scratch("usage");
    assert_eq!(quiet(&home, &["true"]).status.code(), Some(2));
    assert_eq!(quiet(&home, &[]).status.code(), Some(2));
    assert_eq!(quiet(&home, &["--"]).status.code(), Some(2));
    assert_eq!(quiet(&home, &["--max-bytes", "1k"]).status.code(), Some(2));
    let out = quiet(&home, &["--max-bytes", "0", "--", "true"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("usage:"));
    assert_eq!(
        quiet(&home, &["--max-bytes", "49k", "--", "true"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        quiet(&home, &["--max-bytes", "48k", "--", "true"])
            .status
            .code(),
        Some(0)
    );
}

#[test]
fn a_missing_command_is_127_and_an_unexecutable_one_126() {
    let home = scratch("missing");
    let out = quiet(&home, &["--", "definitely-not-a-command-xyz"]);
    assert_eq!(out.status.code(), Some(127));
    assert!(files(&logs(&home), ".tmp").is_empty());
    let out = quiet(&home, &["--", "/etc/passwd"]);
    assert_eq!(out.status.code(), Some(126));
}

#[test]
fn pruning_keeps_fifty_and_never_a_tmp() {
    let home = scratch("prune");
    let dir = logs(&home);
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..60 {
        std::fs::write(dir.join(format!("20200101T0000{i:02}.000Z-old.log")), "x").unwrap();
    }
    for i in 0..5 {
        std::fs::write(
            dir.join(format!("20190101T0000{i:02}.000Z-old.log.tmp")),
            "x",
        )
        .unwrap();
    }
    let out = quiet(&home, &["--", "true"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(files(&dir, ".log").len(), 50);
    assert_eq!(files(&dir, ".tmp").len(), 5);
    // The newest survived, the oldest did not.
    assert!(!dir.join("20200101T000000.000Z-old.log").exists());
}

#[test]
fn max_bytes_bounds_the_excerpt() {
    let home = scratch("bound");
    let script = "i=0; while [ $i -lt 3000 ]; do echo \"error: failure number $i\"; i=$((i+1)); done; exit 1";
    let out = quiet(&home, &["--max-bytes", "200", "--", "sh", "-c", script]);
    assert_eq!(out.status.code(), Some(1));
    let stdout = text(&out.stdout);
    assert!(stdout.len() < 1200, "{} bytes", stdout.len());
    assert!(stdout.contains("more lines, full log:"), "{stdout}");
}

#[test]
fn a_closed_stdout_changes_nothing() {
    let home = scratch("epipe");
    let mut child = quiet_cmd(&home, &["--", "sh", "-c", "sleep 0.4; echo error; exit 4"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(text(&out.stderr), "");
}

#[test]
fn sigint_is_forwarded_and_leaves_no_tmp() {
    let home = scratch("sigint");
    let child = quiet_cmd(&home, &["--", "sleep", "30"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Wait until the wrapper is ready rather than for a fixed time: macOS can
    // hold the first exec of a freshly built binary for seconds, and a SIGINT
    // that lands before `main` runs kills it with the default action. The
    // `.tmp` log is opened only after the handlers are installed.
    let ready_by = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while files(&logs(&home), ".tmp").is_empty() {
        assert!(
            std::time::Instant::now() < ready_by,
            "no .tmp log after 30 s"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let pid = child.id().to_string();
    assert!(Command::new("kill")
        .args(["-INT", &pid])
        .status()
        .unwrap()
        .success());
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(130));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("interrupted, full log: "), "{stdout}");
    let log = stdout
        .lines()
        .find_map(|l| l.strip_prefix("interrupted, full log: "))
        .unwrap();
    assert!(Path::new(log).exists());
    assert!(files(&logs(&home), ".tmp").is_empty());
}

// ------------------------------------------------------------ the fixtures

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/excerpt")
        .join(name)
}

/// `quiet -- sh -c 'cat <fixture>; exit <code>'`
fn replay(name: &str, code: u8) -> (Output, String) {
    let home = scratch(name);
    let script = format!("cat \"$1\"; exit {code}");
    let path = fixture(name);
    let out = quiet(
        &home,
        &["--", "sh", "-c", &script, "sh", path.to_str().unwrap()],
    );
    let stdout = text(&out.stdout);
    (out, stdout)
}

#[test]
fn a_failing_cargo_test_shows_the_whole_panic_and_no_build_noise() {
    let (out, stdout) = replay("cargo-test-fail.txt", 101);
    assert_eq!(out.status.code(), Some(101));
    for must in [
        "panicked at src/render.rs:88:9",
        "left: \"alpha\\nbeta\\ngamma\"",
        "right: \"alpha\\nBETA\\ngamma\"",
        "empty input must be refused,",
        "but the renderer returned 0 bytes",
        "and then kept going",
        "test result: FAILED. 2 passed; 2 failed",
        "full log: ",
    ] {
        assert!(stdout.contains(must), "missing {must:?} in\n{stdout}");
    }
    assert!(!stdout.contains("Compiling"), "{stdout}");
    assert!(!stdout.contains("parse::reads_header"), "{stdout}");
}

#[test]
fn the_other_runners_show_their_failure() {
    let (_, py) = replay("pytest-fail.txt", 1);
    assert!(py.contains("assert 0.30000000000000004 == 0.3"), "{py}");
    assert!(py.contains("1 failed, 2 passed"), "{py}");
    let (_, vi) = replay("vitest-fail.txt", 1);
    assert!(vi.contains("AssertionError: expected '$1,000.00'"), "{vi}");
    assert!(vi.contains("× formats currency"), "{vi}");
    let (_, es) = replay("eslint-fail.txt", 1);
    assert!(es.contains("'foo' is not defined"), "{es}");
    assert!(es.contains("4 problems (3 errors, 1 warning)"), "{es}");
}

#[test]
fn a_passing_run_that_says_zero_failed_is_one_ok_line() {
    let (out, stdout) = replay("pass.txt", 0);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(stdout.starts_with("ok "));
}

#[test]
fn ci_prints_the_whole_log_of_a_failure() {
    let home = scratch("ci");
    let big =
        "i=0; while [ $i -lt 2000 ]; do echo \"line $i of a long log\"; i=$((i+1)); done; exit 1";
    let out = quiet_cmd(&home, &["--", "sh", "-c", big])
        .env("CI", "true")
        .output()
        .unwrap();
    let stdout = text(&out.stdout);
    assert!(stdout.contains("line 0 of a long log"));
    assert!(stdout.contains("line 1999 of a long log"));
}

// ------------------------------------------------------------- the manifest

fn sha256_of(path: &Path) -> String {
    for argv in [["sha256sum"].as_slice(), ["shasum", "-a", "256"].as_slice()] {
        if let Ok(o) = Command::new(argv[0]).args(&argv[1..]).arg(path).output() {
            if o.status.success() {
                return text(&o.stdout)
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
            }
        }
    }
    panic!("neither sha256sum nor shasum is available");
}

#[test]
fn the_manifest_is_pinned_and_matches_the_fixtures() {
    let manifest = fixture("MANIFEST.sha256");
    assert_eq!(
        sha256_of(&manifest),
        MANIFEST_PIN,
        "MANIFEST.sha256 changed: update the pin here AND in amont's copy"
    );
    let body = std::fs::read_to_string(&manifest).unwrap();
    let mut listed = Vec::new();
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        let (hash, name) = line.split_once("  ").expect("`<sha>  <name>` lines");
        assert_eq!(
            sha256_of(&fixture(name)),
            hash,
            "{name} differs from the manifest"
        );
        listed.push(name.to_string());
    }
    let mut on_disk: Vec<String> = std::fs::read_dir(fixture(""))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "MANIFEST.sha256")
        .collect();
    on_disk.sort();
    listed.sort();
    assert_eq!(listed, on_disk, "every fixture is listed, and only those");
}

// --------------------------------------------------- failure injection only

#[cfg(feature = "test-hooks")]
mod injected {
    use super::*;

    #[test]
    fn a_mid_run_write_failure_keeps_the_status_and_the_excerpt() {
        let home = scratch("writefail");
        let script =
            "i=0; while [ $i -lt 200 ]; do echo \"error: line $i\"; i=$((i+1)); done; exit 7";
        let out = quiet_cmd(&home, &["--", "sh", "-c", script])
            .env("AMONT_AGENT_TEST_LOG_FAIL_AFTER", "100")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(7));
        let stdout = text(&out.stdout);
        assert!(stdout.starts_with("exit 7 "), "{stdout}");
        assert!(stdout.contains("error: line 199"), "{stdout}");
        assert!(stdout.contains("log not written: "), "{stdout}");
        assert!(files(&logs(&home), ".log").is_empty());
        assert!(files(&logs(&home), ".tmp").is_empty());
    }

    #[test]
    fn a_log_dir_that_cannot_be_created_still_runs_the_command() {
        let home = scratch("nodir");
        // `amont-agent` is a file, so `amont-agent/logs` cannot exist.
        std::fs::write(home.join("amont-agent"), "not a dir").unwrap();
        let out = quiet(&home, &["--", "sh", "-c", "echo boom; exit 3"]);
        assert_eq!(out.status.code(), Some(3));
        let stdout = text(&out.stdout);
        assert!(stdout.contains("boom"), "{stdout}");
        assert!(stdout.contains("log not written: "), "{stdout}");
    }

    #[test]
    fn a_rename_failure_names_the_tmp_it_left() {
        use std::os::unix::fs::PermissionsExt;
        let home = scratch("renamefail");
        let dir = logs(&home);
        // The child takes the directory's write bit away mid-run: the open
        // file keeps working, the rename cannot.
        let script = "chmod 555 \"$1\"; echo error: hi; exit 3";
        let out = quiet(
            &home,
            &["--", "sh", "-c", script, "sh", dir.to_str().unwrap()],
        );
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        assert_eq!(out.status.code(), Some(3));
        let stdout = text(&out.stdout);
        assert!(stdout.contains("log left at "), "{stdout}");
        assert_eq!(files(&dir, ".tmp").len(), 1);
    }

    #[test]
    fn fifty_megabytes_keep_the_accumulator_bounded() {
        let home = scratch("big");
        let script = "yes 'error: failure on a line that is not short at all, really' | head -c 50000000; exit 1";
        let out = quiet_cmd(&home, &["--", "sh", "-c", script])
            .env("AMONT_AGENT_TEST_REPORT_PEAK", "1")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        let peak: usize = text(&out.stderr)
            .lines()
            .find_map(|l| l.strip_prefix("accumulator peak "))
            .expect("peak line")
            .parse()
            .unwrap();
        assert!(peak <= 64 * 1024, "peak {peak}");
        let log = full_log_path(&text(&out.stdout));
        assert_eq!(std::fs::metadata(log).unwrap().len(), 50_000_000);
    }

    #[test]
    fn the_running_line_appears_only_after_the_delay() {
        let home = scratch("running");
        let out = quiet_cmd(&home, &["--", "sleep", "1"])
            .env("AMONT_AGENT_TEST_RUNNING_AFTER_MS", "200")
            .output()
            .unwrap();
        let stderr = text(&out.stderr);
        assert!(stderr.starts_with("running sleep, log "), "{stderr}");
        assert!(stderr.trim_end().ends_with(".log.tmp"), "{stderr}");
    }
}

#[test]
fn an_active_entry_lives_through_the_run_and_stays_after_it() {
    use std::os::unix::fs::PermissionsExt;
    let home = scratch("active");
    let mut child = quiet_cmd(&home, &["--", "sleep", "3"])
        .env("CLAUDE_CODE_SESSION_ID", "sess-1")
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    let dir = active(&home);
    let path = wait_for_entry(&dir);
    let v = read_entry(&path);
    assert_eq!(v["v"], 1);
    assert_eq!(v["tool"], "amont-agent");
    assert_eq!(v["session"], "sess-1");
    assert_eq!(v["check"], "sleep");
    assert!(v["repo"]
        .as_str()
        .is_some_and(|r| Path::new(r).is_absolute()));
    let log = v["log"].as_str().unwrap();
    assert!(
        Path::new(log).is_absolute() && log.ends_with(".log.tmp"),
        "{log}"
    );
    assert!(Path::new(log).exists());
    assert_eq!(v["pid"], child.id());
    assert!(v["started"].as_u64().unwrap() > 0);
    assert!(v.get("ended").is_none() && v.get("exit").is_none());
    assert_eq!(v.as_object().unwrap().len(), 8);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let dmode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(dmode, 0o700);
    assert!(child.wait().unwrap().success());
    let v = read_entry(&path);
    assert_eq!(v["exit"], 0);
    assert!(v["ended"].as_u64().unwrap() > 0);
    // The ended entry names the renamed log, so a watcher's last drain reads it.
    let done = v["log"].as_str().unwrap();
    assert!(done.ends_with(".log") && Path::new(done).exists(), "{done}");
}

#[test]
fn a_killed_writers_temp_file_is_swept_once_old() {
    let home = scratch("tmp-sweep");
    let dir = active(&home);
    std::fs::create_dir_all(&dir).unwrap();
    let stale = dir.join(".amont-agent-tmp-1-1-0.json");
    std::fs::write(&stale, "{").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let fresh = dir.join(".amont-agent-tmp-2-2-0.json");
    std::fs::write(&fresh, "{").unwrap();
    let out = quiet_cmd(&home, &["--", "true"]).output().unwrap();
    assert!(out.status.success());
    assert!(!stale.exists(), "an hour-old temp file is litter");
    assert!(fresh.exists(), "a fresh one may be a write in flight");
}

#[test]
fn a_signal_still_ends_the_entry() {
    let home = scratch("active-int");
    let mut child = quiet_cmd(&home, &["--", "sleep", "3"])
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn");
    let path = wait_for_entry(&active(&home));
    let _ = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status();
    child.wait().unwrap();
    let v = read_entry(&path);
    assert!(v["exit"].is_null() || v["exit"] == 130, "{v}");
    assert!(v["ended"].as_u64().is_some());
}

#[test]
fn the_next_run_sweeps_dead_and_old_entries() {
    let home = scratch("active-sweep");
    let dir = active(&home);
    let dead = plant(&dir, "999999999-0.json", 999_999_999, None);
    let old = plant(&dir, "1-1.json", 1, Some(now_ms() - 120_000));
    let fresh = plant(&dir, "1-2.json", 1, Some(now_ms() - 1_000));
    let out = quiet(&home, &["--", "true"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(!dead.exists());
    assert!(!old.exists());
    assert!(fresh.exists());
}

#[test]
fn an_unwritable_active_dir_changes_nothing() {
    let home = scratch("active-bad");
    let blocker = home.join("file");
    std::fs::write(&blocker, "x").unwrap();
    let mut c = quiet_cmd(&home, &["--", "sh", "-c", "exit 4"]);
    c.env("QUIET_ACTIVE_DIR", blocker.join("sub"));
    let out = c.output().unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(text(&out.stdout).starts_with("exit 4 "));
    let mut c = quiet_cmd(&home, &["--", "true"]);
    c.env("QUIET_ACTIVE_DIR", blocker.join("sub"));
    let out = c.output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).starts_with("ok "));
}

#[test]
fn no_session_writes_no_entry() {
    let home = scratch("active-nosession");
    let out = quiet_cmd(&home, &["--", "true"])
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .output()
        .unwrap();
    assert!(out.status.success());
    let n = std::fs::read_dir(active(&home)).map_or(0, |d| d.count());
    assert_eq!(n, 0, "an entry outside a session is never drawn");
}

#[test]
fn a_command_that_cannot_start_records_its_exit() {
    let home = scratch("active-127");
    let out = quiet(&home, &["--", "no-such-command-xyz"]);
    assert_eq!(out.status.code(), Some(127));
    // The log and the entry exist before the spawn is tried, so a command
    // that never ran still leaves one entry, ending with its real exit.
    let entries = entries(&active(&home));
    assert_eq!(entries.len(), 1, "{entries:?}");
    let v = read_entry(&entries[0]);
    assert_eq!(v["exit"], 127);
    assert!(v["ended"].as_u64().is_some());
}
