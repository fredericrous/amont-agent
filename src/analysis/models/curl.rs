//! `curl`: URL operands and `--url`, in transfer groups split by `--next`.
//!
//! The count per destination is the product of what one command line asks
//! for: the words the operand expanded to, curl's own URL globbing, and
//! `--retry`. Redirects and `~/.curlrc` are named, never counted — the
//! command line does not say how many there are.

use super::super::domain::{Count, Upper};
use super::super::effects::{Factor, FactorKind, TargetSet, Unresolved};
use super::super::ir::Span;
use super::super::state::AbsVal;
use super::{
    exact_u64, flag, flag_c, maybe_fields, operand_target, parse, transfer, val, val_c, Arg,
    OptSpec, Syntax, Tok, Transfer,
};

const OPTS: &[OptSpec] = &[
    // Destinations and groups.
    val("url"),
    flag_c(':', "next"),
    val_c('K', "config"),
    flag_c('q', "disable"),
    flag_c('g', "globoff"),
    // Value-taking.
    val("abstract-unix-socket"),
    val("alt-svc"),
    val("aws-sigv4"),
    val("cacert"),
    val("capath"),
    val_c('E', "cert"),
    val("cert-type"),
    val("ciphers"),
    val("connect-timeout"),
    val("connect-to"),
    val_c('C', "continue-at"),
    val_c('b', "cookie"),
    val_c('c', "cookie-jar"),
    val("create-file-mode"),
    val("crlfile"),
    val("curves"),
    val_c('d', "data"),
    val("data-ascii"),
    val("data-binary"),
    val("data-raw"),
    val("data-urlencode"),
    val("delegation"),
    val("dns-interface"),
    val("dns-ipv4-addr"),
    val("dns-ipv6-addr"),
    val("dns-servers"),
    val("doh-url"),
    val_c('D', "dump-header"),
    val("ech"),
    val("egd-file"),
    val("engine"),
    val("etag-compare"),
    val("etag-save"),
    val("expect100-timeout"),
    val_c('F', "form"),
    val("form-string"),
    val("ftp-account"),
    val("ftp-alternative-to-user"),
    val("ftp-method"),
    val_c('P', "ftp-port"),
    val("ftp-ssl-ccc-mode"),
    val("happy-eyeballs-timeout-ms"),
    val_c('H', "header"),
    val("hostpubmd5"),
    val("hostpubsha256"),
    val("hsts"),
    val("interface"),
    val("ip-tos"),
    val("ipfs-gateway"),
    val("json"),
    val("keepalive-cnt"),
    val("keepalive-time"),
    val("key"),
    val("key-type"),
    val("krb"),
    val("libcurl"),
    val("limit-rate"),
    val("local-port"),
    val("login-options"),
    val("mail-auth"),
    val("mail-from"),
    val("mail-rcpt"),
    val("max-filesize"),
    val("max-redirs"),
    val_c('m', "max-time"),
    val("netrc-file"),
    val("noproxy"),
    val("oauth2-bearer"),
    val_c('o', "output"),
    val("output-dir"),
    val("parallel-max"),
    val("pass"),
    val("pinnedpubkey"),
    val("preproxy"),
    val("proto"),
    val("proto-default"),
    val("proto-redir"),
    val_c('x', "proxy"),
    val("proxy-cacert"),
    val("proxy-capath"),
    val("proxy-cert"),
    val("proxy-cert-type"),
    val("proxy-ciphers"),
    val("proxy-crlfile"),
    val("proxy-header"),
    val("proxy-key"),
    val("proxy-key-type"),
    val("proxy-pass"),
    val("proxy-pinnedpubkey"),
    val("proxy-service-name"),
    val("proxy-tls13-ciphers"),
    val("proxy-tlsauthtype"),
    val("proxy-tlspassword"),
    val("proxy-tlsuser"),
    val_c('U', "proxy-user"),
    val("proxy1.0"),
    val("pubkey"),
    val_c('Q', "quote"),
    val("random-file"),
    val_c('r', "range"),
    val("rate"),
    val_c('e', "referer"),
    val_c('X', "request"),
    val("request-target"),
    val("resolve"),
    val("retry"),
    val("retry-delay"),
    val("retry-max-time"),
    val("sasl-authzid"),
    val("service-name"),
    val("socks4"),
    val("socks4a"),
    val("socks5"),
    val("socks5-gssapi-service"),
    val("socks5-hostname"),
    val_c('Y', "speed-limit"),
    val_c('y', "speed-time"),
    val("stderr"),
    val_c('t', "telnet-option"),
    val("tftp-blksize"),
    val_c('z', "time-cond"),
    val("tls-max"),
    val("tls13-ciphers"),
    val("tlsauthtype"),
    val("tlspassword"),
    val("tlsuser"),
    val("trace"),
    val("trace-ascii"),
    val("trace-config"),
    val("unix-socket"),
    val_c('T', "upload-file"),
    val("url-query"),
    val_c('u', "user"),
    val_c('A', "user-agent"),
    val("variable"),
    val_c('w', "write-out"),
    // Flags.
    flag_c('a', "append"),
    flag("anyauth"),
    flag("basic"),
    flag("ca-native"),
    flag("cert-status"),
    flag("compressed"),
    flag("compressed-ssh"),
    flag("create-dirs"),
    flag("crlf"),
    flag("digest"),
    flag("disable-eprt"),
    flag("disable-epsv"),
    flag("disallow-username-in-url"),
    flag("doh-cert-status"),
    flag("doh-insecure"),
    flag_c('f', "fail"),
    flag("fail-early"),
    flag("fail-with-body"),
    flag("false-start"),
    flag("ftp-create-dirs"),
    flag("ftp-pasv"),
    flag("ftp-pret"),
    flag("ftp-skip-pasv-ip"),
    flag("ftp-ssl-control"),
    flag_c('G', "get"),
    flag("haproxy-protocol"),
    flag_c('I', "head"),
    flag_c('h', "help"),
    flag("http0.9"),
    flag_c('0', "http1.0"),
    flag("http1.1"),
    flag("http2"),
    flag("http2-prior-knowledge"),
    flag("http3"),
    flag("http3-only"),
    flag("ignore-content-length"),
    flag_c('i', "include"),
    flag_c('k', "insecure"),
    flag_c('4', "ipv4"),
    flag_c('6', "ipv6"),
    flag_c('j', "junk-session-cookies"),
    flag_c('l', "list-only"),
    flag_c('L', "location"),
    flag("location-trusted"),
    flag_c('M', "manual"),
    flag("mptcp"),
    flag("negotiate"),
    flag_c('n', "netrc"),
    flag("netrc-optional"),
    flag("no-alpn"),
    flag_c('N', "no-buffer"),
    flag("no-clobber"),
    flag("no-keepalive"),
    flag("no-npn"),
    flag("no-progress-meter"),
    flag("no-sessionid"),
    flag("ntlm"),
    flag("ntlm-wb"),
    flag_c('Z', "parallel"),
    flag("parallel-immediate"),
    flag("path-as-is"),
    flag("post301"),
    flag("post302"),
    flag("post303"),
    flag_c('#', "progress-bar"),
    flag("proxy-anyauth"),
    flag("proxy-basic"),
    flag("proxy-ca-native"),
    flag("proxy-digest"),
    flag("proxy-http2"),
    flag("proxy-insecure"),
    flag("proxy-negotiate"),
    flag("proxy-ntlm"),
    flag("proxy-ssl-allow-beast"),
    flag("proxy-ssl-auto-client-cert"),
    flag_c('p', "proxytunnel"),
    flag("raw"),
    flag_c('J', "remote-header-name"),
    flag_c('O', "remote-name"),
    flag("remote-name-all"),
    flag_c('R', "remote-time"),
    flag("remove-on-error"),
    flag("retry-all-errors"),
    flag("retry-connrefused"),
    flag("sasl-ir"),
    flag_c('S', "show-error"),
    flag("show-headers"),
    flag_c('s', "silent"),
    flag("skip-existing"),
    flag("socks5-basic"),
    flag("socks5-gssapi"),
    flag("socks5-gssapi-nec"),
    flag("ssl"),
    flag("ssl-allow-beast"),
    flag("ssl-auto-client-cert"),
    flag("ssl-no-revoke"),
    flag("ssl-reqd"),
    flag("ssl-revoke-best-effort"),
    flag_c('2', "sslv2"),
    flag_c('3', "sslv3"),
    flag("styled-output"),
    flag("suppress-connect-headers"),
    flag("tcp-fastopen"),
    flag("tcp-nodelay"),
    flag("tftp-no-options"),
    flag_c('1', "tlsv1"),
    flag("tlsv1.0"),
    flag("tlsv1.1"),
    flag("tlsv1.2"),
    flag("tlsv1.3"),
    flag("tr-encoding"),
    flag("trace-ids"),
    flag("trace-time"),
    flag_c('B', "use-ascii"),
    flag_c('v', "verbose"),
    flag_c('V', "version"),
    flag("xattr"),
];

const SYNTAX: Syntax = Syntax {
    opts: OPTS,
    negatable: true,
    go_bools: false,
};

const CURLRC: &str = "~/.curlrc defaults";

/// A destination in one transfer group.
struct Dest {
    val: AbsVal,
    fields: Count,
    span: Span,
    after_unknown: bool,
    /// Might not be an operand at all (its value is unknown).
    opaque: bool,
}

/// The options `--next` resets, and the destinations they apply to.
#[derive(Default)]
struct Group {
    globoff: bool,
    location: bool,
    retry: Option<Factor>,
    dests: Vec<Dest>,
}

pub(super) fn model(args: &[Arg]) -> Vec<Transfer> {
    // curl reads ~/.curlrc unless -q is the very first argument; anywhere
    // else it is too late to matter.
    let curlrc = !matches!(
        args.first().and_then(|a| a.val.as_exact()),
        Some("-q" | "--disable")
    );
    let mut out = Vec::new();
    let mut group = Group::default();
    for tok in parse(args, &SYNTAX) {
        match tok {
            Tok::Flag { name: "next", .. } => flush(std::mem::take(&mut group), &mut out),
            Tok::Flag {
                name: "globoff",
                on,
            } => group.globoff = on,
            Tok::Flag {
                name: "location" | "location-trusted",
                on,
            } => group.location = on,
            Tok::Value {
                name: "url",
                val,
                span,
                after_unknown,
            } => group.dests.push(Dest {
                val,
                fields: Count::ONE,
                span,
                after_unknown,
                opaque: false,
            }),
            Tok::Value {
                name: "config",
                span,
                ..
            } => out.push(transfer(
                "curl",
                Count::UNKNOWN,
                TargetSet::unresolved(Unresolved::FromFile),
                span,
            )),
            Tok::Value {
                name: "retry",
                val,
                span,
                ..
            } => {
                group.retry = retries(&val).map(|count| Factor {
                    kind: FactorKind::Builtin {
                        what: "retries".into(),
                    },
                    count,
                    span,
                })
            }
            Tok::Operand { arg, after_unknown } | Tok::Opaque { arg, after_unknown } => {
                group.dests.push(Dest {
                    val: arg.val.clone(),
                    fields: arg.fields,
                    span: arg.span.clone(),
                    after_unknown,
                    opaque: matches!(tok, Tok::Opaque { .. }),
                })
            }
            _ => {}
        }
    }
    flush(group, &mut out);
    if curlrc {
        for t in &mut out {
            t.uncounted.push(CURLRC);
        }
    }
    out
}

/// `--retry N`: each transfer is tried at most N+1 times, at least once.
/// An unknown N leaves the upper bound unknown, not unbounded.
fn retries(val: &AbsVal) -> Option<Count> {
    match exact_u64(val) {
        Some(0) => None,
        Some(n) => Some(Count::between(1, n.saturating_add(1))),
        None => Some(Count {
            lower: 1,
            upper: Upper::Unknown,
        }),
    }
}

fn flush(group: Group, out: &mut Vec<Transfer>) {
    for d in group.dests {
        let mut per_call = if d.opaque {
            maybe_fields(d.fields)
        } else {
            d.fields
        };
        let mut targets = operand_target(&d.val, d.after_unknown);
        let mut builtin = Vec::new();
        if !group.globoff {
            if let AbsVal::Values(vs) = &d.val {
                let globs: Vec<Glob> = vs.iter().map(|v| glob(v)).collect();
                // One of the values is the URL; which is not known, so the
                // range is the join over them.
                let count = globs
                    .iter()
                    .map(|g| g.count)
                    .reduce(Count::join)
                    .unwrap_or(Count::ONE);
                if count != Count::ONE {
                    builtin.push(Factor {
                        kind: FactorKind::Builtin {
                            what: "URL range".into(),
                        },
                        count,
                        span: d.span.clone(),
                    });
                    per_call = per_call.mul(count);
                }
                // `https://{a,b}.example/` names two hosts, neither of them
                // the literal text; the analysis does not enumerate them.
                if !d.after_unknown && globs.iter().any(|g| g.in_authority) {
                    targets = TargetSet::unresolved(Unresolved::Dynamic);
                }
            }
        }
        if let Some(r) = &group.retry {
            per_call = per_call.mul(r.count);
            builtin.push(r.clone());
        }
        let mut t = transfer("curl", per_call, targets, d.span);
        t.builtin = builtin;
        if group.location {
            t.uncounted.push("redirects");
        }
        out.push(t);
    }
}

/// What curl's URL globbing makes of one URL.
#[derive(Debug, PartialEq, Eq)]
struct Glob {
    /// The number of URLs it expands to. A malformed pattern makes curl
    /// refuse the URL; that is counted as the one URL, not as zero.
    count: Count,
    /// A pattern sits in the host part.
    in_authority: bool,
}

/// Count curl's globbing of `url` without expanding it: `{a,b}` sets and
/// `[1-100]`, `[001-100]`, `[a-z]`, `[1-100:10]` ranges multiply. A
/// bracketed IPv6 literal is not a range.
fn glob(url: &str) -> Glob {
    const LITERAL: Glob = Glob {
        count: Count::ONE,
        in_authority: false,
    };
    let auth_start = url.find("://").map_or(0, |i| i + 3);
    let mut count = Count::ONE;
    let mut in_authority = false;
    let mut in_auth_part = true;
    let bytes = url.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let here = in_auth_part && i >= auth_start;
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'/' | b'?' | b'#' if i >= auth_start => in_auth_part = false,
            open @ (b'{' | b'[') => {
                let close = if open == b'{' { b'}' } else { b']' };
                let Some(len) = bytes[i + 1..].iter().position(|&b| b == close) else {
                    return LITERAL;
                };
                let body = &url[i + 1..i + 1 + len];
                i += len + 2;
                let n = if open == b'{' {
                    Count::exactly(body.split(',').count() as u64)
                } else if is_ipv6(body) {
                    continue;
                } else {
                    match range(body) {
                        Some(n) => n,
                        None => return LITERAL,
                    }
                };
                count = count.mul(n);
                in_authority |= here;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    Glob {
        count,
        in_authority,
    }
}

/// `[::1]`, `[fe80::1%25eth0]`: an address, which curl leaves alone.
fn is_ipv6(body: &str) -> bool {
    let addr = body.split('%').next().unwrap_or(body);
    addr.contains(':')
        && addr
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
}

/// The size of `lo-hi[:step]`, or `None` when curl would reject it.
fn range(body: &str) -> Option<Count> {
    let (span, step) = match body.split_once(':') {
        Some((s, st)) => (s, st.parse::<u64>().ok()?),
        None => (body, 1),
    };
    if step == 0 {
        return None;
    }
    let (lo, hi) = span.split_once('-')?;
    let size = |lo: u64, hi: u64| -> Option<Count> {
        let steps = hi.checked_sub(lo)? / step;
        Some(match steps.checked_add(1) {
            Some(n) => Count::exactly(n),
            None => Count {
                lower: u64::MAX,
                upper: Upper::Saturated,
            },
        })
    };
    let letter = |s: &str| {
        let mut cs = s.chars();
        match (cs.next(), cs.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => Some(c),
            _ => None,
        }
    };
    if let (Some(a), Some(b)) = (letter(lo), letter(hi)) {
        if a.is_ascii_lowercase() != b.is_ascii_lowercase() {
            return None;
        }
        return size(a as u64, b as u64);
    }
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !(digits(lo) && digits(hi)) {
        return None;
    }
    match (lo.parse::<u64>(), hi.parse::<u64>()) {
        (Ok(a), Ok(b)) => size(a, b),
        // Wider than a u64: finite, and more than it can hold.
        _ => Some(Count {
            lower: 1,
            upper: Upper::Saturated,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;

    fn hosts(t: &Transfer) -> Vec<&str> {
        t.targets.hosts.iter().map(String::as_str).collect()
    }

    #[test]
    fn two_url_operands_are_two_transfers() {
        let t = run("curl", &["https://a.example/x", "https://b.example/y"]).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(hosts(&t[0]), ["a.example"]);
        assert_eq!(hosts(&t[1]), ["b.example"]);
        assert!(t.iter().all(|t| t.per_call == Count::ONE));
    }

    #[test]
    fn url_option_is_a_destination() {
        assert_eq!(
            hosts(&only("curl", &["--url", "https://a.example/x"])),
            ["a.example"]
        );
        assert_eq!(
            hosts(&only("curl", &["--url=https://a.example/x"])),
            ["a.example"]
        );
    }

    #[test]
    fn a_header_value_is_never_a_destination() {
        let t = only(
            "curl",
            &["-H", "Referer: https://b.example/", "https://a.example/x"],
        );
        assert_eq!(hosts(&t), ["a.example"]);
    }

    #[test]
    fn globoff_disables_url_ranges() {
        let t = only("curl", &["-g", "https://a.example/[1-5]"]);
        assert_eq!(t.per_call, Count::ONE);
        assert!(t.builtin.is_empty());
    }

    #[test]
    fn a_url_range_multiplies_and_is_recorded() {
        let t = only("curl", &["https://a.example/[1-5]"]);
        assert_eq!(t.per_call, Count::exactly(5));
        assert_eq!(
            t.builtin,
            [Factor {
                kind: FactorKind::Builtin {
                    what: "URL range".into()
                },
                count: Count::exactly(5),
                span: 0..23,
            }]
        );
    }

    #[test]
    fn nested_ranges_multiply() {
        let t = only("curl", &["https://a.example/[1-100]/[1-100]"]);
        assert_eq!(t.per_call, Count::exactly(10_000));
    }

    #[test]
    fn range_forms() {
        assert_eq!(
            glob("https://a.example/[001-100]").count,
            Count::exactly(100)
        );
        assert_eq!(
            glob("https://a.example/[1-100:10]").count,
            Count::exactly(10)
        );
        assert_eq!(glob("https://a.example/[a-z]").count, Count::exactly(26));
        assert_eq!(glob("https://a.example/{a,b,c}").count, Count::exactly(3));
        assert_eq!(glob("https://a.example/\\[1-5]").count, Count::ONE);
        assert!(glob("https://{a,b}.example/").in_authority);
    }

    #[test]
    fn a_range_product_past_u64_is_saturated() {
        let t = only("curl", &["https://a.example/[1-4294967296]/[1-4294967296]"]);
        assert_eq!(t.per_call.upper, Upper::Saturated);
        let t = only("curl", &["https://a.example/[1-99999999999999999999999]"]);
        assert_eq!(t.per_call.upper, Upper::Saturated);
    }

    #[test]
    fn an_ipv6_literal_is_not_a_range() {
        let t = only("curl", &["http://[::1]:80/"]);
        assert!(t.targets.is_local_only());
        assert_eq!(t.per_call, Count::ONE);
        assert!(t.builtin.is_empty());
        assert_eq!(glob("http://[fe80::1%25eth0]/").count, Count::ONE);
    }

    #[test]
    fn localhost_is_local() {
        assert!(only("curl", &["localhost:8080"]).targets.is_local_only());
    }

    #[test]
    fn an_unknown_value_is_dynamic() {
        let args = [Arg {
            val: AbsVal::Top,
            fields: Count::ONE,
            span: 0..6,
        }];
        let t = super::super::model("curl", &args, 0..1).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].targets, TargetSet::unresolved(Unresolved::Dynamic));
    }

    #[test]
    fn an_unknown_option_hides_later_operands() {
        let t = only("curl", &["--frobnicate", "https://a.example/x"]);
        assert_eq!(t.targets, TargetSet::unresolved(Unresolved::UnknownOption));
        let t = only("curl", &["-sZz", "x", "-W", "https://a.example/x"]);
        assert_eq!(t.targets, TargetSet::unresolved(Unresolved::UnknownOption));
    }

    #[test]
    fn a_value_letter_at_the_end_of_a_cluster_takes_the_next_argument() {
        let t = only("curl", &["-sSo", "out.json", "https://a.example/x"]);
        assert_eq!(hosts(&t), ["a.example"]);
        assert!(t.targets.unresolved.is_empty());
    }

    #[test]
    fn a_value_letter_mid_cluster_takes_the_rest() {
        let t = only("curl", &["-o-", "https://a.example/x"]);
        assert_eq!(hosts(&t), ["a.example"]);
        let t = only("curl", &["-sSfL", "https://a.example/x"]);
        assert_eq!(hosts(&t), ["a.example"]);
        assert!(t.uncounted.contains(&"redirects"));
    }

    #[test]
    fn next_resets_group_options() {
        let t = run(
            "curl",
            &[
                "-L",
                "--retry",
                "2",
                "-g",
                "https://a.example/[1-3]",
                "--next",
                "https://b.example/[1-3]",
            ],
        )
        .unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].per_call, Count::between(1, 3));
        assert!(t[0].uncounted.contains(&"redirects"));
        assert_eq!(t[1].per_call, Count::exactly(3));
        assert!(!t[1].uncounted.contains(&"redirects"));
    }

    #[test]
    fn retry_raises_the_upper_bound_only() {
        let t = only("curl", &["--retry", "3", "https://a.example/x"]);
        assert_eq!(t.per_call, Count::between(1, 4));
        assert_eq!(t.builtin.len(), 1);
        assert_eq!(t.builtin[0].count, Count::between(1, 4));
        assert_eq!(
            t.builtin[0].kind,
            FactorKind::Builtin {
                what: "retries".into()
            }
        );
    }

    #[test]
    fn a_config_file_is_an_unknown_transfer() {
        let t = only("curl", &["-K", "urls.txt"]);
        assert_eq!(t.targets, TargetSet::unresolved(Unresolved::FromFile));
        assert_eq!(t.per_call, Count::UNKNOWN);
    }

    #[test]
    fn q_first_suppresses_the_curlrc_note() {
        assert!(only("curl", &["https://a.example/"])
            .uncounted
            .contains(&CURLRC));
        assert!(!only("curl", &["-q", "https://a.example/"])
            .uncounted
            .contains(&CURLRC));
        assert!(only("curl", &["-s", "-q", "https://a.example/"])
            .uncounted
            .contains(&CURLRC));
    }

    #[test]
    fn no_destination_is_no_transfer() {
        assert_eq!(run("curl", &["--version"]), Some(vec![]));
    }
}
