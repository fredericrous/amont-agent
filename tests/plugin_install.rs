//! `install` and `uninstall` place the `clear-relay` plugin beside the hooks,
//! through the one settings plan.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

const ID: &str = "clear-relay@amont-agent";

struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let dir =
            std::env::temp_dir().join(format!("amont-agent-plugin-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir); // a home left by a killed run, if any
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Home(dir)
    }
    fn settings(&self) -> PathBuf {
        self.0.join("settings.json")
    }
    fn state(&self) -> PathBuf {
        self.0.join("xdg")
    }
    fn marketplace(&self) -> PathBuf {
        self.state().join("amont-agent").join("claude-marketplace")
    }
    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_greenpiste"))
            .args(args)
            .env("CLAUDE_CONFIG_DIR", &self.0)
            .env("XDG_STATE_HOME", self.state())
            .output()
            .expect("the binary runs");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
    fn json(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.settings()).expect("settings"))
            .expect("valid JSON")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0); // a leftover temp directory is harmless
    }
}

fn files_under(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("readable directory")
            .flat_map(|entry| {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    walk(&path, root)
                } else {
                    vec![path
                        .strip_prefix(root)
                        .expect("under root")
                        .to_string_lossy()
                        // `/` on every platform, as git and the embedded list spell it.
                        .replace('\\', "/")]
                }
            })
            .collect()
    }
    let mut files = walk(root, root);
    files.sort();
    files
}

/// The files git tracks under `dir`, relative to it. `claude plugin test`
/// leaves its scaffolding (`tsconfig.json`, `.claude-plugin/types/`) in the
/// source directory; it is not part of the plugin, so a walk of the
/// directory would count it.
fn tracked_files_under(dir: &Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .args(["ls-files", "-z", "--", "."])
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git ls-files failed");
    let mut files: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8(name.to_vec()).expect("UTF-8 path"))
        .collect();
    files.sort();
    files
}

#[test]
fn a_dry_run_prints_the_plan_and_writes_nothing() {
    let home = Home::new("dry");
    let (code, out, err) = home.run(&["install"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("extraKnownMarketplaces"), "{out}");
    assert!(out.contains("enabledPlugins"), "{out}");
    assert!(out.contains(ID), "{out}");
    assert!(!home.settings().exists());
    assert!(!home.state().exists());
}

#[test]
fn write_places_the_plugin_and_twice_gives_one_entry_each() {
    let home = Home::new("twice");
    for _ in 0..2 {
        let (code, _, err) = home.run(&["install", "--write"]);
        assert_eq!(code, 0, "{err}");
    }
    let settings = home.json();
    let markets = settings["extraKnownMarketplaces"]
        .as_object()
        .expect("markets");
    assert_eq!(markets.len(), 1);
    assert_eq!(
        markets["amont-agent"]["source"],
        json!({ "source": "directory", "path": home.marketplace().display().to_string() })
    );
    let enabled = settings["enabledPlugins"].as_object().expect("enabled");
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[ID], json!(true));
    assert_eq!(
        settings["pluginConfigs"][ID]["options"]["stateDir"],
        json!(home.state().join("amont-agent").display().to_string())
    );
    assert!(home
        .marketplace()
        .join(".claude-plugin/marketplace.json")
        .is_file());
}

#[test]
fn other_settings_keys_survive_install_and_uninstall() {
    let home = Home::new("survive");
    let original = json!({
        "model": "opus",
        "enabledPlugins": { "other@elsewhere": true },
        "extraKnownMarketplaces": {
            "elsewhere": { "source": { "source": "github", "repo": "a/b" } }
        }
    });
    std::fs::write(
        home.settings(),
        format!("{}\n", serde_json::to_string_pretty(&original).unwrap()),
    )
    .unwrap();

    let (code, _, err) = home.run(&["install", "--write"]);
    assert_eq!(code, 0, "{err}");
    let installed = home.json();
    assert_eq!(installed["model"], json!("opus"));
    assert_eq!(installed["enabledPlugins"]["other@elsewhere"], json!(true));
    assert_eq!(
        installed["extraKnownMarketplaces"]["elsewhere"]["source"]["repo"],
        json!("a/b")
    );

    let (code, _, err) = home.run(&["uninstall", "--write"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(home.json(), original);
    assert!(!home.marketplace().exists());
}

#[test]
fn uninstall_removes_the_file_and_the_directory_only_with_write() {
    let home = Home::new("remove");
    home.run(&["install", "--write"]);
    let (code, out, _) = home.run(&["uninstall"]);
    assert_eq!(code, 0);
    assert!(out.contains("would remove"), "{out}");
    assert!(home.marketplace().exists(), "a dry run removes nothing");
    assert!(home.settings().exists());

    let (code, _, err) = home.run(&["uninstall", "--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(!home.settings().exists());
    assert!(!home.marketplace().exists());
}

/// An uninstall that removed the settings file and then failed to remove the
/// plugin directory is finished by the next run.
#[test]
fn uninstall_with_no_settings_left_still_removes_the_directory() {
    let home = Home::new("orphan");
    let (code, _, err) = home.run(&["install", "--write"]);
    assert_eq!(code, 0, "{err}");
    std::fs::remove_file(home.settings()).expect("settings written by install");
    assert!(home.marketplace().exists());
    let (code, out, err) = home.run(&["uninstall", "--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(!home.marketplace().exists(), "{out}");
    assert!(err.contains("removed"), "{err}");
}

#[test]
fn the_embedded_files_are_the_files_under_the_plugin_directory() {
    let home = Home::new("list");
    let (code, _, err) = home.run(&["install", "--write"]);
    assert_eq!(code, 0, "{err}");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugin/clear-relay");
    let placed = home.marketplace().join("plugins/clear-relay");
    let tracked = tracked_files_under(&source);
    assert_eq!(files_under(&placed), tracked);
    for file in tracked {
        assert_eq!(
            std::fs::read_to_string(placed.join(&file)).unwrap(),
            std::fs::read_to_string(source.join(&file)).unwrap(),
            "{file}"
        );
    }
}
