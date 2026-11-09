//! The gate program under its two names. The tool is being renamed from
//! `amont` to `greengate`; until the rename ends, the guard reads both as the
//! same program and runs whichever is installed.

use std::ffi::OsStr;
use std::path::Path;

const OLD_GATE: &str = "amont";
const NEW_GATE: &str = "greengate";

/// Is this program name the gate (`amont` or `greengate`)?
pub fn is_gate_program(name: &str) -> bool {
    name == OLD_GATE || name == NEW_GATE
}

/// Does this text (a hook script, say) name the gate under either name?
pub fn names_gate(text: &str) -> bool {
    text.contains(OLD_GATE) || text.contains(NEW_GATE)
}

/// Is this program name the gate's fleet companion?
pub fn is_gate_fleet_program(name: &str) -> bool {
    name == "amont-fleet" || name == "greengate-fleet"
}

/// The binary to spawn: `greengate` when an executable of that name is on
/// PATH, else `amont`.
pub fn gate_binary() -> &'static str {
    let path = std::env::var_os("PATH");
    if is_executable_on(NEW_GATE, path.as_deref()) {
        NEW_GATE
    } else {
        OLD_GATE
    }
}

fn is_executable_on(name: &str, path: Option<&OsStr>) -> bool {
    path.is_some_and(|dirs| {
        std::env::split_paths(dirs).any(|dir| is_executable_file(&dir.join(name)))
    })
}

#[cfg(unix)]
fn is_executable_file(file: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(file).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(file: &Path) -> bool {
    file.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_names_are_the_gate() {
        assert!(is_gate_program("amont"));
        assert!(is_gate_program("greengate"));
        assert!(!is_gate_program("amont-agent"));
        assert!(is_gate_fleet_program("amont-fleet"));
        assert!(is_gate_fleet_program("greengate-fleet"));
        assert!(!is_gate_fleet_program("amont"));
    }

    #[cfg(unix)]
    #[test]
    fn greengate_wins_only_when_an_executable_is_on_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("amont-agent-gate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("greengate");
        let path = std::env::join_paths([&dir]).unwrap();
        assert!(!is_executable_on("greengate", Some(&path)));
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable_on("greengate", Some(&path)));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable_on("greengate", Some(&path)));
        assert!(!is_executable_on("greengate", None));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
