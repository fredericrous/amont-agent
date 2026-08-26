//! Replacing `settings.json` without ever leaving it half-written.
//!
//! Vendored from `amont-runtime::hookfile`, which stages several files and
//! then swaps them as a group because installing hooks means writing five
//! shims that must land together. There is exactly one file here, so the
//! two-phase API collapses into [`write_atomic`].
//!
//! What is kept from the original, because each line is load-bearing:
//!
//!   * **A sibling temp file, not `/tmp`.** `fs::rename` across filesystems
//!     fails with `EXDEV`, and a home directory is a separate mount often
//!     enough — an encrypted volume, a network home, a container — that
//!     staging elsewhere would turn a rename that cannot fail into one that
//!     fails on exactly the machines hardest to reproduce.
//!   * **Remove the temp file before writing it.** A leftover from a killed
//!     run could be a symlink, and `fs::write` would follow it.
//!   * **Rename over the destination.** A symlinked `settings.json` is
//!     REPLACED rather than written through, and a crash mid-write cannot
//!     leave a truncated file where a working config was.
//!   * **The pid in the temp name**, so two runs racing in the same
//!     directory do not stage over each other, and a leading dot so it does
//!     not look like a config file to anyone reading the directory.
//!
//! One deliberate difference from the original: the mode. `hookfile::stage`
//! sets 0644 unconditionally, which is right for a hook shim in `.git/hooks`
//! and wrong for this file. `~/.claude/settings.json` can hold MCP server
//! environment blocks, and those hold credentials — so an existing file
//! keeps the mode it already had (never widened by our rewrite), and a file
//! we create ourselves starts at 0600.

use std::io;
use std::path::{Path, PathBuf};

/// Write `body` over `dest`, atomically.
pub fn write_atomic(dest: &Path, body: &str) -> io::Result<()> {
    let dir = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "settings.json".to_string());
    let tmp = dir.join(format!(".amont-agent-tmp-{}-{name}", std::process::id()));

    // A leftover from a killed run could be a symlink; `fs::write` follows.
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, body)?;

    if let Err(e) = carry_mode(dest, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    match std::fs::rename(&tmp, dest) {
        Ok(()) => Ok(()),
        Err(e) => {
            // The swap failed, so the temp file is litter in somebody's
            // config directory. The original's `Drop for Staged` did this;
            // one file needs no destructor to say it.
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Give `tmp` the mode `dest` already had, or 0600 if we are creating it.
///
/// Reading the mode off the DESTINATION rather than preserving whatever
/// `fs::write` produced under the umask: the point is that our rewrite never
/// changes who can read a file that already existed.
#[cfg(unix)]
fn carry_mode(dest: &Path, tmp: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = match std::fs::metadata(dest) {
        Ok(m) => m.permissions().mode() & 0o7777,
        Err(e) if e.kind() == io::ErrorKind::NotFound => 0o600,
        Err(e) => return Err(e),
    };
    std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn carry_mode(_dest: &Path, _tmp: &Path) -> io::Result<()> {
    // Windows has no mode bits to carry; the file inherits the directory's
    // ACL, which is what every other writer of this file gets too.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("amont-agent-atomic-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn writes_a_new_file_and_leaves_no_temp_behind() {
        let d = scratch("new");
        let dest = d.join("settings.json");
        write_atomic(&dest, "{\"a\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "{\"a\":1}");
        let leftovers: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn replaces_a_symlink_rather_than_writing_through_it() {
        let d = scratch("symlink");
        let real = d.join("real.json");
        std::fs::write(&real, "ORIGINAL").unwrap();
        let link = d.join("settings.json");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(not(unix))]
        std::fs::write(&link, "ORIGINAL").unwrap();

        write_atomic(&link, "REWRITTEN").unwrap();
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "REWRITTEN");
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_to_string(&real).unwrap(),
            "ORIGINAL",
            "the link's TARGET must be untouched"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The difference from `hookfile::stage`, asserted so it cannot drift
    /// back to an unconditional 0644.
    #[cfg(unix)]
    #[test]
    fn an_existing_files_mode_is_never_widened() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("mode");
        let dest = d.join("settings.json");
        std::fs::write(&dest, "{}").unwrap();
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic(&dest, "{\"a\":1}").unwrap();
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "rewriting must not widen 0600 to 0644");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_we_create_starts_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("fresh");
        let dest = d.join("settings.json");
        write_atomic(&dest, "{}").unwrap();
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a settings.json we create is ours alone");
        let _ = std::fs::remove_dir_all(&d);
    }
}
