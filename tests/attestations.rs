//! The attestations sweep end to end: the `SessionStart` hook, the
//! `attestations` subcommand and the `doctor` line.
//!
//! Every test runs the binary against a scratch `CLAUDE_CONFIG_DIR`, `HOME`
//! and `AMONT_AGENT_PLAN_ROOT`; none of them touches the real `~/.claude`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

const DAY: u64 = 86_400;

struct World(PathBuf);

impl World {
    fn new(name: &str) -> World {
        let dir =
            std::env::temp_dir().join(format!("amont-agent-att-e2e-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for d in ["claude/amont-agent/attestations", "home", "root", "work"] {
            fs::create_dir_all(dir.join(d)).unwrap();
        }
        World(dir)
    }
    fn state(&self) -> PathBuf {
        self.0.join("claude/amont-agent")
    }
    fn store(&self) -> PathBuf {
        self.state().join("attestations")
    }
    fn journal(&self) -> String {
        fs::read_to_string(self.state().join("journal.log")).unwrap_or_default()
    }
    fn count(&self, outcome: &str) -> usize {
        self.journal()
            .lines()
            .filter(|l| l.split(' ').nth(4) == Some(outcome))
            .count()
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_amont-agent"));
        c.env("CLAUDE_CONFIG_DIR", self.0.join("claude"))
            .env("HOME", self.0.join("home"))
            .env("AMONT_AGENT_PLAN_ROOT", self.0.join("root"))
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("AMONT_AGENT_OFF")
            .env("XDG_CONFIG_HOME", self.0.join("home/.config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("NO_COLOR", "1");
        c
    }
    /// A directory whose every entry is `days` old.
    fn dir(&self, name: &str, days: u64) -> PathBuf {
        let d = self.store().join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("a.txt"), "12345").unwrap();
        age(&d.join("a.txt"), days * DAY);
        age(&d, days * DAY);
        d
    }
    fn plan(&self, rel: &str, text: &str) {
        let f = self.0.join("root").join(rel);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, text).unwrap();
    }
    fn session_start(&self) -> std::process::Child {
        let mut child = self
            .cmd()
            .arg("hook")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        use std::io::Write;
        let payload = format!(
            r#"{{"hook_event_name":"SessionStart","source":"startup","session_id":"sess1234","cwd":{:?}}}"#,
            self.0.join("work").to_string_lossy()
        );
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        child
    }
    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let o = self.cmd().args(args).output().unwrap();
        (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    }
    fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.store())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
    fn stamp(&self, secs_ago: u64) {
        let p = self.state().join("attestations.swept");
        fs::write(&p, "").unwrap();
        age(&p, secs_ago);
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A directory opened so that its times can be set: Windows needs
/// `FILE_FLAG_BACKUP_SEMANTICS` and write access to open one at all.
fn open_dir(path: &Path) -> fs::File {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        fs::OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .unwrap()
    }
    #[cfg(not(windows))]
    fs::File::open(path).unwrap()
}

fn age(path: &Path, secs: u64) {
    let t = SystemTime::now() - Duration::from_secs(secs);
    if path.is_dir() {
        open_dir(path).set_modified(t).unwrap();
    } else {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }
}

/// `SessionStart` sweeps in a detached child: wait, bounded, until a run has
/// journaled and released the lock, then a little longer so that a late child
/// would have had time to journal a second run.
fn settle(w: &World) {
    let lock = w.state().join("attestations.sweeping");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while w.count("attestations-kept") == 0 || lock.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "no sweep finished within 30 s: {}",
            w.journal()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
}

fn trash(w: &World) -> Vec<String> {
    w.names()
        .into_iter()
        .filter(|n| n.starts_with(".trash-"))
        .collect()
}

/// The scratch store of the plan's table: five directories, one cited.
fn table(w: &World) {
    w.dir("old", 40);
    w.dir("fresh", 2);
    w.dir("cited", 40);
    let pinned = w.dir("pinned", 40);
    fs::write(pinned.join(".keep"), "").unwrap();
    age(&pinned.join(".keep"), 40 * DAY);
    age(&pinned, 40 * DAY);
    let deep = w.dir("deep", 40);
    fs::write(deep.join("new.png"), "x").unwrap();
    age(&deep.join("new.png"), 2 * DAY);
    w.plan(
        "repo/docs/plans/p.md",
        "evidence: attestations/cited/guide.md\n",
    );
}

#[test]
fn session_start_sweeps_once_a_day() {
    let w = World::new("daily");
    w.dir("old", 40);
    assert!(w.session_start().wait().unwrap().success());
    settle(&w);
    assert_eq!(trash(&w).len(), 1, "{:?}", w.names());
    assert!(!w.names().contains(&"old".to_string()));
    assert_eq!(w.count("attestation-swept"), 1);
    assert_eq!(w.count("attestations-kept"), 1);

    w.dir("older", 40);
    assert!(w.session_start().wait().unwrap().success());
    std::thread::sleep(Duration::from_millis(500));
    assert!(w.names().contains(&"older".to_string()), "no second scan");
    assert_eq!(w.count("attestations-kept"), 1);
}

#[test]
fn concurrent_session_starts_sweep_once() {
    let w = World::new("concurrent");
    w.dir("old", 40);
    let mut kids: Vec<_> = (0..2).map(|_| w.session_start()).collect();
    for k in &mut kids {
        k.wait().unwrap();
    }
    settle(&w);
    assert_eq!(w.count("attestations-kept"), 1, "{}", w.journal());
}

#[test]
fn twenty_session_starts_on_a_stale_stamp_sweep_once() {
    let w = World::new("twenty");
    w.dir("old", 40);
    w.stamp(3 * DAY);
    let mut kids: Vec<_> = (0..20).map(|_| w.session_start()).collect();
    for k in &mut kids {
        k.wait().unwrap();
    }
    settle(&w);
    assert_eq!(w.count("attestations-kept"), 1, "{}", w.journal());
    assert_eq!(w.count("attestation-swept"), 1);
}

#[test]
fn twenty_session_starts_over_a_two_hour_lock_sweep_once() {
    // 50 rounds: the takeover race shows only now and then (plan review).
    for round in 0..50 {
        let w = World::new(&format!("stale-lock-{round}"));
        w.dir("old", 40);
        w.stamp(3 * DAY);
        let lock = w.state().join("attestations.sweeping");
        fs::write(&lock, "").unwrap();
        age(&lock, 7200);
        let mut kids: Vec<_> = (0..20).map(|_| w.session_start()).collect();
        for k in &mut kids {
            k.wait().unwrap();
        }
        settle(&w);
        assert_eq!(
            w.count("attestations-kept"),
            1,
            "round {round}: {}",
            w.journal()
        );
        assert!(!lock.exists());
        let stale = fs::read_dir(w.state())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains(".stale-"));
        assert!(!stale, "no .stale-<pid> left behind");
    }
}

#[test]
fn the_subcommand_lists_and_removes_nothing_by_default() {
    let w = World::new("dry");
    table(&w);
    let (code, out, err) = w.run(&["attestations"]);
    assert_eq!(code, 0, "{err}");
    let lines: Vec<Vec<&str>> = out.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(lines.len(), 5, "{out}");
    assert!(lines.iter().all(|l| l.len() == 5), "{out}");
    assert_eq!(lines.last().unwrap()[..2], ["remove", "old"]);
    assert!(lines
        .iter()
        // The path is shown with the platform's separator.
        .any(|l| l[1] == "cited"
            && l[4]
                .replace('\\', "/")
                .starts_with("cited by repo/docs/plans/p.md")));
    assert!(lines
        .iter()
        .any(|l| l[1] == "deep" && l[4].starts_with("recent")));
    assert!(lines.iter().any(|l| l[1] == "pinned" && l[4] == ".keep"));
    assert!(
        err.contains("nothing removed; amont-agent attestations --apply removes 1 ("),
        "{err}"
    );
    assert!(!out.contains('\u{1b}'));
    assert_eq!(w.names().len(), 5);
}

#[test]
fn apply_moves_to_the_trash() {
    let w = World::new("apply");
    table(&w);
    let (code, _, err) = w.run(&["attestations", "--apply"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(trash(&w).len(), 1);
    assert!(trash(&w)[0].ends_with("-old"));
    assert!(!w.names().contains(&"old".to_string()));
    assert_eq!(w.count("attestation-swept"), 1);
    assert!(w.journal().contains("attestation-swept"));
}

#[test]
fn a_scan_failure_exits_1_and_removes_nothing() {
    let w = World::new("scanfail");
    w.dir("old", 40);
    let gone = w.0.join("nowhere");
    let (code, out, err) = w
        .cmd()
        .env("AMONT_AGENT_PLAN_ROOT", &gone)
        .args(["attestations", "--apply"])
        .output()
        .map(|o| {
            (
                o.status.code().unwrap(),
                String::from_utf8_lossy(&o.stdout).into_owned(),
                String::from_utf8_lossy(&o.stderr).into_owned(),
            )
        })
        .unwrap();
    assert_eq!(code, 1);
    assert!(
        err.contains(&format!("amont-agent: {}:", gone.display())),
        "{err}"
    );
    assert!(err.contains("; no directory removed"), "{err}");
    assert!(out.contains("held (scan failed:"), "{out}");
    assert!(w.names().contains(&"old".to_string()));
    assert_eq!(w.count("attestations-scan-failed"), 1);
}

#[test]
fn an_unknown_flag_exits_2() {
    let w = World::new("flag");
    let (code, _, err) = w.run(&["attestations", "--force"]);
    assert_eq!(code, 2, "{err}");
}

#[test]
fn apply_refuses_while_a_sweep_is_running() {
    let w = World::new("held");
    w.dir("old", 40);
    let lock = w.state().join("attestations.sweeping");
    fs::write(&lock, "").unwrap();
    age(&lock, 600);
    let (code, _, err) = w.run(&["attestations", "--apply"]);
    assert_eq!(code, 1);
    assert!(err.contains("a sweep is running"), "{err}");
    assert!(w.names().contains(&"old".to_string()));
    assert!(lock.exists(), "someone else's lock is left alone");
}

#[test]
fn a_failed_removal_is_named_and_the_rest_proceed() {
    let w = World::new("partial");
    w.dir("aaa", 40);
    w.dir("bad", 40);
    // A non-empty directory where `bad` would be trashed makes its rename
    // fail; the trash name carries the second, so cover a window around it.
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for t in now.saturating_sub(5)..now + 60 {
        let blocker = w.store().join(format!(".trash-{t}-bad"));
        fs::create_dir_all(&blocker).unwrap();
        fs::write(blocker.join("x"), "x").unwrap();
    }
    let (code, _, err) = w.run(&["attestations", "--apply"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("✗ bad:"), "{err}");
    assert!(w.names().contains(&"bad".to_string()));
    assert!(!w.names().contains(&"aaa".to_string()));
}

// Windows refuses a control character in a file name; the unit test
// `rendering_is_tab_separated_when_piped_and_sanitised` covers it there.
#[cfg(unix)]
#[test]
fn a_name_with_an_escape_is_printed_sanitised() {
    let w = World::new("escape");
    w.dir("x\u{1b}[8my", 40);
    let (code, out, _) = w.run(&["attestations"]);
    assert_eq!(code, 0);
    assert!(out.contains("x\\x1b[8my"), "{out:?}");
    assert!(!out.contains('\u{1b}'));
}

#[test]
fn a_global_git_root_outside_the_plan_root_is_scanned() {
    let w = World::new("gitroot");
    w.dir("by-dots", 40);
    let dots = w.0.join("home/dots");
    fs::create_dir_all(dots.join("docs/plans")).unwrap();
    fs::write(dots.join("docs/plans/x.md"), "attestations/by-dots").unwrap();
    let st = Command::new("git")
        .args([
            "config",
            "--global",
            "amont.agent.plan-review.dots.path",
            "~/dots",
        ])
        .env("HOME", w.0.join("home"))
        .env("XDG_CONFIG_HOME", w.0.join("home/.config"))
        .env_remove("GIT_CONFIG_GLOBAL")
        .status()
        .unwrap();
    assert!(st.success());
    let (code, out, err) = w.run(&["attestations"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("keep\tby-dots\t") && out.contains("cited by"),
        "{out}"
    );
}

#[test]
#[cfg(unix)]
fn a_closed_pipe_is_quiet() {
    let w = World::new("pipe");
    table(&w);
    let err = w.0.join("stderr.txt");
    let script = format!(
        "'{}' attestations 2>'{}' | head -1",
        env!("CARGO_BIN_EXE_amont-agent"),
        err.display()
    );
    let mut sh = Command::new("sh");
    sh.arg("-c").arg(&script);
    let o = sh
        .env("CLAUDE_CONFIG_DIR", w.0.join("claude"))
        .env("HOME", w.0.join("home"))
        .env("XDG_CONFIG_HOME", w.0.join("home/.config"))
        .env("AMONT_AGENT_PLAN_ROOT", w.0.join("root"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    let e = fs::read_to_string(&err).unwrap();
    assert!(!e.contains("Broken pipe") && !e.contains("panicked"), "{e}");
}

#[test]
fn doctor_reports_the_store() {
    let w = World::new("doctor");
    table(&w);
    w.stamp(0);
    let (_, out, _) = w.run(&["doctor"]);
    let line = out
        .lines()
        .find(|l| l.contains("attestations:"))
        .expect(&out);
    assert!(line.contains("attestations: 5 directories, "), "{line}");
    assert!(line.contains("oldest 40d; 1 cited; swept "), "{line}");
    assert!(line.contains(" ago"), "{line}");
    assert!(!line.contains("  !"), "no warning: {line}");
}

#[test]
fn doctor_warns_on_a_failed_scan_and_on_a_stale_stamp() {
    let w = World::new("doctor-warn");
    w.dir("old", 40);
    w.stamp(3 * DAY);
    let (_, out, _) = w.run(&["doctor"]);
    let at = out
        .lines()
        .position(|l| l.contains("attestations:"))
        .expect(&out);
    assert!(out.lines().nth(at).unwrap().contains('!'), "{out}");
    assert!(out.contains("over 2 days"), "{out}");

    let w = World::new("doctor-fail");
    w.dir("old", 40);
    w.stamp(0);
    let gone = w.0.join("nowhere");
    let o = w
        .cmd()
        .env("AMONT_AGENT_PLAN_ROOT", &gone)
        .args(["attestations", "--apply"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    // The sweep journals through a session start as well; either way the
    // last run failed.
    let o = w
        .cmd()
        .env("AMONT_AGENT_PLAN_ROOT", &gone)
        .arg("doctor")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains(&gone.display().to_string()), "{out}");
    assert!(out.contains("`amont-agent attestations`"), "{out}");
}

#[test]
fn an_unset_plan_root_removes_nothing_and_says_so() {
    let w = World::new("unset-root");
    w.dir("old", 40);
    let o = w
        .cmd()
        .env_remove("AMONT_AGENT_PLAN_ROOT")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["attestations", "--apply"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(err.contains("unset"), "{err}");
    assert!(err.contains("AMONT_AGENT_PLAN_ROOT"), "{err}");
    assert!(w.names().contains(&"old".to_string()));
}
