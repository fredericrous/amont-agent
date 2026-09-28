//! The shell state the interpreter tracks: variables as abstract values,
//! the options that change semantics, and the functions defined so far.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use super::ir::Cmd;
use crate::rules::Dialect;

/// How many distinct exact values a variable may hold before it is widened
/// to their common prefix.
const MAX_VALUES: usize = 4;

/// What the analysis knows about a string value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbsVal {
    /// One of these exact values (at most [`MAX_VALUES`]).
    Values(BTreeSet<String>),
    /// Starts with this, and continues with something unknown.
    Prefix(String),
    /// One non-empty word of unknown text: no whitespace, no glob character.
    /// What a loop over `{1..500}` or `$(seq …)` binds its variable to — the
    /// value is unknown, but that `$i` stays one word is not.
    Token,
    /// Nothing is known.
    Top,
}

impl AbsVal {
    pub fn exact(s: impl Into<String>) -> AbsVal {
        AbsVal::Values(BTreeSet::from([s.into()]))
    }

    pub fn empty() -> AbsVal {
        AbsVal::exact("")
    }

    /// The single exact value, when there is exactly one.
    pub fn as_exact(&self) -> Option<&str> {
        match self {
            AbsVal::Values(v) if v.len() == 1 => v.iter().next().map(String::as_str),
            _ => None,
        }
    }

    /// The single exact value as an integer, the way `[ … -lt … ]` and
    /// `$(( ))` read it.
    pub fn as_int(&self) -> Option<i64> {
        self.as_exact().and_then(|s| s.trim().parse().ok())
    }

    /// Definitely non-empty, definitely empty, or either.
    pub fn emptiness(&self) -> Tri {
        match self {
            AbsVal::Values(v) => {
                let any_empty = v.iter().any(String::is_empty);
                let any_full = v.iter().any(|s| !s.is_empty());
                match (any_empty, any_full) {
                    (true, false) => Tri::Yes,
                    (false, true) => Tri::No,
                    _ => Tri::Maybe,
                }
            }
            AbsVal::Prefix(p) if !p.is_empty() => Tri::No,
            AbsVal::Token => Tri::No,
            _ => Tri::Maybe,
        }
    }

    /// `self` followed by `other`, as word expansion joins pieces.
    pub fn concat(&self, other: &AbsVal) -> AbsVal {
        // The empty string is the identity: `""$v` is `$v`, whatever `$v` is.
        if self.as_exact() == Some("") {
            return other.clone();
        }
        if other.as_exact() == Some("") {
            return self.clone();
        }
        match (self, other) {
            (AbsVal::Top | AbsVal::Token, _) => AbsVal::Top,
            (AbsVal::Prefix(p), _) => AbsVal::Prefix(p.clone()),
            (AbsVal::Values(a), AbsVal::Values(b)) => {
                let mut out = BTreeSet::new();
                for x in a {
                    for y in b {
                        out.insert(format!("{x}{y}"));
                    }
                }
                widen(out)
            }
            (AbsVal::Values(a), AbsVal::Prefix(q)) => a
                .iter()
                .map(|x| AbsVal::Prefix(format!("{x}{q}")))
                .reduce(|l, r| l.join(&r))
                .unwrap_or(AbsVal::Top),
            (AbsVal::Values(a), AbsVal::Top | AbsVal::Token) => a
                .iter()
                .map(|x| {
                    if x.is_empty() {
                        AbsVal::Top
                    } else {
                        AbsVal::Prefix(x.clone())
                    }
                })
                .reduce(|l, r| l.join(&r))
                .unwrap_or(AbsVal::Top),
        }
    }

    /// Either value may hold.
    pub fn join(&self, other: &AbsVal) -> AbsVal {
        match (self, other) {
            (AbsVal::Top, _) | (_, AbsVal::Top) => AbsVal::Top,
            (AbsVal::Token, AbsVal::Token) => AbsVal::Token,
            (AbsVal::Token, AbsVal::Values(v)) | (AbsVal::Values(v), AbsVal::Token)
                if v.iter().all(|s| is_token(s)) =>
            {
                AbsVal::Token
            }
            (AbsVal::Token, _) | (_, AbsVal::Token) => AbsVal::Top,
            (AbsVal::Values(a), AbsVal::Values(b)) => {
                widen(a.iter().chain(b.iter()).cloned().collect())
            }
            (AbsVal::Values(a), AbsVal::Prefix(p)) | (AbsVal::Prefix(p), AbsVal::Values(a)) => {
                prefix_of(a.iter().map(String::as_str).chain([p.as_str()]))
            }
            (AbsVal::Prefix(p), AbsVal::Prefix(q)) => {
                prefix_of([p.as_str(), q.as_str()].into_iter())
            }
        }
    }
}

/// Non-empty, one word however it is split, and not a glob.
pub fn is_token(s: &str) -> bool {
    !s.is_empty()
        && !s
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '*' | '?' | '['))
}

/// Keep up to [`MAX_VALUES`] exact values; past that, their common prefix —
/// or, when every one is a single word and they share no prefix, a token.
fn widen(values: BTreeSet<String>) -> AbsVal {
    if values.len() <= MAX_VALUES {
        AbsVal::Values(values)
    } else if values.iter().all(|s| is_token(s)) {
        match prefix_of(values.iter().map(String::as_str)) {
            AbsVal::Top => AbsVal::Token,
            other => other,
        }
    } else {
        prefix_of(values.iter().map(String::as_str))
    }
}

fn prefix_of<'a>(mut strs: impl Iterator<Item = &'a str>) -> AbsVal {
    let Some(first) = strs.next() else {
        return AbsVal::Top;
    };
    let mut lcp = first.to_string();
    for s in strs {
        let n = lcp
            .char_indices()
            .zip(s.chars())
            .take_while(|((_, a), b)| a == b)
            .map(|((i, a), _)| i + a.len_utf8())
            .last()
            .unwrap_or(0);
        lcp.truncate(n);
    }
    if lcp.is_empty() {
        AbsVal::Top
    } else {
        AbsVal::Prefix(lcp)
    }
}

/// Yes / no / can't tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tri {
    Yes,
    No,
    Maybe,
}

impl Tri {
    pub fn join(self, other: Tri) -> Tri {
        if self == other {
            self
        } else {
            Tri::Maybe
        }
    }
}

/// The shell options that change what a command does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellOpts {
    pub errexit: Tri,
    pub pipefail: Tri,
    /// bash's `lastpipe`: the last element of a pipeline runs in the current
    /// shell. zsh always does; this option only matters for bash.
    pub lastpipe: Tri,
}

impl ShellOpts {
    /// The analysis assumption: none of these is set on entry.
    pub const ASSUMED: ShellOpts = ShellOpts {
        errexit: Tri::No,
        pipefail: Tri::No,
        lastpipe: Tri::No,
    };

    pub fn join(self, other: ShellOpts) -> ShellOpts {
        ShellOpts {
            errexit: self.errexit.join(other.errexit),
            pipefail: self.pipefail.join(other.pipefail),
            lastpipe: self.lastpipe.join(other.lastpipe),
        }
    }
}

/// Everything the interpreter knows at a point in the command.
#[derive(Debug, Clone)]
pub struct State {
    /// Variables the command has assigned. A name that is absent was
    /// inherited from an unknown environment (`Top`), unless `havoc`
    /// says otherwise — absence never means empty.
    pub vars: BTreeMap<String, AbsVal>,
    /// Positional parameters: `$1`, `$2`, … `None` means unknown (`$@` could
    /// be anything). At the top of a `-c` command there are none.
    pub positional: Option<Vec<AbsVal>>,
    pub opts: ShellOpts,
    pub dialect: Dialect,
    /// Functions defined so far, by name.
    pub funcs: BTreeMap<String, Rc<Cmd>>,
    /// Set when something the analysis cannot model (an `eval`, a `source`)
    /// ran: it may have defined functions or set options too, so a call to
    /// an unknown name is no longer "an external program".
    pub havocked: bool,
}

/// Equality for the loop fixpoint: same values, and the same function
/// DEFINITIONS (by identity — two textually equal bodies defined at two
/// places are two definitions).
impl PartialEq for State {
    fn eq(&self, other: &State) -> bool {
        self.vars == other.vars
            && self.positional == other.positional
            && self.opts == other.opts
            && self.dialect == other.dialect
            && self.havocked == other.havocked
            && self.funcs.len() == other.funcs.len()
            && self
                .funcs
                .iter()
                .zip(&other.funcs)
                .all(|((a, x), (b, y))| a == b && Rc::ptr_eq(x, y))
    }
}

impl State {
    pub fn initial(dialect: Dialect) -> State {
        State {
            vars: BTreeMap::new(),
            positional: Some(Vec::new()),
            opts: ShellOpts::ASSUMED,
            dialect,
            funcs: BTreeMap::new(),
            havocked: false,
        }
    }

    pub fn get(&self, name: &str) -> AbsVal {
        self.vars.get(name).cloned().unwrap_or(AbsVal::Top)
    }

    pub fn set(&mut self, name: &str, value: AbsVal) {
        self.vars.insert(name.to_string(), value);
    }

    /// Something unmodelled ran: every variable, option and function binding
    /// may have changed.
    pub fn havoc(&mut self) {
        for v in self.vars.values_mut() {
            *v = AbsVal::Top;
        }
        self.positional = None;
        self.opts = ShellOpts {
            errexit: Tri::Maybe,
            pipefail: Tri::Maybe,
            lastpipe: Tri::Maybe,
        };
        self.havocked = true;
    }

    /// Either state may hold.
    pub fn join(&self, other: &State) -> State {
        let mut vars = BTreeMap::new();
        for name in self.vars.keys().chain(other.vars.keys()) {
            if vars.contains_key(name) {
                continue;
            }
            vars.insert(name.clone(), self.get(name).join(&other.get(name)));
        }
        let positional = match (&self.positional, &other.positional) {
            (Some(a), Some(b)) if a.len() == b.len() => {
                Some(a.iter().zip(b).map(|(x, y)| x.join(y)).collect())
            }
            _ => None,
        };
        let mut funcs = self.funcs.clone();
        for (name, body) in &other.funcs {
            match funcs.get(name) {
                Some(mine) if Rc::ptr_eq(mine, body) => {}
                // Defined differently on two paths: which body a later call
                // runs is not known.
                Some(_) => {
                    funcs.remove(name);
                }
                None => {
                    funcs.insert(name.clone(), body.clone());
                }
            }
        }
        State {
            vars,
            positional,
            opts: self.opts.join(other.opts),
            dialect: self.dialect,
            funcs,
            havocked: self.havocked || other.havocked,
        }
    }
}

/// The host a URL-like value names, when the known part of it settles that.
///
/// A host is known only when its authority is complete within the KNOWN
/// text: `https://ghcr.io/v2/x` does, `https://ghcr.io` followed by an
/// unknown suffix does not — the suffix could be `.attacker.example/…` — and
/// neither does a value whose scheme or authority is unknown. curl's
/// scheme-less form (`ghcr.io/v2/x`) counts, since curl guesses `http://`.
pub fn url_host(value: &str, complete: bool) -> Option<String> {
    let rest = match value.find("://") {
        Some(i) => &value[i + 3..],
        None => value,
    };
    let end = rest.find(['/', '?', '#']);
    let authority = match end {
        Some(e) => &rest[..e],
        None if complete => rest,
        None => return None,
    };
    // user:pass@host:port
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(stripped) = hostport.strip_prefix('[') {
        // [v6]:port
        stripped.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// A host the homelab reaches without going through its shared egress IP.
pub fn is_local_host(host: &str) -> bool {
    let h = host.trim_end_matches('.');
    if h == "localhost" || h.ends_with(".localhost") || h == "::1" || h == "0.0.0.0" {
        return true;
    }
    if h.ends_with(".svc") || h.ends_with(".svc.cluster.local") || h.ends_with(".local") {
        return true;
    }
    // A single label resolves through the search domains: a cluster Service
    // or a LAN name, never the public internet.
    if !h.contains('.') && !h.contains(':') {
        return true;
    }
    if let Some(v4) = parse_v4(h) {
        return match v4 {
            [127, ..] | [10, ..] | [192, 168, ..] => true,
            [172, b, ..] => (16..=31).contains(&b),
            [169, 254, ..] => true,
            _ => false,
        };
    }
    false
}

fn parse_v4(h: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = h.split('.');
    for slot in &mut out {
        *slot = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vals(xs: &[&str]) -> AbsVal {
        AbsVal::Values(xs.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn concat_of_exact_values_is_exact() {
        assert_eq!(
            AbsVal::exact("https://").concat(&AbsVal::exact("ghcr.io")),
            AbsVal::exact("https://ghcr.io")
        );
    }

    #[test]
    fn concat_with_unknown_keeps_the_known_prefix() {
        assert_eq!(
            AbsVal::exact("https://ghcr.io").concat(&AbsVal::Top),
            AbsVal::Prefix("https://ghcr.io".into())
        );
        assert_eq!(AbsVal::empty().concat(&AbsVal::Top), AbsVal::Top);
        assert_eq!(AbsVal::Top.concat(&AbsVal::exact("x")), AbsVal::Top);
    }

    #[test]
    fn join_keeps_a_few_values_then_widens_to_a_prefix() {
        assert_eq!(vals(&["a"]).join(&vals(&["b"])), vals(&["a", "b"]));
        let many = vals(&["https://x/1", "https://x/2", "https://x/3"])
            .join(&vals(&["https://x/4", "https://x/5"]));
        assert_eq!(many, AbsVal::Prefix("https://x/".into()));
        assert_eq!(vals(&["a"]).join(&AbsVal::Top), AbsVal::Top);
        assert_eq!(
            vals(&["abc"]).join(&vals(&["xyz", "q", "r", "s"])),
            AbsVal::Token
        );
        assert_eq!(
            vals(&["a c"]).join(&vals(&["xyz", "q", "r", "s"])),
            AbsVal::Top
        );
    }

    #[test]
    fn emptiness_is_three_valued() {
        assert_eq!(AbsVal::exact("x").emptiness(), Tri::No);
        assert_eq!(AbsVal::empty().emptiness(), Tri::Yes);
        assert_eq!(vals(&["", "x"]).emptiness(), Tri::Maybe);
        assert_eq!(AbsVal::Prefix("h".into()).emptiness(), Tri::No);
        assert_eq!(AbsVal::Top.emptiness(), Tri::Maybe);
    }

    #[test]
    fn a_host_is_known_only_when_its_authority_is_complete() {
        assert_eq!(
            url_host("https://ghcr.io/v2/x", true).as_deref(),
            Some("ghcr.io")
        );
        assert_eq!(
            url_host("https://ghcr.io/v2", false).as_deref(),
            Some("ghcr.io")
        );
        assert_eq!(
            url_host("https://ghcr.io", true).as_deref(),
            Some("ghcr.io")
        );
        // A known prefix with an unknown continuation: `.evil/…` could follow.
        assert_eq!(url_host("https://ghcr.io", false), None);
        assert_eq!(url_host("ghcr.io/v2/x", true).as_deref(), Some("ghcr.io"));
        assert_eq!(
            url_host("https://u:p@Host.EXAMPLE:8443/x", true).as_deref(),
            Some("host.example")
        );
        assert_eq!(url_host("http://[::1]:80/", true).as_deref(), Some("::1"));
    }

    #[test]
    fn local_hosts() {
        for h in [
            "localhost",
            "127.0.0.1",
            "::1",
            "10.0.0.5",
            "172.20.1.1",
            "192.168.1.43",
            "api",
            "cluster-vision.cluster-vision.svc",
            "x.y.svc.cluster.local",
            "nas.local",
        ] {
            assert!(is_local_host(h), "{h}");
        }
        for h in [
            "ghcr.io",
            "registry-1.docker.io",
            "172.32.0.1",
            "8.8.8.8",
            "api.github.com",
        ] {
            assert!(!is_local_host(h), "{h}");
        }
    }
}
