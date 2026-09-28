//! `request-fanout` — one command that may make hundreds of network requests.
//!
//! ```sh
//! count() { … while [ -n "$url" ] && [ $pages -lt 400 ]; do
//!   hdr=$(curl -sI "$url" …); link=$(… rel="next" …); … done; }
//! count ghcr.io immich-app/immich-machine-learning "$T"
//! ```
//!
//! On 2026-09-27 a tag count written like that walked 139 ghcr.io pages from
//! the homelab's one egress address, and cluster-vision's image checker,
//! behind the same address, got 429s on its next two runs. Nothing about the
//! command looked expensive: one short function, two calls.
//!
//! ## Mechanism here, policy here, analysis elsewhere
//!
//! The counting is [`crate::analysis`]: which transfers a command may and
//! must make, per destination, with the loops, calls and options that
//! multiply them, under stated assumptions. This module only decides whether
//! that is too many and says so from the provenance. It never counts, and it
//! never fires on what the analysis could not read — an `Unknown` count is a
//! missing fact, not a large one.
//!
//! ## Why it can never refuse
//!
//! Every bound here is an estimate of what a program MAY do at run time: the
//! loop may end on its first page. A refusal would claim a certainty the
//! analysis does not have, so the rule's ceiling is `Advise`
//! ([`Rule::max_stance`]), and no configuration raises it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::ops::Range;

use crate::analysis::domain::{Count, Upper};
use crate::analysis::effects::{Auth, Contribution, FactorKind, Unresolved};
use crate::analysis::models;
use crate::rules::{Evidence, Examine, Finding, Input, Rule, Stance, Trend};

pub const RULE: Rule = Rule {
    id: "request-fanout",
    // Ships observing, and `Advise` is as far as it may ever go. Replayed
    // over 30 days (57,361 calls) it fired twice, on the two forms of the
    // incident command and nothing else: rare, kept for the cost of a miss.
    default_stance: Stance::Observe,
    max_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 0.03,
        measured: "2026-09-28",
        trend: Trend::Rare,
    },
    examine: Examine::Analysis(examine),
    confirm: None,
};

/// More explicit transfers than this to one destination group is fan-out.
/// A constant, not configuration: see the module note on estimates.
pub const BUDGET: u64 = 50;

/// A loop that definitely sleeps this long between repetitions is a poll, not
/// a burst: sixty checks thirty seconds apart are thirty minutes of one
/// request at a time, which no rate limit is about. Waiting that long in the
/// foreground is `foreground-poll`'s business.
///
/// Measured before this existed (2026-09-28, 57,257 calls over 30 days): 124
/// of the 126 firings were exactly that shape — `curl …; sleep 15..60` under
/// a counter — and the other two were the incident. With it: those two only.
///
/// Exempt only when the burst between sleeps is itself within [`BUDGET`]: a
/// hundred requests, then a minute's sleep, then a hundred more is a burst
/// on a timer.
pub const PACED_SECS: u64 = 10;

/// Where a group of transfers goes, for the decision and the message.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Dest {
    Host(String),
    /// Destinations the analysis could not name. Never merged into a host,
    /// and never claimed to be one host.
    Unresolved,
}

#[derive(Debug)]
struct Tally<'a> {
    /// Sum of lower bounds of contributions that go to exactly this host.
    established: u64,
    /// Sum of finite upper bounds of contributions that MAY go here.
    potential: Count,
    contributions: Vec<&'a Contribution>,
}

impl Default for Tally<'_> {
    fn default() -> Self {
        Tally {
            established: 0,
            potential: Count::ZERO,
            contributions: Vec::new(),
        }
    }
}

fn examine(input: &Input) -> Option<Finding> {
    if !mentions_a_client(input.src) {
        return None;
    }
    let analysis = input.analysis();
    let effects = &analysis.effects;

    let mut tallies: BTreeMap<Dest, Tally> = BTreeMap::new();
    for c in &effects.contributions {
        if c.targets.is_local_only() || is_a_poll(c) {
            continue;
        }
        let exact = c.targets.only_host();
        for h in &c.targets.hosts {
            let t = tallies.entry(Dest::Host(h.clone())).or_default();
            if exact == Some(h.as_str()) {
                t.established = t.established.saturating_add(c.transfers.lower);
            }
            t.potential = t.potential.add(Count {
                lower: 0,
                upper: c.transfers.upper,
            });
            t.contributions.push(c);
        }
        if !c.targets.unresolved.is_empty() {
            let t = tallies.entry(Dest::Unresolved).or_default();
            t.potential = t.potential.add(Count {
                lower: 0,
                upper: c.transfers.upper,
            });
            t.contributions.push(c);
        }
    }

    let firing: Vec<(&Dest, &Tally)> = tallies.iter().filter(|(_, t)| fires(t)).collect();
    if firing.is_empty() {
        return None;
    }

    let worst = firing
        .iter()
        .flat_map(|(_, t)| t.contributions.iter())
        .max_by_key(|c| rank(c.transfers))?;
    let span = worst
        .factors
        .first()
        .map(|f| f.span.clone())
        .unwrap_or_else(|| worst.span.clone());

    Some(Finding {
        reason: reason(input.src, &firing, analysis),
        remedy: remedy(&firing),
        span: clamp(span, input.src.len()),
    })
}

/// Paced at least [`PACED_SECS`] apart, a burst within budget each time.
fn is_a_poll(c: &Contribution) -> bool {
    c.paced.is_some_and(|p| {
        p.secs >= PACED_SECS && !p.burst.may_exceed(BUDGET) && p.burst.upper != Upper::Unknown
    })
}

/// The decision, on one destination group.
fn fires(t: &Tally) -> bool {
    let established = t.established > BUDGET;
    // A count the analysis could not bound is not a large count:
    // `may_exceed` is false for `Unknown`.
    let one_is_large = t
        .contributions
        .iter()
        .any(|c| c.transfers.may_exceed(BUDGET));
    let finite_sum = t
        .contributions
        .iter()
        .filter_map(|c| match c.transfers.upper {
            Upper::Finite(u) => Some(u),
            _ => None,
        })
        .fold(0u64, u64::saturating_add)
        > BUDGET;
    established || one_is_large || finite_sum
}

/// The cost heuristic in front of the analysis: a command that names no
/// modelled client, even through quoting (`cu"rl"`), makes no counted
/// transfers. A client name reached only through a variable set elsewhere is
/// `Unsupported` in the analysis anyway, which never fires.
fn mentions_a_client(src: &str) -> bool {
    let bare: String = src
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect();
    models::PROGRAMS.iter().any(|p| contains_word(&bare, p))
}

fn contains_word(hay: &str, word: &str) -> bool {
    let is_id = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.';
    hay.match_indices(word).any(|(i, _)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + word.len()..].chars().next();
        before.is_none_or(|c| !is_id(c)) && after.is_none_or(|c| !is_id(c))
    })
}

/// How alarming a count is, for choosing which contribution to point at.
fn rank(c: Count) -> (u8, u64) {
    match c.upper {
        Upper::Uncapped(_) => (3, c.lower),
        Upper::Saturated => (2, c.lower),
        Upper::Finite(u) => (1, u),
        Upper::Unknown => (0, c.lower),
    }
}

fn clamp(span: Range<usize>, len: usize) -> Range<usize> {
    span.start.min(len)..span.end.min(len)
}

fn excerpt(src: &str, span: &Range<usize>) -> String {
    let text = src.get(span.clone()).unwrap_or("").trim();
    let first = text.lines().next().unwrap_or("");
    if first.chars().count() > 60 || text.lines().nth(1).is_some() {
        let cut: String = first.chars().take(57).collect();
        format!("`{}…`", cut.trim_end())
    } else {
        format!("`{first}`")
    }
}

fn dest_name(d: &Dest) -> String {
    match d {
        Dest::Host(h) => h.clone(),
        Dest::Unresolved => "destinations the command builds at run time".to_string(),
    }
}

fn reason(src: &str, firing: &[(&Dest, &Tally)], analysis: &crate::analysis::Analysis) -> String {
    let mut out = String::new();
    let parts: Vec<String> = firing
        .iter()
        .map(|(d, t)| {
            let mut s = format!("{} to {}", potential_words(t), dest_name(d));
            if t.established > 0 {
                let _ = write!(s, " ({} established)", t.established);
            }
            s
        })
        .collect();
    let _ = write!(
        out,
        "This command may make {} — explicit command-line transfers, as far as the analysis can count them.",
        join_and(&parts)
    );

    // Provenance: how the largest contributions were multiplied.
    let mut seen: Vec<&Range<usize>> = Vec::new();
    for (_, t) in firing {
        let mut cs = t.contributions.clone();
        cs.sort_by_key(|c| std::cmp::Reverse(rank(c.transfers)));
        for c in cs.into_iter().take(3) {
            if seen.contains(&&c.span) && c.factors.is_empty() {
                continue;
            }
            seen.push(&c.span);
            let _ = write!(out, " {}", provenance(src, c));
        }
    }

    // The same call sites' transfers that did not fire on their own — the
    // first page of a walk that goes to the named host — are the context
    // that makes an unresolved group readable.
    let sites: Vec<&Range<usize>> = firing
        .iter()
        .flat_map(|(_, t)| t.contributions.iter().map(|c| &c.span))
        .collect();
    let beside: std::collections::BTreeSet<&str> = analysis
        .effects
        .contributions
        .iter()
        .filter(|c| sites.contains(&&c.span))
        .filter(|c| {
            !firing
                .iter()
                .any(|(_, t)| t.contributions.iter().any(|f| std::ptr::eq(*f, *c)))
        })
        .flat_map(|c| c.targets.hosts.iter().map(String::as_str))
        .collect();
    if !beside.is_empty() {
        let v: Vec<&str> = beside.into_iter().collect();
        let _ = write!(
            out,
            " The same call sites also reach {} on the first iteration.",
            v.join(", ")
        );
    }

    if firing.iter().any(|(d, _)| matches!(d, Dest::Unresolved)) {
        let reasons: Vec<&str> = firing
            .iter()
            .flat_map(|(_, t)| t.contributions.iter())
            .flat_map(|c| c.targets.unresolved.iter().copied())
            .map(Unresolved::describe)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let _ = write!(
            out,
            " Some destinations could not be named ({}); they are counted as a group, not as one host.",
            reasons.join(", ")
        );
    }

    let gh = firing
        .iter()
        .flat_map(|(_, t)| t.contributions.iter())
        .any(|c| c.auth == Auth::GhApi);
    let other = firing
        .iter()
        .flat_map(|(_, t)| t.contributions.iter())
        .any(|c| c.auth == Auth::Other);
    if gh {
        out.push_str(" `gh api` calls consume the authenticated API rate budget.");
    }
    if other {
        out.push_str(
            " Registry and web requests may count against rate limits shared by everything behind the homelab's egress IP.",
        );
    }

    let uncounted: std::collections::BTreeSet<&str> = firing
        .iter()
        .flat_map(|(_, t)| t.contributions.iter())
        .flat_map(|c| c.uncounted.iter().copied())
        .collect();
    if !uncounted.is_empty() {
        let v: Vec<&str> = uncounted.into_iter().collect();
        let _ = write!(out, " Not counted: {}.", v.join(", "));
    }
    if !analysis.effects.unknown.is_empty() {
        let spans: Vec<String> = analysis
            .effects
            .unknown
            .iter()
            .take(3)
            .map(|(s, why)| format!("{} ({why})", excerpt(src, s)))
            .collect();
        let _ = write!(
            out,
            " Plus activity the analysis could not follow: {}.",
            spans.join("; ")
        );
    }
    if let Some(inc) = &analysis.incomplete {
        let _ = write!(out, " The analysis stopped early: {}.", inc.why);
    }
    let _ = write!(
        out,
        " Assumed: {}.",
        crate::analysis::ASSUMPTIONS.join("; ")
    );
    out
}

fn potential_words(t: &Tally) -> String {
    let uncapped = t
        .contributions
        .iter()
        .find_map(|c| match c.transfers.upper {
            Upper::Uncapped(w) => Some(w),
            _ => None,
        });
    if let Some(w) = uncapped {
        return format!("an unbounded number of requests (it {})", w.describe());
    }
    if t.contributions
        .iter()
        .any(|c| c.transfers.upper == Upper::Saturated)
    {
        return "more requests than can be counted".to_string();
    }
    match t.potential.upper {
        Upper::Finite(u) => format!("up to {u} requests"),
        _ => {
            let finite: u64 = t
                .contributions
                .iter()
                .filter_map(|c| match c.transfers.upper {
                    Upper::Finite(u) => Some(u),
                    _ => None,
                })
                .fold(0, u64::saturating_add);
            format!("at least {finite} possible requests")
        }
    }
}

/// `curl` at `…` runs up to 400 times: loop `…` (…), called from `count`.
fn provenance(src: &str, c: &Contribution) -> String {
    let mut steps: Vec<String> = Vec::new();
    for f in &c.factors {
        match &f.kind {
            FactorKind::Loop { bound } => steps.push(format!(
                "in a loop at {} ({}: {})",
                excerpt(src, &f.span),
                f.count,
                bound
            )),
            FactorKind::Call { function } => steps.push(format!(
                "in a call to `{function}` at {}",
                excerpt(src, &f.span)
            )),
            FactorKind::Builtin { what } => steps.push(format!("×{} ({what})", f.count)),
            FactorKind::FirstIteration => steps.push("on the first iteration".to_string()),
            FactorKind::LaterIterations => {
                steps.push(format!("on the {} iterations after the first", f.count))
            }
            FactorKind::Evaluation => {}
        }
    }
    let mut dests: Vec<String> = c.targets.hosts.iter().cloned().collect();
    if !c.targets.unresolved.is_empty() {
        dests.push("unresolved".to_string());
    }
    let noun = if c.transfers == Count::ONE {
        "transfer"
    } else {
        "transfers"
    };
    let mut s = format!(
        "{} at {} makes {} {noun} to {{{}}}",
        c.program,
        excerpt(src, &c.span),
        c.transfers,
        dests.join(", ")
    );
    if !steps.is_empty() {
        let _ = write!(s, ", {}", steps.join(", "));
    }
    s.push('.');
    s
}

fn remedy(firing: &[(&Dest, &Tally)]) -> String {
    let gh = firing
        .iter()
        .flat_map(|(_, t)| t.contributions.iter())
        .any(|c| c.auth == Auth::GhApi);
    let mut r = String::from(
        "Bound the work before running it: cap the loop at the few pages the answer needs, \
         ask the API for the aggregate instead of walking pages (a count, `?n=`/`per_page=` \
         with one page, a search endpoint), or reuse a result you already have.",
    );
    if gh {
        r.push_str(" For `gh api --paginate`, add a `--jq` limit or a `per_page` and a page cap.");
    }
    r.push_str(" If the fan-out is intended, run it once, in the background, and say so.");
    r
}

fn join_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::domain::Why;
    use crate::analysis::effects::{Factor, TargetSet};

    fn contribution(targets: TargetSet, transfers: Count) -> Contribution {
        Contribution {
            program: "curl",
            transfers,
            targets,
            auth: Auth::Other,
            factors: Vec::new(),
            uncounted: Vec::new(),
            span: 0..4,
            paced: None,
        }
    }

    fn tally(cs: &[Contribution]) -> Tally<'_> {
        let mut t = Tally::default();
        for c in cs {
            t.potential = t.potential.add(Count {
                lower: 0,
                upper: c.transfers.upper,
            });
            if c.targets.only_host().is_some() {
                t.established += c.transfers.lower;
            }
            t.contributions.push(c);
        }
        t
    }

    #[test]
    fn a_hundred_to_one_host_fires() {
        let cs = [contribution(TargetSet::host("h"), Count::between(0, 100))];
        assert!(fires(&tally(&cs)));
    }

    #[test]
    fn fifty_does_not() {
        let cs = [contribution(TargetSet::host("h"), Count::exactly(50))];
        assert!(!fires(&tally(&cs)));
    }

    #[test]
    fn an_unknown_count_alone_never_fires() {
        let cs = [contribution(TargetSet::host("h"), Count::UNKNOWN)];
        assert!(!fires(&tally(&cs)));
    }

    #[test]
    fn a_known_hundred_beside_an_unknown_still_fires() {
        let cs = [
            contribution(TargetSet::host("h"), Count::between(0, 100)),
            contribution(TargetSet::host("h"), Count::UNKNOWN),
        ];
        assert!(fires(&tally(&cs)));
    }

    #[test]
    fn uncapped_fires_whatever_its_lower_bound() {
        let cs = [contribution(
            TargetSet::host("h"),
            Count::uncapped(0, Why::Paginate),
        )];
        assert!(fires(&tally(&cs)));
    }

    #[test]
    fn small_contributions_that_add_up_fire() {
        let cs = [
            contribution(TargetSet::host("h"), Count::exactly(30)),
            contribution(TargetSet::host("h"), Count::exactly(30)),
        ];
        assert!(fires(&tally(&cs)));
    }

    #[test]
    fn the_words_name_the_mechanism_and_the_assumptions() {
        let mut c = contribution(TargetSet::host("ghcr.io"), Count::between(1, 400));
        c.factors.push(Factor {
            kind: FactorKind::Loop {
                bound: "`[ $pages -lt 400 ]`".into(),
            },
            count: Count::between(1, 400),
            span: 0..4,
        });
        let cs = [c];
        let t = tally(&cs);
        let d = Dest::Host("ghcr.io".into());
        let analysis = crate::analysis::Analysis {
            effects: Default::default(),
            incomplete: None,
        };
        let text = reason("curl", &[(&d, &t)], &analysis);
        assert!(text.contains("up to 400 requests to ghcr.io"), "{text}");
        assert!(text.contains("explicit command-line transfers"), "{text}");
        assert!(text.contains("egress IP"), "{text}");
        assert!(text.contains("Assumed:"), "{text}");
    }

    #[test]
    fn a_word_match_needs_word_boundaries() {
        assert!(contains_word("x; curl -s", "curl"));
        assert!(contains_word("/usr/bin/curl -s", "curl"));
        assert!(!contains_word("libcurl-dev", "curl"));
        assert!(!contains_word("curly", "curl"));
    }
}
