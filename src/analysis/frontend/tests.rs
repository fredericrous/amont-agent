//! Golden tests on the IR. [`show`] renders a command compactly so an
//! expectation reads like the shell it came from:
//!
//! - simple commands are `[assigns words redirects]`;
//! - a non-literal part is marked: `G(*)` glob, `B{…}` brace, `T(~)` tilde,
//!   `%{name|raw}` parameter operator, `$(…)` substitution, `$((…))`
//!   arithmetic, `<(…)` process substitution;
//! - quoting shows as written, `\c` is an escaped character.

use super::super::ir::*;
use super::super::limits::Exhausted;
use super::{parse, ParseError};

fn p(src: &str) -> Cmd {
    match parse(src) {
        Ok(c) => {
            check_spans(&c, src);
            c
        }
        Err(e) => panic!("{src:?} did not parse: {e:?}"),
    }
}

fn show_src(src: &str) -> String {
    show(&p(src))
}

fn show(c: &Cmd) -> String {
    match c {
        Cmd::Simple(s) => {
            let mut out: Vec<String> = Vec::new();
            for a in &s.assigns {
                out.push(format!(
                    "{}{}={}",
                    a.name,
                    if a.append { "+" } else { "" },
                    word(&a.value)
                ));
            }
            out.extend(s.words.iter().map(word));
            out.extend(s.redirects.iter().map(redirect));
            format!("[{}]", out.join(" "))
        }
        Cmd::Seq { items, .. } => {
            let items: Vec<String> = items
                .iter()
                .map(|i| format!("{}{}", show(&i.cmd), if i.background { " &" } else { "" }))
                .collect();
            format!("seq({})", items.join("; "))
        }
        Cmd::AndOr { first, rest, .. } => {
            let mut out = show(first);
            for (op, c) in rest {
                out.push_str(match op {
                    AndOrOp::And => " && ",
                    AndOrOp::Or => " || ",
                });
                out.push_str(&show(c));
            }
            format!("({out})")
        }
        Cmd::Pipeline { negated, cmds, .. } => {
            let cmds: Vec<String> = cmds.iter().map(show).collect();
            format!(
                "pipe({}{})",
                if *negated { "! " } else { "" },
                cmds.join(" | ")
            )
        }
        Cmd::If {
            arms, otherwise, ..
        } => {
            let arms: Vec<String> = arms
                .iter()
                .map(|(c, b)| format!("{} then {}", show(c), show(b)))
                .collect();
            let mut out = format!("if({}", arms.join(" elif "));
            if let Some(o) = otherwise {
                out.push_str(&format!(" else {}", show(o)));
            }
            out + ")"
        }
        Cmd::Case { word: w, arms, .. } => {
            let arms: Vec<String> = arms
                .iter()
                .map(|a| {
                    let pats: Vec<String> = a.patterns.iter().map(word).collect();
                    let term = match a.term {
                        CaseTerm::Break => ";;",
                        CaseTerm::FallThrough => ";&",
                        CaseTerm::TestNext => ";;&",
                    };
                    format!("{}) {} {term}", pats.join("|"), show(&a.body))
                })
                .collect();
            format!("case({}: {})", word(w), arms.join(" "))
        }
        Cmd::For {
            var, items, body, ..
        } => match items {
            Some(items) => {
                let items: Vec<String> = items.iter().map(word).collect();
                format!("for({var} in {}; {})", items.join(" "), show(body))
            }
            None => format!("for({var}; {})", show(body)),
        },
        Cmd::ArithFor {
            init,
            cond,
            step,
            body,
            ..
        } => {
            let o = |e: &Option<ArithExpr>| e.as_ref().map_or("_".to_string(), arith);
            format!(
                "for(({}; {}; {}); {})",
                o(init),
                o(cond),
                o(step),
                show(body)
            )
        }
        Cmd::Loop {
            kind, cond, body, ..
        } => format!(
            "{}({}; {})",
            match kind {
                LoopKind::While => "while",
                LoopKind::Until => "until",
            },
            show(cond),
            show(body)
        ),
        Cmd::Group { body, .. } => format!("{{{}}}", show(body)),
        Cmd::Subshell { body, .. } => format!("sub({})", show(body)),
        Cmd::FuncDef { name, body, .. } => format!("fn {name}({})", show(body)),
        Cmd::Arith { expr, .. } => format!("arith({})", arith(expr)),
        Cmd::Cond { expr, .. } => format!("cond({})", test(expr)),
        Cmd::Redirected {
            body, redirects, ..
        } => {
            let r: Vec<String> = redirects.iter().map(redirect).collect();
            format!("redir({} {})", show(body), r.join(" "))
        }
        Cmd::Unsupported { why, .. } => format!("unsupported({why})"),
    }
}

fn redirect(r: &Redirect) -> String {
    match &r.heredoc {
        Some(body) => format!("{}{}{body:?}", r.op, word(&r.target)),
        None => format!("{}{}", r.op, word(&r.target)),
    }
}

fn word(w: &Word) -> String {
    w.parts.iter().map(part).collect()
}

fn part(p: &Part) -> String {
    match p {
        Part::Lit(s) => s.clone(),
        Part::SingleQuoted(s) => format!("'{s}'"),
        Part::DoubleQuoted(ps) => format!("\"{}\"", ps.iter().map(part).collect::<String>()),
        Part::Escaped(c) => format!("\\{c}"),
        Part::Param(r) => match r {
            ParamRef::Named(n) => format!("${n}"),
            ParamRef::Positional(n) => format!("${n}"),
            ParamRef::All => "$@".to_string(),
            ParamRef::Special(c) => format!("${c}"),
        },
        Part::ParamOp { name, raw } => format!("%{{{name}|{raw}}}"),
        Part::CmdSubst { body, .. } => format!("$({})", show(body)),
        Part::Arith(e) => format!("$(({}))", arith(e)),
        Part::Brace(b) => match b {
            BraceExpr::IntRange {
                from,
                to,
                step,
                width,
            } => format!("B{{{from}..{to} s{step} w{width}}}"),
            BraceExpr::CharRange { from, to, step } => format!("B{{{from}..{to} s{step}}}"),
            BraceExpr::List(ws) => {
                format!("B{{{}}}", ws.iter().map(word).collect::<Vec<_>>().join(","))
            }
        },
        Part::Glob(c) => format!("G({c})"),
        Part::Tilde(s) => format!("T({s})"),
        Part::ProcSubst { body, .. } => format!("<({})", show(body)),
    }
}

fn arith(e: &ArithExpr) -> String {
    use ArithBinOp::*;
    match e {
        ArithExpr::Num(n) => n.to_string(),
        ArithExpr::Var(v) => v.clone(),
        ArithExpr::Unary(op, e) => {
            let op = match op {
                ArithUnOp::Neg => "-",
                ArithUnOp::Not => "!",
                ArithUnOp::BitNot => "~",
                ArithUnOp::Plus => "+",
            };
            format!("({op}{})", arith(e))
        }
        ArithExpr::Binary(op, a, b) => {
            let op = match op {
                Add => "+",
                Sub => "-",
                Mul => "*",
                Div => "/",
                Rem => "%",
                Lt => "<",
                Le => "<=",
                Gt => ">",
                Ge => ">=",
                Eq => "==",
                Ne => "!=",
                And => "&&",
                Or => "||",
                BitAnd => "&",
                BitOr => "|",
                BitXor => "^",
                Shl => "<<",
                Shr => ">>",
                Pow => "**",
            };
            format!("({} {op} {})", arith(a), arith(b))
        }
        ArithExpr::Assign { var, op, value } => {
            let op = match op {
                None => "=",
                Some(Add) => "+=",
                Some(Sub) => "-=",
                Some(Mul) => "*=",
                Some(Shl) => "<<=",
                Some(_) => "?=",
            };
            format!("({var} {op} {})", arith(value))
        }
        ArithExpr::IncDec { var, delta, prefix } => {
            let op = if *delta > 0 { "++" } else { "--" };
            if *prefix {
                format!("{op}{var}")
            } else {
                format!("{var}{op}")
            }
        }
        ArithExpr::Comma(a, b) => format!("({}, {})", arith(a), arith(b)),
        ArithExpr::Ternary(c, a, b) => format!("({} ? {} : {})", arith(c), arith(a), arith(b)),
        ArithExpr::Unknown(s) => format!("?{{{s}}}"),
    }
}

fn test(t: &TestExpr) -> String {
    match t {
        TestExpr::Unary { op, operand } => format!("({op} {})", word(operand)),
        TestExpr::Binary { left, op, right } => {
            format!("({} {op} {})", word(left), word(right))
        }
        TestExpr::Word(w) => word(w),
        TestExpr::Not(t) => format!("(! {})", test(t)),
        TestExpr::And(a, b) => format!("({} && {})", test(a), test(b)),
        TestExpr::Or(a, b) => format!("({} || {})", test(a), test(b)),
        TestExpr::Unknown => "?".to_string(),
    }
}

/// Every span lies inside the source, on character boundaries, and inside
/// its parent's span.
fn check_spans(c: &Cmd, src: &str) {
    fn ok(span: &Span, parent: &Span, src: &str, what: &str) {
        assert!(
            span.start <= span.end
                && span.end <= src.len()
                && src.is_char_boundary(span.start)
                && src.is_char_boundary(span.end)
                && parent.start <= span.start
                && span.end <= parent.end,
            "{what} span {span:?} escapes {parent:?} in {src:?}"
        );
    }
    fn cmd(c: &Cmd, parent: &Span, src: &str) {
        let s = c.span();
        ok(&s, parent, src, "command");
        match c {
            Cmd::Simple(sc) => {
                for a in &sc.assigns {
                    ok(&a.span, &s, src, "assignment");
                    word(&a.value, &a.span, src);
                }
                for w in &sc.words {
                    word(w, &s, src);
                }
                for r in &sc.redirects {
                    ok(&r.span, &s, src, "redirect");
                    word(&r.target, &r.span, src);
                }
            }
            Cmd::Seq { items, .. } => items.iter().for_each(|i| cmd(&i.cmd, &s, src)),
            Cmd::AndOr { first, rest, .. } => {
                cmd(first, &s, src);
                rest.iter().for_each(|(_, c)| cmd(c, &s, src));
            }
            Cmd::Pipeline { cmds, .. } => cmds.iter().for_each(|c| cmd(c, &s, src)),
            Cmd::If {
                arms, otherwise, ..
            } => {
                for (a, b) in arms {
                    cmd(a, &s, src);
                    cmd(b, &s, src);
                }
                if let Some(o) = otherwise {
                    cmd(o, &s, src);
                }
            }
            Cmd::Case { word: w, arms, .. } => {
                word(w, &s, src);
                for a in arms {
                    ok(&a.span, &s, src, "case arm");
                    a.patterns.iter().for_each(|p| word(p, &a.span, src));
                    cmd(&a.body, &a.span, src);
                }
            }
            Cmd::For { items, body, .. } => {
                items.iter().flatten().for_each(|w| word(w, &s, src));
                cmd(body, &s, src);
            }
            Cmd::ArithFor { body, .. }
            | Cmd::Group { body, .. }
            | Cmd::Subshell { body, .. }
            | Cmd::FuncDef { body, .. } => cmd(body, &s, src),
            Cmd::Loop { cond, body, .. } => {
                cmd(cond, &s, src);
                cmd(body, &s, src);
            }
            Cmd::Redirected {
                body, redirects, ..
            } => {
                cmd(body, &s, src);
                for r in redirects {
                    ok(&r.span, &s, src, "redirect");
                    word(&r.target, &r.span, src);
                }
            }
            Cmd::Cond { expr, .. } => test_words(expr, &s, src),
            Cmd::Arith { .. } | Cmd::Unsupported { .. } => {}
        }
    }
    fn test_words(t: &TestExpr, parent: &Span, src: &str) {
        match t {
            TestExpr::Unary { operand, .. } => word(operand, parent, src),
            TestExpr::Binary { left, right, .. } => {
                word(left, parent, src);
                word(right, parent, src);
            }
            TestExpr::Word(w) => word(w, parent, src),
            TestExpr::Not(t) => test_words(t, parent, src),
            TestExpr::And(a, b) | TestExpr::Or(a, b) => {
                test_words(a, parent, src);
                test_words(b, parent, src);
            }
            TestExpr::Unknown => {}
        }
    }
    fn word(w: &Word, parent: &Span, src: &str) {
        ok(&w.span, parent, src, "word");
        parts(&w.parts, &w.span, src);
    }
    fn parts(ps: &[Part], parent: &Span, src: &str) {
        for p in ps {
            match p {
                Part::DoubleQuoted(inner) => parts(inner, parent, src),
                Part::CmdSubst { body, span } | Part::ProcSubst { body, span } => {
                    ok(span, parent, src, "substitution");
                    cmd(body, span, src);
                }
                Part::Brace(BraceExpr::List(ws)) => ws.iter().for_each(|w| word(w, parent, src)),
                _ => {}
            }
        }
    }
    cmd(c, &(0..src.len()), src);
}

/// Every command node, substitutions included, in source order.
fn all_cmds(c: &Cmd) -> Vec<&Cmd> {
    fn from_parts<'a>(ps: &'a [Part], out: &mut Vec<&'a Cmd>) {
        for p in ps {
            match p {
                Part::DoubleQuoted(inner) => from_parts(inner, out),
                Part::CmdSubst { body, .. } | Part::ProcSubst { body, .. } => go(body, out),
                Part::Brace(BraceExpr::List(ws)) => {
                    ws.iter().for_each(|w| from_parts(&w.parts, out))
                }
                _ => {}
            }
        }
    }
    fn go<'a>(c: &'a Cmd, out: &mut Vec<&'a Cmd>) {
        out.push(c);
        match c {
            Cmd::Simple(s) => {
                s.assigns
                    .iter()
                    .for_each(|a| from_parts(&a.value.parts, out));
                s.words.iter().for_each(|w| from_parts(&w.parts, out));
                s.redirects
                    .iter()
                    .for_each(|r| from_parts(&r.target.parts, out));
            }
            Cmd::Seq { items, .. } => items.iter().for_each(|i| go(&i.cmd, out)),
            Cmd::AndOr { first, rest, .. } => {
                go(first, out);
                rest.iter().for_each(|(_, c)| go(c, out));
            }
            Cmd::Pipeline { cmds, .. } => cmds.iter().for_each(|c| go(c, out)),
            Cmd::If {
                arms, otherwise, ..
            } => {
                for (a, b) in arms {
                    go(a, out);
                    go(b, out);
                }
                if let Some(o) = otherwise {
                    go(o, out);
                }
            }
            Cmd::Case { word, arms, .. } => {
                from_parts(&word.parts, out);
                arms.iter().for_each(|a| go(&a.body, out));
            }
            Cmd::For { items, body, .. } => {
                items
                    .iter()
                    .flatten()
                    .for_each(|w| from_parts(&w.parts, out));
                go(body, out);
            }
            Cmd::ArithFor { body, .. }
            | Cmd::Group { body, .. }
            | Cmd::Subshell { body, .. }
            | Cmd::FuncDef { body, .. } => go(body, out),
            Cmd::Loop { cond, body, .. } => {
                go(cond, out);
                go(body, out);
            }
            Cmd::Redirected {
                body, redirects, ..
            } => {
                go(body, out);
                redirects
                    .iter()
                    .for_each(|r| from_parts(&r.target.parts, out));
            }
            Cmd::Arith { .. } | Cmd::Cond { .. } | Cmd::Unsupported { .. } => {}
        }
    }
    let mut out = Vec::new();
    go(c, &mut out);
    out
}

/// The first simple command whose first word is `program`.
fn find_simple<'a>(c: &'a Cmd, program: &str) -> &'a SimpleCmd {
    all_cmds(c)
        .into_iter()
        .find_map(|c| match c {
            Cmd::Simple(s)
                if s.words.first().and_then(Word::literal).as_deref() == Some(program) =>
            {
                Some(s)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no `{program}` command"))
}

fn syntax_err(src: &str) -> &'static str {
    match parse(src) {
        Err(ParseError::Syntax { why, .. }) => why,
        other => panic!("{src:?}: expected a syntax error, got {other:?}"),
    }
}

/// The words of the only `for` in `src`.
fn for_items(src: &str) -> Vec<Word> {
    match p(src) {
        Cmd::For {
            items: Some(items), ..
        } => items,
        other => panic!("not a for with items: {}", show(&other)),
    }
}

const INCIDENT: &str = r#"count() { host=$1; repo=$2; tok=$3; url="https://$host/v2/$repo/tags/list?n=1000"; pages=0; total=0; first=""; while [ -n "$url" ] && [ $pages -lt 400 ]; do hdr=$(mktemp); body=$(curl -sS -D $hdr -H "Authorization: Bearer $tok" "$url"); n=$(echo "$body" | jq '.tags|length' 2>/dev/null); [ -z "$first" ] && first=$n; total=$((total+${n:-0})); pages=$((pages+1)); link=$(grep -i '^link:' $hdr | sed -E 's/.*<([^>]+)>.*/\1/' | tr -d '\r'); rm $hdr; if [ -n "$link" ]; then case "$link" in http*) url=$link;; *) url="https://$host$link";; esac; else url=""; fi; done; echo "$host/$repo: pages=$pages first_page=$first total=$total"; }; T=$(curl -s "https://auth.docker.io/token?service=registry.docker.io&scope=repository:envoyproxy/envoy:pull" | jq -r .token); count registry-1.docker.io envoyproxy/envoy "$T"; T=$(curl -s "https://ghcr.io/token?scope=repository:immich-app/immich-machine-learning:pull" | jq -r .token); count ghcr.io immich-app/immich-machine-learning "$T""#;

// Brace expansion and for-lists.

#[test]
fn brace_int_range_in_for() {
    let items = for_items("for i in {1..60}; do :; done");
    assert_eq!(items.len(), 1);
    assert!(matches!(
        items[0].parts.as_slice(),
        [Part::Brace(BraceExpr::IntRange {
            from: 1,
            to: 60,
            step: 1,
            width: 0
        })]
    ));
    assert_eq!(
        show_src("for i in {1..60}; do :; done"),
        "for(i in B{1..60 s1 w0}; [:])"
    );
}

#[test]
fn brace_descending_range() {
    assert_eq!(
        word(&for_items("for i in {5..1}; do :; done")[0]),
        "B{5..1 s1 w0}"
    );
}

#[test]
fn brace_zero_padded_range_has_width() {
    assert_eq!(
        word(&for_items("for i in {01..10}; do :; done")[0]),
        "B{1..10 s1 w2}"
    );
}

#[test]
fn brace_range_with_step() {
    assert_eq!(
        word(&for_items("for i in {1..10..3}; do :; done")[0]),
        "B{1..10 s3 w0}"
    );
}

#[test]
fn brace_char_range() {
    assert_eq!(
        word(&for_items("for c in {a..e}; do :; done")[0]),
        "B{a..e s1}"
    );
}

#[test]
fn brace_list() {
    let items = for_items("for h in {x,y,z}; do :; done");
    assert_eq!(word(&items[0]), "B{x,y,z}");
    match &items[0].parts[0] {
        Part::Brace(BraceExpr::List(alts)) => {
            let lits: Vec<_> = alts
                .iter()
                .map(|w| w.literal().unwrap_or_default())
                .collect();
            assert_eq!(lits, ["x", "y", "z"]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn brace_list_holds_words_with_expansions() {
    assert_eq!(
        show_src("echo pre{$a,\"b c\",{1..2}}post"),
        "[echo preB{$a,\"b c\",B{1..2 s1 w0}}post]"
    );
}

#[test]
fn quoted_brace_is_literal() {
    let items = for_items("for i in \"{1..5}\"; do :; done");
    match items[0].parts.as_slice() {
        [Part::DoubleQuoted(inner)] => {
            assert!(matches!(inner.as_slice(), [Part::Lit(s)] if s == "{1..5}"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn escaped_brace_is_literal() {
    let items = for_items(r"for i in \{1..3\}; do :; done");
    assert_eq!(word(&items[0]), r"\{1..3\}");
    assert!(!items[0].parts.iter().any(|p| matches!(p, Part::Brace(_))));
    assert_eq!(items[0].literal().as_deref(), Some("{1..3}"));
}

#[test]
fn non_expanding_braces_are_literal() {
    assert_eq!(show_src("echo {} {x} a{b c}"), "[echo {} {x} a{b c}]");
    assert_eq!(
        show_src("echo ${x} '{a,b}' {1..}"),
        "[echo $x '{a,b}' {1..}]"
    );
    // No brace expansion in an assignment value.
    assert_eq!(show_src("x={a,b}"), "[x={a,b}]");
}

#[test]
fn for_over_parameter() {
    let items = for_items("for i in $items; do :; done");
    assert!(matches!(
        items[0].parts.as_slice(),
        [Part::Param(ParamRef::Named(n))] if n == "items"
    ));
}

#[test]
fn for_over_glob() {
    let items = for_items("for f in *.json; do :; done");
    assert!(matches!(items[0].parts.as_slice(), [Part::Glob('*'), Part::Lit(s)] if s == ".json"));
}

#[test]
fn array_expansion_is_param_op() {
    let items = for_items("for x in ${arr[@]}; do :; done");
    assert!(matches!(
        items[0].parts.as_slice(),
        [Part::ParamOp { name, raw }] if name == "arr" && raw == "${arr[@]}"
    ));
}

#[test]
fn for_without_in_and_newline_before_do() {
    assert_eq!(show_src("for v; do echo $v; done"), "for(v; [echo $v])");
    assert_eq!(show_src("for v do echo $v; done"), "for(v; [echo $v])");
    assert_eq!(
        show_src("for v in a b\ndo\n  echo $v\ndone"),
        "for(v in a b; [echo $v])"
    );
}

#[test]
fn arithmetic_for() {
    assert_eq!(
        show_src("for ((i=0; i<3; i++)); do echo $i; done"),
        "for(((i = 0); (i < 3); i++); [echo $i])"
    );
    assert_eq!(
        show_src("for ((;;)); do break; done"),
        "for((_; _; _); [break])"
    );
}

// Delimiters belong to their own context.

#[test]
fn do_for_nesting() {
    assert_eq!(
        show_src("while true; do for x in a; do echo $x; done; done"),
        "while([true]; for(x in a; [echo $x]))"
    );
}

#[test]
fn substitution_loop_does_not_pair_with_outer_loop() {
    let src = r#"while test "$(for x in a; do printf x; done)" = x; do curl "$url"; done"#;
    assert_eq!(
        show_src(src),
        r#"while([test "$(for(x in a; [printf x]))" = x]; [curl "$url"])"#
    );
}

#[test]
fn reserved_words_only_in_command_position() {
    assert_eq!(
        show_src("echo do done if fi { }"),
        "[echo do done if fi { }]"
    );
}

#[test]
fn while_with_curl_condition() {
    assert_eq!(
        show_src("while curl -fsS https://x/a; do sleep 1; done"),
        "while([curl -fsS https://x/a]; [sleep 1])"
    );
}

#[test]
fn until_loop() {
    assert_eq!(
        show_src("until gh pr view 1; do sleep 15; done"),
        "until([gh pr view 1]; [sleep 15])"
    );
}

#[test]
fn if_is_not_a_loop() {
    assert_eq!(
        show_src("if curl -fsS https://x; then curl https://y; fi"),
        "if([curl -fsS https://x] then [curl https://y])"
    );
    assert_eq!(
        show_src("if a; then b; elif c; then d; else e; fi"),
        "if([a] then [b] elif [c] then [d] else [e])"
    );
}

// Case.

#[test]
fn case_terminators_and_patterns() {
    let src = "case $x in a|b) echo ab;; (c) echo c;& d) echo d;;& *) echo other;; esac";
    assert_eq!(
        show_src(src),
        "case($x: a|b) [echo ab] ;; c) [echo c] ;& d) [echo d] ;;& G(*)) [echo other] ;;)"
    );
}

#[test]
fn case_last_arm_without_terminator_and_empty_body() {
    assert_eq!(
        show_src("case $x in\n  a) ;;\n  b) echo b\nesac"),
        "case($x: a) seq() ;; b) [echo b] ;;)"
    );
}

// Functions.

#[test]
fn function_definition_forms() {
    assert_eq!(show_src("f() { echo a; }"), "fn f({[echo a]})");
    assert_eq!(show_src("f () ( echo a )"), "fn f(sub([echo a]))");
    assert_eq!(show_src("function f { echo a; }"), "fn f({[echo a]})");
    assert_eq!(show_src("function f() { echo a; }"), "fn f({[echo a]})");
    assert_eq!(show_src("f() {\n  echo a\n}"), "fn f({[echo a]})");
}

#[test]
fn function_then_calls() {
    assert_eq!(
        show_src("count() { echo \"$1\"; }; count a b; count c"),
        "seq(fn count({[echo \"$1\"]}); [count a b]; [count c])"
    );
}

#[test]
fn function_body_must_be_compound() {
    assert_eq!(
        syntax_err("f() echo a"),
        "a function body must be a compound command"
    );
}

// Counters and subshells.

#[test]
fn counter_loop_with_subshell_increment() {
    let src = r#"i=0; while [ "$i" -lt 3 ]; do curl "$url"; ( i=$((i+1)) ); done"#;
    assert_eq!(
        show_src(src),
        r#"seq([i=0]; while([[ "$i" -lt 3 ]]; seq([curl "$url"]; sub([i=$(((i + 1)))]))))"#
    );
    let c = p(src);
    let sub = all_cmds(&c)
        .into_iter()
        .find(|c| matches!(c, Cmd::Subshell { .. }))
        .expect("a subshell");
    let Cmd::Subshell { body, .. } = sub else {
        unreachable!()
    };
    let Cmd::Simple(s) = body.as_ref() else {
        panic!("{}", show(body))
    };
    assert_eq!(s.assigns[0].name, "i");
    assert!(matches!(
        s.assigns[0].value.parts.as_slice(),
        [Part::Arith(_)]
    ));
}

// Words, quoting, redirections.

#[test]
fn heredoc() {
    let src = "cat <<EOF > out\nhello $x\n  EOF\nEOF\necho after";
    assert_eq!(
        show_src(src),
        "seq([cat <<EOF\"hello $x\\n  EOF\\n\" >out]; [echo after])"
    );
}

#[test]
fn heredoc_forms() {
    assert_eq!(
        show_src("cat <<-'END'\n\tbody $x\n\tEND\n"),
        "[cat <<-'END'\"body $x\\n\"]"
    );
    assert_eq!(show_src("cat <<\"E\"\nx\nE"), "[cat <<\"E\"\"x\\n\"]");
    // Two heredocs on one line: bodies follow in order.
    assert_eq!(
        show_src("cat <<A; cat <<B\na\nA\nb\nB\necho z"),
        "seq([cat <<A\"a\\n\"]; [cat <<B\"b\\n\"]; [echo z])"
    );
    // The body is data: nothing in it is parsed.
    assert_eq!(
        show_src("cat <<X\ndone ) ;;\nX"),
        "[cat <<X\"done ) ;;\\n\"]"
    );
}

#[test]
fn redirections() {
    assert_eq!(
        show_src("cmd >out 2>&1 2>/dev/null <in >>log &>all <<<word >|f 3<&0"),
        "[cmd >out 2>&1 2>/dev/null <in >>log &>all <<<word >|f 3<&0]"
    );
    let c = p("cmd 2>&1");
    let Cmd::Simple(s) = &c else { panic!() };
    assert_eq!(s.redirects[0].op, "2>&");
    assert_eq!(s.redirects[0].target.literal().as_deref(), Some("1"));
}

#[test]
fn redirected_compound_command() {
    assert_eq!(
        show_src("while read l; do echo $l; done < <(curl -s https://h/x) 2>/dev/null"),
        "redir(while([read l]; [echo $l]) <<([curl -s https://h/x]) 2>/dev/null)"
    );
    assert_eq!(show_src("{ a; b; } >log"), "redir({seq([a]; [b])} >log)");
}

#[test]
fn ansi_c_quoting() {
    let c = p(r"printf $'a\tb\x41\n'");
    let Cmd::Simple(s) = &c else { panic!() };
    assert!(matches!(s.words[1].parts.as_slice(), [Part::SingleQuoted(v)] if v == "a\tbA\n"));
}

#[test]
fn backquotes() {
    assert_eq!(
        show_src("echo `date +%s` \"`id -u`\""),
        "[echo $([date +%s]) \"$([id -u])\"]"
    );
    assert_eq!(
        show_src(r"echo `echo \`date\``"),
        "[echo $(unsupported(escaped characters inside backquotes))]"
    );
}

#[test]
fn arithmetic_expansion() {
    assert_eq!(show_src("echo $(( i + 1 ))"), "[echo $(((i + 1)))]");
    assert_eq!(
        show_src("echo $((a = b * 2 + -c ** 2, x ? 0x1f : 010))"),
        "[echo $((((a = ((b * 2) + ((-c) ** 2))), (x ? 31 : 8))))]"
    );
    assert_eq!(show_src("echo $((2#101 + 16#ff))"), "[echo $(((5 + 255)))]");
    assert_eq!(
        show_src("echo $((total+${n:-0}))"),
        "[echo $(((total + ?{${n:-0}})))]"
    );
    assert_eq!(
        show_src("echo $((a[1] + $1))"),
        "[echo $(((?{a[1]} + ?{$1})))]"
    );
    assert_eq!(show_src("echo $((1 +* 2))"), "[echo $((?{1 +* 2}))]");
    assert_eq!(show_src("echo $((09))"), "[echo $((?{09}))]");
}

#[test]
fn dollar_paren_paren_that_is_a_subshell() {
    assert_eq!(
        show_src("echo $((a) | b)"),
        "[echo $(pipe(sub([a]) | [b]))]"
    );
}

#[test]
fn arithmetic_command() {
    assert_eq!(show_src("(( i++ ))"), "arith(i++)");
    assert_eq!(
        show_src("((x += 2)) && echo"),
        "(arith((x += 2)) && [echo])"
    );
    assert_eq!(show_src("(( --i ))"), "arith(--i)");
}

#[test]
fn double_bracket_conditional() {
    assert_eq!(
        show_src("[[ -n $x && $y -lt 3 ]]"),
        "cond(((-n $x) && ($y -lt 3)))"
    );
    assert_eq!(
        show_src("[[ ! ( $a == b* || $a < c ) ]]"),
        "cond((! (($a == bG(*)) || ($a < c))))"
    );
    assert_eq!(
        show_src("[[ $x =~ ^(a|b)$ ]] && echo m"),
        "(cond(($x =~ ^(a|b)$)) && [echo m])"
    );
}

#[test]
fn pipelines_and_lists() {
    assert_eq!(show_src("! a | b"), "pipe(! [a] | [b])");
    assert_eq!(show_src("a |& b"), "pipe([a] | [b])");
    assert_eq!(show_src("a && b || c"), "([a] && [b] || [c])");
    assert_eq!(show_src("a & b"), "seq([a] &; [b])");
    assert_eq!(show_src("a &"), "seq([a] &)");
    assert_eq!(show_src("a\nb\n\nc"), "seq([a]; [b]; [c])");
    assert_eq!(show_src("a &&\n b |\n c"), "([a] && pipe([b] | [c]))");
    assert_eq!(show_src(""), "seq()");
    assert_eq!(show_src("time -p curl x"), "[curl x]");
}

#[test]
fn comments_and_continuations() {
    assert_eq!(
        show_src("# lead\necho a#b # tail\ncurl \\\n  -s x"),
        "seq([echo a#b]; [curl -s x])"
    );
}

#[test]
fn parameters() {
    assert_eq!(
        show_src("echo $a ${b} $1 ${10} $@ $* $# $? $$ $! $- $0 ${#a} ${a:-x} $ $"),
        "[echo $a $b $1 $10 $@ $@ $# $? $$ $! $- $0 %{a|${#a}} %{a|${a:-x}} $ $]"
    );
    let c = p("echo ${10} $0");
    let Cmd::Simple(s) = &c else { panic!() };
    assert!(matches!(
        s.words[1].parts[0],
        Part::Param(ParamRef::Positional(10))
    ));
    assert!(matches!(
        s.words[2].parts[0],
        Part::Param(ParamRef::Special('0'))
    ));
}

#[test]
fn double_quote_escapes() {
    assert_eq!(show_src(r#"echo "a\$b\"c\d\\""#), r#"[echo "a\$b\"c\d\\"]"#);
    let c = p(r#"echo "\d""#);
    let Cmd::Simple(s) = &c else { panic!() };
    assert!(matches!(
        &s.words[1].parts[0],
        Part::DoubleQuoted(inner) if matches!(inner.as_slice(), [Part::Lit(l)] if l == "\\d")
    ));
}

#[test]
fn globs_tildes_process_substitution() {
    assert_eq!(
        show_src("ls [ab]*.? ~/x ~root a~ [ x"),
        "[ls G([)ab]G(*).G(?) T(~)/x T(~root) a~ [ x]"
    );
    assert_eq!(show_src("diff <(a) >(b)"), "[diff <([a]) <([b])]");
    assert_eq!(show_src("x=~/bin"), "[x=T(~)/bin]");
}

#[test]
fn assignments() {
    assert_eq!(show_src("a=1 b+=2 c= cmd d=4"), "[a=1 b+=2 c= cmd d=4]");
    let c = p("a=1 b+=2 cmd");
    let Cmd::Simple(s) = &c else { panic!() };
    assert_eq!(s.assigns.len(), 2);
    assert!(s.assigns[1].append);
}

// Unsupported constructs and errors.

#[test]
fn unsupported_constructs() {
    assert_eq!(show_src("arr=(a b c)"), "unsupported(array assignment)");
    assert_eq!(
        show_src("a[1]=x; echo"),
        "seq(unsupported(array assignment); [echo])"
    );
    assert_eq!(
        show_src("select x in a b; do echo $x; done; echo after"),
        "seq(unsupported(select); [echo after])"
    );
    assert_eq!(
        show_src("coproc NAME { cat; }; echo after"),
        "seq(unsupported(coproc); [echo after])"
    );
    assert_eq!(show_src("coproc cat"), "unsupported(coproc)");
    assert_eq!(
        show_src("echo $(case)"),
        "[echo $(unsupported(unparseable substitution))]"
    );
}

#[test]
fn syntax_errors() {
    assert_eq!(syntax_err("echo 'abc"), "unterminated single quote");
    assert_eq!(syntax_err("echo \"abc"), "unterminated double quote");
    assert_eq!(syntax_err("echo $(ls"), "unterminated substitution");
    assert_eq!(syntax_err("echo `ls"), "unterminated backquote");
    assert_eq!(syntax_err("cat <<EOF\nabc"), "unterminated heredoc");
    assert_eq!(syntax_err("while a; do b"), "unbalanced `do`/`done`");
    assert_eq!(syntax_err("a; done"), "unbalanced `do`/`done`");
    assert_eq!(syntax_err("if a; then b"), "`if` without `fi`");
    assert_eq!(syntax_err("case x in a) b"), "expected `;;` or `esac`");
    assert_eq!(syntax_err("( a"), "`(` without `)`");
    assert_eq!(syntax_err("a )"), "unexpected `)`");
    assert_eq!(syntax_err("a && ;"), "expected a command");
}

// The incident, verbatim.

#[test]
fn incident_command_parses_fully() {
    let c = p(INCIDENT);
    let cmds = all_cmds(&c);
    assert!(
        !cmds.iter().any(|c| matches!(c, Cmd::Unsupported { .. })),
        "{}",
        show(&c)
    );
    let Cmd::Seq { items, .. } = &c else {
        panic!("{}", show(&c))
    };
    let top: Vec<String> = items.iter().skip(1).map(|i| show(&i.cmd)).collect();
    assert_eq!(
        top,
        [
            r#"[T=$(pipe([curl -s "https://auth.docker.io/token?service=registry.docker.io&scope=repository:envoyproxy/envoy:pull"] | [jq -r .token]))]"#,
            r#"[count registry-1.docker.io envoyproxy/envoy "$T"]"#,
            r#"[T=$(pipe([curl -s "https://ghcr.io/token?scope=repository:immich-app/immich-machine-learning:pull"] | [jq -r .token]))]"#,
            r#"[count ghcr.io immich-app/immich-machine-learning "$T"]"#,
        ]
    );
    let Cmd::FuncDef { name, body, .. } = &items[0].cmd else {
        panic!("{}", show(&items[0].cmd))
    };
    assert_eq!(name, "count");
    let Cmd::Group { body, .. } = body.as_ref() else {
        panic!()
    };
    let Cmd::Seq { items: fbody, .. } = body.as_ref() else {
        panic!()
    };
    let fbody: Vec<String> = fbody.iter().map(|i| show(&i.cmd)).collect();
    assert_eq!(
        fbody,
        [
            "[host=$1]",
            "[repo=$2]",
            "[tok=$3]",
            r#"[url="https://$host/v2/$repo/tags/list?n=1000"]"#,
            "[pages=0]",
            "[total=0]",
            r#"[first=""]"#,
            concat!(
                r#"while(([[ -n "$url" ]] && [[ $pages -lt 400 ]]); seq("#,
                r#"[hdr=$([mktemp])]; "#,
                r#"[body=$([curl -sS -D $hdr -H "Authorization: Bearer $tok" "$url"])]; "#,
                r#"[n=$(pipe([echo "$body"] | [jq '.tags|length' 2>/dev/null]))]; "#,
                r#"([[ -z "$first" ]] && [first=$n]); "#,
                r#"[total=$(((total + ?{${n:-0}})))]; "#,
                r#"[pages=$(((pages + 1)))]; "#,
                r#"[link=$(pipe([grep -i '^link:' $hdr] | [sed -E 's/.*<([^>]+)>.*/\1/'] | [tr -d '\r']))]; "#,
                r#"[rm $hdr]; "#,
                r#"if([[ -n "$link" ]] then case("$link": httpG(*)) [url=$link] ;; G(*)) [url="https://$host$link"] ;;) else [url=""])))"#,
            ),
            r#"[echo "$host/$repo: pages=$pages first_page=$first total=$total"]"#,
        ]
    );
    let curl = find_simple(&c, "curl");
    assert_eq!(
        &INCIDENT[curl.span.clone()],
        r#"curl -sS -D $hdr -H "Authorization: Bearer $tok" "$url""#
    );
}

// Limits.

#[test]
fn deep_nesting_is_a_depth_limit() {
    let src = format!("{}a{}", "(".repeat(1000), ")".repeat(1000));
    assert_eq!(
        parse(&src).unwrap_err(),
        ParseError::Limit(Exhausted::Depth)
    );
    let src = format!("echo {}x{}", "$(".repeat(1000), ")".repeat(1000));
    assert_eq!(
        parse(&src).unwrap_err(),
        ParseError::Limit(Exhausted::Depth)
    );
    let src = format!("echo $(({}1{}))", "(".repeat(1000), ")".repeat(1000));
    assert_eq!(
        parse(&src).unwrap_err(),
        ParseError::Limit(Exhausted::Depth)
    );
    let src = format!("[[ {}x{} ]]", "( ".repeat(1000), " )".repeat(1000));
    assert_eq!(
        parse(&src).unwrap_err(),
        ParseError::Limit(Exhausted::Depth)
    );
    let src = format!("echo {}x{}", "{a,".repeat(1000), "}".repeat(1000));
    assert_eq!(
        parse(&src).unwrap_err(),
        ParseError::Limit(Exhausted::Depth)
    );
}

#[test]
fn huge_brace_range_is_not_materialised() {
    let start = std::time::Instant::now();
    let items = for_items("for i in {1..999999999999}; do :; done");
    assert!(matches!(
        items[0].parts.as_slice(),
        [Part::Brace(BraceExpr::IntRange {
            from: 1,
            to: 999_999_999_999,
            ..
        })]
    ));
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn huge_sequence_is_a_node_limit() {
    let src = ":;".repeat(10_000);
    assert_eq!(
        parse(&src).unwrap_err(),
        ParseError::Limit(Exhausted::Nodes)
    );
}

#[test]
fn pathological_words_stay_bounded() {
    let start = std::time::Instant::now();
    for src in [
        format!("echo {}", "[".repeat(200_000)),
        format!("echo {}", "{".repeat(200_000)),
        format!("echo {}", "{a".repeat(100_000)),
    ] {
        let _ = parse(&src);
    }
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}

// Spans.

#[test]
fn spans_point_into_the_original_source() {
    let src = "while true; do x=$(curl -s https://h/a); done; if a; then b; fi && c";
    let c = p(src);
    let curl = find_simple(&c, "curl");
    assert_eq!(&src[curl.span.clone()], "curl -s https://h/a");
    let subst = all_cmds(&c)
        .into_iter()
        .find_map(|c| match c {
            Cmd::Simple(s) => s.assigns.first().cloned(),
            _ => None,
        })
        .expect("the assignment");
    assert_eq!(&src[subst.span.clone()], "x=$(curl -s https://h/a)");
    let Part::CmdSubst { span, .. } = &subst.value.parts[0] else {
        panic!()
    };
    assert_eq!(&src[span.clone()], "$(curl -s https://h/a)");
    let Cmd::Seq { items, .. } = &c else { panic!() };
    assert_eq!(
        &src[items[0].cmd.span()],
        "while true; do x=$(curl -s https://h/a); done"
    );
    let Cmd::AndOr { first, .. } = &items[1].cmd else {
        panic!()
    };
    assert_eq!(&src[first.span()], "if a; then b; fi");
    assert_eq!(&src[items[1].cmd.span()], "if a; then b; fi && c");
}

#[test]
fn spans_of_case_arms_words_and_backquotes() {
    let src = "case \"$x\" in (a|b) echo `curl -s u`;; esac";
    let c = p(src);
    let Cmd::Case { word: w, arms, .. } = &c else {
        panic!()
    };
    assert_eq!(&src[w.span.clone()], "\"$x\"");
    assert_eq!(&src[arms[0].span.clone()], "(a|b) echo `curl -s u`;;");
    let curl = find_simple(&c, "curl");
    assert_eq!(&src[curl.span.clone()], "curl -s u");
}

/// Every prefix of every input either parses or fails — never panics — and
/// whatever parses has consistent spans.
#[test]
fn every_prefix_parses_or_fails_cleanly() {
    let corpus = [
        INCIDENT,
        "for i in {1..60}; do :; done",
        "for i in {01..10..2} {a..e} {x,y{1..2},z} \"{1..5}\" \\{1..3\\}; do :; done",
        r#"while test "$(for x in a; do printf x; done)" = x; do curl "$url"; done"#,
        "case $x in a|b) echo ab;; (c) echo c;& d) echo d;;& *) echo other;; esac",
        "f() { echo a; }; function g() ( b ); count() { :; }; count a",
        "cat <<A; cat <<-'B'\na\nA\n\tb\n\tB\necho z 2>&1 &>/dev/null",
        "echo $'a\\tb' `date` $(( i + 1 )) ${a:-$(b)} ~/x [ab]* <(c) $@",
        "(( i++ )); [[ -n $x && $y =~ ^(a|b)$ ]] || ! a | b & for ((i=0;i<3;i++)); do :; done",
        "coproc c { :; }; select x in a; do :; done; arr=(a b); a[1]=2",
        "while read l; do echo $l; done < <(curl -s x) 2>/dev/null",
        "é=1; echo \"é$é\" 'é' é\\é",
        "echo ${é} ${#é} ${!é} $((é)) {é,a} [é] ~é",
    ];
    for src in corpus {
        for end in (0..=src.len()).filter(|&i| src.is_char_boundary(i)) {
            let prefix = &src[..end];
            if let Ok(c) = parse(prefix) {
                check_spans(&c, prefix);
            }
        }
    }
}

/// Random short strings over the characters that matter to the grammar:
/// no input may panic or hang. A fixed-seed generator keeps it
/// deterministic without a dependency.
#[test]
fn random_inputs_never_panic() {
    const ALPHABET: &[u8] = b" \t\n'\"\\$(){}[]<>|&;#`~*?=!,.:-+012abdefinorstuw";
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as usize
    };
    for _ in 0..20_000 {
        let len = next() % 48;
        let src: String = (0..len)
            .map(|_| ALPHABET[next() % ALPHABET.len()] as char)
            .collect();
        if let Ok(c) = parse(&src) {
            check_spans(&c, &src);
        }
    }
}
