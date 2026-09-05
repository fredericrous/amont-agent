//! Where a stance may come from — and, more to the point, where it may not.
//!
//! `amont-agent` refuses commands a coding agent is about to run. If the
//! repository that agent is standing in can lower a stance, the guard is one
//! `git config` away from being switched off by the very process it exists to
//! guard — and `graduate`/`demote`, which write `--global`, would be silently
//! outranked by a key nobody remembers setting.
//!
//! So the reader takes `--global` then `--system`, and these tests hold that
//! line from outside the binary, against real `git config` files.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let dir =
            std::env::temp_dir().join(format!("amont-agent-scope-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo")).expect("scratch dir");
        std::fs::write(dir.join("global"), "").expect("global config");
        std::fs::write(dir.join("system"), "").expect("system config");
        let f = Fixture { dir };
        f.git(&["init", "-q", "--template=", "."]);
        f
    }

    fn repo(&self) -> PathBuf {
        self.dir.join("repo")
    }

    /// git, pinned to the fixture's own three config files so nothing on the
    /// machine running the tests can reach in.
    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(self.repo())
            .env("GIT_CONFIG_GLOBAL", self.dir.join("global"))
            .env("GIT_CONFIG_SYSTEM", self.dir.join("system"))
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "fixture: git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn set(&self, scope: &str, key: &str, value: &str) {
        self.git(&["config", scope, key, value]);
    }

    /// The `now` column of `amont-agent status`, for one rule.
    fn stance_of(&self, rule: &str) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .arg("status")
            .current_dir(self.repo())
            .env("CLAUDE_CONFIG_DIR", self.dir.join("claude"))
            .env("GIT_CONFIG_GLOBAL", self.dir.join("global"))
            .env("GIT_CONFIG_SYSTEM", self.dir.join("system"))
            .env_remove("AMONT_AGENT_OFF")
            .output()
            .expect("the binary runs");
        assert!(
            out.status.success(),
            "status exited {:?}",
            out.status.code()
        );
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let line = text
            .lines()
            .find(|l| l.starts_with(rule))
            .unwrap_or_else(|| panic!("no line for {rule} in:\n{text}"));
        // `<rule…><ships as…><now…>  evidence`
        line.split_whitespace()
            .nth(2)
            .unwrap_or_default()
            .to_string()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The one that matters. A repository cannot demote a rule that would refuse
/// the command a model is about to run in it.
#[test]
fn a_repository_cannot_lower_a_stance() {
    let f = Fixture::new("local-demote");
    f.set("--local", "amont.agent.pipe-to-tail.stance", "observe");
    assert_eq!(
        f.stance_of("pipe-to-tail"),
        "deny",
        "a --local key moved the stance the binary enforces"
    );
}

/// …nor raise one. The direction is not the point: the machine decides.
#[test]
fn a_repository_cannot_raise_a_stance_either() {
    let f = Fixture::new("local-promote");
    f.set("--local", "amont.agent.git-add-broad.stance", "deny");
    assert_eq!(f.stance_of("git-add-broad"), "observe");
}

/// A repository cannot switch the whole guard off, which is the same hole
/// with one key instead of seven.
#[test]
fn a_repository_cannot_disable_the_guard() {
    let f = Fixture::new("local-enabled");
    f.set("--local", "amont.agent.enabled", "false");
    assert_eq!(f.stance_of("pipe-to-tail"), "deny");
}

/// The person still decides, from their own config — this is what `graduate`
/// and `demote` write, and it has to keep working.
#[test]
fn the_machines_own_config_still_decides() {
    let f = Fixture::new("global");
    f.set("--global", "amont.agent.pipe-to-tail.stance", "observe");
    assert_eq!(f.stance_of("pipe-to-tail"), "observe");

    let f = Fixture::new("global-off");
    f.set("--global", "amont.agent.enabled", "false");
    assert_eq!(f.stance_of("pipe-to-tail"), "observe");
}

/// `--system` answers when the user's own file is silent, and loses to it
/// when it is not — git's precedence, minus the scopes a repository owns.
#[test]
fn system_is_the_floor_and_global_outranks_it() {
    let f = Fixture::new("system");
    f.set("--system", "amont.agent.git-add-broad.stance", "advise");
    assert_eq!(f.stance_of("git-add-broad"), "advise");

    f.set("--global", "amont.agent.git-add-broad.stance", "observe");
    assert_eq!(f.stance_of("git-add-broad"), "observe");
}

/// The blanket floor works the same way, and a repository cannot set it.
#[test]
fn the_blanket_floor_is_the_machines_too() {
    let f = Fixture::new("floor");
    f.set("--local", "amont.agent.stance", "deny");
    assert_eq!(f.stance_of("git-add-broad"), "observe");

    f.set("--global", "amont.agent.stance", "advise");
    assert_eq!(f.stance_of("git-add-broad"), "advise");
}

/// `GIT_CONFIG_GLOBAL` says where the user's config LIVES; it does not say
/// which repository this is. Stripping it with the rest of `GIT_*` made every
/// stance in a relocated config invisible — and made this whole file
/// untestable.
#[test]
fn a_relocated_global_config_is_still_read() {
    let f = Fixture::new("relocated");
    f.set("--global", "amont.agent.no-verify.stance", "deny");
    assert_eq!(f.stance_of("no-verify"), "deny");
    assert!(
        Path::new(&f.dir.join("global")).exists(),
        "the fixture's global config is the one that answered"
    );
}
