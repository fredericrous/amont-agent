//! What a command does to the network, with the evidence for each claim.
//!
//! Mechanism, not policy: this module records transfers and how their counts
//! were derived. Whether a count is too many is `rules::request_fanout`'s
//! business.

use std::collections::BTreeSet;

use super::domain::Count;
use super::ir::Span;

/// Where a transfer goes. Never merged across contributions: two unresolved
/// destinations are not established to be one host.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TargetSet {
    pub hosts: BTreeSet<String>,
    pub unresolved: BTreeSet<Unresolved>,
    pub local: bool,
}

impl TargetSet {
    pub fn host(h: impl Into<String>) -> TargetSet {
        TargetSet {
            hosts: BTreeSet::from([h.into()]),
            unresolved: BTreeSet::new(),
            local: false,
        }
    }

    pub fn local() -> TargetSet {
        TargetSet {
            hosts: BTreeSet::new(),
            unresolved: BTreeSet::new(),
            local: true,
        }
    }

    pub fn unresolved(why: Unresolved) -> TargetSet {
        TargetSet {
            hosts: BTreeSet::new(),
            unresolved: BTreeSet::from([why]),
            local: false,
        }
    }

    pub fn union(&self, other: &TargetSet) -> TargetSet {
        TargetSet {
            hosts: self.hosts.union(&other.hosts).cloned().collect(),
            unresolved: self.unresolved.union(&other.unresolved).cloned().collect(),
            local: self.local || other.local,
        }
    }

    /// Only local destinations are possible.
    pub fn is_local_only(&self) -> bool {
        self.local && self.hosts.is_empty() && self.unresolved.is_empty()
    }

    /// Exactly one external host, and nothing else possible.
    pub fn only_host(&self) -> Option<&str> {
        (self.hosts.len() == 1 && self.unresolved.is_empty() && !self.local)
            .then(|| self.hosts.iter().next().map(String::as_str))
            .flatten()
    }
}

/// Why a destination could not be named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unresolved {
    /// Built at run time from something the analysis does not know — a
    /// variable set from a response, an inherited environment variable.
    Dynamic,
    /// An option the command model does not know came before it, so which
    /// argument is the destination is not established.
    UnknownOption,
    /// Read from a file (`curl -K`, `wget -i`).
    FromFile,
}

impl Unresolved {
    pub fn describe(self) -> &'static str {
        match self {
            Unresolved::Dynamic => "built at run time",
            Unresolved::UnknownOption => "after an option the analysis does not model",
            Unresolved::FromFile => "read from a file",
        }
    }
}

/// Which rate budget a transfer draws on, as far as the command shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Auth {
    /// `gh api`: the authenticated GitHub API budget.
    GhApi,
    /// Everything else: the quota mechanism is not established.
    Other,
}

/// One step in how a count was derived, with where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Factor {
    pub kind: FactorKind,
    pub count: Count,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactorKind {
    /// A loop's iterations, with what bounds them ("`$pages -lt 400` with
    /// `pages=$((pages+1))`").
    Loop { bound: String },
    /// The first iteration of a loop, analysed on its own.
    FirstIteration,
    /// The iterations after the first.
    LaterIterations,
    /// A call to a function defined in the command.
    Call { function: String },
    /// A command substitution or a condition evaluated this many times.
    Evaluation,
    /// A multiplier inside one invocation (`--retry`, a curl URL range).
    Builtin { what: String },
}

/// Transfers made by one call site, in one execution context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contribution {
    /// The program, as the model recognised it (`curl`, `gh api`).
    pub program: &'static str,
    pub transfers: Count,
    pub targets: TargetSet,
    pub auth: Auth,
    /// Outermost first.
    pub factors: Vec<Factor>,
    /// Requests the model knows may happen and does not count: redirects,
    /// auth hops, retries of a client's own, `~/.curlrc`.
    pub uncounted: Vec<&'static str>,
    /// The call site.
    pub span: Span,
    /// How the loop that repeats this call site spaces it out, when every
    /// path through that loop's body sleeps. `None`: nothing established.
    pub paced: Option<Pace>,
}

/// A paced repetition: `burst` transfers, then at least `secs` seconds of
/// sleep, then the next burst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pace {
    pub secs: u64,
    pub burst: Count,
}

/// Everything a command may do to the network.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    pub contributions: Vec<Contribution>,
    /// Regions the analysis could not model, which may have made transfers
    /// of their own. Kept beside the known contributions, never folded into
    /// them: an unknown region does not erase a known count.
    pub unknown: Vec<(Span, &'static str)>,
}

impl Effects {
    /// `self`, then `other`.
    pub fn then(mut self, other: Effects) -> Effects {
        self.contributions.extend(other.contributions);
        self.unknown.extend(other.unknown);
        self
    }

    /// Everything here happens `count` times (a loop, a call, a condition),
    /// recorded as `factor`. A proven-zero count removes the contributions:
    /// code that never runs makes no transfers.
    pub fn scaled(self, count: Count, factor: Option<Factor>) -> Effects {
        if count.is_zero() {
            return Effects::default();
        }
        let contributions = self
            .contributions
            .into_iter()
            .map(|mut c| {
                c.transfers = c.transfers.mul(count);
                if let Some(f) = &factor {
                    c.factors.insert(0, f.clone());
                }
                c
            })
            .collect();
        Effects {
            contributions,
            unknown: self.unknown,
        }
    }

    /// Exactly one of the alternatives happens (`if`/`else`, `case` arms).
    ///
    /// Contributions from the same call site with the same destinations are
    /// joined, not summed: sixty iterations of `if c; then curl A; else curl
    /// A2; fi` to one host make at most sixty transfers, not a hundred and
    /// twenty. Each alternative's contributions carry the count of that
    /// alternative running; a call site present in only some alternatives is
    /// joined with zero.
    pub fn alternatives(alts: Vec<Effects>) -> Effects {
        let n = alts.len();
        let mut merged: Vec<Contribution> = Vec::new();
        let mut unknown = Vec::new();
        // For each distinct (targets, auth, program) class, the per-branch
        // total; a branch without the class contributes zero.
        // Paced and unpaced transfers are different kinds of load and never
        // join: a poll in one branch and a one-off POST in the other are not
        // one burst.
        type Key = (TargetSet, Auth, &'static str, bool);
        let mut classes: Vec<(Key, Vec<Count>, Contribution)> = Vec::new();
        for (i, alt) in alts.into_iter().enumerate() {
            unknown.extend(alt.unknown);
            for c in alt.contributions {
                let key: Key = (c.targets.clone(), c.auth, c.program, c.paced.is_some());
                match classes.iter_mut().find(|(k, _, _)| *k == key) {
                    Some((_, per_branch, rep)) => {
                        per_branch[i] = per_branch[i].add(c.transfers);
                        // Both paced (the key says so): the slower claim.
                        rep.paced = rep.paced.zip(c.paced).map(|(a, b)| Pace {
                            secs: a.secs.min(b.secs),
                            burst: a.burst.join(b.burst),
                        });
                        for f in c.factors {
                            if !rep.factors.contains(&f) {
                                rep.factors.push(f);
                            }
                        }
                        for u in c.uncounted {
                            if !rep.uncounted.contains(&u) {
                                rep.uncounted.push(u);
                            }
                        }
                    }
                    None => {
                        let mut per_branch = vec![Count::ZERO; n];
                        per_branch[i] = c.transfers;
                        classes.push((key, per_branch, c));
                    }
                }
            }
        }
        for (_, per_branch, mut rep) in classes {
            rep.transfers = per_branch
                .into_iter()
                .reduce(Count::join)
                .unwrap_or(Count::ZERO);
            merged.push(rep);
        }
        Effects {
            contributions: merged,
            unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curl_to(host: &str, at: usize) -> Contribution {
        Contribution {
            program: "curl",
            transfers: Count::ONE,
            targets: TargetSet::host(host),
            auth: Auth::Other,
            factors: Vec::new(),
            uncounted: Vec::new(),
            span: at..at + 4,
            paced: None,
        }
    }

    fn one(c: Contribution) -> Effects {
        Effects {
            contributions: vec![c],
            unknown: Vec::new(),
        }
    }

    #[test]
    fn exclusive_branches_to_one_host_join_rather_than_add() {
        let e = Effects::alternatives(vec![one(curl_to("a", 0)), one(curl_to("a", 10))]);
        assert_eq!(e.contributions.len(), 1);
        assert_eq!(e.contributions[0].transfers, Count::ONE);
        let looped = e.scaled(Count::exactly(60), None);
        assert_eq!(looped.contributions[0].transfers, Count::exactly(60));
    }

    #[test]
    fn a_call_in_one_branch_only_is_maybe() {
        let e = Effects::alternatives(vec![one(curl_to("a", 0)), Effects::default()]);
        assert_eq!(e.contributions[0].transfers, Count::MAYBE);
    }

    #[test]
    fn different_destinations_stay_separate() {
        let e = Effects::alternatives(vec![one(curl_to("a", 0)), one(curl_to("b", 10))]);
        assert_eq!(e.contributions.len(), 2);
        assert!(e.contributions.iter().all(|c| c.transfers == Count::MAYBE));
    }

    #[test]
    fn a_proven_zero_removes_contributions() {
        let e = one(curl_to("a", 0)).scaled(Count::ZERO, None);
        assert!(e.contributions.is_empty());
    }

    #[test]
    fn sequence_keeps_both() {
        let e = one(curl_to("a", 0)).then(one(curl_to("a", 10)));
        assert_eq!(e.contributions.len(), 2);
    }
}
