//! Helpers shared by the test files that write fake programs.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// Writes `script` (its first line a `#!` line) to `path`, makes it
/// executable, and runs it once before returning.
///
/// A parallel test that forks while the file is still open for writing keeps
/// a write descriptor until it execs, and an exec of the script then fails
/// with ETXTBSY (raw OS error 26) on Linux. The first run retries that error
/// for up to 10 s, so the hook never meets it. The run is a priming run: a
/// guard line after the shebang makes the script exit 0 at once when
/// `FAKE_PRIME` is `1`, so a fake that counts its calls does not count it.
#[cfg(unix)] // scripts and mode bits; the fakes are #!/bin/sh
pub fn write_fake(path: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let (shebang, rest) = script
        .split_once('\n')
        .expect("a fake script starts with a shebang line");
    std::fs::write(
        path,
        format!("{shebang}\n[ \"$FAKE_PRIME\" = 1 ] && exit 0\n{rest}"),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let give_up = Instant::now() + Duration::from_secs(10);
    loop {
        match Command::new(path).env("FAKE_PRIME", "1").output() {
            Err(e) if e.raw_os_error() == Some(26) && Instant::now() < give_up => {
                std::thread::sleep(Duration::from_millis(20));
            }
            other => {
                other.expect("the fake runs");
                break;
            }
        }
    }
}
