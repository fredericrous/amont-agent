//! Every name the tool says about itself, in one place (ADR-0064
//! `harness.bundle-names`, ADR-0068 `harness.renames`; plan "amont-agent ships
//! as greenpiste").
//!
//! Messages, usage text, error prefixes, `--version` lines, command hints and
//! the hook decision prefix are built from these, never from a literal: a
//! later name is one change here. The names test (`tests/names.rs`) fails on a
//! literal `amont-agent` in any other Rust string, except the data names the
//! tool still reads and writes (plan 3b-2): the `~/.claude/amont-agent/`
//! config directory, the `amont-agent-*` schema strings and temp-file
//! prefixes, and the `"tool":"amont-agent"` record field.
//!
//! In a format string, name the constant inline: `eprintln!("{BIN}: …")`.
//! For a `&'static str` built at compile time, use the [`bin!`] macro with
//! `concat!`.

/// The binary's name as a literal, for `concat!` in a `&'static str`.
#[doc(hidden)]
#[macro_export]
macro_rules! bin {
    () => {
        "greenpiste"
    };
}

/// The product's name, as it introduces itself.
pub const PRODUCT: &str = "greenpiste";

/// The binary.
pub const BIN: &str = bin!();

/// The binary's old name, still installed as an alias (a copy).
pub const ALIAS: &str = "amont-agent";

/// The one-line installer for Linux and macOS.
pub const INSTALLER_URL: &str =
    "https://raw.githubusercontent.com/greenharness/greenpiste/main/install/install.sh";

/// The Homebrew formula, tap-qualified.
pub const BREW_FORMULA: &str = "fredericrous/tap/greenpiste";

/// Whether `program` (a command word's program, with or without a directory
/// and a trailing `.exe`) is this tool under its name or its alias.
pub fn is_ours_program(program: &str) -> bool {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let name = name.strip_suffix(".exe").unwrap_or(name);
    name == BIN || name == ALIAS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_names_are_ours_with_a_directory_or_exe() {
        for program in [
            "greenpiste",
            "amont-agent",
            "/usr/local/bin/greenpiste",
            "/usr/local/bin/amont-agent",
            "greenpiste.exe",
            "C:\\bin\\amont-agent.exe",
        ] {
            assert!(is_ours_program(program), "{program}");
        }
    }

    #[test]
    fn other_programs_are_not_ours() {
        for program in [
            "",
            "git",
            "amont",
            "greenpiste-fleet",
            "my-greenpiste",
            "amont-agent2",
        ] {
            assert!(!is_ours_program(program), "{program}");
        }
    }
}
