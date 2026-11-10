//! Where a rule's stance comes from.
//!
//! Compiled default, overridden by git config, with an environment kill switch
//! on top:
//!
//! ```text
//! rule.default_stance  <  amont.agent.stance  <  amont.agent.<id>.stance
//! ```
//!
//! then capped at the rule's own [`Rule::max_stance`], and clamped to
//! `Observe` if the guard is switched off.
//!
//! The cap is applied last, after every configured key, so no configuration
//! path — the global floor or the rule's own key — can promote a rule past
//! what its analysis can support.
//!
//! ## Why git config and not `amont.conf`
//!
//! `amont.conf` is committed. A rule that a committed file could promote to
//! `Deny` would mean cloning a repository hands it the power to refuse your
//! shell commands — the trust prompt helps, but it is a yes/no over a whole
//! file, which is the wrong granularity for "which of my commands may be
//! blocked". Promotion power stays on the machine, with the person.
//!
//! `amont.conf`'s parser also rejects any stage but `pre-commit`/`pre-push`,
//! and `policy::from_lines` validates targets against `registry::CHECKS`, which
//! will never contain these rules. Teaching it otherwise would be a change to
//! the dependency-free commit path in service of this crate — exactly the
//! coupling the separate crate exists to avoid.
//!
//! ## The kill switch is an environment variable on purpose
//!
//! Reading a git config key costs a process. `$AMONT_AGENT_OFF` costs nothing
//! and is checked first, before any rule has run. `amont.agent.enabled` exists
//! too, but it is consulted only once something has already fired — where its
//! effect is to clamp to silence anyway. That asymmetry looks like an
//! inconsistency and is a deliberate one.

use crate::gitconfig as config;

use crate::rules::{Rule, Stance};

pub const KEY_ENABLED: &str = "amont.agent.enabled";
pub const KEY_STANCE: &str = "amont.agent.stance";
pub const ENV_OFF: &str = "AMONT_AGENT_OFF";

const ALLOWED: &[&str] = &["observe", "advise", "deny"];

/// Whether the guard may act at all. Checked before any rule runs.
pub fn switched_off() -> bool {
    std::env::var_os(ENV_OFF).is_some_and(|v| !v.is_empty() && v != "0")
}

/// The key naming one rule's stance.
///
/// `amont.agent.<id>.stance` parses as section `amont`, subsection
/// `agent.<id>`, key `stance` — and git treats subsections as case-SENSITIVE
/// while lowercasing sections and keys. Every rule id is lowercase kebab-case,
/// which keeps that from mattering; `rule_ids_are_lowercase_kebab` is what
/// keeps it true.
pub fn key_for(rule: &Rule) -> String {
    format!("amont.agent.{}.stance", rule.id)
}

/// The effective stance for a rule that has already fired.
///
/// Only called on the fired set: each lookup here is a `git config` process,
/// and the no-fire path must not pay for one.
pub fn resolve(rule: &Rule) -> Stance {
    if switched_off() || !config::boolean_or(KEY_ENABLED, true) {
        return Stance::Observe;
    }
    // A floor for every rule, then the per-rule key on top of it. A typo in
    // either is reported by `config`'s own `complain` rather than silently
    // disabling the guard: a stance you believe you set and did not is the
    // failure this whole crate is about.
    let floor = config::enumerated_or(KEY_STANCE, ALLOWED, rule.default_stance.as_str());
    let mine = config::enumerated_or(&key_for(rule), ALLOWED, floor);
    let st = ladder(rule, Stance::parse(mine));
    // ADR-0042: a part in the shadow arm observes for this session.
    if st != Stance::Observe && crate::shadow::drops(rule.id, rule.irreversible) {
        return Stance::Observe;
    }
    st
}

/// The key listing the agent types a rule is raised to `Deny` for.
pub fn deny_agents_key(rule: &Rule) -> String {
    format!("amont.agent.{}.denyAgents", rule.id)
}

/// Whether an agent type matches one pattern: equal, or the pattern is a
/// prefix ending in `*`.
fn type_matches(pattern: &str, agent_type: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => agent_type.starts_with(prefix),
        None => pattern == agent_type,
    }
}

/// The pure half of [`resolve_for`]: `Advise` becomes `Deny` for a named
/// agent whose type matches a pattern, clamped by the rule's ceiling. Every
/// other stance, and every other agent, is returned as it came.
pub fn raise_for(
    rule: &Rule,
    base: Stance,
    agent: &crate::lsp::Agent,
    patterns: impl FnOnce() -> Vec<String>,
) -> Stance {
    let crate::lsp::Agent::Named(agent_type) = agent else {
        return base;
    };
    if base != Stance::Advise {
        return base;
    }
    if patterns().iter().any(|p| type_matches(p, agent_type)) {
        Stance::Deny.min(rule.max_stance)
    } else {
        base
    }
}

/// Where a rule's stance for one agent came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The configured ladder: the default, the floor, the rule's own key.
    Configured,
    /// `Advise` raised to `Deny` because the agent's type is in the rule's
    /// `denyAgents`.
    DenyAgents,
}

/// [`resolve`], raised from `Advise` to `Deny` for the agent types the
/// machine's `amont.agent.<id>.denyAgents` lists (global and system git
/// config only, like the stance keys), with where the stance came from. The
/// key is read only for a named agent whose stance is `Advise`.
pub fn resolve_for(rule: &Rule, agent: &crate::lsp::Agent) -> (Stance, Source) {
    let base = resolve(rule);
    let stance = raise_for(rule, base, agent, || config::values(&deny_agents_key(rule)));
    let source = if stance == base {
        Source::Configured
    } else {
        Source::DenyAgents
    };
    (stance, source)
}

/// The configured stance, or the rule's default, then the rule's ceiling.
///
/// The pure half of [`resolve`]: `configured` is whatever the two keys
/// produced (the per-rule key when set, else the global floor, else the
/// default). Whichever path produced it, the ceiling wins.
pub fn ladder(rule: &Rule, configured: Option<Stance>) -> Stance {
    configured
        .unwrap_or(rule.default_stance)
        .min(rule.max_stance)
}

/// The same ladder for an assertion.
///
/// Deliberately the same config keys — `amont.agent.<id>.stance`, global or
/// system only — because "which of these is speaking" is one question, and
/// splitting it into two vocabularies would mean two places to look when
/// something is too loud. `deny` resolves like any other stance here; the hook
/// is where it speaks instead of refusing, since an assertion has nothing left
/// to refuse.
pub fn resolve_assertion(assertion: &crate::assertions::Assertion) -> Stance {
    if switched_off() || !config::boolean_or(KEY_ENABLED, true) {
        return Stance::Observe;
    }
    let floor = config::enumerated_or(KEY_STANCE, ALLOWED, assertion.default_stance.as_str());
    let key = format!("amont.agent.{}.stance", assertion.id);
    let mine = config::enumerated_or(&key, ALLOWED, floor);
    let st = Stance::parse(mine).unwrap_or(assertion.default_stance);
    if st != Stance::Observe && crate::shadow::drops(assertion.id, assertion.irreversible) {
        return Stance::Observe;
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capped(max: Stance) -> Rule {
        Rule {
            id: "capped",
            prevents: "test",
            irreversible: false,
            default_stance: Stance::Observe,
            max_stance: max,
            evidence: crate::rules::Evidence {
                per_1000: 0.0,
                measured: "-",
                trend: crate::rules::Trend::Rare,
            },
            examine: crate::rules::Examine::Legacy(|_| None),
            confirm: None,
        }
    }

    /// Both configuration paths — the global `amont.agent.stance` floor and
    /// the rule's own key — arrive here as the same configured value, and
    /// neither may carry a capped rule past its ceiling.
    #[test]
    fn no_configured_stance_passes_the_ceiling() {
        let rule = capped(Stance::Advise);
        assert_eq!(ladder(&rule, Some(Stance::Deny)), Stance::Advise);
        assert_eq!(ladder(&rule, Some(Stance::Advise)), Stance::Advise);
        assert_eq!(ladder(&rule, Some(Stance::Observe)), Stance::Observe);
        assert_eq!(ladder(&rule, None), Stance::Observe);
    }

    /// Every rule that shipped before the ceiling existed keeps `Deny` as its
    /// ceiling, so the cap changes nothing for them.
    #[test]
    fn an_uncapped_rule_still_resolves_as_configured() {
        let rule = capped(Stance::Deny);
        assert_eq!(ladder(&rule, Some(Stance::Deny)), Stance::Deny);
    }

    #[test]
    fn the_key_names_the_rule() {
        let r = &crate::rules::pipe_to_tail::RULE;
        assert_eq!(key_for(r), format!("amont.agent.{}.stance", r.id));
    }

    fn named(t: &str) -> crate::lsp::Agent {
        crate::lsp::Agent::Named(t.to_string())
    }

    fn patterns_of(names: &[&str]) -> impl FnOnce() -> Vec<String> {
        let owned: Vec<String> = names.iter().map(|name| name.to_string()).collect();
        move || owned
    }

    #[test]
    fn an_exact_value_and_a_trailing_star_match_a_named_agent() {
        let rule = capped(Stance::Deny);
        let haiku = named("relais:relais-worker-haiku-default");
        assert_eq!(
            raise_for(
                &rule,
                Stance::Advise,
                &haiku,
                patterns_of(&["relais:relais-worker-haiku-default"])
            ),
            Stance::Deny
        );
        assert_eq!(
            raise_for(
                &rule,
                Stance::Advise,
                &haiku,
                patterns_of(&["relais:relais-worker-haiku-*"])
            ),
            Stance::Deny
        );
        assert_eq!(
            raise_for(
                &rule,
                Stance::Advise,
                &named("relais:relais-worker-sonnet-default"),
                patterns_of(&["relais:relais-worker-haiku-*"])
            ),
            Stance::Advise
        );
    }

    #[test]
    fn main_and_unnamed_agents_never_match() {
        let rule = capped(Stance::Deny);
        for agent in [crate::lsp::Agent::Main, crate::lsp::Agent::Unnamed] {
            assert_eq!(
                raise_for(&rule, Stance::Advise, &agent, patterns_of(&["x*", "x"])),
                Stance::Advise
            );
        }
    }

    #[test]
    fn the_raise_is_clamped_by_the_ceiling() {
        let rule = capped(Stance::Advise);
        assert_eq!(
            raise_for(&rule, Stance::Advise, &named("x"), patterns_of(&["x"])),
            Stance::Advise
        );
    }

    #[test]
    fn observe_is_never_raised() {
        let rule = capped(Stance::Deny);
        assert_eq!(
            raise_for(&rule, Stance::Observe, &named("x"), patterns_of(&["x"])),
            Stance::Observe
        );
    }
}
