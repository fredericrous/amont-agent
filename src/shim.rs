//! The grep that is not grep.
//!
//! Claude Code's shell snapshot (`<config>/shell-snapshots/snapshot-*.sh`)
//! defines a `grep` function that runs ugrep from the claude binary itself,
//! with `--ignore-files`: a recursive search skips every path a `.gitignore`
//! names — `dist/`, `node_modules/`, build output — with exit 0 and not one
//! word, whether or not a `.git` directory is present. Naming the directory
//! explicitly searches it; `--no-ignore-files` and `command grep` search
//! everything. Measured over 32,710 calls: 3,128 recursive greps, and
//! `--no-ignore-files` used zero times.
//!
//! No command-level rule can catch this — the shape, a recursive grep rooted
//! at a directory, is one call in ten, and whether the answer lived in an
//! ignored subtree is unknowable before the search runs. So it is said once,
//! at session start, on the machines where the function exists.

use std::path::Path;

/// The line for the session notice, or `None` where no snapshot defines
/// the function (every test, and every machine that is not running Claude
/// Code's Bash tool).
pub fn notice() -> Option<String> {
    if crate::stance::switched_off() {
        return None;
    }
    let dir = crate::settings::config_dir()?.join("shell-snapshots");
    if !defines_ugrep_grep(&dir) {
        return None;
    }
    Some(String::from(
        "amont-agent/grep-shim: `grep` in this shell is Claude Code's function over ugrep with \
         `--ignore-files`, so a recursive grep skips every .gitignored path (dist/, \
         node_modules/, build output) silently — exit 0, no message. Name the directory \
         explicitly (`grep -rn X node_modules/pkg` does search it), pass `--no-ignore-files`, \
         or use `command grep`. `find` is bfs, a GNU superset, and needs nothing.",
    ))
}

fn defines_ugrep_grep(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        name.starts_with("snapshot-")
            && name.ends_with(".sh")
            && std::fs::read_to_string(e.path())
                .is_ok_and(|s| s.contains("function grep") && s.contains("ugrep"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_that_defines_the_function_is_recognised() {
        let dir = std::env::temp_dir().join(format!("amont-agent-shim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!defines_ugrep_grep(&dir));
        std::fs::write(
            dir.join("snapshot-zsh-1.sh"),
            "function grep {\n ARGV0=ugrep x\n}\n",
        )
        .unwrap();
        assert!(defines_ugrep_grep(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_directory_is_silence() {
        assert!(!defines_ugrep_grep(Path::new(
            "/nonexistent/amont-agent-shim"
        )));
    }
}
