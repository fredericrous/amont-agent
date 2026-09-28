//! The semantic check: run scripts under real bash and zsh with stubbed
//! clients, count the transfers per host, and require every count to lie
//! inside the interval `amont-agent analyze` claims for that dialect.
//!
//! - `--dialect bash` is checked against bash, `--dialect zsh` against zsh,
//!   and `--dialect unknown` against BOTH (it must be the join).
//! - A run that ends by itself checks both bounds; a run stopped by the
//!   transfer budget or the clock checks the upper bound only.
//! - A run that made no request must not face a contribution with lower > 0.
//!
//! `ORACLE_SEEDS=n` changes how many generated scripts run (default 500);
//! `ORACLE_SEED=s` runs just one and prints it.

use std::path::Path;
use std::time::Duration;

use shell_oracle::{amont_bin, analyze, check, claim, gen, Analysis, End, Sandbox, Shell, Upper};

const STUB: &str = env!("CARGO_BIN_EXE_oracle-stub");
const BUDGET: usize = 3000;
const TIMEOUT: Duration = Duration::from_secs(10);

struct Verdict {
    /// Per checked (dialect, shell) pair: did it hold?
    pairs: Vec<(&'static str, Shell, bool)>,
    failures: Vec<String>,
    ends: [End; 2],
    /// Host claims checked, and how many of them were exact (lower = upper)
    /// or infinite (vacuous upper): the harness is only as strong as the
    /// share of claims that could have failed.
    claims: usize,
    exact: usize,
    infinite: usize,
    transfers: u128,
}

fn verify(sb: &Sandbox, bin: &Path, script: &str) -> Verdict {
    let obs_bash = sb.run(Shell::Bash, script, BUDGET, TIMEOUT);
    let obs_zsh = sb.run(Shell::Zsh, script, BUDGET, TIMEOUT);
    let a_bash = analyze(bin, "bash", script);
    let a_zsh = analyze(bin, "zsh", script);
    let a_unk = analyze(bin, "unknown", script);
    let mut pairs = Vec::new();
    let mut failures = Vec::new();
    let checks: [(&'static str, &Analysis, Shell, &shell_oracle::Observed); 4] = [
        ("bash", &a_bash, Shell::Bash, &obs_bash),
        ("zsh", &a_zsh, Shell::Zsh, &obs_zsh),
        ("unknown", &a_unk, Shell::Bash, &obs_bash),
        ("unknown", &a_unk, Shell::Zsh, &obs_zsh),
    ];
    let (mut claims, mut exact, mut infinite) = (0, 0, 0);
    for (d, a, sh, obs) in checks {
        let mut hosts = shell_oracle::named_hosts(a);
        hosts.extend(obs.counts.keys().cloned());
        hosts.sort();
        hosts.dedup();
        for h in &hosts {
            let (lo, up) = claim(a, h);
            claims += 1;
            exact += (up == Upper::Finite(lo)) as usize;
            infinite += up.is_infinite() as usize;
        }
        let ms = check(a, d, sh, obs);
        pairs.push((d, sh, ms.is_empty()));
        for m in ms {
            failures.push(format!("{m}\n    analysis: {}", a.raw.trim()));
        }
    }
    Verdict {
        pairs,
        failures,
        ends: [obs_bash.end, obs_zsh.end],
        claims,
        exact,
        infinite,
        transfers: obs_bash.total() + obs_zsh.total(),
    }
}

/// `ORACLE_AVOID_KNOWN_BUGS=1`: generate past the known bugs, to hunt for
/// new ones. The default sweep generates everything.
fn options() -> gen::Options {
    gen::Options {
        avoid_known_bugs: std::env::var_os("ORACLE_AVOID_KNOWN_BUGS").is_some(),
    }
}

fn seeds() -> Vec<u64> {
    if let Ok(s) = std::env::var("ORACLE_SEED") {
        return vec![s.parse().expect("ORACLE_SEED")];
    }
    let n: u64 = std::env::var("ORACLE_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    (1..=n).collect()
}

/// Seeds (default options, up to 1000) whose script exposes a KNOWN analyzer
/// bug, with the bug. Each bug has a minimal `#[ignore]`d regression test
/// below, and `known_bug_seeds_still_fail` replays these. Skipped by the
/// sweep so it stays a gate for NEW regressions at any `ORACLE_SEEDS` up to
/// 1000. Remove an entry when its bug is fixed — the replay test says when.
const KNOWN_BUG_SEEDS: &[(u64, Bug)] = &[
    (525, Bug::SplitLoopVar),
    (596, Bug::SubshellJump),
    (653, Bug::SplitLoopVar),
    (739, Bug::SplitLoopVar),
    (756, Bug::SplitLoopVar),
    (761, Bug::SplitLoopVar),
    (808, Bug::SplitLoopVar),
    (835, Bug::SplitLoopVar),
    (905, Bug::SplitLoopVar),
    (907, Bug::SplitLoopVar),
];

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
enum Bug {
    /// bash: `for x in $v` counts the split words but binds `x` to the
    /// UNSPLIT value. See `known_bug_split_loop_var_*`.
    SplitLoopVar,
    /// bash: `break`/`continue` inside `( )` treated as leaving the rest of
    /// the subshell; bash errors and runs on. See `known_bug_subshell_jump_*`.
    SubshellJump,
    /// `let` treated as always succeeding. See `known_bug_let_status_*`.
    LetStatus,
}

fn is_known(seed: u64) -> bool {
    !options().avoid_known_bugs && KNOWN_BUG_SEEDS.iter().any(|k| k.0 == seed)
}

#[test]
fn generated_scripts_stay_inside_the_claimed_interval() {
    let bin = amont_bin();
    let seeds = seeds();
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8);
    let chunks: Vec<Vec<u64>> = (0..workers)
        .map(|w| seeds.iter().copied().skip(w).step_by(workers).collect())
        .collect();
    let results: Vec<(u64, String, Verdict)> = std::thread::scope(|s| {
        let hs: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                let bin = bin.clone();
                s.spawn(move || {
                    let sb = Sandbox::new(Path::new(STUB));
                    chunk
                        .into_iter()
                        .filter(|s| !is_known(*s))
                        .map(|seed| {
                            let script = gen::script_with(seed, options());
                            let v = verify(&sb, &bin, &script);
                            (seed, script, v)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });

    let mut report = String::new();
    for (d, sh) in [
        ("bash", Shell::Bash),
        ("zsh", Shell::Zsh),
        ("unknown", Shell::Bash),
        ("unknown", Shell::Zsh),
    ] {
        let (ok, all) = results.iter().fold((0, 0), |(ok, all), (_, _, v)| {
            let held = v.pairs.iter().any(|p| p.0 == d && p.1 == sh && p.2);
            (ok + held as usize, all + 1)
        });
        report.push_str(&format!("  --dialect {d:<7} vs {sh:?}: {ok}/{all} held\n"));
    }
    let vs: Vec<&Verdict> = results.iter().map(|r| &r.2).collect();
    let ends = |pred: fn(&End) -> bool| {
        vs.iter()
            .flat_map(|v| v.ends.iter())
            .filter(|e| pred(e))
            .count()
    };
    let claims: usize = vs.iter().map(|v| v.claims).sum();
    let exact: usize = vs.iter().map(|v| v.exact).sum();
    let infinite: usize = vs.iter().map(|v| v.infinite).sum();
    let transfers: u128 = vs.iter().map(|v| v.transfers).sum();
    let silent = vs.iter().filter(|v| v.transfers == 0).count();
    eprintln!(
        "{} generated scripts ({} known-bug seeds skipped)\n{report}\
         \x20 runs: {} exited, {} stopped at the budget, {} timed out\n\
         \x20 {transfers} transfers observed; {silent} scripts made none\n\
         \x20 {claims} host claims: {exact} exact, {infinite} with an infinite upper",
        results.len(),
        seeds.iter().filter(|s| is_known(**s)).count(),
        ends(|e| matches!(e, End::Exited(_))),
        ends(|e| matches!(e, End::Budget)),
        ends(|e| matches!(e, End::Timeout)),
    );
    let slow: Vec<String> = results
        .iter()
        .filter(|r| r.2.ends.contains(&End::Timeout))
        .map(|r| r.0.to_string())
        .collect();
    if !slow.is_empty() {
        eprintln!(
            "  timed out (upper bound checked only): seeds {}",
            slow.join(", ")
        );
    }
    if std::env::var("ORACLE_SEED").is_ok() {
        for (_, s, _) in &results {
            eprintln!("script: {s}");
        }
    }
    let failed: Vec<_> = results
        .iter()
        .filter(|(_, _, v)| !v.failures.is_empty())
        .collect();
    if !failed.is_empty() {
        let mut msg = format!("{} scripts left the claimed interval:\n", failed.len());
        for (seed, script, v) in failed.iter().take(15) {
            msg.push_str(&format!("\nseed {seed}: {script}\n"));
            for f in &v.failures {
                msg.push_str(&format!("  {f}\n"));
            }
        }
        let seeds: Vec<String> = failed.iter().map(|f| f.0.to_string()).collect();
        msg.push_str(&format!("\nall failing seeds: {}\n", seeds.join(", ")));
        panic!("{msg}");
    }
}

// ── fixed scripts: Appendix C, runnable subset ──────────────────────────────

/// The Appendix-C commands (src/analysis/interp/tests.rs) that run to
/// completion against stubs (run in an empty directory, so the glob case
/// matches nothing). Adapted where a helper is not guaranteed on a runner:
/// `jq` became `head -c0`; the count-away loop gained a `break`. Left out:
/// the 10^12 ranges and seq limits (`limits.rs`' business), recursion, and
/// `seq 5 1` — whose output depends on whose `seq` is on PATH (GNU:
/// nothing; BSD/macOS: 5 4 3 2 1), where the analyzer assumes GNU.
const APPENDIX_C: &[&str] = &[
    "for i in {1..60}; do curl -s https://h.example/$i; done",
    r#"for i in "{1..5}"; do curl https://h.example/; done"#,
    r"for i in \{1..3\}; do curl https://h.example/; done",
    "for i in {5..1}; do curl https://h.example/; done",
    r#"items="a b c"; for i in $items; do curl https://h.example/; done"#,
    "for i in *.json; do curl https://h.example/; done",
    "for t in $(curl -s https://h.example/list); do echo $t; done",
    "i=0; while curl -fs https://h.example/ && [ $i -lt 3 ]; do i=$((i+1)); done",
    "i=0; while [ $i -lt 400 ]; do curl https://h.example/; i=$((i+1)); done",
    "i=399; while [ $i -lt 400 ]; do curl https://h.example/; i=$((i+1)); done",
    "i=1; while [ $i -le 5 ]; do curl https://h.example/; i=$((i+1)); done",
    "i=0; until [ $i -ge 400 ]; do curl https://h.example/; i=$((i+1)); done",
    "i=0; while [ $i -lt 400 ]; do curl https://h.example/ || continue; i=$((i+1)); done",
    r#"i=0; while [ $i -lt 400 ]; do curl https://h.example/; [ -n "$x" ] && i=$((i+1)); done"#,
    "i=10; while [ $i -gt 0 ]; do curl https://h.example/; i=$((i+1)); [ $i -gt 20 ] && break; done",
    "for ((i=0; i<30; i++)); do curl https://h.example/; done",
    "f() { return 0; curl https://h.example/; }; f",
    "f() { for i in {1..100}; do curl https://h.example/; done; }; echo ok",
    "f() { curl https://h.example/; }; f; f; f",
    r#"u=https://h.example/; if [ -n "$X" ]; then u=https://other.example/; fi; curl "$u""#,
    "set -e; false; curl https://h.example/",
    "echo https://x | read u; u=${u:-https://h.example/}; curl https://h.example/; echo | read v",
    r#"u=https://h.example/; echo x | read u; curl "$u""#,
    r#"u=https://h.example/1; while [ -n "$u" ]; do u=$(curl -s "$u" | head -c0); done"#,
    "for x in; do gh api --paginate /repos/o/r/runs; done",
    r#"for i in {1..60}; do if [ -n "$X" ]; then curl https://h.example/a; else curl https://h.example/b; fi; done"#,
    "for i in {1..30}; do curl https://h.example/a; curl https://h.example/b; done",
    "for i in $(seq 1 300); do curl -s https://h.example/$i; done",
    r#"for t in "$(echo list)"; do curl -s https://h.example/; done"#,
];

fn run_fixed(scripts: &[&str]) -> Vec<String> {
    let bin = amont_bin();
    let sb = Sandbox::new(Path::new(STUB));
    let mut out = Vec::new();
    for s in scripts {
        let v = verify(&sb, &bin, s);
        for f in v.failures {
            out.push(format!("{s}\n  {f}"));
        }
    }
    out
}

#[test]
fn appendix_c_scripts_stay_inside_the_claimed_interval() {
    let f = run_fixed(APPENDIX_C);
    assert!(f.is_empty(), "{}", f.join("\n\n"));
}

// ── scripts that never end ──────────────────────────────────────────────────

/// Loops with no way out: the stub kills the group at the budget, and the
/// analysis must have called the host uncapped.
#[test]
fn a_loop_with_no_way_out_is_uncapped() {
    let bin = amont_bin();
    let sb = Sandbox::new(Path::new(STUB));
    let scripts = [
        "while true; do curl -s https://a.example/; done",
        "while :; do curl -s https://a.example/; done",
        "echo start; while true; do gh api /x; curl -s https://a.example/p; done",
        "for i in 1 2; do while true; do curl -s https://a.example/; done; done",
    ];
    let mut bad = Vec::new();
    for s in scripts {
        for sh in [Shell::Bash, Shell::Zsh] {
            let obs = sb.run(sh, s, 40, TIMEOUT);
            assert_eq!(obs.end, End::Budget, "{sh:?} did not hit the budget: {s}");
            for d in [sh.dialect(), "unknown"] {
                let a = analyze(&bin, d, s);
                for h in obs.counts.keys() {
                    let (_, up) = claim(&a, h);
                    if up != Upper::Uncapped {
                        bad.push(format!(
                            "{d} {sh:?} {h}: upper {up}, want uncapped: {s}\n  {}",
                            a.raw.trim()
                        ));
                    }
                }
                bad.extend(check(&a, d, sh, &obs).iter().map(|m| format!("{m}: {s}")));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

// ── known analyzer bugs (regressions, ignored until fixed) ──────────────────

fn replay(script: &str) {
    let f = run_fixed(&[script]);
    assert!(f.is_empty(), "{}", f.join("\n\n"));
}

#[test]
#[ignore = "known analyzer bugs: replays KNOWN_BUG_SEEDS"]
fn known_bug_seeds_still_fail() {
    // Inverted on purpose: this passes while every listed seed still fails,
    // and names the seeds that started passing (their bug is fixed; drop
    // them from KNOWN_BUG_SEEDS).
    let bin = amont_bin();
    let sb = Sandbox::new(Path::new(STUB));
    let fixed: Vec<String> = KNOWN_BUG_SEEDS
        .iter()
        .filter(|(seed, _)| verify(&sb, &bin, &gen::script(*seed)).failures.is_empty())
        .map(|(seed, bug)| format!("{seed} ({bug:?})"))
        .collect();
    assert!(
        fixed.is_empty(),
        "now pass — remove from KNOWN_BUG_SEEDS: {}",
        fixed.join(", ")
    );
}

/// BUG (bash, and `unknown` against bash): `for x in $items` with
/// `items="a b c"` runs three times — the analyzer counts that right — but
/// binds `x` to the whole unsplit "a b c" on each trip instead of "a", "b",
/// "c". A test on the loop variable is then decided wrongly, in both
/// directions:
///   bash observes 1 transfer; `--dialect bash` and `unknown` claim [0, 0].
#[test]
#[ignore = "analyzer bug: split for-list binds the unsplit value (bash)"]
fn known_bug_split_loop_var_upper() {
    replay(
        r#"items="a b c"; for x in $items; do [ "$x" = c ] && curl -s https://a.example/; done"#,
    );
}

/// Same bug, the lower bound: bash observes 0; `--dialect bash` claims
/// [3, 3], `unknown` [1, 3].
#[test]
#[ignore = "analyzer bug: split for-list binds the unsplit value (bash)"]
fn known_bug_split_loop_var_lower() {
    replay(
        r#"items="a b c"; for x in $items; do [ "$x" = "a b c" ] && curl -s https://a.example/; done"#,
    );
}

/// BUG (bash, and `unknown` against bash): `continue`/`break` inside a
/// subshell. bash resets the loop level in `( )`: the builtin prints
/// "only meaningful in a loop", returns 1, and the subshell RUNS ON. zsh
/// leaves the subshell instead. The analyzer applies the zsh reading to
/// bash: bash observes 2 transfers; `bash` and `unknown` claim [0, 0].
#[test]
#[ignore = "analyzer bug: break/continue in ( ) does not stop a bash subshell"]
fn known_bug_subshell_jump_continue() {
    replay("for i in 1 2; do ( continue; curl -s https://a.example/ ); done");
}

#[test]
#[ignore = "analyzer bug: break/continue in ( ) does not stop a bash subshell"]
fn known_bug_subshell_jump_break() {
    replay("for i in 1 2; do ( break; curl -s https://a.example/ ); done");
}

/// BUG (every dialect): `let` returns 1 when its last expression is 0 —
/// `let i++` with i=0 evaluates to 0. The analyzer treats `let` as always
/// succeeding (`((i++))` is modelled right). Under `set -e` the shell exits:
/// both shells observe 0; every dialect claims [1, 1].
#[test]
#[ignore = "analyzer bug: `let` exit status is not modelled"]
fn known_bug_let_status_errexit() {
    replay("set -e; i=0; let i++; curl -s https://a.example/");
}

/// Same bug without errexit: both shells observe 0; every dialect claims
/// [1, 1].
#[test]
#[ignore = "analyzer bug: `let` exit status is not modelled"]
fn known_bug_let_status_and() {
    replay("i=0; let i++ && curl -s https://a.example/");
}

/// PLATFORM (macOS/BSD): `seq 5 1` prints nothing with GNU coreutils and
/// `5 4 3 2 1` with BSD `seq` (/usr/bin/seq on macOS). The analyzer assumes
/// GNU and claims [0, 0]; where BSD `seq` is first on PATH, bash and zsh
/// observe 5. Not in docs/analysis.md's assumptions. Passes on Linux.
#[test]
#[ignore = "analyzer assumes GNU seq: `seq 5 1` counts down on BSD/macOS"]
fn known_bug_bsd_seq_descending() {
    replay("for i in $(seq 5 1); do curl -s https://a.example/; done");
}

/// PRECISION, not soundness: `until false` is `while true` spelled the
/// other way, but the analysis calls its later iterations `unknown` rather
/// than `uncapped` (the interval still holds; the evidence is lost). Every
/// dialect. docs/analysis.md names "`while true` with no way out" as
/// uncapped; the negated form should be too.
#[test]
#[ignore = "analyzer precision gap: `until false` is unknown, not uncapped"]
fn until_false_is_uncapped() {
    let bin = amont_bin();
    let s = "until false; do wget -q https://a.example/x; done";
    for d in ["bash", "zsh", "unknown"] {
        let (_, up) = claim(&analyze(&bin, d, s), "a.example");
        assert_eq!(up, Upper::Uncapped, "--dialect {d}: {s}");
    }
}

/// `ORACLE_SCRIPT='…' cargo test --test execution adhoc -- --nocapture`
/// checks one hand-written script the same way — for minimising a failing
/// seed. Does nothing without the variable.
#[test]
fn adhoc_script() {
    let Ok(script) = std::env::var("ORACLE_SCRIPT") else {
        return;
    };
    let f = run_fixed(&[script.as_str()]);
    assert!(f.is_empty(), "{}", f.join("\n\n"));
}
