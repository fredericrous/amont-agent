//! The interpreter, through the frontend: Appendix C of the design, one
//! case per test. Each asserts on the effects, not on internals, so a
//! refactor of the fixpoint cannot quietly change what a command is said to
//! do.

use crate::analysis::analyze;
use crate::analysis::domain::{Count, Upper, Why};
use crate::analysis::effects::{Contribution, Unresolved};
use crate::rules::Dialect;

fn run(src: &str, dialect: Dialect) -> crate::analysis::Analysis {
    let a = analyze(src, dialect);
    assert!(
        a.incomplete.is_none() || src.len() > 2000,
        "incomplete on {src:?}: {:?}",
        a.incomplete
    );
    a
}

/// Total transfers, per contribution, to `host`.
fn to_host<'a>(a: &'a crate::analysis::Analysis, host: &str) -> Vec<&'a Contribution> {
    a.effects
        .contributions
        .iter()
        .filter(|c| c.targets.hosts.contains(host))
        .collect()
}

fn total(cs: &[&Contribution]) -> Count {
    cs.iter().fold(Count::ZERO, |acc, c| acc.add(c.transfers))
}

fn bash(src: &str) -> crate::analysis::Analysis {
    run(src, Dialect::Bash)
}

fn zsh(src: &str) -> crate::analysis::Analysis {
    run(src, Dialect::Zsh)
}

const H: &str = "h.example";

// ── counts ────────────────────────────────────────────────────────────────

#[test]
fn a_brace_range_loop_runs_sixty_times() {
    let a = bash("for i in {1..60}; do curl -s https://h.example/$i; done");
    assert_eq!(total(&to_host(&a, H)), Count::exactly(60));
}

#[test]
fn a_quoted_brace_is_one_word() {
    let a = bash(r#"for i in "{1..5}"; do curl https://h.example/; done"#);
    assert_eq!(total(&to_host(&a, H)), Count::ONE);
}

#[test]
fn an_escaped_brace_is_literal() {
    let a = bash(r"for i in \{1..3\}; do curl https://h.example/; done");
    assert_eq!(total(&to_host(&a, H)), Count::ONE);
}

#[test]
fn a_descending_range_counts_too() {
    let a = bash("for i in {5..1}; do curl https://h.example/; done");
    assert_eq!(total(&to_host(&a, H)), Count::exactly(5));
}

#[test]
fn an_unquoted_parameter_splits_in_bash_and_not_in_zsh() {
    let src = r#"items="a b c"; for i in $items; do curl https://h.example/; done"#;
    assert_eq!(total(&to_host(&bash(src), H)), Count::exactly(3));
    assert_eq!(total(&to_host(&zsh(src), H)), Count::ONE);
}

#[test]
fn a_glob_list_is_unknown() {
    let a = bash("for f in *.json; do curl https://h.example/; done");
    assert_eq!(total(&to_host(&a, H)).upper, Upper::Unknown);
}

#[test]
fn a_substitution_in_a_for_header_runs_once() {
    let a = bash("for t in $(curl -s https://h.example/list); do echo $t; done");
    assert_eq!(total(&to_host(&a, H)), Count::ONE);
}

#[test]
fn a_while_condition_runs_once_more_than_the_body() {
    let a = bash("i=0; while curl -fs https://h.example/ && [ $i -lt 3 ]; do i=$((i+1)); done");
    let t = total(&to_host(&a, H));
    assert_eq!(t.upper, Upper::Finite(4), "{t:?}");
}

// ── counters ──────────────────────────────────────────────────────────────

fn counter(body: &str) -> Count {
    let src = format!("i=0; while [ $i -lt 400 ]; do {body}; done");
    total(&to_host(&bash(&src), H))
}

#[test]
fn the_counter_pattern_bounds_the_loop() {
    assert_eq!(
        counter("curl https://h.example/; i=$((i+1))"),
        Count::exactly(400)
    );
}

#[test]
fn a_counter_near_its_limit_runs_once() {
    let a = bash("i=399; while [ $i -lt 400 ]; do curl https://h.example/; i=$((i+1)); done");
    assert_eq!(total(&to_host(&a, H)), Count::ONE);
}

#[test]
fn le_includes_the_limit() {
    let a = bash("i=1; while [ $i -le 5 ]; do curl https://h.example/; i=$((i+1)); done");
    assert_eq!(total(&to_host(&a, H)), Count::exactly(5));
}

#[test]
fn until_ge_is_the_same_loop() {
    let a = bash("i=0; until [ $i -ge 400 ]; do curl https://h.example/; i=$((i+1)); done");
    assert_eq!(total(&to_host(&a, H)), Count::exactly(400));
}

#[test]
fn no_increment_is_unknown() {
    assert_eq!(counter("curl https://h.example/").upper, Upper::Unknown);
}

#[test]
fn continue_before_the_increment_is_unknown() {
    assert_eq!(
        counter("curl https://h.example/ || continue; i=$((i+1))").upper,
        Upper::Unknown
    );
}

#[test]
fn an_increment_in_a_subshell_is_unknown() {
    assert_eq!(
        counter("curl https://h.example/; ( i=$((i+1)) )").upper,
        Upper::Unknown
    );
}

#[test]
fn a_conditional_increment_is_unknown() {
    assert_eq!(
        counter("curl https://h.example/; [ -n \"$x\" ] && i=$((i+1))").upper,
        Upper::Unknown
    );
}

#[test]
fn a_reset_in_the_body_is_unknown() {
    assert_eq!(
        counter("curl https://h.example/; i=$((i+1)); i=0").upper,
        Upper::Unknown
    );
}

#[test]
fn a_function_that_resets_the_counter_is_unknown() {
    let a = bash(
        "reset() { i=0; }; i=0; while [ $i -lt 400 ]; do curl https://h.example/; i=$((i+1)); reset; done",
    );
    assert_eq!(total(&to_host(&a, H)).upper, Upper::Unknown);
}

#[test]
fn a_comparison_that_is_not_the_final_command_bounds_nothing() {
    // `true`'s status is the condition's: the counter is compared and
    // ignored, and nothing leaves the loop.
    let a = bash("i=0; while [ $i -lt 400 ]; true; do curl https://h.example/; i=$((i+1)); done");
    assert_eq!(total(&to_host(&a, H)).upper, Upper::Uncapped(Why::Infinite));
}

#[test]
fn a_counter_that_counts_away_from_its_limit_is_unknown() {
    let a = bash("i=10; while [ $i -gt 0 ]; do curl https://h.example/; i=$((i+1)); done");
    assert_eq!(total(&to_host(&a, H)).upper, Upper::Unknown);
}

#[test]
fn an_arithmetic_for_is_bounded() {
    let a = bash("for ((i=0; i<30; i++)); do curl https://h.example/; done");
    assert_eq!(total(&to_host(&a, H)).upper, Upper::Finite(30));
}

// ── state and flow ────────────────────────────────────────────────────────

#[test]
fn nothing_after_return_runs() {
    let a = bash("f() { return 0; curl https://h.example/; }; f");
    assert!(to_host(&a, H).is_empty());
}

#[test]
fn a_function_defined_and_never_called_does_nothing() {
    let a = bash("f() { for i in {1..100}; do curl https://h.example/; done; }; echo ok");
    assert!(to_host(&a, H).is_empty());
}

#[test]
fn a_function_called_three_times_counts_three_times() {
    let a = bash("f() { curl https://h.example/; }; f; f; f");
    assert_eq!(total(&to_host(&a, H)), Count::exactly(3));
}

#[test]
fn recursion_is_not_followed() {
    let a = analyze("f() { curl https://h.example/; f; }; f", Dialect::Bash);
    assert!(!a.effects.unknown.is_empty());
}

#[test]
fn a_variable_set_on_one_branch_is_unknown_after() {
    let a = bash(
        r#"u=https://h.example/; if [ -n "$X" ]; then u=https://other.example/; fi; curl "$u""#,
    );
    let c = &a.effects.contributions;
    assert_eq!(c.len(), 1);
    assert!(!c[0].targets.unresolved.is_empty() || c[0].targets.hosts.len() == 2);
}

#[test]
fn eval_invalidates_a_constant() {
    let a = bash(r#"u=https://h.example/; eval "$X"; curl "$u""#);
    let c = &a.effects.contributions;
    assert_eq!(c.len(), 1);
    assert!(c[0].targets.unresolved.contains(&Unresolved::Dynamic));
    assert!(!a.effects.unknown.is_empty());
}

#[test]
fn errexit_stops_at_a_failure() {
    let a = bash("set -e; false; curl https://h.example/");
    assert!(to_host(&a, H).is_empty());
}

#[test]
fn the_last_pipeline_element_keeps_its_writes_in_zsh_only() {
    let src = "echo https://x | read u; u=${u:-https://h.example/}; curl https://h.example/; echo | read v";
    // Smoke: both dialects analyse it without losing the plain transfer.
    assert_eq!(total(&to_host(&bash(src), H)), Count::ONE);
    assert_eq!(total(&to_host(&zsh(src), H)), Count::ONE);
    let keep = r#"u=https://h.example/; echo x | read u; curl "$u""#;
    // zsh: `read` ran in this shell, so `u` is whatever was read.
    assert!(zsh(keep).effects.contributions[0]
        .targets
        .unresolved
        .contains(&Unresolved::Dynamic));
    // bash: the pipeline's `read` ran in a subshell; `u` is untouched.
    assert_eq!(
        bash(keep).effects.contributions[0].targets.only_host(),
        Some(H)
    );
}

// ── the incident ──────────────────────────────────────────────────────────

const INCIDENT: &str = r#"count() { host=$1; repo=$2; tok=$3; url="https://$host/v2/$repo/tags/list?n=1000"; pages=0; total=0; first=""; while [ -n "$url" ] && [ $pages -lt 400 ]; do hdr=$(mktemp); body=$(curl -sS -D $hdr -H "Authorization: Bearer $tok" "$url"); n=$(echo "$body" | jq '.tags|length' 2>/dev/null); [ -z "$first" ] && first=$n; total=$((total+${n:-0})); pages=$((pages+1)); link=$(grep -i '^link:' $hdr | sed -E 's/.*<([^>]+)>.*/\1/' | tr -d '\r'); rm $hdr; if [ -n "$link" ]; then case "$link" in http*) url=$link;; *) url="https://$host$link";; esac; else url=""; fi; done; echo "$host/$repo: pages=$pages first_page=$first total=$total"; }; T=$(curl -s "https://auth.docker.io/token?service=registry.docker.io&scope=repository:envoyproxy/envoy:pull" | jq -r .token); count registry-1.docker.io envoyproxy/envoy "$T"; T=$(curl -s "https://ghcr.io/token?scope=repository:immich-app/immich-machine-learning:pull" | jq -r .token); count ghcr.io immich-app/immich-machine-learning "$T""#;

#[test]
fn the_incident_first_page_goes_to_the_named_registry_once_per_call() {
    for d in [Dialect::Bash, Dialect::Zsh, Dialect::Unknown] {
        let a = run(INCIDENT, d);
        for host in ["ghcr.io", "registry-1.docker.io"] {
            let exact: Vec<_> = a
                .effects
                .contributions
                .iter()
                .filter(|c| c.targets.only_host() == Some(host) && c.span.start < 400)
                .collect();
            assert_eq!(
                total(&exact),
                Count::ONE,
                "{d:?} {host}: {:#?}",
                a.effects.contributions
            );
        }
    }
}

#[test]
fn the_incident_later_pages_are_bounded_by_the_counter_not_doubled() {
    let a = run(INCIDENT, Dialect::Zsh);
    let later: Vec<_> = a
        .effects
        .contributions
        .iter()
        .filter(|c| !c.targets.unresolved.is_empty())
        .collect();
    assert!(!later.is_empty(), "{:#?}", a.effects.contributions);
    for c in later {
        // One call per host, each up to 399 pages after the first — never
        // 400 × 2 attributed to one host.
        assert_eq!(c.transfers.upper, Upper::Finite(399), "{c:#?}");
    }
}

#[test]
fn an_infinite_loop_is_uncapped() {
    let a = bash("while true; do curl https://h.example/; done");
    assert_eq!(total(&to_host(&a, H)).upper, Upper::Uncapped(Why::Infinite));
}

#[test]
fn next_link_following_is_uncapped() {
    let a = bash(
        r#"u=https://h.example/1; while [ -n "$u" ]; do u=$(curl -s "$u" | jq -r .next); done"#,
    );
    let ups: Vec<Upper> = a
        .effects
        .contributions
        .iter()
        .map(|c| c.transfers.upper)
        .collect();
    assert!(
        ups.contains(&Upper::Uncapped(Why::NextLinkFollow)),
        "{ups:?}"
    );
}

#[test]
fn a_zero_iteration_loop_around_paginate_is_nothing() {
    let a = bash("for x in; do gh api --paginate /repos/o/r/runs; done");
    assert!(a.effects.contributions.is_empty());
}

#[test]
fn sixty_if_else_to_one_host_is_at_most_sixty() {
    let a = bash(
        r#"for i in {1..60}; do if [ -n "$X" ]; then curl https://h.example/a; else curl https://h.example/b; fi; done"#,
    );
    assert_eq!(total(&to_host(&a, H)).upper, Upper::Finite(60));
}

#[test]
fn two_calls_per_iteration_add() {
    let a = bash("for i in {1..30}; do curl https://h.example/a; curl https://h.example/b; done");
    assert_eq!(total(&to_host(&a, H)), Count::exactly(60));
}

// ── limits ────────────────────────────────────────────────────────────────

#[test]
fn a_huge_range_saturates_promptly() {
    let t = std::time::Instant::now();
    let a = analyze(
        "for i in {1..999999999999}; do for j in {1..999999999999}; do curl https://h.example/; done; done",
        Dialect::Bash,
    );
    assert!(t.elapsed().as_secs() < 2);
    assert_eq!(total(&to_host(&a, H)).upper, Upper::Saturated);
}

#[test]
fn deep_nesting_is_incomplete_not_a_crash() {
    let t = std::time::Instant::now();
    let src = format!(
        "{}curl https://h.example/{}",
        "( ".repeat(1000),
        " )".repeat(1000)
    );
    let a = analyze(&src, Dialect::Bash);
    assert!(t.elapsed().as_secs() < 2);
    assert!(a.incomplete.is_some());
}

#[test]
fn a_long_call_chain_is_bounded() {
    let t = std::time::Instant::now();
    let mut src = String::from("f0() { curl https://h.example/; }; ");
    for i in 1..40 {
        src.push_str(&format!("f{i}() {{ f{}; f{}; }}; ", i - 1, i - 1));
    }
    src.push_str("f39");
    let a = analyze(&src, Dialect::Bash);
    assert!(t.elapsed().as_secs() < 5);
    assert!(a.incomplete.is_some() || !a.effects.unknown.is_empty());
}

#[test]
fn seq_is_counted_arithmetically() {
    let a = bash("for i in $(seq 1 300); do curl -s https://h.example/$i; done");
    assert_eq!(total(&to_host(&a, H)), Count::exactly(300));
    let a = bash("for i in $(seq 5 1); do curl -s https://h.example/; done");
    assert!(to_host(&a, H).is_empty());
    let a = bash("for i in $(seq 0 10 99999999999999); do curl -s https://h.example/; done");
    assert_eq!(
        total(&to_host(&a, H)),
        Count::exactly(9_999_999_999_999 + 1)
    );
}

#[test]
fn zsh_splits_a_substitution_even_though_it_does_not_split_parameters() {
    let a = zsh("for t in $(cat list); do curl -s https://h.example/; done");
    assert_eq!(total(&to_host(&a, H)).upper, Upper::Unknown);
}

#[test]
fn a_quoted_substitution_is_one_word() {
    let a = zsh(r#"for t in "$(cat list)"; do curl -s https://h.example/; done"#);
    assert_eq!(total(&to_host(&a, H)), Count::ONE);
}
