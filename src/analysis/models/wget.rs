//! `wget`: every operand is a URL; `-i` reads more from a file, and `-r`
//! or `--mirror` follows links with no count the command line bounds.

use super::super::domain::{Count, Upper, Why};
use super::super::effects::{Factor, FactorKind, TargetSet, Unresolved};
use super::{
    exact_u64, flag, flag_c, maybe_fields, operand_target, parse, transfer, val, val_c, Arg,
    OptSpec, Syntax, Tok, Transfer,
};

const OPTS: &[OptSpec] = &[
    val_c('i', "input-file"),
    flag_c('r', "recursive"),
    flag_c('m', "mirror"),
    flag_c('p', "page-requisites"),
    val_c('t', "tries"),
    flag("no-config"),
    // Value-taking.
    val_c('O', "output-document"),
    val_c('o', "output-file"),
    val_c('a', "append-output"),
    val_c('P', "directory-prefix"),
    val_c('U', "user-agent"),
    val_c('e', "execute"),
    val_c('T', "timeout"),
    val_c('w', "wait"),
    val_c('Q', "quota"),
    val_c('B', "base"),
    val_c('l', "level"),
    val_c('A', "accept"),
    val_c('R', "reject"),
    val_c('D', "domains"),
    val_c('I', "include-directories"),
    val_c('X', "exclude-directories"),
    // `-nv`, `-nc`, `-np`: getopt reads `-n` with the letter as its value.
    val_c('n', "n"),
    val("header"),
    val("user"),
    val("password"),
    val("http-user"),
    val("http-password"),
    val("ftp-user"),
    val("ftp-password"),
    val("proxy-user"),
    val("proxy-password"),
    val("post-data"),
    val("post-file"),
    val("body-data"),
    val("body-file"),
    val("method"),
    val("dns-timeout"),
    val("connect-timeout"),
    val("read-timeout"),
    val("waitretry"),
    val("limit-rate"),
    val("config"),
    val("exclude-domains"),
    val("referer"),
    val("save-cookies"),
    val("load-cookies"),
    val("certificate"),
    val("certificate-type"),
    val("private-key"),
    val("private-key-type"),
    val("ca-certificate"),
    val("ca-directory"),
    val("crl-file"),
    val("secure-protocol"),
    val("ciphers"),
    val("bind-address"),
    val("cut-dirs"),
    val("default-page"),
    val("restrict-file-names"),
    val("progress"),
    val("backups"),
    val("report-speed"),
    val("retry-on-http-error"),
    val("accept-regex"),
    val("reject-regex"),
    val("regex-type"),
    val("local-encoding"),
    val("remote-encoding"),
    val("compression"),
    val("prefer-family"),
    val("rejected-log"),
    val("max-redirect"),
    // Flags.
    flag_c('V', "version"),
    flag_c('h', "help"),
    flag_c('b', "background"),
    flag_c('q', "quiet"),
    flag_c('v', "verbose"),
    flag_c('d', "debug"),
    flag_c('F', "force-html"),
    flag_c('c', "continue"),
    flag_c('N', "timestamping"),
    flag_c('S', "server-response"),
    flag_c('x', "force-directories"),
    flag_c('k', "convert-links"),
    flag_c('K', "backup-converted"),
    flag_c('H', "span-hosts"),
    flag_c('L', "relative"),
    flag_c('E', "adjust-extension"),
    flag_c('4', "inet4-only"),
    flag_c('6', "inet6-only"),
    flag("spider"),
    flag("check-certificate"),
    flag("clobber"),
    flag("directories"),
    flag("host-directories"),
    flag("parent"),
    flag("content-disposition"),
    flag("trust-server-names"),
    flag("ignore-case"),
    flag("cookies"),
    flag("keep-session-cookies"),
    flag("ignore-length"),
    flag("auth-no-challenge"),
    flag("http-keep-alive"),
    flag("cache"),
    flag("https-only"),
    flag("delete-after"),
    flag("random-wait"),
    flag("retry-connrefused"),
    flag("dns-cache"),
    flag("show-progress"),
    flag("save-headers"),
    flag("proxy"),
    flag("glob"),
    flag("unlink"),
    flag("xattr"),
];

const SYNTAX: Syntax = Syntax {
    opts: OPTS,
    negatable: true,
    go_bools: false,
};

pub(super) fn model(args: &[Arg]) -> Vec<Transfer> {
    let toks = parse(args, &SYNTAX);
    let mut recursive = false;
    let mut requisites = false;
    let mut wgetrc = true;
    let mut tries = None;
    let mut out = Vec::new();
    for tok in &toks {
        match tok {
            Tok::Flag {
                name: "recursive" | "mirror",
                on,
            } => recursive = *on,
            Tok::Flag {
                name: "page-requisites",
                on,
            } => requisites = *on,
            Tok::Flag {
                name: "no-config", ..
            } => wgetrc = false,
            Tok::Value {
                name: "tries",
                val,
                span,
                ..
            } => {
                // `-t N` is N attempts in all; 0 and `inf` retry forever,
                // which is not a number the analysis can state.
                let count = match exact_u64(val) {
                    Some(1) => None,
                    Some(n) if n > 1 => Some(Count::between(1, n)),
                    _ => Some(Count {
                        lower: 1,
                        upper: Upper::Unknown,
                    }),
                };
                tries = count.map(|count| Factor {
                    kind: FactorKind::Builtin {
                        what: "tries".into(),
                    },
                    count,
                    span: span.clone(),
                });
            }
            _ => {}
        }
    }
    // Following links: every fetched page may name more, and nothing on
    // the command line caps how many. Page requisites are bounded by the
    // page, which the command line does not show.
    let each = if recursive {
        Count::uncapped(1, Why::Recursive)
    } else if requisites {
        Count {
            lower: 1,
            upper: Upper::Unknown,
        }
    } else {
        Count::ONE
    };
    let finish = |mut t: Transfer| {
        if let Some(f) = &tries {
            t.per_call = t.per_call.mul(f.count);
            t.builtin.push(f.clone());
        } else {
            t.uncounted.push("retries (wget tries 20 times by default)");
        }
        t.uncounted.push("redirects");
        if recursive {
            t.uncounted.push("robots.txt");
        }
        if wgetrc {
            t.uncounted.push("~/.wgetrc defaults");
        }
        t
    };
    for tok in toks {
        match tok {
            Tok::Value {
                name: "input-file",
                span,
                ..
            } => out.push(finish(transfer(
                "wget",
                Count::UNKNOWN,
                TargetSet::unresolved(Unresolved::FromFile),
                span,
            ))),
            Tok::Operand { arg, after_unknown } | Tok::Opaque { arg, after_unknown } => {
                let words = if matches!(tok, Tok::Opaque { .. }) {
                    maybe_fields(arg.fields)
                } else {
                    arg.fields
                };
                out.push(finish(transfer(
                    "wget",
                    words.mul(each),
                    operand_target(&arg.val, after_unknown),
                    arg.span.clone(),
                )));
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;

    #[test]
    fn recursion_is_uncapped() {
        let t = only("wget", &["-r", "https://a.example/"]);
        assert_eq!(t.per_call, Count::uncapped(1, Why::Recursive));
        assert_eq!(t.targets, TargetSet::host("a.example"));
    }

    #[test]
    fn operands_are_urls_and_option_values_are_not() {
        let t = run(
            "wget",
            &[
                "-q",
                "-O",
                "https://x.example/",
                "-nv",
                "https://a.example/",
                "b.example/y",
            ],
        )
        .unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].targets, TargetSet::host("a.example"));
        assert_eq!(t[1].targets, TargetSet::host("b.example"));
    }

    #[test]
    fn an_input_file_is_an_unknown_transfer() {
        let t = only("wget", &["-i", "urls.txt"]);
        assert_eq!(t.targets, TargetSet::unresolved(Unresolved::FromFile));
        assert_eq!(t.per_call, Count::UNKNOWN);
    }

    #[test]
    fn tries_multiply_the_upper_bound() {
        let t = only("wget", &["-t", "3", "https://a.example/"]);
        assert_eq!(t.per_call, Count::between(1, 3));
    }
}
