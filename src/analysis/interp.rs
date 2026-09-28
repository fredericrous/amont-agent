//! Abstract interpretation of the IR.
//!
//! Each command is analysed from an abstract [`State`] and yields the set of
//! ways it can end — normal, `break`, `continue`, `return`, `exit`, or never —
//! each with the state it leaves behind and its exit status, plus the network
//! [`Effects`] it may have. Sequencing continues only from normal endings;
//! conditions branch on status; alternatives join; loops run to a state
//! fixpoint; functions are analysed at their call sites, in the caller's state
//! (dynamic scope). What the analysis cannot model invalidates everything it
//! could have changed. The full contract is in `docs/analysis.md`.

use std::collections::BTreeMap;
use std::rc::Rc;

use super::domain::{Count, Upper, Why};
use super::effects::{Contribution, Effects, Factor, FactorKind, Pace};
use super::ir::{
    AndOrOp, ArithBinOp, ArithExpr, ArithUnOp, BraceExpr, CaseArm, CaseTerm, Cmd, LoopKind,
    ParamRef, Part, SeqItem, SimpleCmd, Span, TestExpr, Word,
};
use super::limits::{self, Budget, Exhausted};
use super::models;
use super::state::{AbsVal, State, Tri};
use crate::rules::Dialect;

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Normal,
    Break(u32),
    Continue(u32),
    Return,
    Exit,
    /// It may never end.
    NonTerm,
}

/// A command's exit status, as far as it is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Zero,
    NonZero,
    Unknown,
}

impl Status {
    fn join(self, other: Status) -> Status {
        if self == other {
            self
        } else {
            Status::Unknown
        }
    }

    fn negate(self) -> Status {
        match self {
            Status::Zero => Status::NonZero,
            Status::NonZero => Status::Zero,
            Status::Unknown => Status::Unknown,
        }
    }

    fn of(t: Tri) -> Status {
        match t {
            Tri::Yes => Status::Zero,
            Tri::No => Status::NonZero,
            Tri::Maybe => Status::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
struct Outcome {
    flow: Flow,
    status: Status,
    state: State,
    /// Seconds this path definitely slept (`sleep 30`) since the command it
    /// ends started — a lower bound, so 0 is always sound. What tells a paced
    /// poll from a burst: a loop is paced when every path back to its head
    /// slept.
    slept: u64,
}

/// What analysing one command produced.
#[derive(Debug, Clone)]
struct Res {
    outs: Vec<Outcome>,
    effects: Effects,
}

impl Res {
    fn new(outs: Vec<Outcome>, effects: Effects) -> Res {
        Res {
            outs: merge(outs),
            effects,
        }
    }

    fn normal(state: State, status: Status) -> Res {
        Res::new(
            vec![Outcome {
                flow: Flow::Normal,
                status,
                state,
                slept: 0,
            }],
            Effects::default(),
        )
    }

    /// The state after the command ended normally, if it can.
    fn normal_state(&self) -> Option<State> {
        join_states(self.outs.iter().filter(|o| o.flow == Flow::Normal))
    }

    /// How often what follows runs: never, maybe, or once.
    fn reach(&self) -> Count {
        reach(&self.outs)
    }
}

fn reach(outs: &[Outcome]) -> Count {
    let normal = outs.iter().filter(|o| o.flow == Flow::Normal).count();
    if normal == 0 {
        Count::ZERO
    } else if normal == outs.len() {
        Count::ONE
    } else {
        Count::MAYBE
    }
}

fn join_states<'a>(mut outs: impl Iterator<Item = &'a Outcome>) -> Option<State> {
    let first = outs.next()?.state.clone();
    Some(outs.fold(first, |acc, o| acc.join(&o.state)))
}

/// The least any of these paths slept: a lower bound for what runs after
/// all of them.
fn least_slept<'a>(outs: impl Iterator<Item = &'a Outcome>) -> u64 {
    outs.map(|o| o.slept).min().unwrap_or(0)
}

/// Outcomes of a command that ran after `base` seconds of sleep.
fn after(base: u64, outs: Vec<Outcome>) -> Vec<Outcome> {
    outs.into_iter()
        .map(|o| Outcome {
            slept: o.slept.saturating_add(base),
            ..o
        })
        .collect()
}

/// One outcome per kind of ending, so the set stays small.
fn merge(outs: Vec<Outcome>) -> Vec<Outcome> {
    let mut merged: Vec<Outcome> = Vec::new();
    for o in outs {
        match merged.iter_mut().find(|m| m.flow == o.flow) {
            Some(m) => {
                m.state = m.state.join(&o.state);
                m.status = m.status.join(o.status);
                m.slept = m.slept.min(o.slept);
            }
            None => merged.push(o),
        }
    }
    merged
}

/// Where the analysis stopped short, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incomplete {
    pub span: Span,
    pub why: &'static str,
}

/// The result of analysing a whole command.
#[derive(Debug, Clone)]
pub struct Outcomes {
    pub effects: Effects,
    /// Set when a resource limit ended the analysis. `effects` then holds
    /// what was found before it, and nothing after it is known.
    pub incomplete: Option<Incomplete>,
}

/// The assumptions every "established" count holds under.
pub const ASSUMPTIONS: &[&str] = &[
    "the shell is not killed from outside",
    "redirections succeed",
    "errexit and pipefail are off unless the command sets them",
    "no aliases are defined",
    "inherited environment variables are unknown",
    "the command has no positional arguments of its own",
];

/// Analyse `cmd`, parsed from `src`, as run by `dialect`.
pub fn run(cmd: &Cmd, src: &str, dialect: Dialect) -> Outcomes {
    let mut it = Interp {
        src,
        budget: Budget::new(),
        calls: Vec::new(),
        func_bodies: BTreeMap::new(),
        locals: Vec::new(),
        summaries: Vec::new(),
    };
    match it.exec(cmd, State::initial(dialect), Ctx::TOP) {
        Ok(res) => Outcomes {
            effects: res.effects,
            incomplete: None,
        },
        Err(e) => Outcomes {
            effects: Effects {
                contributions: Vec::new(),
                unknown: vec![(cmd.span(), e.describe())],
            },
            incomplete: Some(Incomplete {
                span: cmd.span(),
                why: e.describe(),
            }),
        },
    }
}

/// Context that changes a command's meaning without being part of it.
#[derive(Debug, Clone, Copy)]
struct Ctx {
    /// errexit does not fire here: a condition, a non-final `&&`/`||`
    /// element, a negated pipeline.
    no_errexit: bool,
}

impl Ctx {
    const TOP: Ctx = Ctx { no_errexit: false };
    const CONDITION: Ctx = Ctx { no_errexit: true };
}

struct Interp<'s> {
    src: &'s str,
    budget: Budget,
    /// Functions being analysed, innermost last — recursion detection.
    calls: Vec<String>,
    /// One `Rc` per definition site, so re-analysing a definition inside a
    /// loop's fixpoint yields the same function, and the fixpoint converges.
    func_bodies: BTreeMap<usize, Rc<Cmd>>,
    /// Per active function call, the variables it made `local` and their
    /// values before, restored when the call returns.
    locals: Vec<Vec<(String, Option<AbsVal>)>>,
    /// Function summaries: a body analysed once per entry state and call
    /// stack, reused by every call that matches. The stack is part of the key
    /// because what counts as recursion depends on it.
    summaries: Vec<Summary>,
}

type R<T> = Result<T, Exhausted>;

impl Interp<'_> {
    fn text(&self, span: &Span) -> &str {
        self.src.get(span.clone()).unwrap_or("")
    }

    fn exec(&mut self, cmd: &Cmd, state: State, ctx: Ctx) -> R<Res> {
        self.budget.step()?;
        match cmd {
            Cmd::Simple(s) => self.simple(s, state, ctx),
            Cmd::Seq { items, .. } => self.seq(items, state, ctx),
            Cmd::AndOr { first, rest, .. } => self.and_or(first, rest, state, ctx),
            Cmd::Pipeline { negated, cmds, .. } => self.pipeline(*negated, cmds, state, ctx),
            Cmd::If {
                arms, otherwise, ..
            } => self.if_(arms, otherwise.as_deref(), state, ctx),
            Cmd::Case { word, arms, .. } => self.case(word, arms, state, ctx),
            Cmd::For {
                var,
                items,
                body,
                span,
            } => self.for_(var, items.as_deref(), body, span, state, ctx),
            Cmd::ArithFor {
                init,
                cond,
                step,
                body,
                span,
            } => self.arith_for(init, cond, step, body, span, state, ctx),
            Cmd::Loop {
                kind,
                cond,
                body,
                span,
            } => self.loop_(*kind, cond, body, span, state, ctx),
            Cmd::Group { body, .. } => self.exec(body, state, ctx),
            Cmd::Subshell { body, .. } => self.subshell(body, state, ctx),
            Cmd::FuncDef { name, body, span } => {
                let mut state = state;
                let rc = self
                    .func_bodies
                    .entry(span.start)
                    .or_insert_with(|| Rc::new((**body).clone()))
                    .clone();
                state.funcs.insert(name.clone(), rc);
                Ok(Res::normal(state, Status::Zero))
            }
            Cmd::Arith { expr, span } => {
                let _ = span;
                let mut state = state;
                let v = eval_arith(expr, &mut state);
                let status = match v {
                    Some(0) => Status::NonZero,
                    Some(_) => Status::Zero,
                    None => Status::Unknown,
                };
                let res = Res::new(
                    vec![Outcome {
                        flow: Flow::Normal,
                        status,
                        state,
                        slept: 0,
                    }],
                    Effects::default(),
                );
                Ok(self.errexit(res, ctx))
            }
            Cmd::Cond { expr, .. } => {
                let mut state = state;
                let mut eff = Effects::default();
                let t = self.test_expr(expr, &mut state, &mut eff)?;
                let res = Res::new(
                    vec![Outcome {
                        flow: Flow::Normal,
                        status: Status::of(t),
                        state,
                        slept: 0,
                    }],
                    eff,
                );
                Ok(self.errexit(res, ctx))
            }
            Cmd::Redirected {
                body, redirects, ..
            } => {
                // The targets are expanded once, before the body runs: a
                // `done < <(curl …)` fetches once however long the loop is.
                let mut state = state;
                let mut effects = Effects::default();
                for r in redirects {
                    self.expand(&r.target, &mut state, &mut effects, false)?;
                }
                let r = self.exec(body, state, ctx)?;
                Ok(Res::new(r.outs, effects.then(r.effects)))
            }
            Cmd::Unsupported { why, span } => Ok(self.unsupported(state, span, why)),
        }
    }

    /// Something unmodelled ran in the current shell: it may have changed
    /// anything, made any transfer, and ended any way.
    fn unsupported(&self, mut state: State, span: &Span, why: &'static str) -> Res {
        state.havoc();
        let outs = vec![
            Outcome {
                flow: Flow::Normal,
                status: Status::Unknown,
                state: state.clone(),
                slept: 0,
            },
            Outcome {
                flow: Flow::Exit,
                status: Status::Unknown,
                state: state.clone(),
                slept: 0,
            },
            Outcome {
                flow: Flow::NonTerm,
                status: Status::Unknown,
                state,
                slept: 0,
            },
        ];
        Res::new(
            outs,
            Effects {
                contributions: Vec::new(),
                unknown: vec![(span.clone(), why)],
            },
        )
    }

    /// Apply `set -e`: a failing command ends the shell.
    fn errexit(&self, res: Res, ctx: Ctx) -> Res {
        if ctx.no_errexit {
            return res;
        }
        let mut outs = Vec::new();
        for o in res.outs {
            if o.flow != Flow::Normal || o.status == Status::Zero || o.state.opts.errexit == Tri::No
            {
                outs.push(o);
                continue;
            }
            let certain = o.status == Status::NonZero && o.state.opts.errexit == Tri::Yes;
            outs.push(Outcome {
                flow: Flow::Exit,
                ..o.clone()
            });
            if !certain {
                outs.push(o);
            }
        }
        Res::new(outs, res.effects)
    }

    // ── sequencing and branching ─────────────────────────────────────────

    fn seq(&mut self, items: &[SeqItem], state: State, ctx: Ctx) -> R<Res> {
        let mut pending: Vec<Outcome> = vec![Outcome {
            flow: Flow::Normal,
            status: Status::Zero,
            state,
            slept: 0,
        }];
        let mut effects = Effects::default();
        for item in items {
            let Some(entry) = join_states(pending.iter().filter(|o| o.flow == Flow::Normal)) else {
                break;
            };
            let reach = reach(&pending);
            let base = least_slept(pending.iter().filter(|o| o.flow == Flow::Normal));
            pending.retain(|o| o.flow != Flow::Normal);
            if item.background {
                // `cmd &` runs in a subshell; its status is 0 and nothing it
                // writes survives, but its transfers happen.
                let r = self.exec(&item.cmd, entry.clone(), ctx)?;
                effects = effects.then(r.effects.scaled(reach, None));
                // Nothing waits for it.
                pending.push(Outcome {
                    flow: Flow::Normal,
                    status: Status::Zero,
                    state: entry,
                    slept: base,
                });
            } else {
                let r = self.exec(&item.cmd, entry, ctx)?;
                effects = effects.then(r.effects.scaled(reach, None));
                pending.extend(after(base, r.outs));
            }
            pending = merge(pending);
        }
        Ok(Res::new(pending, effects))
    }

    fn and_or(&mut self, first: &Cmd, rest: &[(AndOrOp, Cmd)], state: State, ctx: Ctx) -> R<Res> {
        let last = rest.len();
        let first_ctx = if last == 0 { ctx } else { Ctx::CONDITION };
        let r = self.exec(first, state, first_ctx)?;
        let mut outs = r.outs;
        let mut effects = r.effects;
        for (i, (op, cmd)) in rest.iter().enumerate() {
            let runs = |s: Status| match (op, s) {
                (_, Status::Unknown) => Tri::Maybe,
                (AndOrOp::And, Status::Zero) | (AndOrOp::Or, Status::NonZero) => Tri::Yes,
                _ => Tri::No,
            };
            let mut runs_on = Vec::new();
            let mut passes = Vec::new();
            for o in outs {
                if o.flow != Flow::Normal {
                    passes.push(o);
                    continue;
                }
                match runs(o.status) {
                    Tri::Yes => runs_on.push(o),
                    Tri::No => passes.push(o),
                    Tri::Maybe => {
                        runs_on.push(o.clone());
                        passes.push(o);
                    }
                }
            }
            let count = if runs_on.is_empty() {
                Count::ZERO
            } else if passes.is_empty() {
                Count::ONE
            } else {
                Count::MAYBE
            };
            let mut next = passes;
            if let Some(entry) = join_states(runs_on.iter()) {
                let base = least_slept(runs_on.iter());
                let c = if i + 1 == last { ctx } else { Ctx::CONDITION };
                let r = self.exec(cmd, entry, c)?;
                effects = effects.then(r.effects.scaled(count, None));
                next.extend(after(base, r.outs));
            }
            outs = merge(next);
        }
        Ok(Res::new(outs, effects))
    }

    fn if_(
        &mut self,
        arms: &[(Cmd, Cmd)],
        otherwise: Option<&Cmd>,
        state: State,
        ctx: Ctx,
    ) -> R<Res> {
        let Some(((cond, body), rest)) = arms.split_first() else {
            return match otherwise {
                Some(o) => self.exec(o, state, ctx),
                None => Ok(Res::normal(state, Status::Zero)),
            };
        };
        let c = self.exec(cond, state, Ctx::CONDITION)?;
        let mut outs: Vec<Outcome> = c
            .outs
            .iter()
            .filter(|o| o.flow != Flow::Normal)
            .cloned()
            .collect();
        let (yes, no) = split_status(&c.outs, Status::Zero);
        let base = least_slept(c.outs.iter().filter(|o| o.flow == Flow::Normal));
        let mut alts = Vec::new();
        if let Some(s) = yes {
            let r = self.exec(body, s, ctx)?;
            outs.extend(after(base, r.outs));
            alts.push(r.effects);
        }
        if let Some(s) = no {
            let r = self.if_(rest, otherwise, s, ctx)?;
            outs.extend(after(base, r.outs));
            alts.push(r.effects);
        }
        // Exactly one branch runs once the condition completes, so the
        // branches are alternatives; what follows the condition runs as often
        // as the condition completes normally.
        let branches = Effects::alternatives(alts).scaled(c.reach(), None);
        Ok(Res::new(outs, c.effects.then(branches)))
    }

    fn case(&mut self, word: &Word, arms: &[CaseArm], state: State, ctx: Ctx) -> R<Res> {
        let mut state = state;
        let mut effects = Effects::default();
        let subject = self.expand(word, &mut state, &mut effects, false)?.val;
        let only_breaks = arms.iter().all(|a| a.term == CaseTerm::Break);
        let mut outs = Vec::new();
        let mut alts = Vec::new();
        // `testing`: a state in which the arms so far have not matched.
        let mut testing: Option<State> = Some(state);
        // `fall`: a state entering the next arm's body without a test (`;&`).
        let mut fall: Option<State> = None;
        for arm in arms {
            let mut entered: Option<State> = fall.take();
            let mut not_matched: Option<State> = None;
            if let Some(mut s) = testing.take() {
                let mut hit = Tri::No;
                for p in &arm.patterns {
                    let pat = self.expand(p, &mut s, &mut effects, false)?.val;
                    hit = or(hit, pattern_match(&subject, &pat));
                    if hit == Tri::Yes {
                        break;
                    }
                }
                match hit {
                    Tri::Yes => entered = join_opt(entered, Some(s)),
                    Tri::No => not_matched = Some(s),
                    Tri::Maybe => {
                        entered = join_opt(entered, Some(s.clone()));
                        not_matched = Some(s);
                    }
                }
            }
            if let Some(s) = entered {
                let r = self.exec(&arm.body, s, ctx)?;
                let after = r.normal_state();
                if only_breaks {
                    alts.push(r.effects);
                } else {
                    // With fall-through or re-testing, arms are not simply
                    // exclusive: count each as possibly running.
                    effects = effects.then(r.effects.scaled(Count::MAYBE, None));
                }
                for o in r.outs {
                    if o.flow != Flow::Normal || arm.term == CaseTerm::Break {
                        outs.push(o);
                    }
                }
                match arm.term {
                    CaseTerm::Break => {}
                    CaseTerm::FallThrough => fall = after,
                    CaseTerm::TestNext => not_matched = join_opt(not_matched, after),
                }
            }
            testing = not_matched;
        }
        // Falling off the end: no arm matched (status 0), or the last arm
        // fell through.
        if let Some(s) = join_opt(testing, fall) {
            if only_breaks {
                alts.push(Effects::default());
            }
            outs.push(Outcome {
                flow: Flow::Normal,
                status: Status::Zero,
                state: s,
                slept: 0,
            });
        }
        if only_breaks {
            effects = effects.then(Effects::alternatives(alts));
        }
        Ok(Res::new(outs, effects))
    }

    fn subshell(&mut self, body: &Cmd, state: State, ctx: Ctx) -> R<Res> {
        let entry = state.clone();
        let r = self.exec(body, state, ctx)?;
        let status = r
            .outs
            .iter()
            .map(|o| o.status)
            .reduce(Status::join)
            .unwrap_or(Status::Unknown);
        let res = Res::new(
            vec![Outcome {
                flow: Flow::Normal,
                status,
                state: entry,
                slept: 0,
            }],
            r.effects,
        );
        Ok(self.errexit(res, ctx))
    }

    fn pipeline(&mut self, negated: bool, cmds: &[Cmd], state: State, ctx: Ctx) -> R<Res> {
        let inner = if negated { Ctx::CONDITION } else { ctx };
        let mut effects = Effects::default();
        let mut statuses = Vec::new();
        let last = cmds.len().saturating_sub(1);
        let mut final_outs: Vec<Outcome> = Vec::new();
        for (i, c) in cmds.iter().enumerate() {
            let r = self.exec(c, state.clone(), inner)?;
            effects = effects.then(r.effects);
            let status = r
                .outs
                .iter()
                .map(|o| o.status)
                .reduce(Status::join)
                .unwrap_or(Status::Unknown);
            statuses.push(status);
            if i == last && cmds.len() == 1 {
                final_outs = r.outs;
            } else if i == last {
                final_outs = self.last_pipe_element(r.outs, &state);
            }
        }
        let status = pipe_status(&statuses, state.opts.pipefail);
        let status = if negated { status.negate() } else { status };
        let outs = final_outs
            .into_iter()
            .map(|o| Outcome {
                status: if o.flow == Flow::Normal {
                    status
                } else {
                    o.status
                },
                ..o
            })
            .collect();
        let res = Res::new(outs, effects);
        Ok(if negated { res } else { self.errexit(res, ctx) })
    }

    /// The last element of a multi-command pipeline runs in the current shell
    /// in zsh, and in bash only with `lastpipe`. Where that is not known,
    /// both readings are kept.
    fn last_pipe_element(&self, outs: Vec<Outcome>, entry: &State) -> Vec<Outcome> {
        let discarded: Vec<Outcome> = outs
            .iter()
            .map(|o| Outcome {
                flow: Flow::Normal,
                status: o.status,
                state: entry.clone(),
                slept: 0,
            })
            .collect();
        let keeps = match entry.dialect {
            Dialect::Zsh => Tri::Yes,
            Dialect::Bash => entry.opts.lastpipe,
            Dialect::Unknown => match entry.opts.lastpipe {
                Tri::Yes => Tri::Yes,
                _ => Tri::Maybe,
            },
        };
        match keeps {
            Tri::Yes => outs,
            Tri::No => discarded,
            Tri::Maybe => outs.into_iter().chain(discarded).collect(),
        }
    }

    // ── loops ─────────────────────────────────────────────────────────────

    fn loop_(
        &mut self,
        kind: LoopKind,
        cond: &Cmd,
        body: &Cmd,
        span: &Span,
        state: State,
        ctx: Ctx,
    ) -> R<Res> {
        let counter = self.counter_bound(kind, cond, body, &state);
        let continues = |s: Status| match (kind, s) {
            (_, Status::Unknown) => Tri::Maybe,
            (LoopKind::While, Status::Zero) | (LoopKind::Until, Status::NonZero) => Tri::Yes,
            _ => Tri::No,
        };
        let cond_split = |outs: &[Outcome]| -> (Option<State>, Option<State>, Vec<Outcome>) {
            let mut go = Vec::new();
            let mut stop = Vec::new();
            let mut other = Vec::new();
            for o in outs {
                if o.flow != Flow::Normal {
                    other.push(o.clone());
                    continue;
                }
                match continues(o.status) {
                    Tri::Yes => go.push(o.clone()),
                    Tri::No => stop.push(o.clone()),
                    Tri::Maybe => {
                        go.push(o.clone());
                        stop.push(o.clone());
                    }
                }
            }
            (join_states(go.iter()), join_states(stop.iter()), other)
        };

        // The first check and the first iteration, from the entry state.
        let c1 = self.exec(cond, state.clone(), Ctx::CONDITION)?;
        let (go1, stop1, mut outs) = cond_split(&c1.outs);
        let first_enters = match (&go1, &stop1) {
            (Some(_), None) => Count::ONE,
            (Some(_), Some(_)) => Count::MAYBE,
            (None, _) => Count::ZERO,
        };
        let mut exits: Vec<Outcome> = stop1
            .into_iter()
            .map(|s| Outcome {
                flow: Flow::Normal,
                status: Status::Zero,
                state: s,
                slept: 0,
            })
            .collect();
        let Some(go1) = go1 else {
            outs.extend(exits);
            return Ok(Res::new(outs, c1.effects));
        };
        let b1 = self.exec(body, go1, ctx)?;
        let (back1, mut ended1) = back_edge(&b1.outs);
        outs.append(&mut ended1);

        // Later iterations, from the fixpoint of the loop-carried state.
        let mut fix = back1.clone();
        let mut later: Option<(Res, Res)> = None;
        if let Some(mut s) = fix.take() {
            let mut rounds = 0;
            loop {
                rounds += 1;
                let c = self.exec(cond, s.clone(), Ctx::CONDITION)?;
                let (go, _, _) = cond_split(&c.outs);
                let Some(go) = go else {
                    later = Some((c, Res::new(Vec::new(), Effects::default())));
                    break;
                };
                let b = self.exec(body, go, ctx)?;
                let (back, _) = back_edge(&b.outs);
                let next = match back {
                    Some(bs) => s.join(&bs),
                    None => s.clone(),
                };
                if next == s || rounds >= limits::MAX_ROUNDS {
                    later = Some((c, b));
                    break;
                }
                s = if rounds >= limits::WIDEN_AFTER {
                    widen(&s, &next)
                } else {
                    next
                };
            }
            fix = Some(s);
        }

        // How many iterations, and on what evidence.
        let body_breaks = has_early_exit(&b1.outs)
            || later.as_ref().is_some_and(|(_, b)| has_early_exit(&b.outs));
        let later_network = later.as_ref().is_some_and(|(_, b)| {
            b.effects
                .contributions
                .iter()
                .any(|c| !c.targets.is_local_only())
        }) || b1
            .effects
            .contributions
            .iter()
            .any(|c| !c.targets.is_local_only());
        let (iterations, bound_desc) = match &counter {
            Some(cb) => {
                let exact = cb.alone && !body_breaks && first_enters == Count::ONE;
                let lower = if exact {
                    cb.max
                } else {
                    first_enters.lower.min(cb.max)
                };
                (Count::between(lower, cb.max), cb.desc.clone())
            }
            None => {
                let lower = first_enters.lower;
                if self.is_literal_true(cond) && !body_breaks {
                    (
                        Count::uncapped(lower, Why::Infinite),
                        "nothing stops it".to_string(),
                    )
                } else if later_network && self.follows_next(kind, cond, fix.as_ref(), &state) {
                    (
                        Count::uncapped(lower, Why::NextLinkFollow),
                        format!("`{}` until it is empty", self.text(&cond.span()).trim()),
                    )
                } else {
                    (
                        Count {
                            lower,
                            upper: Upper::Unknown,
                        },
                        "no established bound".to_string(),
                    )
                }
            }
        };
        let loop_factor = Factor {
            kind: FactorKind::Loop { bound: bound_desc },
            count: iterations,
            span: span.clone(),
        };

        let later_pause = later.as_ref().map_or(u64::MAX, |(c, b)| {
            let cond = least_slept(c.outs.iter().filter(|o| o.flow == Flow::Normal));
            cond.saturating_add(back_edge_slept(&b.outs))
        });
        let per_iteration = least_slept(c1.outs.iter().filter(|o| o.flow == Flow::Normal))
            .saturating_add(back_edge_slept(&b1.outs))
            .min(later_pause);
        let mut c1 = c1;
        let mut b1 = b1;
        pace(&mut c1.effects, per_iteration);
        pace(&mut b1.effects, per_iteration);
        let later = later.map(|(mut c, mut b)| {
            pace(&mut c.effects, per_iteration);
            pace(&mut b.effects, per_iteration);
            (c, b)
        });
        let mut effects = c1.effects;
        effects = effects.then(b1.effects.scaled(
            first_enters,
            Some(Factor {
                kind: FactorKind::FirstIteration,
                count: first_enters,
                span: span.clone(),
            }),
        ));
        let rest = iterations.minus_one();
        if let Some((c, b)) = later {
            // The condition runs once more per later iteration, plus the final
            // check that ends the loop.
            effects = effects.then(c.effects.scaled(
                rest.add(Count::MAYBE),
                Some(Factor {
                    kind: FactorKind::Evaluation,
                    count: rest.add(Count::MAYBE),
                    span: cond.span(),
                }),
            ));
            effects = effects.then(b.effects.scaled(
                rest,
                Some(Factor {
                    kind: FactorKind::LaterIterations,
                    count: rest,
                    span: span.clone(),
                }),
            ));
            let (_, mut ended) = back_edge(&b.outs);
            outs.append(&mut ended);
        }
        for c in &mut effects.contributions {
            if !c.factors.contains(&loop_factor) {
                c.factors.insert(0, loop_factor.clone());
            }
        }
        // Where the loop can stop normally: the condition fails at the fixpoint
        // (which includes the first check), or a `break` ends it.
        if let Some(s) = fix {
            let c = self.exec(cond, s, Ctx::CONDITION)?;
            let (_, stop, _) = cond_split(&c.outs);
            if let Some(s) = stop {
                exits.push(Outcome {
                    flow: Flow::Normal,
                    status: Status::Zero,
                    state: s,
                    slept: 0,
                });
            }
        }
        if iterations.upper == Upper::Uncapped(Why::Infinite) && !body_breaks {
            outs.retain(|o| o.flow != Flow::Normal);
            outs.push(Outcome {
                flow: Flow::NonTerm,
                status: Status::Unknown,
                state,
                slept: 0,
            });
        } else {
            outs.extend(exits);
        }
        Ok(Res::new(outs, effects))
    }

    fn is_literal_true(&self, cond: &Cmd) -> bool {
        let Some(s) = last_simple(cond) else {
            return false;
        };
        s.assigns.is_empty()
            && s.words.len() == 1
            && matches!(s.words[0].literal().as_deref(), Some("true" | ":"))
    }

    /// Positive evidence of pagination: the condition's final command is a
    /// non-emptiness test on a variable, and the loop-carried value of that
    /// variable is no longer what it was on entry — the body keeps
    /// reassigning it from what its own transfers returned.
    fn follows_next(&self, kind: LoopKind, cond: &Cmd, fix: Option<&State>, entry: &State) -> bool {
        if kind != LoopKind::While {
            return false;
        }
        let Some(fix) = fix else { return false };
        conjuncts(cond, AndOrOp::And).into_iter().any(|c| {
            nonempty_test_var(c)
                .is_some_and(|v| fix.get(&v) != entry.get(&v) && fix.get(&v).as_exact().is_none())
        })
    }

    fn for_(
        &mut self,
        var: &str,
        items: Option<&[Word]>,
        body: &Cmd,
        span: &Span,
        state: State,
        ctx: Ctx,
    ) -> R<Res> {
        let mut state = state;
        let mut effects = Effects::default();
        let (count, val) = match items {
            Some(words) => {
                let mut count = Count::ZERO;
                let mut val: Option<AbsVal> = None;
                for w in words {
                    let e = self.expand(w, &mut state, &mut effects, true)?;
                    count = count.add(e.fields);
                    val = Some(match val {
                        Some(v) => v.join(&e.val),
                        None => e.val,
                    });
                }
                (count, val.unwrap_or(AbsVal::Top))
            }
            None => match &state.positional {
                Some(p) => (
                    Count::exactly(p.len() as u64),
                    p.iter()
                        .cloned()
                        .reduce(|a, b| a.join(&b))
                        .unwrap_or(AbsVal::Top),
                ),
                None => (Count::UNKNOWN, AbsVal::Top),
            },
        };
        let desc = format!("{} items", count);
        self.iterate(
            count,
            desc,
            body,
            span,
            state,
            ctx,
            effects,
            |s: &mut State, _| s.set(var, val.clone()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn arith_for(
        &mut self,
        init: &Option<ArithExpr>,
        cond: &Option<ArithExpr>,
        step: &Option<ArithExpr>,
        body: &Cmd,
        span: &Span,
        state: State,
        ctx: Ctx,
    ) -> R<Res> {
        let mut state = state;
        if let Some(e) = init {
            eval_arith(e, &mut state);
        }
        let count = match (cond, step) {
            (Some(c), Some(st)) => arith_counter(c, st, &state, body)
                .map(|n| Count::between(0, n))
                .unwrap_or(Count::UNKNOWN),
            _ => Count::UNKNOWN,
        };
        let desc = match count.upper {
            Upper::Finite(n) => format!("at most {n} iterations of `for ((…))`"),
            _ => "no established bound".to_string(),
        };
        let writes: Vec<String> = step.as_ref().map(arith_writes).unwrap_or_default();
        self.iterate(
            count,
            desc,
            body,
            span,
            state,
            ctx,
            Effects::default(),
            |s, first| {
                // The step runs between iterations: what it writes is unknown
                // from the second iteration on.
                if !first {
                    for v in &writes {
                        s.set(v, AbsVal::Top);
                    }
                }
            },
        )
    }

    /// A counted loop (`for`, `for ((…))`): the first iteration from the entry
    /// state, the rest from a fixpoint, `count` iterations in all.
    #[allow(clippy::too_many_arguments)]
    fn iterate(
        &mut self,
        count: Count,
        desc: String,
        body: &Cmd,
        span: &Span,
        state: State,
        ctx: Ctx,
        effects: Effects,
        each: impl Fn(&mut State, bool),
    ) -> R<Res> {
        let mut effects = effects;
        let mut outs = Vec::new();
        if count.is_zero() {
            outs.push(Outcome {
                flow: Flow::Normal,
                status: Status::Zero,
                state,
                slept: 0,
            });
            return Ok(Res::new(outs, effects));
        }
        let mut s1 = state.clone();
        each(&mut s1, true);
        let b1 = self.exec(body, s1, ctx)?;
        let (back1, mut ended1) = back_edge(&b1.outs);
        outs.append(&mut ended1);
        let breaks = has_early_exit(&b1.outs);
        let first = if count.lower >= 1 {
            Count::ONE
        } else {
            Count::MAYBE
        };
        let mut fix = back1.clone().unwrap_or_else(|| state.clone());
        let later;
        let mut rounds = 0;
        loop {
            rounds += 1;
            let mut s = fix.clone();
            each(&mut s, false);
            let b = self.exec(body, s, ctx)?;
            let (back, _) = back_edge(&b.outs);
            let next = back.map(|bs| fix.join(&bs)).unwrap_or_else(|| fix.clone());
            if next == fix || rounds >= limits::MAX_ROUNDS {
                later = b;
                break;
            }
            fix = if rounds >= limits::WIDEN_AFTER {
                widen(&fix, &next)
            } else {
                next
            };
        }
        let iterations = if breaks || has_early_exit(&later.outs) {
            Count {
                lower: count.lower.min(1),
                upper: count.upper,
            }
        } else {
            count
        };
        let loop_factor = Factor {
            kind: FactorKind::Loop { bound: desc },
            count: iterations,
            span: span.clone(),
        };
        let per_iteration = back_edge_slept(&b1.outs).min(back_edge_slept(&later.outs));
        let (mut b1, mut later) = (b1, later);
        pace(&mut b1.effects, per_iteration);
        pace(&mut later.effects, per_iteration);
        effects = effects.then(b1.effects.scaled(
            first,
            Some(Factor {
                kind: FactorKind::FirstIteration,
                count: first,
                span: span.clone(),
            }),
        ));
        let rest = iterations.minus_one();
        effects = effects.then(later.effects.scaled(
            rest,
            Some(Factor {
                kind: FactorKind::LaterIterations,
                count: rest,
                span: span.clone(),
            }),
        ));
        let (_, mut ended) = back_edge(&later.outs);
        outs.append(&mut ended);
        for c in &mut effects.contributions {
            if !c.factors.contains(&loop_factor) {
                c.factors.insert(0, loop_factor.clone());
            }
        }
        outs.push(Outcome {
            flow: Flow::Normal,
            status: Status::Unknown,
            state: state.join(&fix),
            slept: 0,
        });
        Ok(Res::new(outs, effects))
    }

    /// The narrowly defined counter pattern, or nothing.
    ///
    /// All of these must hold: the counter is initialised to a literal
    /// integer before the loop (its entry value is exact); the condition's
    /// final command is (a conjunction containing) `[ $v -lt M ]`-style
    /// comparison; the body's top-level sequence increments it by one exactly
    /// once, unconditionally; nothing else in the body — including functions
    /// it calls and anything unmodelled — can write it; and no `continue` can
    /// skip the increment.
    fn counter_bound(
        &self,
        kind: LoopKind,
        cond: &Cmd,
        body: &Cmd,
        entry: &State,
    ) -> Option<CounterBound> {
        let op = match kind {
            LoopKind::While => AndOrOp::And,
            LoopKind::Until => AndOrOp::Or,
        };
        let parts = conjuncts(cond, op);
        let alone = parts.len() == 1;
        let mut best: Option<CounterBound> = None;
        for part in parts {
            let Some((var, cmp, limit)) = comparison(part, entry) else {
                continue;
            };
            let Some(start) = entry.get(&var).as_int() else {
                continue;
            };
            // Iterations while `var CMP limit` holds (while) / until it holds.
            let continue_while = match (kind, cmp.as_str()) {
                (LoopKind::While, "-lt") | (LoopKind::Until, "-ge") => Some(false),
                (LoopKind::While, "-le") | (LoopKind::Until, "-gt") => Some(true),
                _ => None,
            };
            let Some(inclusive) = continue_while else {
                continue;
            };
            if !increments_once(body, &var) || writes_elsewhere(body, cond, &var, entry) {
                continue;
            }
            let span_text = self.text(&part.span()).trim().to_string();
            let n = limit.saturating_sub(start);
            let max = if inclusive { n.saturating_add(1) } else { n };
            let max = u64::try_from(max.max(0)).unwrap_or(0);
            let desc = format!("`{span_text}` with `{var}` counting up from {start}");
            if best.as_ref().is_none_or(|b| max < b.max) {
                best = Some(CounterBound { max, desc, alone });
            }
        }
        best
    }

    // ── simple commands ───────────────────────────────────────────────────

    fn simple(&mut self, s: &SimpleCmd, state: State, ctx: Ctx) -> R<Res> {
        let mut state = state;
        let mut effects = Effects::default();
        let mut subst_status: Option<Status> = None;
        // Words first, then assignments' values, in source order is close
        // enough: both expand before the command runs.
        let mut args = Vec::new();
        for w in &s.words {
            let e = self.expand(w, &mut state, &mut effects, true)?;
            if e.subst {
                subst_status = Some(Status::Unknown);
            }
            args.push(models::Arg {
                val: e.val,
                fields: e.fields,
                span: w.span.clone(),
            });
        }
        let mut assigned = Vec::new();
        for a in &s.assigns {
            let e = self.expand(&a.value, &mut state, &mut effects, false)?;
            if e.subst {
                subst_status = Some(Status::Unknown);
            }
            let v = if a.append {
                state.get(&a.name).concat(&e.val)
            } else {
                e.val
            };
            assigned.push((a.name.clone(), v));
        }
        for r in &s.redirects {
            self.expand(&r.target, &mut state, &mut effects, false)?;
        }
        if args.is_empty() {
            for (name, v) in assigned {
                self.assign(&mut state, &name, v);
            }
            let status = subst_status.unwrap_or(Status::Zero);
            let res = Res::new(
                vec![Outcome {
                    flow: Flow::Normal,
                    status,
                    state,
                    slept: 0,
                }],
                effects,
            );
            return Ok(self.errexit(res, ctx));
        }
        let r = self.command(s, &args, state, ctx)?;
        Ok(Res::new(r.outs, effects.then(r.effects)))
    }

    /// Run a command whose words are expanded.
    fn command(&mut self, s: &SimpleCmd, args: &[models::Arg], state: State, ctx: Ctx) -> R<Res> {
        let name_arg = &args[0];
        let Some(name) = single_exact(name_arg) else {
            // `$cmd …` with a name the analysis cannot know: it could be
            // anything, including `eval` or a function.
            return Ok(self.unsupported(state, &s.span, "a command name built at run time"));
        };
        let rest = &args[1..];
        let res = match name.as_str() {
            ":" | "true" => Res::normal(state, Status::Zero),
            "false" => Res::normal(state, Status::NonZero),
            "[" | "test" => {
                let argv: Vec<&models::Arg> = if name == "[" {
                    match rest.split_last() {
                        Some((last, init)) if single_exact(last).as_deref() == Some("]") => {
                            init.iter().collect()
                        }
                        _ => return Ok(Res::normal(state, Status::NonZero)),
                    }
                } else {
                    rest.iter().collect()
                };
                let st = Status::of(test_argv(&argv));
                Res::normal(state, st)
            }
            "echo" | "printf" | "cd" | "pushd" | "popd" | "wait" | "pwd" | "type" | "hash"
            | "umask" | "ulimit" | "jobs" | "disown" | "kill" => {
                let mut state = state;
                if name == "printf" {
                    if let Some(i) = rest
                        .iter()
                        .position(|a| single_exact(a).as_deref() == Some("-v"))
                    {
                        if let Some(v) = rest.get(i + 1).and_then(single_exact) {
                            self.assign(&mut state, &v, AbsVal::Top);
                        }
                    }
                }
                let st = if matches!(name.as_str(), "echo" | "printf" | "pwd") {
                    Status::Zero
                } else {
                    Status::Unknown
                };
                Res::normal(state, st)
            }
            "sleep" => {
                let pause = rest
                    .iter()
                    .map(|a| single_exact(a).and_then(|t| seconds(&t)))
                    .sum::<Option<u64>>()
                    .unwrap_or(0);
                let mut res = Res::normal(state, Status::Zero);
                for o in &mut res.outs {
                    o.slept = pause;
                }
                res
            }
            "local" | "typeset" | "declare" | "export" | "readonly" | "integer" => {
                self.declare(&name, rest, state)
            }
            "unset" => {
                let mut state = state;
                for a in rest {
                    if let Some(v) = single_exact(a) {
                        if !v.starts_with('-') {
                            self.assign(&mut state, &v, AbsVal::empty());
                        }
                    }
                }
                Res::normal(state, Status::Zero)
            }
            "read" | "mapfile" | "readarray" | "getopts" => {
                let mut state = state;
                for a in rest {
                    match single_exact(a) {
                        Some(v) if !v.starts_with('-') => self.assign(&mut state, &v, AbsVal::Top),
                        Some(_) => {}
                        None => state.havoc(),
                    }
                }
                Res::normal(state, Status::Unknown)
            }
            "let" => {
                let mut state = state;
                // `let` arguments are arithmetic, but reach us as words; only
                // the increment forms the counter pattern needs are read.
                let mut last = None;
                for a in rest {
                    match single_exact(a).and_then(|t| parse_let(&t)) {
                        Some((var, delta)) => {
                            let v = state.get(&var).as_int().and_then(|n| n.checked_add(delta));
                            self.assign(
                                &mut state,
                                &var,
                                v.map(|n| AbsVal::exact(n.to_string()))
                                    .unwrap_or(AbsVal::Top),
                            );
                            last = v;
                        }
                        None => {
                            state.havoc();
                            last = None;
                        }
                    }
                }
                let st = match last {
                    Some(0) => Status::NonZero,
                    Some(_) => Status::Zero,
                    None => Status::Unknown,
                };
                Res::normal(state, st)
            }
            "shift" => {
                let mut state = state;
                let n = rest
                    .first()
                    .and_then(single_exact)
                    .and_then(|t| t.parse::<usize>().ok())
                    .unwrap_or(1);
                state.positional = state.positional.map(|mut p| {
                    let k = n.min(p.len());
                    p.drain(..k);
                    p
                });
                Res::normal(state, Status::Unknown)
            }
            "set" => self.set_builtin(rest, state),
            "shopt" => {
                let mut state = state;
                let words: Vec<Option<String>> = rest.iter().map(single_exact).collect();
                let on = words.iter().any(|w| w.as_deref() == Some("-s"));
                let off = words.iter().any(|w| w.as_deref() == Some("-u"));
                if words.iter().any(|w| w.as_deref() == Some("lastpipe")) {
                    state.opts.lastpipe = match (on, off) {
                        (true, false) => Tri::Yes,
                        (false, true) => Tri::No,
                        _ => state.opts.lastpipe,
                    };
                }
                Res::normal(state, Status::Zero)
            }
            "break" | "continue" => {
                let n = rest
                    .first()
                    .and_then(single_exact)
                    .and_then(|t| t.parse::<u32>().ok())
                    .unwrap_or(1)
                    .max(1);
                let flow = if name == "break" {
                    Flow::Break(n)
                } else {
                    Flow::Continue(n)
                };
                Res::new(
                    vec![Outcome {
                        flow,
                        status: Status::Zero,
                        state,
                        slept: 0,
                    }],
                    Effects::default(),
                )
            }
            "return" | "exit" => {
                let status = match rest
                    .first()
                    .and_then(single_exact)
                    .and_then(|t| t.parse::<i64>().ok())
                {
                    Some(0) => Status::Zero,
                    Some(_) => Status::NonZero,
                    // Bare, it returns the last command's status.
                    None => Status::Unknown,
                };
                let flow = if name == "return" && !self.calls.is_empty() {
                    Flow::Return
                } else {
                    Flow::Exit
                };
                Res::new(
                    vec![Outcome {
                        flow,
                        status,
                        state,
                        slept: 0,
                    }],
                    Effects::default(),
                )
            }
            // Run in THIS shell and can change anything.
            "eval" | "source" | "." | "trap" | "alias" | "unalias" | "enable" | "emulate"
            | "setopt" | "unsetopt" | "zmodload" | "autoload" | "compdef" => {
                return Ok(self.unsupported(state, &s.span, "a builtin that can change the shell"))
            }
            // Wrappers: the rest of the words are the command.
            "command" | "builtin" | "exec" | "nohup" | "time" | "sudo" | "doas" | "env"
            | "nice" | "ionice" | "timeout" | "stdbuf" | "caffeinate" => {
                return self.wrapper(&name, s, rest, state, ctx)
            }
            // A shell or runner reading a program we cannot see.
            "xargs" | "parallel" | "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" | "watch"
            | "flock" | "script" | "ssh" | "kubectl" | "docker-compose" => {
                return Ok(self.external_unknown(state, s, &name, ctx))
            }
            _ => {
                if let Some(body) = state.funcs.get(&name).cloned() {
                    return self.call(&name, &body, rest, s, state, ctx);
                }
                return Ok(self.external(state, s, &name, rest, ctx));
            }
        };
        Ok(self.errexit(res, ctx))
    }

    fn declare(&mut self, name: &str, rest: &[models::Arg], state: State) -> Res {
        let mut state = state;
        let local =
            matches!(name, "local" | "typeset" | "declare" | "integer") && !self.calls.is_empty();
        for a in rest {
            let Some(text) = single_exact(a).or_else(|| match &a.val {
                AbsVal::Prefix(p) => Some(p.clone()),
                _ => None,
            }) else {
                state.havoc();
                continue;
            };
            if text.starts_with('-') || text.starts_with('+') {
                continue;
            }
            let (var, val) = match text.split_once('=') {
                Some((v, _)) => {
                    // The value half of `local x=VALUE` arrived expanded as
                    // part of the same word.
                    let value = match &a.val {
                        AbsVal::Values(vals) => AbsVal::Values(
                            vals.iter()
                                .filter_map(|w| w.split_once('=').map(|(_, r)| r.to_string()))
                                .collect(),
                        ),
                        _ => AbsVal::Top,
                    };
                    (v.to_string(), Some(value))
                }
                None => (text.clone(), None),
            };
            if local {
                let before = state.vars.get(&var).cloned();
                if let Some(frame) = self.locals.last_mut() {
                    frame.push((var.clone(), before));
                }
                self.assign(&mut state, &var, val.unwrap_or_else(AbsVal::empty));
            } else if let Some(v) = val {
                self.assign(&mut state, &var, v);
            }
        }
        Res::normal(state, Status::Zero)
    }

    fn set_builtin(&mut self, rest: &[models::Arg], state: State) -> Res {
        let mut state = state;
        let words: Vec<Option<String>> = rest.iter().map(single_exact).collect();
        let mut i = 0;
        while i < words.len() {
            let Some(w) = &words[i] else {
                state.havoc();
                return Res::normal(state, Status::Unknown);
            };
            if w == "--" {
                state.positional = Some(rest[i + 1..].iter().map(|a| a.val.clone()).collect());
                if rest[i + 1..].iter().any(|a| a.fields != Count::ONE) {
                    state.positional = None;
                }
                break;
            }
            let (on, flags) = match w.strip_prefix('-') {
                Some(f) => (true, f),
                None => match w.strip_prefix('+') {
                    Some(f) => (false, f),
                    None => {
                        state.positional = Some(rest[i..].iter().map(|a| a.val.clone()).collect());
                        break;
                    }
                },
            };
            let t = if on { Tri::Yes } else { Tri::No };
            for c in flags.chars() {
                match c {
                    'e' => state.opts.errexit = t,
                    'o' => {
                        i += 1;
                        match words.get(i).cloned().flatten().as_deref() {
                            Some("errexit") => state.opts.errexit = t,
                            Some("pipefail") => state.opts.pipefail = t,
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        Res::normal(state, Status::Zero)
    }

    fn wrapper(
        &mut self,
        name: &str,
        s: &SimpleCmd,
        rest: &[models::Arg],
        state: State,
        ctx: Ctx,
    ) -> R<Res> {
        // Skip the wrapper's own options and arguments to reach the command.
        let mut i = 0;
        while i < rest.len() {
            let Some(w) = single_exact(&rest[i]) else {
                break;
            };
            let takes_value = match name {
                "timeout" => i == 0 && !w.starts_with('-'),
                "nice" => w == "-n",
                "sudo" | "doas" => matches!(w.as_str(), "-u" | "-g" | "-C" | "-D" | "-h" | "-p"),
                "env" => w.contains('=') || matches!(w.as_str(), "-u" | "-C" | "-S"),
                "ionice" => matches!(w.as_str(), "-c" | "-n" | "-p"),
                _ => false,
            };
            if w.starts_with('-') || takes_value {
                if name == "env" && w == "-S" {
                    return Ok(self.external_unknown(state, s, name, ctx));
                }
                i += 1;
                if takes_value && w.starts_with('-') && !w.contains('=') {
                    i += 1;
                }
                continue;
            }
            break;
        }
        if i >= rest.len() {
            // `exec` with only redirections, `time` alone, …
            return Ok(Res::normal(state, Status::Zero));
        }
        let Some(program) = single_exact(&rest[i]) else {
            return Ok(self.unsupported(state, &s.span, "a command name built at run time"));
        };
        let args = &rest[i + 1..];
        let mut res = if matches!(name, "command" | "builtin" | "exec" | "time") {
            // These can still reach builtins; only the program lookup
            // skips functions (`command`) — close enough to dispatch again,
            // minus functions.
            if matches!(program.as_str(), "eval" | "source" | ".") {
                self.unsupported(state, &s.span, "a builtin that can change the shell")
            } else if let Some(body) = state
                .funcs
                .get(&program)
                .filter(|_| name == "time")
                .cloned()
            {
                self.call(&program, &body, args, s, state, ctx)?
            } else {
                self.external(state, s, &program, args, ctx)
            }
        } else {
            // sudo, env, nice, … run an external program, never a function.
            self.external(state, s, &program, args, ctx)
        };
        if name == "exec" {
            res.outs = res
                .outs
                .into_iter()
                .map(|o| Outcome {
                    flow: if o.flow == Flow::Normal {
                        Flow::Exit
                    } else {
                        o.flow
                    },
                    ..o
                })
                .collect();
        }
        Ok(res)
    }

    /// An external program: it cannot change this shell's state. A modelled
    /// network client contributes its transfers.
    fn external(
        &mut self,
        state: State,
        s: &SimpleCmd,
        program: &str,
        args: &[models::Arg],
        ctx: Ctx,
    ) -> Res {
        let base = program.rsplit('/').next().unwrap_or(program);
        let mut effects = Effects::default();
        if let Some(transfers) = models::model(base, args, s.span.clone()) {
            for t in transfers {
                effects.contributions.push(Contribution {
                    program: t.program,
                    transfers: t.per_call,
                    targets: t.targets,
                    auth: t.auth,
                    factors: t.builtin,
                    uncounted: t.uncounted,
                    span: t.span,
                    paced: None,
                });
            }
        } else if state.havocked {
            // After an `eval` or `source`, an unknown name may be a function
            // they defined.
            effects.unknown.push((
                s.span.clone(),
                "a command that may be a function defined by unmodelled code",
            ));
        }
        let res = Res::new(
            vec![Outcome {
                flow: Flow::Normal,
                status: Status::Unknown,
                state,
                slept: 0,
            }],
            effects,
        );
        self.errexit(res, ctx)
    }

    /// A program that runs other programs the analysis cannot see.
    fn external_unknown(&mut self, state: State, s: &SimpleCmd, name: &str, ctx: Ctx) -> Res {
        let why = match name {
            "xargs" | "parallel" => "a command run once per input line",
            "ssh" => "a command run on another machine",
            "kubectl" | "docker-compose" => "a command run elsewhere",
            _ => "a command run by another program",
        };
        let res = Res::new(
            vec![Outcome {
                flow: Flow::Normal,
                status: Status::Unknown,
                state,
                slept: 0,
            }],
            Effects {
                contributions: Vec::new(),
                unknown: vec![(s.span.clone(), why)],
            },
        );
        self.errexit(res, ctx)
    }

    /// A call to a function defined earlier in the command, analysed here,
    /// in the caller's state: bash and zsh scope dynamically, so what the
    /// function writes is visible to the caller unless it is `local`.
    fn call(
        &mut self,
        name: &str,
        body: &Rc<Cmd>,
        args: &[models::Arg],
        s: &SimpleCmd,
        state: State,
        ctx: Ctx,
    ) -> R<Res> {
        if self.calls.iter().any(|c| c == name) {
            return Ok(self.unsupported(state, &s.span, Exhausted::Recursion.describe()));
        }
        if self.calls.len() >= limits::MAX_CALL_DEPTH {
            return Ok(self.unsupported(state, &s.span, Exhausted::CallDepth.describe()));
        }
        let mut inner = state.clone();
        let saved_positional = state.positional.clone();
        inner.positional = if args.iter().all(|a| a.fields == Count::ONE) {
            Some(args.iter().map(|a| a.val.clone()).collect())
        } else {
            None
        };
        let cached = self
            .summaries
            .iter()
            .find(|m| m.name == name && m.calls == self.calls && m.entry == inner)
            .map(|m| (m.res.clone(), m.frame.clone()));
        let (r, frame) = match cached {
            Some(hit) => hit,
            None => {
                let entry = inner.clone();
                self.calls.push(name.to_string());
                self.locals.push(Vec::new());
                let r = self.exec(body, inner, Ctx::TOP);
                let frame = self.locals.pop().unwrap_or_default();
                self.calls.pop();
                let r = r?;
                if self.summaries.len() < limits::MAX_SUMMARIES {
                    self.summaries.push(Summary {
                        name: name.to_string(),
                        calls: self.calls.clone(),
                        entry,
                        res: r.clone(),
                        frame: frame.clone(),
                    });
                }
                (r, frame)
            }
        };
        let outs = r
            .outs
            .into_iter()
            .map(|mut o| {
                // Restore what `local` shadowed, and the caller's `$1…`.
                for (var, before) in frame.iter().rev() {
                    match before {
                        Some(v) => o.state.set(var, v.clone()),
                        None => {
                            o.state.vars.remove(var);
                        }
                    }
                }
                o.state.positional = saved_positional.clone();
                o.flow = match o.flow {
                    Flow::Return | Flow::Normal | Flow::Break(_) | Flow::Continue(_) => {
                        Flow::Normal
                    }
                    other => other,
                };
                o
            })
            .collect();
        let effects = r.effects.scaled(
            Count::ONE,
            Some(Factor {
                kind: FactorKind::Call {
                    function: name.to_string(),
                },
                count: Count::ONE,
                span: s.span.clone(),
            }),
        );
        Ok(self.errexit(Res::new(outs, effects), ctx))
    }

    fn assign(&self, state: &mut State, name: &str, value: AbsVal) {
        if state.vars.len() >= limits::MAX_VARS && !state.vars.contains_key(name) {
            return;
        }
        state.set(name, value);
    }

    // ── expansion ─────────────────────────────────────────────────────────

    /// Expand a word: its value, how many words it becomes, and the effects
    /// of any substitution in it (which run now, once).
    fn expand(
        &mut self,
        word: &Word,
        state: &mut State,
        effects: &mut Effects,
        split: bool,
    ) -> R<Expanded> {
        let mut val = AbsVal::empty();
        let mut fields = Count::ONE;
        let mut flags = WordFlags::default();
        let mut url_like = false;
        for part in &word.parts {
            let (v, f) = self.part(part, state, effects, &mut flags)?;
            val = val.concat(&v);
            fields = fields.mul(f);
        }
        if let Some(t) = val.as_exact() {
            url_like = t.contains("://");
        }
        if split && flags.subst_split {
            // Both shells split what a substitution printed.
            fields = fields.mul(split_count(&val, Dialect::Bash));
        } else if split && flags.param_split {
            fields = fields.mul(split_count(&val, state.dialect));
        }
        if split && flags.glob {
            // An unquoted glob in a URL-shaped word never matches a file: bash
            // passes it through, zsh refuses the command (NOMATCH). Anywhere
            // else it may become any number of words.
            fields = if url_like {
                match state.dialect {
                    Dialect::Bash => fields,
                    Dialect::Zsh => Count::ZERO,
                    Dialect::Unknown => fields.join(Count::ZERO),
                }
            } else {
                Count::UNKNOWN
            };
        }
        Ok(Expanded {
            val,
            fields,
            subst: flags.subst,
        })
    }

    fn part(
        &mut self,
        part: &Part,
        state: &mut State,
        effects: &mut Effects,
        flags: &mut WordFlags,
    ) -> R<(AbsVal, Count)> {
        Ok(match part {
            Part::Lit(s) | Part::SingleQuoted(s) => (AbsVal::exact(s.clone()), Count::ONE),
            Part::Escaped(c) => (AbsVal::exact(c.to_string()), Count::ONE),
            Part::DoubleQuoted(inner) => {
                // `"$@"` is the one quoted form that is several words.
                if let [Part::Param(ParamRef::All)] = inner.as_slice() {
                    return Ok(match &state.positional {
                        Some(p) => (
                            p.iter()
                                .cloned()
                                .reduce(|a, b| a.join(&b))
                                .unwrap_or(AbsVal::empty()),
                            Count::exactly(p.len() as u64),
                        ),
                        None => (AbsVal::Top, Count::UNKNOWN),
                    });
                }
                // Nothing inside quotes splits or globs; only the fact that
                // a substitution ran escapes them.
                let mut v = AbsVal::empty();
                let mut quoted = WordFlags::default();
                for q in inner {
                    let (x, _) = self.part(q, state, effects, &mut quoted)?;
                    v = v.concat(&x);
                }
                flags.subst |= quoted.subst;
                (v, Count::ONE)
            }
            Part::Param(p) => {
                let v = self.param(p, state);
                // A value known to be one word splits into one word.
                if v != AbsVal::Token {
                    flags.param_split = true;
                }
                (v, Count::ONE)
            }
            Part::ParamOp { .. } => {
                flags.param_split = true;
                (AbsVal::Top, Count::ONE)
            }
            Part::CmdSubst { body, span } => {
                flags.subst = true;
                let r = self.exec(body, state.clone(), Ctx::TOP)?;
                *effects = std::mem::take(effects).then(r.effects.scaled(
                    Count::ONE,
                    Some(Factor {
                        kind: FactorKind::Evaluation,
                        count: Count::ONE,
                        span: span.clone(),
                    }),
                ));
                if let Some(n) = seq_len(body) {
                    // `$(seq 1 300)` is 300 words, counted and never produced.
                    (AbsVal::Token, Count::exactly(n))
                } else if prints_one_word(body) {
                    (AbsVal::Token, Count::ONE)
                } else {
                    flags.subst_split = true;
                    (AbsVal::Top, Count::ONE)
                }
            }
            Part::ProcSubst { body, span } => {
                let r = self.exec(body, state.clone(), Ctx::TOP)?;
                *effects = std::mem::take(effects).then(r.effects.scaled(
                    Count::ONE,
                    Some(Factor {
                        kind: FactorKind::Evaluation,
                        count: Count::ONE,
                        span: span.clone(),
                    }),
                ));
                (AbsVal::Top, Count::ONE)
            }
            Part::Arith(e) => {
                flags.param_split = true;
                let v = eval_arith(e, state);
                (
                    v.map(|n| AbsVal::exact(n.to_string()))
                        .unwrap_or(AbsVal::Top),
                    Count::ONE,
                )
            }
            Part::Brace(b) => match b {
                BraceExpr::IntRange {
                    from,
                    to,
                    step,
                    width,
                } => {
                    let n = range_len(*from, *to, *step);
                    let val = if n <= 4 {
                        let st = step.unsigned_abs().max(1) as i64;
                        let dir = if to >= from { st } else { -st };
                        let mut vals = std::collections::BTreeSet::new();
                        let mut x = *from;
                        for _ in 0..n {
                            vals.insert(format!("{x:0width$}", width = *width));
                            x += dir;
                        }
                        AbsVal::Values(vals)
                    } else {
                        AbsVal::Token
                    };
                    (val, Count::exactly(n))
                }
                BraceExpr::CharRange { from, to, step } => {
                    let n = range_len(*from as i64, *to as i64, *step);
                    (AbsVal::Token, Count::exactly(n))
                }
                BraceExpr::List(words) => {
                    let mut total = Count::ZERO;
                    let mut val: Option<AbsVal> = None;
                    for w in words {
                        let e = self.expand(w, state, effects, false)?;
                        total = total.add(e.fields);
                        val = Some(match val {
                            Some(v) => v.join(&e.val),
                            None => e.val,
                        });
                    }
                    (val.unwrap_or(AbsVal::Top), total)
                }
            },
            Part::Glob(c) => {
                flags.glob = true;
                (AbsVal::exact(c.to_string()), Count::ONE)
            }
            Part::Tilde(_) => (AbsVal::Top, Count::ONE),
        })
    }

    fn param(&self, p: &ParamRef, state: &State) -> AbsVal {
        match p {
            ParamRef::Named(n) => state.get(n),
            ParamRef::Positional(0) => AbsVal::Top,
            ParamRef::Positional(i) => match &state.positional {
                Some(ps) => ps.get(*i as usize - 1).cloned().unwrap_or(AbsVal::empty()),
                None => AbsVal::Top,
            },
            ParamRef::All => match &state.positional {
                Some(ps) if ps.is_empty() => AbsVal::empty(),
                _ => AbsVal::Top,
            },
            ParamRef::Special('#') => match &state.positional {
                Some(ps) => AbsVal::exact(ps.len().to_string()),
                None => AbsVal::Top,
            },
            ParamRef::Special(_) => AbsVal::Top,
        }
    }

    // ── arithmetic and tests ──────────────────────────────────────────────

    fn test_expr(&mut self, e: &TestExpr, state: &mut State, effects: &mut Effects) -> R<Tri> {
        Ok(match e {
            TestExpr::Unary { op, operand } => {
                let v = self.expand(operand, state, effects, false)?.val;
                match op.trim_start_matches('-') {
                    "n" => not(v.emptiness()),
                    "z" => v.emptiness(),
                    _ => Tri::Maybe,
                }
            }
            TestExpr::Binary { left, op, right } => {
                let l = self.expand(left, state, effects, false)?.val;
                let r = self.expand(right, state, effects, false)?.val;
                binary(&l, op, &r)
            }
            TestExpr::Word(w) => not(self.expand(w, state, effects, false)?.val.emptiness()),
            TestExpr::Not(x) => not(self.test_expr(x, state, effects)?),
            TestExpr::And(a, b) => {
                let x = self.test_expr(a, state, effects)?;
                if x == Tri::No {
                    Tri::No
                } else {
                    and(x, self.test_expr(b, state, effects)?)
                }
            }
            TestExpr::Or(a, b) => {
                let x = self.test_expr(a, state, effects)?;
                if x == Tri::Yes {
                    Tri::Yes
                } else {
                    or(x, self.test_expr(b, state, effects)?)
                }
            }
            TestExpr::Unknown => Tri::Maybe,
        })
    }
}

struct Summary {
    name: String,
    calls: Vec<String>,
    entry: State,
    res: Res,
    frame: Vec<(String, Option<AbsVal>)>,
}

/// What the parts of one word may do to it once it is expanded.
#[derive(Default)]
struct WordFlags {
    /// An unquoted parameter or arithmetic result: bash splits it, zsh not.
    param_split: bool,
    /// An unquoted command substitution: both shells split it.
    subst_split: bool,
    /// A command substitution ran, quoted or not.
    subst: bool,
    /// An unquoted glob character.
    glob: bool,
}

struct Expanded {
    val: AbsVal,
    fields: Count,
    /// It contained a command substitution, whose status becomes the
    /// command's when the command is only assignments.
    subst: bool,
}

#[derive(Debug, Clone)]
struct CounterBound {
    max: u64,
    desc: String,
    /// The comparison is the whole condition, not one conjunct of several.
    alone: bool,
}

/// Split endings into the states where the command reached `status` and
/// where it did not (Unknown goes both ways).
fn split_status(outs: &[Outcome], yes: Status) -> (Option<State>, Option<State>) {
    let mut y = Vec::new();
    let mut n = Vec::new();
    for o in outs.iter().filter(|o| o.flow == Flow::Normal) {
        if o.status == yes || o.status == Status::Unknown {
            y.push(o.clone());
        }
        if o.status != yes {
            n.push(o.clone());
        }
    }
    (join_states(y.iter()), join_states(n.iter()))
}

/// The states that go round the loop again (normal end or `continue`), and
/// the endings that leave it (`break` as normal, deeper ones one level up).
fn back_edge(outs: &[Outcome]) -> (Option<State>, Vec<Outcome>) {
    let mut back = Vec::new();
    let mut ended = Vec::new();
    for o in outs {
        match o.flow {
            Flow::Normal | Flow::Continue(1) => back.push(o.clone()),
            Flow::Break(1) => ended.push(Outcome {
                flow: Flow::Normal,
                ..o.clone()
            }),
            Flow::Break(n) => ended.push(Outcome {
                flow: Flow::Break(n - 1),
                ..o.clone()
            }),
            Flow::Continue(n) => ended.push(Outcome {
                flow: Flow::Continue(n - 1),
                ..o.clone()
            }),
            Flow::Return | Flow::Exit | Flow::NonTerm => ended.push(o.clone()),
        }
    }
    (join_states(back.iter()), ended)
}

/// The least a loop body slept on the paths that go round again — its end,
/// or a `continue` of this loop. `u64::MAX` when no path goes round: nothing
/// repeats, so nothing is unpaced.
fn back_edge_slept(outs: &[Outcome]) -> u64 {
    outs.iter()
        .filter(|o| matches!(o.flow, Flow::Normal | Flow::Continue(1)))
        .map(|o| o.slept)
        .min()
        .unwrap_or(u64::MAX)
}

/// Can the body leave the loop early (`break`, `return`, `exit`)?
fn has_early_exit(outs: &[Outcome]) -> bool {
    outs.iter()
        .any(|o| matches!(o.flow, Flow::Break(_) | Flow::Return | Flow::Exit))
}

/// Values that changed between rounds become unknown.
fn widen(old: &State, new: &State) -> State {
    let mut s = new.clone();
    for (k, v) in &new.vars {
        if old.vars.get(k) != Some(v) {
            s.vars.insert(k.clone(), AbsVal::Top);
        }
    }
    if old.positional != new.positional {
        s.positional = None;
    }
    s
}

fn join_opt(a: Option<State>, b: Option<State>) -> Option<State> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.join(&y)),
        (x, None) => x,
        (None, y) => y,
    }
}

fn pipe_status(statuses: &[Status], pipefail: Tri) -> Status {
    let last = statuses.last().copied().unwrap_or(Status::Zero);
    match pipefail {
        Tri::No => last,
        Tri::Yes => {
            if statuses.contains(&Status::NonZero) {
                Status::NonZero
            } else if statuses.iter().all(|s| *s == Status::Zero) {
                Status::Zero
            } else {
                Status::Unknown
            }
        }
        Tri::Maybe => {
            let with = pipe_status(statuses, Tri::Yes);
            if with == last {
                last
            } else {
                Status::Unknown
            }
        }
    }
}

fn single_exact(a: &models::Arg) -> Option<String> {
    (a.fields == Count::ONE)
        .then(|| a.val.as_exact().map(str::to_string))
        .flatten()
}

fn not(t: Tri) -> Tri {
    match t {
        Tri::Yes => Tri::No,
        Tri::No => Tri::Yes,
        Tri::Maybe => Tri::Maybe,
    }
}

fn and(a: Tri, b: Tri) -> Tri {
    match (a, b) {
        (Tri::No, _) | (_, Tri::No) => Tri::No,
        (Tri::Yes, Tri::Yes) => Tri::Yes,
        _ => Tri::Maybe,
    }
}

fn or(a: Tri, b: Tri) -> Tri {
    match (a, b) {
        (Tri::Yes, _) | (_, Tri::Yes) => Tri::Yes,
        (Tri::No, Tri::No) => Tri::No,
        _ => Tri::Maybe,
    }
}

/// `[`/`test` over expanded arguments.
fn test_argv(argv: &[&models::Arg]) -> Tri {
    if argv.iter().any(|a| a.fields != Count::ONE) {
        return Tri::Maybe;
    }
    let text = |a: &models::Arg| a.val.as_exact().map(str::to_string);
    match argv {
        [] => Tri::No,
        [x] => not(x.val.emptiness()),
        [op, x] => match text(op).as_deref() {
            Some("!") => x.val.emptiness(),
            Some("-n") => not(x.val.emptiness()),
            Some("-z") => x.val.emptiness(),
            _ => Tri::Maybe,
        },
        [l, op, r] => match text(op) {
            Some(op) => binary(&l.val, &op, &r.val),
            None => Tri::Maybe,
        },
        [bang, rest @ ..] if text(bang).as_deref() == Some("!") => not(test_argv(rest)),
        _ => Tri::Maybe,
    }
}

fn binary(l: &AbsVal, op: &str, r: &AbsVal) -> Tri {
    let ints = || Some((l.as_int()?, r.as_int()?));
    let cmp = |f: fn(i64, i64) -> bool| match ints() {
        Some((a, b)) => {
            if f(a, b) {
                Tri::Yes
            } else {
                Tri::No
            }
        }
        None => Tri::Maybe,
    };
    match op {
        "-lt" => cmp(|a, b| a < b),
        "-le" => cmp(|a, b| a <= b),
        "-gt" => cmp(|a, b| a > b),
        "-ge" => cmp(|a, b| a >= b),
        "-eq" => cmp(|a, b| a == b),
        "-ne" => cmp(|a, b| a != b),
        "=" | "==" | "!=" => {
            let eq = match (l.as_exact(), r.as_exact()) {
                (Some(a), Some(b)) => {
                    if a == b {
                        Tri::Yes
                    } else {
                        Tri::No
                    }
                }
                _ => Tri::Maybe,
            };
            if op == "!=" {
                not(eq)
            } else {
                eq
            }
        }
        _ => Tri::Maybe,
    }
}

/// Does a `case` subject match a pattern? Only literal patterns and a
/// trailing or leading `*` are decided; anything else is Maybe.
fn pattern_match(subject: &AbsVal, pattern: &AbsVal) -> Tri {
    let Some(p) = pattern.as_exact() else {
        return Tri::Maybe;
    };
    if p == "*" {
        return Tri::Yes;
    }
    let plain = |x: &&str| !x.contains(['*', '?', '[']);
    let decide = |s: &str| -> Option<bool> {
        if plain(&p) {
            Some(s == p)
        } else if let Some(pre) = p.strip_suffix('*').filter(plain) {
            Some(s.starts_with(pre))
        } else {
            p.strip_prefix('*')
                .filter(plain)
                .map(|suf| s.ends_with(suf))
        }
    };
    match subject {
        AbsVal::Values(vs) => {
            let mut hit = None;
            for s in vs {
                let d = match decide(s) {
                    Some(true) => Tri::Yes,
                    Some(false) => Tri::No,
                    None => return Tri::Maybe,
                };
                hit = Some(match hit {
                    Some(h) if h == d => h,
                    Some(_) => Tri::Maybe,
                    None => d,
                });
            }
            hit.unwrap_or(Tri::Maybe)
        }
        AbsVal::Prefix(pre) => match p.strip_suffix('*') {
            Some(pp) if !pp.contains(['*', '?', '[']) && pre.starts_with(pp) => Tri::Yes,
            Some(pp)
                if !pp.contains(['*', '?', '['])
                    && !pp.starts_with(pre.as_str())
                    && !pre.starts_with(pp) =>
            {
                Tri::No
            }
            _ => Tri::Maybe,
        },
        AbsVal::Token | AbsVal::Top => Tri::Maybe,
    }
}

/// How many words `val` splits into, unquoted.
fn split_count(val: &AbsVal, dialect: Dialect) -> Count {
    let bash = match val {
        AbsVal::Values(vs) => vs
            .iter()
            .map(|s| Count::exactly(s.split_whitespace().count() as u64))
            .reduce(Count::join)
            .unwrap_or(Count::UNKNOWN),
        AbsVal::Token => Count::ONE,
        _ => Count::UNKNOWN,
    };
    // zsh does not split parameters; an empty unquoted one disappears.
    let zsh = match val.emptiness() {
        Tri::Yes => Count::ZERO,
        Tri::No => Count::ONE,
        Tri::Maybe => Count::MAYBE,
    };
    match dialect {
        Dialect::Bash => bash,
        Dialect::Zsh => zsh,
        Dialect::Unknown => bash.join(zsh),
    }
}

fn range_len(from: i64, to: i64, step: i64) -> u64 {
    let st = step.unsigned_abs().max(1);
    (from.abs_diff(to) / st).saturating_add(1)
}

/// `sleep`'s operand in whole seconds, rounded down: `30`, `1.5`, `2m`.
fn seconds(t: &str) -> Option<u64> {
    let (num, unit) = match t.char_indices().last()? {
        (i, c @ ('s' | 'm' | 'h' | 'd')) => (&t[..i], c),
        _ => (t, 's'),
    };
    let whole: u64 = num.split('.').next()?.parse().ok()?;
    let per = match unit {
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => 1,
    };
    whole.checked_mul(per)
}

/// Mark the transfers a loop repeats as paced by `pause` seconds, when every
/// path through its body sleeps at least that long, recording how many
/// transfers each iteration makes between sleeps. The innermost paced loop
/// wins: an outer loop's slower pace does not hide an inner poll's burst.
fn pace(effects: &mut Effects, pause: u64) {
    // `u64::MAX`: no path goes round again, so nothing here repeats.
    if pause == 0 || pause == u64::MAX {
        return;
    }
    // One call site can be several contributions (an inner loop's first
    // iteration and the rest): the burst is what the site makes per
    // iteration, all of them together.
    let mut per_site: Vec<(Span, Count)> = Vec::new();
    for c in &effects.contributions {
        match per_site.iter_mut().find(|(s, _)| *s == c.span) {
            Some((_, n)) => *n = n.add(c.transfers),
            None => per_site.push((c.span.clone(), c.transfers)),
        }
    }
    for c in &mut effects.contributions {
        if c.paced.is_none() {
            let burst = per_site
                .iter()
                .find(|(s, _)| *s == c.span)
                .map_or(c.transfers, |(_, n)| *n);
            c.paced = Some(Pace { secs: pause, burst });
        }
    }
}

/// `seq LAST`, `seq FIRST LAST`, `seq FIRST INCR LAST` with literal integers:
/// how many numbers it prints. Options (`-w`, `-f`, `-s`) are not read.
fn seq_len(body: &Cmd) -> Option<u64> {
    let Cmd::Simple(s) = body else { return None };
    let words: Vec<String> = s.words.iter().map(Word::literal).collect::<Option<_>>()?;
    let (name, args) = words.split_first()?;
    if name != "seq" || !s.assigns.is_empty() {
        return None;
    }
    let n: Vec<i64> = args.iter().map(|a| a.parse().ok()).collect::<Option<_>>()?;
    let (first, incr, last) = match n.as_slice() {
        [last] => (1, 1, *last),
        [first, last] => (*first, 1, *last),
        [first, incr, last] => (*first, *incr, *last),
        _ => return None,
    };
    if incr == 0 {
        return None;
    }
    if (incr > 0 && last < first) || (incr < 0 && last > first) {
        return Some(0);
    }
    Some(first.abs_diff(last) / incr.unsigned_abs() + 1)
}

/// A substitution whose output is always one non-empty word without
/// whitespace: `$(mktemp)`, `$(pwd)`. What makes `curl -D $(mktemp) "$url"`
/// readable at all — otherwise an unquoted value there might be zero words or
/// three, and every operand after it would be anybody's guess.
fn prints_one_word(body: &Cmd) -> bool {
    let Cmd::Simple(s) = body else { return false };
    let name = s.words.first().and_then(Word::literal);
    matches!(
        name.as_deref(),
        Some("mktemp" | "pwd" | "whoami" | "hostname" | "uname" | "id" | "nproc")
    ) && s.words.iter().all(|w| w.literal().is_some())
}

/// `v++`, `v+=1`, `++v` — the forms of `let` the counter pattern reads.
fn parse_let(t: &str) -> Option<(String, i64)> {
    let t = t.replace(' ', "");
    let name_ok =
        |n: &str| !n.is_empty() && n.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
    if let Some(v) = t.strip_suffix("++").or_else(|| t.strip_prefix("++")) {
        return name_ok(v).then(|| (v.to_string(), 1));
    }
    if let Some(v) = t.strip_suffix("--").or_else(|| t.strip_prefix("--")) {
        return name_ok(v).then(|| (v.to_string(), -1));
    }
    if let Some((v, n)) = t.split_once("+=") {
        return (name_ok(v))
            .then(|| n.parse().ok().map(|n| (v.to_string(), n)))
            .flatten();
    }
    None
}

fn eval_arith(e: &ArithExpr, state: &mut State) -> Option<i64> {
    match e {
        ArithExpr::Num(n) => Some(*n),
        ArithExpr::Var(v) => match state.get(v) {
            // An unset or empty variable is 0 in arithmetic.
            x if x.emptiness() == Tri::Yes => Some(0),
            x => x.as_int(),
        },
        ArithExpr::Unary(op, x) => {
            let v = eval_arith(x, state)?;
            Some(match op {
                ArithUnOp::Neg => v.checked_neg()?,
                ArithUnOp::Not => (v == 0) as i64,
                ArithUnOp::BitNot => !v,
                ArithUnOp::Plus => v,
            })
        }
        ArithExpr::Binary(op, a, b) => {
            let x = eval_arith(a, state);
            let y = eval_arith(b, state);
            binop(*op, x?, y?)
        }
        ArithExpr::Assign { var, op, value } => {
            let rhs = eval_arith(value, state);
            let v = match op {
                None => rhs,
                Some(o) => {
                    let cur = eval_arith(&ArithExpr::Var(var.clone()), state);
                    match (cur, rhs) {
                        (Some(c), Some(r)) => binop(*o, c, r),
                        _ => None,
                    }
                }
            };
            state.set(
                var,
                v.map(|n| AbsVal::exact(n.to_string()))
                    .unwrap_or(AbsVal::Top),
            );
            v
        }
        ArithExpr::IncDec { var, delta, prefix } => {
            let cur = eval_arith(&ArithExpr::Var(var.clone()), state);
            let next = cur.and_then(|c| c.checked_add(*delta));
            state.set(
                var,
                next.map(|n| AbsVal::exact(n.to_string()))
                    .unwrap_or(AbsVal::Top),
            );
            if *prefix {
                next
            } else {
                cur
            }
        }
        ArithExpr::Comma(a, b) => {
            eval_arith(a, state);
            eval_arith(b, state)
        }
        ArithExpr::Ternary(c, a, b) => match eval_arith(c, state) {
            Some(0) => eval_arith(b, state),
            Some(_) => eval_arith(a, state),
            None => {
                // Either branch may write; make what they write unknown.
                for v in arith_writes(a).into_iter().chain(arith_writes(b)) {
                    state.set(&v, AbsVal::Top);
                }
                None
            }
        },
        ArithExpr::Unknown(_) => None,
    }
}

fn binop(op: ArithBinOp, a: i64, b: i64) -> Option<i64> {
    use ArithBinOp::*;
    Some(match op {
        Add => a.checked_add(b)?,
        Sub => a.checked_sub(b)?,
        Mul => a.checked_mul(b)?,
        Div => a.checked_div(b)?,
        Rem => a.checked_rem(b)?,
        Lt => (a < b) as i64,
        Le => (a <= b) as i64,
        Gt => (a > b) as i64,
        Ge => (a >= b) as i64,
        Eq => (a == b) as i64,
        Ne => (a != b) as i64,
        And => (a != 0 && b != 0) as i64,
        Or => (a != 0 || b != 0) as i64,
        BitAnd => a & b,
        BitOr => a | b,
        BitXor => a ^ b,
        Shl => a.checked_shl(u32::try_from(b).ok()?)?,
        Shr => a.checked_shr(u32::try_from(b).ok()?)?,
        Pow => a.checked_pow(u32::try_from(b).ok()?)?,
    })
}

fn arith_writes(e: &ArithExpr) -> Vec<String> {
    match e {
        ArithExpr::Assign { var, value, .. } => {
            let mut v = vec![var.clone()];
            v.extend(arith_writes(value));
            v
        }
        ArithExpr::IncDec { var, .. } => vec![var.clone()],
        ArithExpr::Unary(_, x) => arith_writes(x),
        ArithExpr::Binary(_, a, b) | ArithExpr::Comma(a, b) => {
            let mut v = arith_writes(a);
            v.extend(arith_writes(b));
            v
        }
        ArithExpr::Ternary(c, a, b) => {
            let mut v = arith_writes(c);
            v.extend(arith_writes(a));
            v.extend(arith_writes(b));
            v
        }
        ArithExpr::Num(_) | ArithExpr::Var(_) | ArithExpr::Unknown(_) => Vec::new(),
    }
}

/// `for ((…; i < N; i++))` with nothing else writing `i`.
fn arith_counter(cond: &ArithExpr, step: &ArithExpr, state: &State, body: &Cmd) -> Option<u64> {
    let (var, inclusive, limit) = match cond {
        ArithExpr::Binary(op @ (ArithBinOp::Lt | ArithBinOp::Le), a, b) => match (&**a, &**b) {
            (ArithExpr::Var(v), ArithExpr::Num(n)) => (v.clone(), *op == ArithBinOp::Le, *n),
            _ => return None,
        },
        _ => return None,
    };
    let steps_by_one = match step {
        ArithExpr::IncDec {
            var: v, delta: 1, ..
        } => *v == var,
        ArithExpr::Assign {
            var: v,
            op: Some(ArithBinOp::Add),
            value,
        } => *v == var && **value == ArithExpr::Num(1),
        _ => false,
    };
    if !steps_by_one {
        return None;
    }
    let start = state.get(&var).as_int()?;
    if writes_var(body, &var, state, &mut Vec::new()) || has_continue(body, 0) {
        return None;
    }
    let n = limit.saturating_sub(start);
    let n = if inclusive { n.saturating_add(1) } else { n };
    u64::try_from(n.max(0)).ok()
}

/// The commands a condition's final status comes from, split on `op` at the
/// top level of its last command (`[ a ] && [ b ]` → both).
fn conjuncts(cond: &Cmd, op: AndOrOp) -> Vec<&Cmd> {
    let last = match cond {
        Cmd::Seq { items, .. } => match items.last() {
            Some(i) => &i.cmd,
            None => return Vec::new(),
        },
        other => other,
    };
    match last {
        Cmd::AndOr { first, rest, .. } if rest.iter().all(|(o, _)| *o == op) => {
            let mut v = vec![&**first];
            v.extend(rest.iter().map(|(_, c)| c));
            v
        }
        Cmd::AndOr { .. } => Vec::new(),
        other => vec![other],
    }
}

fn last_simple(cond: &Cmd) -> Option<&SimpleCmd> {
    match cond {
        Cmd::Simple(s) => Some(s),
        Cmd::Seq { items, .. } => items.last().and_then(|i| last_simple(&i.cmd)),
        Cmd::Pipeline {
            negated: false,
            cmds,
            ..
        } if cmds.len() == 1 => last_simple(&cmds[0]),
        _ => None,
    }
}

/// `[ $v OP N ]`, `[ N OP $v ]` (mirrored), `test …`, `[[ $v OP N ]]`,
/// `(( v OP N ))`: the counter variable, the operator as `-lt`/`-le`/…
/// read from the variable's side, and the literal limit.
fn comparison(cmd: &Cmd, entry: &State) -> Option<(String, String, i64)> {
    let mirror = |op: &str| -> String {
        match op {
            "-lt" => "-gt",
            "-le" => "-ge",
            "-gt" => "-lt",
            "-ge" => "-le",
            other => other,
        }
        .to_string()
    };
    let var_of = |w: &Word| -> Option<String> {
        match w.parts.as_slice() {
            [Part::Param(ParamRef::Named(n))] => Some(n.clone()),
            [Part::DoubleQuoted(inner)] => match inner.as_slice() {
                [Part::Param(ParamRef::Named(n))] => Some(n.clone()),
                _ => None,
            },
            _ => None,
        }
    };
    let num_of = |w: &Word| -> Option<i64> {
        match w.literal() {
            Some(t) => t.trim().parse().ok(),
            None => var_of(w).and_then(|v| entry.get(&v).as_int()),
        }
    };
    let pair = |l: &Word, op: &str, r: &Word| -> Option<(String, String, i64)> {
        if let (Some(v), Some(n)) = (var_of(l), num_of(r)) {
            return Some((v, op.to_string(), n));
        }
        if let (Some(n), Some(v)) = (num_of(l), var_of(r)) {
            return Some((v, mirror(op), n));
        }
        None
    };
    match cmd {
        Cmd::Simple(s) if s.assigns.is_empty() => {
            let lits: Vec<Option<String>> = s.words.iter().map(Word::literal).collect();
            match lits.first()?.as_deref()? {
                "[" if s.words.len() == 5 && lits[4].as_deref() == Some("]") => {
                    pair(&s.words[1], lits[2].as_deref()?, &s.words[3])
                }
                "test" if s.words.len() == 4 => pair(&s.words[1], lits[2].as_deref()?, &s.words[3]),
                _ => None,
            }
        }
        Cmd::Cond {
            expr: TestExpr::Binary { left, op, right },
            ..
        } => pair(left, op, right),
        Cmd::Arith {
            expr: ArithExpr::Binary(op, a, b),
            ..
        } => {
            let op = match op {
                ArithBinOp::Lt => "-lt",
                ArithBinOp::Le => "-le",
                ArithBinOp::Gt => "-gt",
                ArithBinOp::Ge => "-ge",
                _ => return None,
            };
            match (&**a, &**b) {
                (ArithExpr::Var(v), ArithExpr::Num(n)) => Some((v.clone(), op.to_string(), *n)),
                (ArithExpr::Num(n), ArithExpr::Var(v)) => Some((v.clone(), mirror(op), *n)),
                _ => None,
            }
        }
        Cmd::Pipeline {
            negated: false,
            cmds,
            ..
        } if cmds.len() == 1 => comparison(&cmds[0], entry),
        _ => None,
    }
}

/// `[ -n "$u" ]`, `[ "$u" ]`, `[[ -n $u ]]`, `test -n "$u"`, `[ "$u" != "" ]`.
fn nonempty_test_var(cmd: &Cmd) -> Option<String> {
    let var_of = |w: &Word| -> Option<String> {
        match w.parts.as_slice() {
            [Part::Param(ParamRef::Named(n))] => Some(n.clone()),
            [Part::DoubleQuoted(inner)] => match inner.as_slice() {
                [Part::Param(ParamRef::Named(n))] => Some(n.clone()),
                _ => None,
            },
            _ => None,
        }
    };
    match cmd {
        Cmd::Simple(s) if s.assigns.is_empty() => {
            let lits: Vec<Option<String>> = s.words.iter().map(Word::literal).collect();
            let argv: &[Word] = match lits.first()?.as_deref()? {
                "[" if lits.last()?.as_deref() == Some("]") => &s.words[1..s.words.len() - 1],
                "test" => &s.words[1..],
                _ => return None,
            };
            match argv {
                [w] => var_of(w),
                [op, w] if op.literal().as_deref() == Some("-n") => var_of(w),
                [w, op, e]
                    if op.literal().as_deref() == Some("!=")
                        && e.literal().as_deref() == Some("") =>
                {
                    var_of(w)
                }
                _ => None,
            }
        }
        Cmd::Cond {
            expr: TestExpr::Unary { op, operand },
            ..
        } if op.trim_start_matches('-') == "n" => var_of(operand),
        Cmd::Cond {
            expr: TestExpr::Word(w),
            ..
        } => var_of(w),
        Cmd::Pipeline {
            negated: false,
            cmds,
            ..
        } if cmds.len() == 1 => nonempty_test_var(&cmds[0]),
        _ => None,
    }
}

/// The body's top-level sequence increments `var` by one exactly once, in a
/// statement that always runs when the iteration reaches it.
fn increments_once(body: &Cmd, var: &str) -> bool {
    let items: Vec<&Cmd> = match body {
        Cmd::Seq { items, .. } => items
            .iter()
            .filter(|i| !i.background)
            .map(|i| &i.cmd)
            .collect(),
        Cmd::Group { body, .. } => return increments_once(body, var),
        other => vec![other],
    };
    items.iter().filter(|c| is_increment(c, var)).count() == 1
}

fn is_increment(cmd: &Cmd, var: &str) -> bool {
    match cmd {
        // `v=$((v+1))`, `v=$(( 1 + v ))`
        Cmd::Simple(s) if s.words.is_empty() && s.assigns.len() == 1 => {
            let a = &s.assigns[0];
            a.name == var
                && !a.append
                && matches!(a.value.parts.as_slice(), [Part::Arith(e)] if is_plus_one(e, var))
        }
        // `((v++))`, `((++v))`, `((v+=1))`, `((v=v+1))`
        Cmd::Arith { expr, .. } => match expr {
            ArithExpr::IncDec {
                var: v, delta: 1, ..
            } => v == var,
            ArithExpr::Assign {
                var: v,
                op: Some(ArithBinOp::Add),
                value,
            } => v == var && **value == ArithExpr::Num(1),
            ArithExpr::Assign {
                var: v,
                op: None,
                value,
            } => v == var && is_plus_one(value, var),
            _ => false,
        },
        // `let v++`, `let v+=1`
        Cmd::Simple(s) if s.assigns.is_empty() && s.words.len() == 2 => {
            s.words[0].literal().as_deref() == Some("let")
                && s.words[1]
                    .literal()
                    .and_then(|t| parse_let(&t))
                    .is_some_and(|(v, d)| v == var && d == 1)
        }
        _ => false,
    }
}

fn is_plus_one(e: &ArithExpr, var: &str) -> bool {
    match e {
        ArithExpr::Binary(ArithBinOp::Add, a, b) => matches!(
            (&**a, &**b),
            (ArithExpr::Var(v), ArithExpr::Num(1)) | (ArithExpr::Num(1), ArithExpr::Var(v)) if v == var
        ),
        _ => false,
    }
}

/// Anything in the loop other than the one increment that could write
/// `var`: another assignment, `read`, `for var`, `local`, an unmodelled
/// construct, a function that writes it — or a `continue` that could skip
/// the increment.
fn writes_elsewhere(body: &Cmd, cond: &Cmd, var: &str, state: &State) -> bool {
    if has_continue(body, 0) {
        return true;
    }
    let mut seen = Vec::new();
    if writes_var(cond, var, state, &mut seen) {
        return true;
    }
    // Count writes in the body; the one increment is expected.
    let items: Vec<&Cmd> = match body {
        Cmd::Seq { items, .. } => items.iter().map(|i| &i.cmd).collect(),
        Cmd::Group { body, .. } => return writes_elsewhere(body, cond, var, state),
        other => vec![other],
    };
    let mut increments = 0;
    for c in items {
        if is_increment(c, var) && increments == 0 {
            increments += 1;
            continue;
        }
        if writes_var(c, var, state, &mut seen) {
            return true;
        }
    }
    false
}

/// A `continue` that would reach the loop at `depth` 0 from here.
fn has_continue(cmd: &Cmd, depth: u32) -> bool {
    let sub = |c: &Cmd| has_continue(c, depth);
    let deeper = |c: &Cmd| has_continue(c, depth + 1);
    match cmd {
        Cmd::Simple(s) => {
            s.words.first().and_then(Word::literal).as_deref() == Some("continue")
                && s.words
                    .get(1)
                    .and_then(Word::literal)
                    .and_then(|t| t.parse::<u32>().ok())
                    .unwrap_or(1)
                    > depth
        }
        Cmd::Seq { items, .. } => items.iter().any(|i| sub(&i.cmd)),
        Cmd::AndOr { first, rest, .. } => sub(first) || rest.iter().any(|(_, c)| sub(c)),
        Cmd::Pipeline { cmds, .. } => cmds.iter().any(sub),
        Cmd::If {
            arms, otherwise, ..
        } => arms.iter().any(|(c, b)| sub(c) || sub(b)) || otherwise.as_deref().is_some_and(sub),
        Cmd::Case { arms, .. } => arms.iter().any(|a| sub(&a.body)),
        Cmd::For { body, .. } | Cmd::ArithFor { body, .. } => deeper(body),
        Cmd::Loop { cond, body, .. } => deeper(cond) || deeper(body),
        Cmd::Group { body, .. } | Cmd::Subshell { body, .. } | Cmd::Redirected { body, .. } => {
            sub(body)
        }
        Cmd::FuncDef { .. } | Cmd::Arith { .. } | Cmd::Cond { .. } => false,
        Cmd::Unsupported { .. } => true,
    }
}

/// Could running `cmd` assign `var` — directly, through a function it calls,
/// or through something the analysis does not model?
fn writes_var(cmd: &Cmd, var: &str, state: &State, seen: &mut Vec<String>) -> bool {
    let mut w = |c: &Cmd| writes_var(c, var, state, seen);
    match cmd {
        Cmd::Simple(s) => {
            if s.assigns.iter().any(|a| a.name == var) && s.words.is_empty() {
                return true;
            }
            if word_writes(&s.words, var, state, seen)
                || s.assigns
                    .iter()
                    .any(|a| word_writes(std::slice::from_ref(&a.value), var, state, seen))
            {
                return true;
            }
            let Some(name) = s.words.first().and_then(Word::literal) else {
                return !s.words.is_empty();
            };
            let args: Vec<Option<String>> = s.words[1..].iter().map(Word::literal).collect();
            let names_var = |args: &[Option<String>]| {
                args.iter().any(|a| match a {
                    Some(t) => t == var || t.split_once('=').is_some_and(|(n, _)| n == var),
                    None => true,
                })
            };
            match name.as_str() {
                "read" | "mapfile" | "readarray" | "getopts" | "unset" | "local" | "typeset"
                | "declare" | "export" | "readonly" | "integer" | "printf" | "let" => {
                    names_var(&args)
                }
                "eval" | "source" | "." | "trap" => true,
                _ => {
                    if seen.contains(&name) {
                        return false;
                    }
                    match state.funcs.get(&name) {
                        Some(body) => {
                            seen.push(name.clone());
                            writes_var(body, var, state, seen)
                        }
                        None => state.havocked,
                    }
                }
            }
        }
        Cmd::Seq { items, .. } => items.iter().any(|i| w(&i.cmd)),
        Cmd::AndOr { first, rest, .. } => w(first) || rest.iter().any(|(_, c)| w(c)),
        Cmd::Pipeline { cmds, .. } => cmds.iter().any(w),
        Cmd::If {
            arms, otherwise, ..
        } => arms.iter().any(|(c, b)| w(c) || w(b)) || otherwise.as_deref().is_some_and(w),
        Cmd::Case { arms, .. } => arms.iter().any(|a| w(&a.body)),
        Cmd::For { var: v, body, .. } => v == var || w(body),
        Cmd::ArithFor {
            init,
            cond,
            step,
            body,
            ..
        } => {
            [init, cond, step].iter().any(|e| {
                e.as_ref()
                    .is_some_and(|e| arith_writes(e).iter().any(|x| x == var))
            }) || w(body)
        }
        Cmd::Loop { cond, body, .. } => w(cond) || w(body),
        Cmd::Group { body, .. } | Cmd::Subshell { body, .. } | Cmd::Redirected { body, .. } => {
            w(body)
        }
        // A definition runs nothing; calls to it are checked at the call.
        Cmd::FuncDef { .. } => false,
        Cmd::Arith { expr, .. } => arith_writes(expr).iter().any(|x| x == var),
        Cmd::Cond { .. } => false,
        Cmd::Unsupported { .. } => true,
    }
}

/// Substitutions and arithmetic inside words can write too: `$(( v++ ))`,
/// and a function called inside `$( )` cannot (subshell) — but `$((…))` can.
fn word_writes(words: &[Word], var: &str, state: &State, seen: &mut Vec<String>) -> bool {
    fn parts_write(parts: &[Part], var: &str, state: &State, seen: &mut Vec<String>) -> bool {
        parts.iter().any(|p| match p {
            Part::Arith(e) => arith_writes(e).iter().any(|x| x == var),
            Part::DoubleQuoted(inner) => parts_write(inner, var, state, seen),
            Part::Brace(BraceExpr::List(ws)) => word_writes(ws, var, state, seen),
            Part::ParamOp { raw, name } => name == var && (raw.contains(":=") || raw.contains('=')),
            _ => false,
        })
    }
    words
        .iter()
        .any(|w| parts_write(&w.parts, var, state, seen))
}

#[cfg(test)]
mod tests;
