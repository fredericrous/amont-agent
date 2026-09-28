//! The syntax oracle: brush-parser says what bash accepts; `amont-agent
//! analyze --dialect bash` must then report `"parsed": true`, unless the
//! command uses a construct the analyzer documents as outside its grammar.
//!
//! Inputs: every case of `tests/corpus/*.cases` (real commands) and every
//! command literal in `src/analysis/interp/tests.rs` (Appendix C).
//!
//! This validates BASH syntax only. brush-parser is not a zsh parser, and
//! zsh-only commands in the corpus simply fail it and are skipped.
//!
//! A disagreement fails the test unless its command is listed, verbatim, in
//! `syntax-allowlist.txt` with the reason.

use shell_oracle::{amont_bin, analyze, appendix_c, corpus_cases, gen, Sample};

/// The supported subset of docs/analysis.md, one probe per construct, so a
/// construct the corpus happens not to use is still checked.
const PROBES: &[&str] = &[
    "a; b && c || d",
    "a | b | c; ! a | b",
    "if a; then b; elif c; then d; else e; fi",
    "case $x in a) b;; c|d) e;& f) g;;& *) h;; esac",
    "for i in 1 2 3; do echo $i; done",
    "for ((i=0; i<3; i++)); do echo $i; done",
    "while a; do b; done; until c; do d; done",
    "{ a; b; }; ( c; d )",
    "(( i += 2 )); [[ -n $x && $y == z* ]]; [ -f x ]; test -d y",
    "f() { local x=1; typeset y=2; return 0; }; function g { f; }; function h() { g; }",
    "for i in 1 2; do for j in 3 4; do break 2; continue 1; done; done",
    "set -e; set -o pipefail; shopt -s lastpipe; exit 3",
    "x=$(echo `date`); y=${x}; z=${x:-d}${x:+a}${x#p}${x%s}; echo $1 $@ $* $#",
    r#"echo 'single' "double $x" $'ansi\n' "a"'b'c"#,
    "echo {1..9} {a,b}{c,d} \\{1..3\\}",
    "cat <<EOF\nbody $x\nEOF\necho after",
    "cat <<-'EOF'\n\tbody\n\tEOF",
    "a >out 2>&1 <in 3>>log; b &>all; c <<< word",
    "diff <(a) >(b)",
    "a & b; wait",
    "arr=(a b c); echo ${arr[1]}",
    "select x in a b; do echo $x; done",
    "coproc cat",
    "eval \"$X\"; source f; . g; trap 'x' EXIT",
    "# only a comment",
    "",
];

fn brush_accepts(src: &str) -> bool {
    let opts = brush_parser::ParserOptions {
        // Off, as in a non-interactive bash: `@(a|b)` is a syntax error
        // there unless the script runs `shopt -s extglob` first — which,
        // being a runtime option, no parser of a whole string can honour.
        enable_extended_globbing: false,
        ..Default::default()
    };
    let mut p = brush_parser::Parser::new(std::io::Cursor::new(src.as_bytes()), &opts);
    p.parse_program().is_ok()
}

/// Constructs the analyzer documents (docs/analysis.md, frontend) as outside
/// what it parses. A command using one may be refused without being a bug.
fn documented_unsupported(src: &str) -> Option<&'static str> {
    let b = src.as_bytes();
    for i in 0..b.len() {
        // extglob: `@(…)`, `!(…)`, `+(…)`, `?(…)`, `*(…)` outside `$(`.
        if b[i] == b'(' && i > 0 && matches!(b[i - 1], b'@' | b'!' | b'+' | b'?' | b'*') {
            return Some("extglob pattern");
        }
        // `{fd}>file` / `{fd}<file`: a redirection allocating a descriptor.
        if b[i] == b'{' {
            let rest = &src[i + 1..];
            let name_len = rest
                .bytes()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
                .count();
            if name_len > 0
                && rest[name_len..].starts_with('}')
                && rest[name_len + 1..].starts_with(['<', '>'])
            {
                return Some("{fd} redirection");
            }
        }
    }
    None
}

fn allowlist() -> Vec<String> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/syntax-allowlist.txt");
    std::fs::read_to_string(path)
        .expect("syntax-allowlist.txt")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| shell_oracle::unescape(l.split_once('\t').map_or(l, |(_, c)| c)))
        .collect()
}

#[test]
fn what_bash_parses_the_analyzer_parses() {
    let bin = amont_bin();
    let mut samples: Vec<Sample> = corpus_cases();
    let n_corpus = samples.len();
    samples.extend(appendix_c());
    let n_appendix = samples.len() - n_corpus;
    samples.extend(PROBES.iter().enumerate().map(|(i, p)| Sample {
        origin: format!("probe {i}"),
        command: p.to_string(),
    }));
    samples.extend((1..=200).map(|seed| Sample {
        origin: format!("generated seed {seed}"),
        command: gen::script(seed),
    }));
    assert!(n_corpus > 100, "corpus looks empty: {n_corpus} cases");
    assert!(
        n_appendix > 30,
        "Appendix C extraction found too little: {n_appendix}"
    );

    let allow = allowlist();
    let mut accepted = 0;
    let mut documented = 0;
    let mut allowed = 0;
    let mut bad = Vec::new();
    let mut rejected = Vec::new();
    for s in &samples {
        if !brush_accepts(&s.command) {
            rejected.push(s.origin.clone());
            continue;
        }
        accepted += 1;
        let a = analyze(&bin, "bash", &s.command);
        if a.parsed {
            continue;
        }
        if documented_unsupported(&s.command).is_some() {
            documented += 1;
            continue;
        }
        if allow.contains(&s.command) {
            allowed += 1;
            continue;
        }
        bad.push(format!(
            "{}: {:?}\n    analyzer: {}",
            s.origin,
            s.command,
            a.incomplete.as_deref().unwrap_or("?")
        ));
    }
    eprintln!(
        "{} commands ({n_corpus} corpus, {n_appendix} Appendix C, {} probes, 200 generated); \
         bash accepts {accepted}; analyzer refuses {documented} documented-unsupported, \
         {allowed} allowlisted",
        samples.len(),
        PROBES.len()
    );
    eprintln!(
        "  not bash per brush-parser (skipped): {}",
        rejected.join(", ")
    );
    assert!(
        bad.is_empty(),
        "{} commands bash parses and the analyzer does not:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// The allowlist may only hold commands that still disagree: an entry that
/// the analyzer now parses (or bash now refuses) is stale.
#[test]
fn the_allowlist_has_no_stale_entries() {
    let bin = amont_bin();
    let stale: Vec<String> = allowlist()
        .into_iter()
        .filter(|c| !brush_accepts(c) || analyze(&bin, "bash", c).parsed)
        .collect();
    assert!(stale.is_empty(), "stale allowlist entries: {stale:#?}");
}

#[test]
fn the_oracle_itself_tells_bash_from_not_bash() {
    assert!(brush_accepts("for i in 1 2; do curl https://h/$i; done"));
    assert!(brush_accepts("f() { local x=1; }; f && echo ok"));
    assert!(!brush_accepts("if true; then echo"));
    assert!(!brush_accepts("echo 'unterminated"));
    assert!(!brush_accepts("case x in a) b"));
}

/// brush-parser 0.4 refuses some valid bash; such a command is skipped, not
/// checked (coverage lost, never a false alarm). Pinned here so a brush bump
/// that fixes them is noticed and they start counting.
#[test]
fn brush_parser_false_rejections_are_known() {
    for (src, why) in [
        ("( case $k in a) x;; esac )", "`case` directly inside `( )`"),
        ("select x in a b; do echo $x; done", "`select`"),
    ] {
        assert!(!brush_accepts(src), "brush now accepts {why}: {src}");
    }
}
