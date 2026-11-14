//! httpie (`http`, `https`): `[METHOD] URL [REQUEST_ITEM…]` — one request
//! to the URL. Request items (`a=b`, `X-H:v`, `q==1`) are never
//! destinations, even when a value in one is a URL.

use super::super::domain::Count;
use super::super::effects::TargetSet;
use super::super::state::AbsVal;
use super::{
    flag, flag_c, operand_target, operands, parse, transfer, val, val_c, Arg, OptSpec, Syntax, Tok,
    Transfer,
};

const OPTS: &[OptSpec] = &[
    flag("offline"),
    flag_c('F', "follow"),
    val_c('a', "auth"),
    val_c('A', "auth-type"),
    val_c('o', "output"),
    val("session"),
    val("session-read-only"),
    val("verify"),
    val("cert"),
    val("cert-key"),
    val("cert-key-pass"),
    val("ssl"),
    val("ciphers"),
    val("proxy"),
    val("timeout"),
    val("max-redirects"),
    val("max-headers"),
    val_c('p', "print"),
    val_c('P', "history-print"),
    val("pretty"),
    val_c('s', "style"),
    val("format-options"),
    val("response-charset"),
    val("response-mime"),
    val("boundary"),
    val("default-scheme"),
    val("raw"),
    flag_c('j', "json"),
    flag_c('f', "form"),
    flag("multipart"),
    flag_c('h', "headers"),
    flag_c('b', "body"),
    flag_c('m', "meta"),
    flag_c('v', "verbose"),
    flag("all"),
    flag_c('S', "stream"),
    flag_c('q', "quiet"),
    flag_c('d', "download"),
    flag_c('c', "continue"),
    flag("check-status"),
    flag_c('I', "ignore-stdin"),
    flag("ignore-netrc"),
    flag("chunked"),
    flag_c('x', "compress"),
    flag("sorted"),
    flag("unsorted"),
    flag("path-as-is"),
    flag("debug"),
    flag("traceback"),
    flag("version"),
    flag("help"),
];

const SYNTAX: Syntax = Syntax {
    opts: OPTS,
    negatable: true,
    go_bools: false,
};

pub(super) fn model(program: &'static str, args: &[Arg]) -> Vec<Transfer> {
    let toks = parse(args, &SYNTAX);
    let mut follow = false;
    for tok in &toks {
        match tok {
            // Builds the request and prints it; nothing is sent.
            Tok::Flag {
                name: "offline",
                on: true,
            } => return Vec::new(),
            Tok::Flag { name: "follow", on } => follow = *on,
            _ => {}
        }
    }
    let ops = operands(&toks);
    let is_method = |a: &Arg| {
        a.val
            .as_exact()
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_uppercase()))
    };
    let url = match ops.as_slice() {
        [(m, _, false), rest @ ..] if is_method(m) => rest.first(),
        all => all.first(),
    };
    let Some(&(arg, after_unknown, opaque)) = url else {
        return Vec::new();
    };
    let targets = if after_unknown {
        operand_target(&arg.val, true)
    } else {
        target(&arg.val)
    };
    let mut t = transfer(
        program,
        if opaque { Count::MAYBE } else { Count::ONE },
        targets,
        arg.span.clone(),
    );
    if follow {
        t.uncounted.push("redirects");
    }
    vec![t]
}

/// httpie reads `:3000/x` and `:/x` as `localhost`.
fn target(val: &AbsVal) -> TargetSet {
    let local = |s: &str| s.starts_with(':');
    match val {
        AbsVal::Values(vs) => vs
            .iter()
            .map(|v| {
                if local(v) {
                    TargetSet::local()
                } else {
                    super::target_of(&AbsVal::exact(v.as_str()))
                }
            })
            .reduce(|a, b| a.union(&b))
            .unwrap_or_else(|| super::target_of(val)),
        AbsVal::Prefix(p) if local(p) => TargetSet::local(),
        _ => super::target_of(val),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;

    #[test]
    fn a_method_then_a_localhost_shorthand() {
        let t = only("http", &["POST", ":3000/x", "a=b"]);
        assert!(t.targets.is_local_only());
    }

    #[test]
    fn the_url_without_a_method() {
        let t = only(
            "https",
            &["-h", "api.example.com/x", "q==https://b.example/"],
        );
        assert_eq!(t.targets, TargetSet::host("api.example.com"));
        assert_eq!(t.program, "https");
    }

    #[test]
    fn offline_sends_nothing() {
        assert_eq!(run("http", &["--offline", "a.example"]), Some(vec![]));
    }
}
