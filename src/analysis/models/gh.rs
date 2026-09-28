//! `gh api`: one request to the GitHub API per invocation, on the
//! authenticated rate budget, unless `--paginate` follows the pages.
//!
//! The host is fixed by `gh` itself — `api.github.com`, or the API host of
//! `--hostname` — so, unlike curl, an option the model does not know
//! cannot turn some later argument into a destination. Such an option is
//! noted, and the host stays. What CAN redirect the request is an argument
//! whose value is unknown: it may be `--hostname=…` itself, so that adds
//! an unresolved destination. `GH_HOST` in the environment also redirects
//! it; the analysis does not see the environment (see `docs/analysis.md`).

use super::super::domain::{Count, Why};
use super::super::effects::{Auth, Factor, FactorKind, TargetSet, Unresolved};
use super::super::ir::Span;
use super::super::state::AbsVal;
use super::{
    flag, flag_c, parse, target_of, transfer, val, val_c, Arg, OptSpec, Syntax, Tok, Transfer,
};

const OPTS: &[OptSpec] = &[
    val("hostname"),
    flag("paginate"),
    flag("slurp"),
    val_c('X', "method"),
    val_c('H', "header"),
    val_c('f', "raw-field"),
    val_c('F', "field"),
    val_c('q', "jq"),
    val_c('t', "template"),
    val("input"),
    val("cache"),
    val_c('p', "preview"),
    flag_c('i', "include"),
    flag("silent"),
    flag("verbose"),
    flag_c('h', "help"),
];

const SYNTAX: Syntax = Syntax {
    opts: OPTS,
    negatable: false,
    go_bools: true,
};

pub(super) fn model(args: &[Arg], span: Span) -> Option<Vec<Transfer>> {
    if args.first()?.val.as_exact()? != "api" {
        return None;
    }
    let mut host: Option<AbsVal> = None;
    let mut paginate = false;
    let mut endpoint: Option<&Arg> = None;
    let mut unknown = false;
    let mut opaque = false;
    for tok in parse(&args[1..], &SYNTAX) {
        match tok {
            Tok::Value {
                name: "hostname",
                val,
                ..
            } => host = Some(val),
            Tok::Flag {
                name: "paginate",
                on,
            } => paginate = on,
            Tok::Operand { arg, .. } => {
                endpoint = endpoint.or(Some(arg));
            }
            Tok::Opaque { arg, .. } => {
                opaque = true;
                endpoint = endpoint.or(Some(arg));
            }
            Tok::Unknown => unknown = true,
            _ => {}
        }
    }
    // gh refuses to run without an endpoint.
    let Some(ep) = endpoint else {
        return Some(Vec::new());
    };
    let mut targets = match ep.val.as_exact() {
        // A full URL names its own host.
        Some(u) if u.starts_with("https://") || u.starts_with("http://") => target_of(&ep.val),
        _ => match &host {
            Some(h) => api_host(h),
            None => TargetSet::host("api.github.com"),
        },
    };
    if opaque {
        targets = targets.union(&TargetSet::unresolved(Unresolved::Dynamic));
    }
    let mut per_call = Count::ONE;
    let mut builtin = Vec::new();
    if paginate {
        let pages = Count::uncapped(1, Why::Paginate);
        per_call = per_call.mul(pages);
        builtin.push(Factor {
            kind: FactorKind::Builtin {
                what: "--paginate".into(),
            },
            count: pages,
            span: span.clone(),
        });
    }
    let mut t = transfer("gh api", per_call, targets, ep.span.clone());
    t.auth = Auth::GhApi;
    t.builtin = builtin;
    if unknown {
        t.uncounted
            .push("whatever an option the analysis does not model adds");
    }
    Some(vec![t])
}

/// The API host for `--hostname`: github.com's API lives on its own host,
/// GitHub Enterprise Server's on the named host.
fn api_host(h: &AbsVal) -> TargetSet {
    match h.as_exact().map(str::to_ascii_lowercase).as_deref() {
        Some("github.com") => TargetSet::host("api.github.com"),
        Some(g) if g.ends_with(".ghe.com") => TargetSet::host(format!("api.{g}")),
        _ => target_of(h),
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::domain::Upper;
    use super::super::testutil::*;
    use super::*;

    #[test]
    fn api_is_one_authenticated_request() {
        let t = only("gh", &["api", "repos/x/y"]);
        assert_eq!(t.targets, TargetSet::host("api.github.com"));
        assert_eq!(t.auth, Auth::GhApi);
        assert_eq!(t.per_call, Count::ONE);
    }

    #[test]
    fn paginate_is_uncapped() {
        let t = only("gh", &["api", "--paginate", "repos/x/y/issues"]);
        assert_eq!(t.per_call.upper, Upper::Uncapped(Why::Paginate));
    }

    #[test]
    fn hostname_moves_the_host() {
        let t = only("gh", &["api", "--hostname", "ghe.corp", "x"]);
        assert_eq!(t.targets, TargetSet::host("ghe.corp"));
    }

    #[test]
    fn an_unknown_option_keeps_the_fixed_host() {
        let t = only("gh", &["api", "--frob", "x", "repos/x/y"]);
        assert_eq!(t.targets, TargetSet::host("api.github.com"));
        assert!(!t.uncounted.is_empty());
    }

    #[test]
    fn other_subcommands_are_not_modelled() {
        assert!(run("gh", &["pr", "view", "3"]).is_none());
    }
}
