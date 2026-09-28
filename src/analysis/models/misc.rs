//! One-request clients: `npm view` and `git ls-remote`.
//!
//! `git fetch` and `git clone` are not modelled: how many requests they
//! make depends on the protocol negotiation, and they are rarely what a
//! fan-out loop repeats.

use super::super::domain::Count;
use super::super::effects::{TargetSet, Unresolved};
use super::super::ir::Span;
use super::super::state::{url_host, AbsVal};
use super::Arg;
use super::{
    flag, flag_c, host_target, operands, parse, target_of, transfer, val, val_c, Syntax, Tok,
    Transfer,
};

const NPM: Syntax = Syntax {
    opts: &[
        val("registry"),
        val_c('w', "workspace"),
        val("userconfig"),
        val("globalconfig"),
        val("cache"),
        val("loglevel"),
        val("prefix"),
        flag("json"),
        flag_c('l', "long"),
        flag_c('g', "global"),
        flag_c('p', "parseable"),
        flag("workspaces"),
        flag("include-workspace-root"),
        flag("prefer-offline"),
        flag("prefer-online"),
        flag("offline"),
        flag_c('s', "silent"),
        flag_c('q', "quiet"),
    ],
    negatable: true,
    go_bools: true,
};

/// `npm view|info|show|v PKG [FIELD…]`: one registry request.
///
/// The registry is fixed unless `--registry` says otherwise, so an option
/// the table lacks cannot redirect it; an argument whose value is unknown
/// can (it may be `--registry=…`), and adds an unresolved destination. A
/// registry set in `.npmrc` is not visible to the analysis.
pub(super) fn npm(args: &[Arg], span: Span) -> Option<Vec<Transfer>> {
    let toks = parse(args, &NPM);
    let ops = operands(&toks);
    let (cmd, _, false) = ops.first()? else {
        return None;
    };
    if !matches!(cmd.val.as_exact(), Some("view" | "info" | "show" | "v")) {
        return None;
    }
    let mut targets = TargetSet::host("registry.npmjs.org");
    for tok in &toks {
        match tok {
            Tok::Flag {
                name: "offline",
                on: true,
            } => return Some(Vec::new()),
            Tok::Value {
                name: "registry",
                val,
                ..
            } => targets = target_of(val),
            _ => {}
        }
    }
    if ops.iter().any(|(_, _, opaque)| *opaque) {
        targets = targets.union(&TargetSet::unresolved(Unresolved::Dynamic));
    }
    let at = ops.get(1).map_or(span, |(a, _, _)| a.span.clone());
    Some(vec![transfer("npm view", Count::ONE, targets, at)])
}

const GIT: Syntax = Syntax {
    opts: &[
        // Global options, before the subcommand.
        val_c('C', "chdir"),
        val_c('c', "config"),
        val("git-dir"),
        val("work-tree"),
        val("namespace"),
        val("config-env"),
        flag_c('P', "no-pager"),
        flag_c('p', "paginate"),
        flag("bare"),
        flag("no-replace-objects"),
        flag("literal-pathspecs"),
        flag("glob-pathspecs"),
        flag("noglob-pathspecs"),
        flag("icase-pathspecs"),
        flag("no-optional-locks"),
        flag("no-advice"),
        // `ls-remote`.
        flag_c('h', "heads"),
        flag_c('t', "tags"),
        flag_c('b', "branches"),
        flag("refs"),
        flag_c('q', "quiet"),
        flag("exit-code"),
        flag("get-url"),
        flag("symref"),
        val("sort"),
        val_c('o', "server-option"),
        val("upload-pack"),
        val("exec"),
    ],
    negatable: true,
    go_bools: false,
};

/// `git ls-remote [REPO]`: one request to the repository's host.
///
/// `-c url.X.insteadOf=Y` and the user's git config can rewrite the URL;
/// the analysis reads the URL as written.
pub(super) fn git(args: &[Arg], span: Span) -> Option<Vec<Transfer>> {
    let toks = parse(args, &GIT);
    let ops = operands(&toks);
    let (cmd, _, false) = ops.first()? else {
        return None;
    };
    if cmd.val.as_exact() != Some("ls-remote") {
        return None;
    }
    let get_url = toks.iter().any(|t| {
        matches!(
            t,
            Tok::Flag {
                name: "get-url",
                on: true
            }
        )
    });
    if get_url {
        // Prints the expanded URL; contacts nothing.
        return Some(Vec::new());
    }
    let (targets, at, per_call) = match ops.get(1) {
        Some((arg, true, opaque)) => (
            TargetSet::unresolved(Unresolved::UnknownOption),
            arg.span.clone(),
            if *opaque { Count::MAYBE } else { Count::ONE },
        ),
        Some((arg, false, opaque)) => (
            git_target(&arg.val),
            arg.span.clone(),
            if *opaque { Count::MAYBE } else { Count::ONE },
        ),
        // The current branch's remote, from git config.
        None => (TargetSet::unresolved(Unresolved::Dynamic), span, Count::ONE),
    };
    Some(vec![transfer("git ls-remote", per_call, targets, at)])
}

/// Where a git repository argument points: a URL, an scp-like
/// `[user@]host:path`, a local path, or a remote name from config.
fn git_target(val: &AbsVal) -> TargetSet {
    match val {
        AbsVal::Values(vs) => vs
            .iter()
            .map(|v| git_one(v, true))
            .reduce(|a, b| a.union(&b))
            .unwrap_or_else(|| TargetSet::unresolved(Unresolved::Dynamic)),
        AbsVal::Prefix(p) => git_one(p, false),
        AbsVal::Top => TargetSet::unresolved(Unresolved::Dynamic),
    }
}

fn git_one(s: &str, complete: bool) -> TargetSet {
    let dynamic = TargetSet::unresolved(Unresolved::Dynamic);
    if s.starts_with("file://") || s.starts_with(['/', '.', '~']) {
        return TargetSet::local();
    }
    if s.contains("://") {
        return host_target(url_host(s, complete));
    }
    match s.split_once(':') {
        // scp-like. In a prefix, `https:` followed by the unknown rest may
        // still become `https://…`, so the path must have started.
        Some((host, path))
            if !host.contains('/')
                && !path.starts_with("//")
                && (complete || !(path.is_empty() || path.starts_with('/'))) =>
        {
            let h = host.rsplit('@').next().unwrap_or(host);
            host_target((!h.is_empty()).then(|| h.to_ascii_lowercase()))
        }
        // A remote name (resolved from config) or a relative path.
        _ => dynamic,
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;

    #[test]
    fn npm_view_goes_to_the_public_registry() {
        let t = only("npm", &["view", "react", "version"]);
        assert_eq!(t.targets, TargetSet::host("registry.npmjs.org"));
        let t = only(
            "npm",
            &["--registry=https://npm.corp.example/", "info", "react"],
        );
        assert_eq!(t.targets, TargetSet::host("npm.corp.example"));
        assert!(run("npm", &["install"]).is_none());
    }

    #[test]
    fn git_ls_remote_scp_like() {
        let t = only("git", &["ls-remote", "git@github.com:a/b"]);
        assert_eq!(t.targets, TargetSet::host("github.com"));
        let t = only(
            "git",
            &["ls-remote", "--tags", "https://gitlab.com/a/b.git"],
        );
        assert_eq!(t.targets, TargetSet::host("gitlab.com"));
        let t = only("git", &["ls-remote", "origin"]);
        assert_eq!(t.targets, TargetSet::unresolved(Unresolved::Dynamic));
        assert_eq!(
            git_target(&AbsVal::Prefix("https:".into())),
            TargetSet::unresolved(Unresolved::Dynamic)
        );
    }

    #[test]
    fn git_other_subcommands_are_not_modelled() {
        assert!(run("git", &["status"]).is_none());
        assert!(run("git", &["fetch", "origin"]).is_none());
    }
}
