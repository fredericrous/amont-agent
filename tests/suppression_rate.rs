//! `tools/suppression-rate.py --self-test`, run from here rather than from
//! `make check`: the CI mirror check would force a workflow change to run it
//! there. It classifies `tests/fixtures/suppressions.txt`, the file the Rust
//! unit test in `lint_suppression_added.rs` classifies too, so the script and
//! the rule cannot drift apart unseen.

use std::process::Command;

/// `python3`, else `python`; neither is a failure, not a skip: a test that
/// quietly does not run is the drift this one exists to catch.
fn interpreter() -> &'static str {
    for name in ["python3", "python"] {
        if Command::new(name).arg("--version").output().is_ok() {
            return name;
        }
    }
    panic!("neither python3 nor python is on PATH: the suppression-rate self-test cannot run");
}

#[test]
fn the_rate_script_agrees_with_the_shared_fixture() {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tools/suppression-rate.py");
    let out = Command::new(interpreter())
        .args([script, "--self-test"])
        .output()
        .expect("the interpreter runs");
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
