//! Resource limits, through the binary: huge cardinalities are computed, not
//! expanded; deep nesting and call chains end in `incomplete` or `unknown`,
//! never a crash or a hang. Every case: exit 0, valid JSON, under 2 s.

use std::time::Duration;

use shell_oracle::{amont_bin, analyze_raw, claim, parse_analysis, Analysis, Upper};

const LIMIT: Duration = Duration::from_secs(2);

fn run(dialect: &str, src: &str) -> Analysis {
    let bin = amont_bin();
    let r = analyze_raw(&bin, dialect, src);
    let head: String = src.chars().take(80).collect();
    assert!(
        r.status.success(),
        "--dialect {dialect} exited {} on {head:?}…: {}",
        r.status,
        r.stderr
    );
    assert!(
        r.elapsed < LIMIT,
        "--dialect {dialect} took {:?} on {head:?}…",
        r.elapsed
    );
    parse_analysis(&r.stdout).unwrap_or_else(|e| panic!("{head:?}…: {e}"))
}

/// Saturated, or a finite number too large to have been expanded.
fn huge(u: Upper) -> bool {
    match u {
        Upper::Saturated => true,
        Upper::Finite(n) => n >= 1_000_000,
        _ => false,
    }
}

#[test]
fn huge_cardinalities_are_computed_not_expanded() {
    let cases = [
        "for i in {1..999999999999}; do curl -s https://x.example/$i; done",
        "for i in {1..999999999999}; do for j in {1..999999999999}; do curl -s https://x.example/; done; done",
        "for i in $(seq 1 99999999999); do curl -s https://x.example/$i; done",
        "curl -s 'https://x.example/[1-99999999]'",
        "for i in {1..99999}; do curl -s 'https://x.example/[1-99999999]'; done",
    ];
    for d in ["bash", "zsh", "unknown"] {
        for src in cases {
            let a = run(d, src);
            let (_, up) = claim(&a, "x.example");
            assert!(huge(up), "--dialect {d}: upper {up} on {src}\n{}", a.raw);
        }
    }
}

#[test]
fn a_thousand_nested_subshells_are_incomplete_not_a_crash() {
    let src = format!(
        "{}curl -s https://x.example/{}",
        "( ".repeat(1000),
        " )".repeat(1000)
    );
    for d in ["bash", "zsh", "unknown"] {
        let a = run(d, &src);
        assert!(
            a.incomplete.is_some() || a.unknown > 0,
            "--dialect {d}: {}",
            a.raw
        );
    }
}

#[test]
fn a_forty_deep_doubling_call_chain_is_bounded() {
    let mut src = String::from("f0() { curl -s https://x.example/; }; ");
    for i in 1..40 {
        src.push_str(&format!("f{i}() {{ f{}; f{}; }}; ", i - 1, i - 1));
    }
    src.push_str("f39");
    for d in ["bash", "zsh", "unknown"] {
        let a = run(d, &src);
        assert!(
            a.incomplete.is_some() || a.unknown > 0,
            "--dialect {d}: {}",
            a.raw
        );
    }
}

#[test]
fn other_pathological_inputs_do_not_crash() {
    let cases: Vec<String> = vec![
        "{ ".repeat(2000) + &" }".repeat(2000),
        "$(".repeat(500) + &")".repeat(500),
        "if true; then ".repeat(500) + &"fi; ".repeat(500),
        "for i in 1; do ".repeat(500) + "curl -s https://x.example/; " + &"done; ".repeat(500),
        // Linux caps ONE argv string at 128 KiB (MAX_ARG_STRLEN); macOS
        // does not. 4000 × 28 bytes stays under it on every runner.
        "curl -s https://x.example/; ".repeat(4000),
        "a && ".repeat(3000) + "b",
        "echo ".to_string() + &"\"".repeat(10001),
        "f() { f; f; }; f".to_string(),
        "(".repeat(5000),
    ];
    for d in ["bash", "zsh", "unknown"] {
        for src in &cases {
            run(d, src);
        }
    }
}
