//! Container registry clients: `skopeo`, `crane`, `regctl`, `oras`, and
//! `docker pull` / `podman pull`.
//!
//! Each image reference an invocation names is one transfer to that
//! reference's registry. The clients page through tag lists and fetch
//! tokens on their own; how many requests that takes depends on the
//! registry, not on the command line, so it is named and not counted.

use super::super::domain::{Count, Upper};
use super::super::effects::{Factor, FactorKind, TargetSet, Unresolved};
use super::super::state::AbsVal;
use super::{
    exact_u64, flag, flag_c, host_target, maybe_fields, operands, parse, transfer, val, val_c, Arg,
    Syntax, Tok, Transfer,
};

const UNCOUNTED: &str = "the client's own pagination and auth requests";

/// Which operands after the subcommand are image references.
#[derive(Clone, Copy)]
enum Refs {
    One,
    Two,
    /// `crane pull IMAGE… TARBALL`.
    AllButLast,
    All,
    /// A registry host rather than an image (`crane catalog`, `oras repo ls`).
    Registry,
}

struct Sub {
    path: &'static [&'static str],
    refs: Refs,
}

const fn sub(path: &'static [&'static str], refs: Refs) -> Sub {
    Sub { path, refs }
}

struct Client {
    label: &'static str,
    syntax: Syntax,
    subs: &'static [Sub],
    /// References carry a transport (`docker://…`), and only that one
    /// reaches the network.
    transport: bool,
}

const SKOPEO: Client = Client {
    label: "skopeo",
    syntax: Syntax {
        opts: &[
            flag("debug"),
            flag("insecure-policy"),
            val("policy"),
            val("override-arch"),
            val("override-os"),
            val("override-variant"),
            val("registries.d"),
            val("tmpdir"),
            val("command-timeout"),
            flag_c('h', "help"),
            flag_c('v', "version"),
            val("creds"),
            val("authfile"),
            flag("tls-verify"),
            flag("raw"),
            flag("config"),
            val_c('f', "format"),
            flag("no-creds"),
            val("cert-dir"),
            val("retry-times"),
            val("retry-delay"),
            val("src-creds"),
            val("dest-creds"),
            flag("src-tls-verify"),
            flag("dest-tls-verify"),
            flag_c('a', "all"),
            val("multi-arch"),
            flag("preserve-digests"),
            val("src-authfile"),
            val("dest-authfile"),
            val("src-cert-dir"),
            val("dest-cert-dir"),
            flag("src-no-creds"),
            flag("dest-no-creds"),
            flag("remove-signatures"),
            val("sign-by"),
            val("sign-passphrase-file"),
            flag("dest-compress"),
            val("dest-compress-format"),
            val("dest-compress-level"),
            flag_c('q', "quiet"),
            val("image-parallel-copies"),
            val("registry-token"),
            val("src-registry-token"),
            val("dest-registry-token"),
            val("username"),
            val("password"),
            val("src-username"),
            val("src-password"),
            val("dest-username"),
            val("dest-password"),
            val("digestfile"),
            val("encryption-key"),
            val("decryption-key"),
            val("additional-tag"),
            flag_c('n', "no-tags"),
        ],
        negatable: false,
        go_bools: true,
    },
    subs: &[
        sub(&["inspect"], Refs::One),
        sub(&["list-tags"], Refs::One),
        sub(&["delete"], Refs::One),
        sub(&["copy"], Refs::Two),
    ],
    transport: true,
};

const CRANE: Client = Client {
    label: "crane",
    syntax: Syntax {
        opts: &[
            flag("insecure"),
            val("platform"),
            flag_c('v', "verbose"),
            flag("allow-nondistributable-artifacts"),
            flag_c('h', "help"),
            flag("full-ref"),
            flag("omit-digest-tags"),
            val("format"),
            val_c('c', "cache_path"),
            flag_c('a', "all-tags"),
            val_c('j', "jobs"),
            flag_c('n', "no-clobber"),
            flag("legacy"),
            val_c('t', "tag"),
        ],
        negatable: false,
        go_bools: true,
    },
    subs: &[
        sub(&["ls"], Refs::One),
        sub(&["manifest"], Refs::One),
        sub(&["digest"], Refs::One),
        sub(&["config"], Refs::One),
        sub(&["export"], Refs::One),
        sub(&["blob"], Refs::One),
        sub(&["tag"], Refs::One),
        sub(&["pull"], Refs::AllButLast),
        sub(&["copy"], Refs::Two),
        sub(&["cp"], Refs::Two),
        sub(&["catalog"], Refs::Registry),
    ],
    transport: false,
};

const REGCTL: Client = Client {
    label: "regctl",
    syntax: Syntax {
        opts: &[
            val_c('v', "verbosity"),
            val("logopt"),
            val("host"),
            val("user-agent"),
            flag_c('h', "help"),
            val("format"),
            val("platform"),
            val("limit"),
            val("last"),
            flag("list"),
            flag("require-list"),
            flag("include-digest"),
            flag("digest-tags"),
            flag("referrers"),
            flag("fast"),
            flag("force-recursive"),
            flag("include-external"),
        ],
        negatable: false,
        go_bools: true,
    },
    subs: &[
        sub(&["tag", "ls"], Refs::One),
        sub(&["tag", "list"], Refs::One),
        sub(&["manifest", "get"], Refs::One),
        sub(&["manifest", "head"], Refs::One),
        sub(&["image", "copy"], Refs::Two),
        sub(&["image", "cp"], Refs::Two),
        sub(&["image", "digest"], Refs::One),
        sub(&["image", "inspect"], Refs::One),
        sub(&["image", "config"], Refs::One),
        sub(&["image", "manifest"], Refs::One),
        sub(&["image", "export"], Refs::One),
        sub(&["image", "get-file"], Refs::One),
        sub(&["repo", "ls"], Refs::Registry),
    ],
    transport: false,
};

/// `--oci-layout` is left out on purpose: it turns the reference into a
/// local directory, and reading it as an unknown option is the honest
/// outcome.
const ORAS: Client = Client {
    label: "oras",
    syntax: Syntax {
        opts: &[
            flag_c('d', "debug"),
            flag_c('v', "verbose"),
            flag_c('h', "help"),
            val_c('u', "username"),
            val_c('p', "password"),
            flag("password-stdin"),
            val("identity-token"),
            flag("identity-token-stdin"),
            flag("insecure"),
            flag("plain-http"),
            val("registry-config"),
            val("ca-file"),
            val("cert-file"),
            val("key-file"),
            val_c('o', "output"),
            val("platform"),
            flag("include-subject"),
            flag("allow-path-traversal"),
            flag("keep-old-files"),
            val("concurrency"),
            flag("descriptor"),
            flag("pretty"),
            val("format"),
            val("last"),
            flag_c('r', "recursive"),
            val("resolve"),
            val_c('H', "header"),
            val("distribution-spec"),
            val_c('a', "annotation"),
            val("annotation-file"),
            val("artifact-type"),
            val("config"),
            val("export-manifest"),
            val("media-type"),
            flag("no-tty"),
            val("from-username"),
            val("from-password"),
            flag("from-insecure"),
            flag("from-plain-http"),
            val("from-registry-config"),
            val("from-ca-file"),
            val("to-username"),
            val("to-password"),
            flag("to-insecure"),
            flag("to-plain-http"),
            val("to-registry-config"),
            val("to-ca-file"),
        ],
        negatable: false,
        go_bools: true,
    },
    subs: &[
        sub(&["repo", "tags"], Refs::One),
        sub(&["repo", "show-tags"], Refs::One),
        sub(&["repo", "ls"], Refs::Registry),
        sub(&["pull"], Refs::One),
        sub(&["push"], Refs::One),
        sub(&["attach"], Refs::One),
        sub(&["tag"], Refs::One),
        sub(&["discover"], Refs::One),
        sub(&["resolve"], Refs::One),
        sub(&["manifest", "fetch"], Refs::One),
        sub(&["manifest", "fetch-config"], Refs::One),
        sub(&["blob", "fetch"], Refs::One),
        sub(&["copy"], Refs::Two),
        sub(&["cp"], Refs::Two),
    ],
    transport: false,
};

const DOCKER: Client = Client {
    label: "docker pull",
    syntax: Syntax {
        opts: &[
            val_c('H', "host"),
            val_c('c', "context"),
            val("config"),
            flag_c('D', "debug"),
            val_c('l', "log-level"),
            flag("tls"),
            val("tlscacert"),
            val("tlscert"),
            val("tlskey"),
            flag("tlsverify"),
            flag_c('a', "all-tags"),
            flag("disable-content-trust"),
            val("platform"),
            flag_c('q', "quiet"),
        ],
        negatable: false,
        go_bools: true,
    },
    subs: &[sub(&["pull"], Refs::All)],
    transport: false,
};

const PODMAN: Client = Client {
    label: "podman pull",
    syntax: Syntax {
        opts: &[
            flag("remote"),
            val("url"),
            val_c('c', "connection"),
            val("identity"),
            val("log-level"),
            val("root"),
            val("runroot"),
            val("storage-driver"),
            val("storage-opt"),
            val("cgroup-manager"),
            val("events-backend"),
            val("tmpdir"),
            val("module"),
            flag("syslog"),
            flag_c('a', "all-tags"),
            val("authfile"),
            val("creds"),
            flag("tls-verify"),
            val("arch"),
            val("os"),
            val("variant"),
            val("platform"),
            val("retry"),
            val("retry-delay"),
            val("cert-dir"),
            val("policy"),
            val("decryption-key"),
            flag_c('q', "quiet"),
            flag("disable-content-trust"),
        ],
        negatable: false,
        go_bools: true,
    },
    subs: &[sub(&["pull"], Refs::All)],
    transport: false,
};

pub(super) fn model(name: &str, args: &[Arg]) -> Option<Vec<Transfer>> {
    let client = match name {
        "skopeo" => &SKOPEO,
        "crane" => &CRANE,
        "regctl" => &REGCTL,
        "oras" => &ORAS,
        "docker" => &DOCKER,
        "podman" => &PODMAN,
        _ => return None,
    };
    let toks = parse(args, &client.syntax);
    let ops = operands(&toks);
    let sub = client.subs.iter().find(|s| {
        s.path.len() <= ops.len()
            && s.path
                .iter()
                .zip(&ops)
                .all(|(w, (a, _, opaque))| !opaque && a.val.as_exact() == Some(*w))
    })?;
    let rest = &ops[sub.path.len()..];
    let refs = match sub.refs {
        Refs::One | Refs::Registry => &rest[..rest.len().min(1)],
        Refs::Two => &rest[..rest.len().min(2)],
        Refs::AllButLast => &rest[..rest.len().saturating_sub(1)],
        Refs::All => rest,
    };
    let mut builtin: Vec<Factor> = Vec::new();
    for tok in &toks {
        let Tok::Value {
            name: "retry-times" | "retry",
            val,
            span,
            ..
        } = tok
        else {
            continue;
        };
        let count = match exact_u64(val) {
            Some(0) => continue,
            Some(n) => Count::between(1, n.saturating_add(1)),
            None => Count {
                lower: 1,
                upper: Upper::Unknown,
            },
        };
        builtin.push(Factor {
            kind: FactorKind::Builtin {
                what: "retries".into(),
            },
            count,
            span: span.clone(),
        });
    }
    // Every tag of the repository: as many as the registry holds.
    let all_tags = matches!(sub.refs, Refs::All)
        && toks.iter().any(|t| {
            matches!(
                t,
                Tok::Flag {
                    name: "all-tags",
                    on: true
                }
            )
        });
    if all_tags {
        builtin.push(Factor {
            kind: FactorKind::Builtin {
                what: "--all-tags".into(),
            },
            count: Count {
                lower: 1,
                upper: Upper::Unknown,
            },
            span: refs.first().map_or(0..0, |r| r.0.span.clone()),
        });
    }
    let mut out = Vec::new();
    for &(arg, after_unknown, opaque) in refs {
        let targets = if after_unknown {
            TargetSet::unresolved(Unresolved::UnknownOption)
        } else if matches!(sub.refs, Refs::Registry) {
            registry_target(&arg.val)
        } else {
            match ref_target(&arg.val, client.transport) {
                Some(t) => t,
                None => continue,
            }
        };
        let mut per_call = if opaque {
            maybe_fields(arg.fields)
        } else {
            arg.fields
        };
        for f in &builtin {
            per_call = per_call.mul(f.count);
        }
        let mut t = transfer(client.label, per_call, targets, arg.span.clone());
        t.builtin = builtin.clone();
        t.uncounted.push(UNCOUNTED);
        out.push(t);
    }
    Some(out)
}

/// Docker Hub's names for itself all resolve to one API host.
fn normalise(host: &str) -> String {
    let h = host.to_ascii_lowercase();
    match h.as_str() {
        "docker.io" | "index.docker.io" => "registry-1.docker.io".into(),
        _ => h,
    }
}

/// `host[:port]` or `[v6]:port` → the host.
fn strip_port(hostport: &str) -> &str {
    match hostport.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => hostport.split(':').next().unwrap_or(""),
    }
}

/// The registry a docker-style reference names, docker's way: a first
/// path component with a `.` or a `:`, or `localhost`, is a host;
/// anything else is a Docker Hub repository. A prefix settles it only once
/// the first component is complete.
fn ref_host(r: &str, complete: bool) -> Option<String> {
    let host = match r.split_once('/') {
        Some((first, _)) if first.contains(['.', ':']) || first == "localhost" => strip_port(first),
        Some(_) => return Some("registry-1.docker.io".into()),
        None if complete && !r.is_empty() => return Some("registry-1.docker.io".into()),
        None => return None,
    };
    (!host.is_empty()).then(|| normalise(host))
}

/// Where a reference points: `None` when it provably names no registry
/// (another skopeo transport, `oci:dir`, `docker-archive:x.tar`).
fn ref_target(val: &AbsVal, transport: bool) -> Option<TargetSet> {
    const DOCKER: &str = "docker://";
    let dynamic = || TargetSet::unresolved(Unresolved::Dynamic);
    let one = |s: &str, complete: bool| -> Option<TargetSet> {
        match s.strip_prefix(DOCKER) {
            Some(rest) => Some(host_target(ref_host(rest, complete))),
            // A prefix of `docker://` may still become one.
            None if transport && !complete && DOCKER.starts_with(s) => Some(dynamic()),
            // Some other transport, or none at all (skopeo refuses it).
            None if transport => None,
            None => Some(host_target(ref_host(s, complete))),
        }
    };
    match val {
        AbsVal::Values(vs) => vs
            .iter()
            .filter_map(|v| one(v, true))
            .reduce(|a, b| a.union(&b)),
        AbsVal::Prefix(p) => one(p, false).or_else(|| (!transport).then(dynamic)),
        AbsVal::Top => Some(dynamic()),
    }
}

/// A registry named by host (`crane catalog ghcr.io`).
fn registry_target(val: &AbsVal) -> TargetSet {
    match val {
        AbsVal::Values(vs) => vs
            .iter()
            .map(|v| {
                let h = strip_port(v.trim_end_matches('/'));
                host_target((!h.is_empty()).then(|| normalise(h)))
            })
            .reduce(|a, b| a.union(&b))
            .unwrap_or_else(|| TargetSet::unresolved(Unresolved::Dynamic)),
        _ => TargetSet::unresolved(Unresolved::Dynamic),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;

    #[test]
    fn skopeo_list_tags() {
        let t = only("skopeo", &["list-tags", "docker://ghcr.io/x/y"]);
        assert_eq!(t.targets, TargetSet::host("ghcr.io"));
        assert_eq!(t.per_call, Count::ONE);
        assert_eq!(t.uncounted, [UNCOUNTED]);
    }

    #[test]
    fn skopeo_copy_reaches_both_registries_but_not_a_local_transport() {
        let t = run(
            "skopeo",
            &[
                "copy",
                "--all",
                "docker://quay.io/a/b:1",
                "docker://ghcr.io/c/d:1",
            ],
        )
        .unwrap();
        assert_eq!(t.len(), 2);
        let t = run("skopeo", &["copy", "docker://quay.io/a/b:1", "oci:dir:x"]).unwrap();
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn a_bare_name_is_docker_hub() {
        let t = only("crane", &["ls", "nginx"]);
        assert_eq!(t.targets, TargetSet::host("registry-1.docker.io"));
        let t = only("crane", &["manifest", "docker.io/library/nginx:1"]);
        assert_eq!(t.targets, TargetSet::host("registry-1.docker.io"));
        let t = only("crane", &["digest", "localhost:5000/x"]);
        assert!(t.targets.is_local_only());
    }

    #[test]
    fn docker_pull() {
        let t = only("docker", &["pull", "quay.io/cilium/cilium:v1"]);
        assert_eq!(t.targets, TargetSet::host("quay.io"));
        assert_eq!(t.program, "docker pull");
        assert!(run("docker", &["ps"]).is_none());
    }

    #[test]
    fn two_word_subcommands() {
        let t = only("regctl", &["tag", "ls", "ghcr.io/x/y"]);
        assert_eq!(t.targets, TargetSet::host("ghcr.io"));
        let t = only("oras", &["repo", "tags", "ghcr.io/x/y"]);
        assert_eq!(t.targets, TargetSet::host("ghcr.io"));
        let t = only("crane", &["catalog", "ghcr.io"]);
        assert_eq!(t.targets, TargetSet::host("ghcr.io"));
    }

    #[test]
    fn a_prefix_reference_needs_its_first_component() {
        assert_eq!(
            ref_target(&AbsVal::Prefix("ghcr.io/x/".into()), false),
            Some(TargetSet::host("ghcr.io"))
        );
        assert_eq!(
            ref_target(&AbsVal::Prefix("ghcr.io".into()), false),
            Some(TargetSet::unresolved(Unresolved::Dynamic))
        );
    }
}
