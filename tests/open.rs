//! `amont-agent open <url|path>`, end to end through the real binary.
//!
//! Unix only: the recording openers are shell scripts made executable with
//! a unix mode.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir =
            std::env::temp_dir().join(format!("amont-agent-open-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); // a stale scratch directory may not exist
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        // The binary reports its working directory as the OS spells it
        // (macOS: /private/var/...), so expectations use that spelling.
        Scratch(dir.canonicalize().unwrap())
    }

    fn with_opener(&self, template: &str) {
        let ok = Command::new("git")
            .args(["config", "--file"])
            .arg(self.0.join("gitconfig"))
            .args(["amont.agent.opener", template])
            .status()
            .unwrap();
        assert!(ok.success());
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_amont-agent"))
            .arg("open")
            .args(args)
            .current_dir(&self.0)
            .env("GIT_CONFIG_GLOBAL", self.0.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("AMONT_AGENT_OFF")
            .output()
            .expect("the binary runs")
    }

    /// A script opener that runs `body` with the page as `$1`.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.0.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// An opener that writes its one argument to `got`.
    fn recording_opener(&self) -> PathBuf {
        let got = self.0.join("got");
        let script = self.script(
            "rec.sh",
            &format!("printf %s \"$1\" > \"{}\"", got.display()),
        );
        self.with_opener(&format!("{} {{}}", script.display()));
        got
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0); // cleanup is best effort
    }
}

fn wait_for(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if !text.is_empty() {
                return text;
            }
        }
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_path_opens_as_an_absolute_file_url() {
    let s = Scratch::new("path");
    std::fs::write(s.0.join("docs/x.html"), "x").unwrap();
    let got = s.recording_opener();
    let out = s.run(&["docs/x.html"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
    let expected = format!("file://{}/docs/x.html", s.0.display());
    assert_eq!(wait_for(&got), expected);
}

#[test]
fn a_path_with_a_space_and_a_hash_is_one_percent_encoded_argument() {
    let s = Scratch::new("encoded");
    std::fs::write(s.0.join("docs/a b#c.html"), "x").unwrap();
    let got = s.recording_opener();
    let out = s.run(&["docs/a b#c.html"]);
    assert_eq!(out.status.code(), Some(0));
    let expected = format!("file://{}/docs/a%20b%23c.html", s.0.display());
    assert_eq!(wait_for(&got), expected);
}

#[test]
fn a_url_is_appended_when_the_template_has_no_placeholder() {
    let s = Scratch::new("append");
    let got = s.0.join("got");
    let script = s.script(
        "rec.sh",
        &format!("printf %s \"$1\" > \"{}\"", got.display()),
    );
    s.with_opener(&script.display().to_string());
    let out = s.run(&["http://localhost:3000/#/A"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(wait_for(&got), "http://localhost:3000/#/A");
}

#[test]
fn a_missing_file_exits_1_with_one_line() {
    let s = Scratch::new("missing");
    s.with_opener("true {}");
    let out = s.run(&["docs/none.html"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stderr).lines().count(), 1);
}

#[test]
fn no_argument_is_a_usage_error() {
    let s = Scratch::new("usage");
    let out = s.run(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage: amont-agent open <url|path>"));
}

#[test]
fn help_exits_0() {
    let s = Scratch::new("help");
    let out = s.run(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("usage: amont-agent open <url|path>"));
}

#[test]
fn an_opener_that_fails_fast_exits_1_naming_the_page_and_the_setting() {
    let s = Scratch::new("fails");
    // `ls` is a stock program that fails at once and says why on stderr; a
    // freshly written script can take seconds to first exec on macOS.
    s.with_opener("ls {}");
    let out = s.run(&["http://localhost:1/"]);
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert_eq!(said.lines().count(), 1, "{said}");
    assert!(said.contains("http://localhost:1/"), "{said}");
    assert!(
        said.contains("No such file") || said.contains("cannot access"),
        "{said}"
    );
    assert!(said.contains("amont.agent.opener"), "{said}");
}

#[test]
fn an_opener_that_is_not_found_exits_1() {
    let s = Scratch::new("notfound");
    s.with_opener("/nonexistent/opener {}");
    let out = s.run(&["http://localhost:1/"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("amont.agent.opener"));
}

#[test]
fn an_unknown_option_exits_2_with_the_option_and_the_usage() {
    let s = Scratch::new("bogus");
    let out = s.run(&["--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    let said = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines[0], "amont-agent: unknown option --bogus", "{said}");
    assert_eq!(lines[1], "usage: amont-agent open <url|path>", "{said}");
}

#[test]
fn an_option_looking_word_before_the_separator_exits_2() {
    let s = Scratch::new("optlike");
    s.with_opener("true {}");
    let out = s.run(&["--user-data-dir=/tmp/x://y", "--", "docs/x.html"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown option --user-data-dir"));
}

#[test]
fn a_separator_lets_a_dash_named_file_open() {
    let s = Scratch::new("dashfile");
    std::fs::write(s.0.join("-x.html"), "x").unwrap();
    let got = s.recording_opener();
    let out = s.run(&["--", "./-x.html"]);
    assert_eq!(out.status.code(), Some(0));
    let expected = format!("file://{}/./-x.html", s.0.display());
    assert_eq!(wait_for(&got), expected);
}
