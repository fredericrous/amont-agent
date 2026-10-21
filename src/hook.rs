//! The guard itself: payload in, decision out.
//!
//! ## The contract
//!
//! **Every failure path exits 0 having written nothing.** Unreadable payload,
//! unknown event, a tool that is not Bash, a command this crate's lexer cannot
//! parse, a working directory that no longer exists, a rule that panics, a
//! journal that cannot be written — all of them are silence.
//!
//! That is not defensiveness, it is the only posture that keeps the guard
//! installed. A hook that fails toward refusing gets in the way of work the
//! author knew was correct, and the fix a person reaches for at that moment is
//! to delete the whole thing from `settings.json`, which switches off every
//! rule at once. A hook that fails toward silence loses one firing.
//!
//! ## The order is a cost decision
//!
//! `examine` runs first and touches nothing. Only if something fires do we pay
//! for `git config` (one process per key) or `confirm` (one process, or a
//! directory read). This runs before every shell command the model issues, so
//! the no-fire path is the path that has to be free.

use std::io::{IsTerminal, Read};
use std::process::ExitCode;

use crate::assertions::{self, Assertion, Claim, Verdict};
use crate::decision::{self, Decision};
use crate::journal;
use crate::payload::{self, Bash, Event, Session};
use crate::rules::{self, Confirmed, Context, Finding, Rule, Stance};
use crate::shell::{self, Parsed, Simple};

pub fn run() -> ExitCode {
    // A person typing `amont-agent hook` with no payload would otherwise block
    // on stdin forever with no indication why.
    if std::io::stdin().is_terminal() {
        eprintln!(
            "amont-agent: `hook` reads a Claude Code payload on stdin.\n\
             Try `amont-agent check '<command>'` to test a command by hand."
        );
        return ExitCode::from(2);
    }

    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return Decision::Silent.emit();
    }

    // A panic anywhere below is a bug in this crate, and a bug in this crate
    // must not become a refused command or a broken session.
    let decided = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decide(&raw)));
    match decided {
        Ok(d) => d.emit(),
        Err(_) => {
            eprintln!("amont-agent: internal error; allowing the command through");
            Decision::Silent.emit()
        }
    }
}

fn decide(raw: &str) -> Decision {
    // Name the session before any stance is looked up (ADR-0042 shadow arm).
    if let Some(session) = serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.get("session_id")?.as_str().map(str::to_string))
    {
        amont_agent_guard::shadow::enter_session(&session);
    }
    match payload::parse(raw) {
        Event::SessionStart(session) => {
            // The fact that we ran is what `doctor` uses to tell "no rule
            // fired" apart from "the guard is dead". Written first, so a slow
            // fetch below can never cost the heartbeat.
            heartbeat();
            crate::session_state::sweep();
            if session.source == "compact" {
                crate::session_state::compact(&session.session);
            }
            crate::preview::sweep();
            crate::plan_phases::sweep();
            crate::attestations::sweep();
            on_session_start(&session)
        }
        Event::NotOurs => Decision::Silent,
        Event::Prompt(prompt) => {
            crate::preview::on_prompt(&prompt);
            crate::plan_phases::on_prompt(&prompt);
            Decision::Silent
        }
        Event::PreAsk(ask) => {
            crate::implementation_review::on_pre_ask(&ask);
            let stance = crate::stance::resolve(&crate::push_preview::RULE);
            match crate::preview::on_pre_ask(&ask, stance) {
                Some(why) => {
                    Decision::Deny(decision::phrase(crate::push_preview::RULE.id, &why, ""))
                }
                None => Decision::Silent,
            }
        }
        Event::PrePlanExit(plan) => {
            let stance = crate::stance::resolve(&rules::plan_review_panel::RULE);
            crate::plan_review::on_plan_exit(&plan, stance)
        }
        Event::PostPlanExit(plan) => {
            crate::preview::on_plan_answered(&plan);
            Decision::Silent
        }
        Event::PostAsk(ask) => {
            let mut said: Vec<String> = Vec::new();
            crate::preview::on_post_ask(&ask);
            if let Some(text) = crate::implementation_review::on_post_ask(&ask) {
                if crate::stance::resolve(&crate::implementation_review_rule::RULE)
                    != Stance::Observe
                {
                    said.push(decision::phrase(
                        crate::implementation_review_rule::RULE.id,
                        &text,
                        "",
                    ));
                }
            }
            if said.is_empty() {
                Decision::Silent
            } else {
                Decision::Assert(said.join("\n\n"))
            }
        }
        Event::Stop(stop) => crate::plan_phases::on_stop(&stop, || {
            crate::stance::resolve(&rules::plan_phases_open::RULE)
        }),
        Event::PreFile(op) => on_file(&op),
        Event::PreGrep(op) => on_grep(&op),
        Event::PostLsp(call) => on_post_lsp(&call),
        Event::PostFile(op) => on_post_file(&op),
        Event::PreBash(bash) => on_bash(&bash),
        Event::PostBash(bash) => on_post_bash(&bash),
        Event::PostBashFailure(bash) => on_post_bash_failure(&bash),
    }
}

/// Where the checkout stands against the remote, stated once per session.
///
/// Governed by the `stale-base` rule's stance, so one key silences both the
/// notice and the branch-creation rule: `observe` measures and journals but
/// says nothing; anything above it speaks. There is nothing to refuse at a
/// session opening, so `deny` speaks exactly like `advise` here.
fn on_session_start(session: &Session) -> Decision {
    if !session.cwd.is_dir() {
        return Decision::Silent;
    }
    let mut lines: Vec<String> = Vec::new();
    if let Some(line) = stale_checkout_notice(session) {
        lines.push(line);
    }
    if let Some(line) = crate::guidance::notice(&session.cwd) {
        lines.push(line);
    }
    if let Some(line) = crate::shim::notice() {
        lines.push(line);
    }
    if let Some(line) = crate::plans::notice(&session.cwd) {
        lines.push(line);
    }
    if lines.is_empty() {
        Decision::Silent
    } else {
        Decision::Context(lines.join("\n\n"))
    }
}

/// The stale-checkout half of the session notice. `None` is silence —
/// up to date, not a repository, or the rule is only observing.
fn stale_checkout_notice(session: &Session) -> Option<String> {
    let rule = &rules::stale_base::RULE;
    let stance = crate::stance::resolve(rule);
    let drift = crate::stale::measure(&session.cwd, "HEAD")?;
    if drift.behind == 0 {
        return None;
    }
    let outcome = match stance {
        Stance::Observe => "watched",
        Stance::Advise | Stance::Deny => "advised",
    };
    journal::record(&journal::Entry {
        rule: rule.id,
        stance: stance.as_str(),
        outcome,
        session: &session.session,
        repo: &drift.repo,
        mode: "-",
        excerpt: &format!("session start: {} behind {}", drift.behind, drift.base),
    });
    match stance {
        Stance::Observe => None,
        Stance::Advise | Stance::Deny => Some(format!(
            "amont-agent/{}: {}",
            rule.id,
            crate::stale::notice(&drift)
        )),
    }
}

fn on_bash(bash: &Bash) -> Decision {
    let parsed = shell::lex(&bash.command);
    // A pass file of the implementation review is written by the hook
    // alone. A command that would write there is refused before any stance
    // is consulted: there is no legitimate writer to advise, and `observe`
    // turns a rule off, not a boundary. A read of the record passes.
    if let Some(span) = crate::implementation_review::store_clause(&parsed) {
        let rule = &crate::implementation_review_rule::RULE;
        journal::record(&journal::Entry {
            rule: rule.id,
            stance: "deny",
            outcome: "denied",
            session: &bash.session,
            repo: &repo_name(&bash.cwd),
            mode: &bash.permission_mode,
            excerpt: &crate::backtest::excerpt(&bash.command, span.start, span.end),
        });
        return Decision::Deny(decision::phrase(
            rule.id,
            crate::implementation_review::GUARD_REASON,
            crate::implementation_review::GUARD_REMEDY,
        ));
    }
    // When a `preview register` call started, for `preview::bind` to tell
    // its own output from a line an earlier clause printed.
    crate::preview::stamp_register(bash, &parsed);
    // No early return on `Parsed::Opaque`: `rules::evaluate` owns that
    // policy, so the hook, `check` and the backtester cannot drift apart on
    // it. Legacy rules are skipped on an unreadable command exactly as they
    // were; a rule built on the shell analysis judges it with its own account
    // of what it could not read.
    let input = rules::Input::new(&bash.command, rules::tool_shell::dialect(), &parsed);
    let fired = rules::evaluate(crate::rule_table::RULES, &input);
    let dumped = rules::dump::dumps(&parsed);
    if fired.is_empty() && dumped.is_empty() {
        // The whole no-fire path: one lex, no processes, no files.
        return Decision::Silent;
    }

    // A pipeline we could not read is recorded even when nothing fires: a
    // guard that says nothing must stay distinguishable from a guard that
    // could not look, and this is the only place that difference is written
    // down.
    for cmd in parsed.hidden() {
        if let Some(why) = &cmd.opaque {
            journal::record(&journal::Entry {
                rule: "-",
                stance: "-",
                outcome: "partial",
                session: &bash.session,
                repo: &repo_name(&bash.cwd),
                mode: &bash.permission_mode,
                excerpt: &why.why(),
            });
            break;
        }
    }

    let mut deny: Vec<String> = Vec::new();
    let mut advise: Vec<String> = Vec::new();

    // A `cat`-shaped dump is a read the session should remember, and may be
    // one it already made.
    if !bash.background {
        for d in &dumped {
            let ctx = Context {
                platform: Some(crate::rules::Platform::host()),
                cwd: &bash.cwd,
                parsed: &parsed,
                background: bash.background,
                timeout_ms: bash.timeout_ms,
                tool_use_id: &bash.tool_use_id,
                transcript: bash.transcript.as_deref(),
                agent_transcript: bash.agent_transcript.as_deref(),
                command: &bash.command,
                output: None,
                session: &bash.session,
                failed: false,
            };
            if writes_or_waits(&ctx, d) {
                continue;
            }
            let path = resolve_path(&ctx.cwd_at(d.at), &d.path);
            let verdict = reread_verdict(
                &bash.session,
                bash.agent_id.as_deref(),
                &path,
                &d.window,
                &bash.permission_mode,
                &bash.cwd,
                &d.path,
            );
            if let Some(text) = verdict.text {
                match verdict.stance {
                    Stance::Deny => deny.push(text),
                    Stance::Advise => advise.push(text),
                    Stance::Observe => {}
                }
            }
            // The read itself is remembered by `on_post_bash`, once the
            // command has actually run: a `cat` that is refused below, or that
            // fails on a path that is not there, put nothing in context.
        }
    }

    for (rule, finding) in &fired {
        let mut stance = crate::stance::resolve(rule);
        let Confirmation {
            floor,
            ceiling,
            said,
        } = match confirmed(rule, stance, finding, bash, &parsed) {
            Ok(c) => c,
            Err(why) => {
                // The reason is the record. `status` tallies these, and "why did
                // confirm say no" is the number that decides whether an observing
                // rule may ever advise — a rule declined for `already in a linked
                // worktree` a thousand times is a rule being obeyed, not one that
                // is wrong.
                note(rule, "unconfirmed", why, bash, finding);
                continue;
            }
        };
        if rule.id == rules::symbol_grep_not_lsp::RULE.id {
            if let Err(why) = grep_gate(bash, &parsed, finding, stance) {
                note(rule, "unconfirmed", why, bash, finding);
                continue;
            }
        }
        if let Some(floor) = floor {
            if stance >= Stance::Advise {
                stance = stance.max(floor).min(rule.max_stance);
            }
        }
        // A `confirm` that learned the reason by looking speaks and journals
        // it in place of the finding's; every other rule keeps the finding's
        // reason and the command span.
        let (reason, excerpt, remedy): (&str, Option<&str>, &str) = match &said {
            Some((reason, excerpt, remedy)) => (
                reason.as_str(),
                Some(excerpt.as_str()),
                remedy.as_deref().unwrap_or(&finding.remedy),
            ),
            None => (finding.reason.as_str(), None, finding.remedy.as_str()),
        };
        let text = decision::phrase(rule.id, reason, remedy);
        // Never refuse on half a reading. A `deny` derived from a command we
        // only partly understood is the worst outcome available here: total
        // opacity would have let it run. It still advises, and it is still
        // journalled under its configured stance, so the evidence for lifting
        // this cap accumulates in the usual place.
        let stance = if parsed.fully_read() {
            stance
        } else {
            stance.min(Stance::Advise)
        };
        // A `confirm` that could not establish its fact may cap itself, so
        // that not knowing never refuses.
        let stance = ceiling.map_or(stance, |c| stance.min(c));
        match stance {
            Stance::Observe => note_with(rule, "observe", "watched", bash, finding, excerpt),
            Stance::Advise => {
                note_with(rule, "advise", "advised", bash, finding, excerpt);
                advise.push(text);
            }
            Stance::Deny => {
                note_with(rule, "deny", "denied", bash, finding, excerpt);
                deny.push(text);
            }
        }
    }

    // Where each branch destination stands before the push, for
    // `push-published` to compare against once it has run. Only a push that
    // is going to run is worth remembering.
    if deny.is_empty() {
        crate::preview::record_before(bash, &parsed);
    }

    // A refusal outranks advice: there is no point advising about a command
    // that is not going to run. The advisory findings are still journalled.
    if !deny.is_empty() {
        Decision::Deny(deny.join("\n\n"))
    } else if !advise.is_empty() {
        Decision::Advise(advise.join("\n\n"))
    } else {
        Decision::Silent
    }
}

/// The call's stdout and stderr joined by a newline, or `None` when it has neither.
fn joined_output(bash: &Bash) -> Option<String> {
    match (bash.stdout.as_deref(), bash.stderr.as_deref()) {
        (None, None) => None,
        (Some(o), None) => Some(o.to_string()),
        (None, Some(e)) => Some(e.to_string()),
        (Some(o), Some(e)) => Some(format!("{o}\n{e}")),
    }
}

/// A call that has already run and reported success.
///
/// The shape mirrors `on_bash` deliberately — lex, pure prefilter, and only
/// then anything that touches the world — because this fires after EVERY
/// successful Bash call, of every session on the machine. A user-scope hook is
/// not per-project: two other sessions' commands arrive here too, which is also
/// why every answer is taken from the payload's `cwd` and never from this
/// process's own.
///
/// There is nothing left to refuse, so `deny` speaks like `advise`, the same
/// way it does at a session opening.
fn on_post_bash(bash: &Bash) -> Decision {
    let parsed = shell::lex(&bash.command);
    if matches!(parsed, Parsed::Opaque(_)) {
        return Decision::Silent;
    }
    // A `preview register` that ran on its own is bound to this session and
    // prompt here, from its printed output. One that did not bind says so —
    // even run in the background, which is one of the reasons it cannot.
    let unbound = crate::preview::bind(bash, &parsed)
        .map(|t| decision::phrase(crate::push_preview::RULE.id, &t, ""));
    if bash.background {
        // Detached: the tool call returned a task id, not a result. Whatever it
        // claimed has not finished happening.
        return unbound.map_or(Decision::Silent, Decision::Assert);
    }

    let output = joined_output(bash);
    let ctx = Context {
        platform: Some(crate::rules::Platform::host()),
        cwd: &bash.cwd,
        parsed: &parsed,
        background: bash.background,
        timeout_ms: bash.timeout_ms,
        tool_use_id: &bash.tool_use_id,
        transcript: bash.transcript.as_deref(),
        agent_transcript: bash.agent_transcript.as_deref(),
        command: &bash.command,
        output: output.as_deref(),
        session: &bash.session,
        failed: false,
    };

    // A `cat`-shaped dump that has now run is a read the session should
    // remember. It needs the whole command: a path recorded from a half-read
    // line would have `file-reread` advise, later, about a file the session
    // may never have read.
    if parsed.fully_read() {
        if let Some(agent) = crate::lsp::agent_key(bash.agent_id.as_deref()) {
            let delivered = bash_delivered_whole(bash);
            for d in rules::dump::dumps(&parsed) {
                let path = resolve_path(&ctx.cwd_at(d.at), &d.path);
                let certain = delivered
                    && d.window != crate::session_state::INEXACT
                    && rules::dump::runs_unconditionally(&bash.command, d.at);
                crate::session_state::record(
                    &bash.session,
                    &agent,
                    "read",
                    &path,
                    &d.window,
                    certain,
                );
            }
        }
    }

    let claimed = assertions::examine_all(&parsed);
    if claimed.is_empty() {
        // The whole no-claim, no-dump path: one lex, no processes, no files.
        return unbound.map_or(Decision::Silent, Decision::Assert);
    }

    let mut spoken: Vec<String> = unbound.into_iter().collect();
    for (assertion, claim) in &claimed {
        let stance = crate::stance::resolve_assertion(assertion);
        match (assertion.verify)(&ctx, claim) {
            Verdict::Unknown(why) => {
                note_claim(assertion, "unverified", why, bash, claim);
            }
            Verdict::Held => note_claim(assertion, stance.as_str(), "held", bash, claim),
            Verdict::Noted(outcome) => note_claim(assertion, stance.as_str(), outcome, bash, claim),
            Verdict::Broken { reason, remedy } => {
                note_claim(assertion, stance.as_str(), "broken", bash, claim);
                if stance != Stance::Observe {
                    spoken.push(decision::phrase(assertion.id, &reason, &remedy));
                }
            }
        }
    }

    if spoken.is_empty() {
        Decision::Silent
    } else {
        Decision::Assert(spoken.join("\n\n"))
    }
}

/// A Bash call that FAILED: only [`assertions::AFTER_FAILURE`] run, with the
/// same verdict handling as [`on_post_bash`]. A detached call has not ended.
fn on_post_bash_failure(bash: &Bash) -> Decision {
    if bash.background {
        return Decision::Silent;
    }
    let parsed = shell::lex(&bash.command);
    if !parsed.fully_read() {
        return Decision::Silent;
    }
    let claimed = assertions::after_failure(&parsed);
    if claimed.is_empty() {
        return Decision::Silent;
    }
    // The failure text is the top-level `error` string, after stdout and stderr.
    let output = [
        joined_output(bash),
        bash.error.clone().filter(|e| !e.is_empty()),
    ]
    .into_iter()
    .flatten()
    .reduce(|acc, e| format!("{acc}\n{e}"));
    let ctx = Context {
        platform: Some(crate::rules::Platform::host()),
        cwd: &bash.cwd,
        parsed: &parsed,
        background: bash.background,
        timeout_ms: bash.timeout_ms,
        tool_use_id: &bash.tool_use_id,
        transcript: bash.transcript.as_deref(),
        agent_transcript: bash.agent_transcript.as_deref(),
        command: &bash.command,
        output: output.as_deref(),
        session: &bash.session,
        failed: true,
    };
    let mut spoken = Vec::new();
    for (assertion, claim) in &claimed {
        let stance = crate::stance::resolve_assertion(assertion);
        match (assertion.verify)(&ctx, claim) {
            Verdict::Unknown(why) => note_claim(assertion, "unverified", why, bash, claim),
            Verdict::Held => note_claim(assertion, stance.as_str(), "held", bash, claim),
            Verdict::Noted(outcome) => note_claim(assertion, stance.as_str(), outcome, bash, claim),
            Verdict::Broken { reason, remedy } => {
                note_claim(assertion, stance.as_str(), "broken", bash, claim);
                if stance != Stance::Observe {
                    spoken.push(decision::phrase(assertion.id, &reason, &remedy));
                }
            }
        }
    }
    if spoken.is_empty() {
        Decision::Silent
    } else {
        Decision::AssertAfterFailure(spoken.join("\n\n"))
    }
}

fn note_claim(assertion: &Assertion, stance: &str, outcome: &str, bash: &Bash, claim: &Claim) {
    let excerpt = crate::backtest::excerpt(&bash.command, claim.span.start, claim.span.end);
    journal::record(&journal::Entry {
        rule: assertion.id,
        stance,
        outcome,
        session: &bash.session,
        repo: &attributed_repo(assertion.id, bash),
        mode: &bash.permission_mode,
        excerpt: &excerpt,
    });
}

/// A rule with no `confirm` is confirmed. A `confirm` that cannot answer is
/// NOT — failing to establish the fact is silence, like everything else here.
/// The `Err` names why, in the words the rule chose, for the journal.
/// A Read, Edit, Write or MultiEdit about to run: remember a write, and
/// before a Read, say whether the session already has that file. The Read
/// itself is remembered by [`on_post_file`], once it has happened.
fn on_file(op: &crate::payload::FileOp) -> Decision {
    if op.writes {
        // A pass file of the implementation review is written by the hook
        // alone; a Write or Edit there is refused whatever the rule's
        // stance, since there is no legitimate writer to advise.
        if crate::implementation_review::guards_path(&op.path) {
            let rule = &crate::implementation_review_rule::RULE;
            journal::record(&journal::Entry {
                rule: rule.id,
                stance: "deny",
                outcome: "denied",
                session: &op.session,
                repo: &repo_name(&op.cwd),
                mode: &op.permission_mode,
                excerpt: &format!("write {}", op.path.display()),
            });
            return Decision::Deny(decision::phrase(
                rule.id,
                crate::implementation_review::GUARD_REASON,
                crate::implementation_review::GUARD_REMEDY,
            ));
        }
        if let Some(agent) = crate::lsp::agent_key(op.agent_id.as_deref()) {
            crate::session_state::record(&op.session, &agent, "write", &op.path, "full", true);
        }
        return write_decision(op);
    }
    let mut advise: Vec<String> = Vec::new();
    let mut deny: Vec<String> = Vec::new();
    let shown = op.path.to_string_lossy().into_owned();

    let persisted = op.window == "full" && rules::persisted_output_dump::is_persisted(&shown);
    if persisted {
        let rule = &rules::persisted_output_dump::RULE;
        let stance = crate::stance::resolve(rule);
        let text = decision::phrase(
            rule.id,
            &rules::persisted_output_dump::reason(),
            &rules::persisted_output_dump::remedy(),
        );
        note_file(rule, stance, op, &shown);
        match stance {
            Stance::Deny => deny.push(text),
            Stance::Advise => advise.push(text),
            Stance::Observe => {}
        }
    }
    let reread = reread_verdict(
        &op.session,
        op.agent_id.as_deref(),
        &op.path,
        &crate::session_state::read_window(&op.path, &op.window),
        &op.permission_mode,
        &op.cwd,
        &shown,
    );
    if let Some(text) = reread.text {
        match reread.stance {
            Stance::Deny => deny.push(text),
            Stance::Advise => advise.push(text),
            Stance::Observe => {}
        }
    }
    // Code that LSP would answer for: the remedy is LSP, and while this rule
    // speaks, `read-unbounded-large` does not. An observing rule leaves it
    // speaking, so turning this rule off never loses the size advice.
    let mut code_spoken = false;
    if !persisted && !reread.seen {
        if let Some(bytes) = rules::code_read_not_lsp::applies(&op.path, &op.window) {
            if crate::lsp::serves(&op.path).is_ok()
                && crate::lsp::agent_has_tool(&op.agent, &op.cwd).is_ok()
            {
                let rule = &rules::code_read_not_lsp::RULE;
                let stance = crate::stance::resolve(rule);
                let lsp_first = crate::lsp::asked(&op.session, op.agent_id.as_deref(), &op.path);
                if lsp_first {
                    // LSP came first: the case the advice was meant to
                    // produce. This rule is silent; `read-unbounded-large`
                    // still speaks, because after LSP named a line the
                    // remedy for a large whole Read is the window around it.
                    journal::record(&journal::Entry {
                        rule: rule.id,
                        stance: stance.as_str(),
                        outcome: "lsp-first",
                        session: &op.session,
                        repo: &repo_name(&op.cwd),
                        mode: &op.permission_mode,
                        excerpt: &shown,
                    });
                }
                // A refusal is not told once: every Read is refused until LSP
                // has been asked, which is the way through the text names.
                let first = !lsp_first
                    && (stance == Stance::Observe
                        || stance == Stance::Deny
                        || first_telling(
                            rule,
                            &op.session,
                            // Per agent: a subagent is told on its own.
                            &match &op.agent_id {
                                None => format!("read {}", op.path.display()),
                                Some(id) => format!("read {} @{id}", op.path.display()),
                            },
                            &op.cwd,
                            &op.permission_mode,
                        ));
                if first {
                    note_file(rule, stance, op, &shown);
                    let (reason, remedy) = if stance == Stance::Deny {
                        rules::code_read_not_lsp::phrase_refusal(&shown, bytes)
                    } else {
                        rules::code_read_not_lsp::phrase(&shown, bytes)
                    };
                    let text = decision::phrase(rule.id, &reason, &remedy);
                    match stance {
                        Stance::Deny => deny.push(text),
                        Stance::Advise => advise.push(text),
                        Stance::Observe => {}
                    }
                    code_spoken = stance != Stance::Observe;
                }
            }
        }
    }
    // Last, and quiet on a file the session has seen whatever `file-reread`'s
    // stance: its remedy already says to use `offset`/`limit`.
    if !persisted && !reread.seen && !code_spoken {
        if let Some(bytes) = rules::read_unbounded_large::applies(&op.path, &op.window) {
            let rule = &rules::read_unbounded_large::RULE;
            let stance = crate::stance::resolve(rule);
            use rules::read_unbounded_large::Finder;
            let finder = if crate::lsp::is_code(&shown) && crate::lsp::serves(&op.path).is_ok() {
                Finder::Lsp
            } else {
                Finder::Grep
            };
            let (reason, remedy) = rules::read_unbounded_large::phrase(&shown, bytes, finder);
            let text = decision::phrase(rule.id, &reason, &remedy);
            note_file(rule, stance, op, &shown);
            match stance {
                Stance::Deny => deny.push(text),
                Stance::Advise => advise.push(text),
                Stance::Observe => {}
            }
        }
    }
    if !deny.is_empty() {
        Decision::Deny(deny.join("\n\n"))
    } else if !advise.is_empty() {
        Decision::Advise(advise.join("\n\n"))
    } else {
        Decision::Silent
    }
}

/// Every write rule's verdict on one Write, Edit or MultiEdit, collected:
/// any Deny makes the result a Deny, and either way every text is carried,
/// joined with blank lines, as the Read branch does.
fn write_decision(op: &crate::payload::FileOp) -> Decision {
    combine([
        lint_suppression(op),
        guidance_written(op),
        guidance_shape(op),
    ])
}

/// The verdicts of several rules as one: any Deny makes a Deny, and either
/// way every text is carried, joined with blank lines.
fn combine(decisions: impl IntoIterator<Item = Decision>) -> Decision {
    let mut denied = false;
    let mut asked = false;
    let mut texts: Vec<String> = Vec::new();
    for decision in decisions {
        match decision {
            Decision::Silent => {}
            Decision::Deny(text) => {
                denied = true;
                texts.push(text);
            }
            Decision::Ask(text) => {
                asked = true;
                texts.push(text);
            }
            Decision::Advise(text) => texts.push(text),
            // A write rule answers Silent, Advise, Ask or Deny; these belong
            // to other events. If one ever reaches here, its text is kept as
            // advice rather than dropped.
            Decision::Context(text)
            | Decision::Assert(text)
            | Decision::AssertAfterFailure(text)
            | Decision::Continue(text)
            | Decision::UserNote(text) => texts.push(text),
        }
    }
    if texts.is_empty() {
        Decision::Silent
    } else if denied {
        Decision::Deny(texts.join("\n\n"))
    } else if asked {
        Decision::Ask(texts.join("\n\n"))
    } else {
        Decision::Advise(texts.join("\n\n"))
    }
}

/// The key a once-per-session telling is kept under: per agent, so a subagent
/// is told on its own.
fn telling_key(what: &str, op: &crate::payload::FileOp) -> String {
    match &op.agent_id {
        None => format!("{what} {}", op.path.display()),
        Some(id) => format!("{what} {} @{id}", op.path.display()),
    }
}

/// A write to a file agents read as guidance: told once per session and file.
fn guidance_written(op: &crate::payload::FileOp) -> Decision {
    use rules::agent_guidance_written as agw;
    if op.change.is_none() || !agw::is_guidance(&op.path) {
        return Decision::Silent;
    }
    let rule = &agw::RULE;
    let stance = crate::stance::resolve(rule);
    let shown = op.path.to_string_lossy().into_owned();
    let first = stance == Stance::Observe
        || first_telling(
            rule,
            &op.session,
            &telling_key("write", op),
            &op.cwd,
            &op.permission_mode,
        );
    if !first {
        journal_repeat(rule, stance, op, &shown);
        return Decision::Silent;
    }
    note_file(rule, stance, op, &shown);
    match stance {
        Stance::Observe => Decision::Silent,
        // The ceiling is advise (`max_stance`); a deny stance still advises.
        Stance::Advise | Stance::Deny => Decision::Advise(agw::phrase(&shown)),
    }
}

/// A write that leaves a skill unfindable or an always-on file over budget.
/// Judged only on a whole-file rebuild; a fragment is journalled and silent.
fn guidance_shape(op: &crate::payload::FileOp) -> Decision {
    use rules::agent_guidance_shape as ags;
    use rules::lint_suppression_added as lsa;
    let (Some(change), Some(kind)) = (&op.change, ags::kind(&op.path)) else {
        return Decision::Silent;
    };
    let rule = &ags::RULE;
    let stance = crate::stance::resolve(rule);
    let shown = op.path.to_string_lossy().into_owned();
    let rebuilt = lsa::reconstruct(&op.path, change);
    if let lsa::Mode::Fragment(why) = rebuilt.mode {
        journal::record(&journal::Entry {
            rule: rule.id,
            stance: stance.as_str(),
            outcome: "fragment",
            session: &op.session,
            repo: &repo_name(&op.cwd),
            mode: &op.permission_mode,
            excerpt: &format!("{shown} {}", why.as_str()),
        });
        return Decision::Silent;
    }
    let Some(text) = ags::check(&op.path, kind, &rebuilt.before, &rebuilt.after) else {
        return Decision::Silent;
    };
    let first = stance == Stance::Observe
        || first_telling(
            rule,
            &op.session,
            &telling_key("shape", op),
            &op.cwd,
            &op.permission_mode,
        );
    if !first {
        journal_repeat(rule, stance, op, &shown);
        return Decision::Silent;
    }
    note_file(rule, stance, op, &ags::excerpt(&shown, kind));
    match stance {
        Stance::Observe => Decision::Silent,
        // The ceiling is advise (`max_stance`); a deny stance still advises.
        Stance::Advise | Stance::Deny => Decision::Advise(text),
    }
}

/// A finding already told this session: journalled, not said again.
fn journal_repeat(rule: &Rule, stance: Stance, op: &crate::payload::FileOp, shown: &str) {
    journal::record(&journal::Entry {
        rule: rule.id,
        stance: stance.as_str(),
        outcome: "told-before",
        session: &op.session,
        repo: &repo_name(&op.cwd),
        mode: &op.permission_mode,
        excerpt: shown,
    });
}

/// An Edit, MultiEdit or Write that adds a lint suppression or loosens a lint
/// configuration. The file is rebuilt before and after only for a file type the
/// rule examines; the effects (the bounded read) stay here at the boundary.
fn lint_suppression(op: &crate::payload::FileOp) -> Decision {
    use rules::lint_suppression_added as lsa;
    let Some(change) = &op.change else {
        return Decision::Silent;
    };
    if !lsa::worth_rebuilding(&op.path, change) {
        return Decision::Silent;
    }
    let rebuilt = lsa::reconstruct(&op.path, change);
    let hits = lsa::examine_change(&op.path, &rebuilt.before, &rebuilt.after);
    if hits.is_empty() {
        return Decision::Silent;
    }
    let rule = &lsa::RULE;
    let stance = crate::stance::resolve(rule);
    let shown = op.path.to_string_lossy().into_owned();
    note_file(
        rule,
        stance,
        op,
        &lsa::excerpt(&shown, &hits, &rebuilt.mode),
    );
    let text = lsa::phrase(&shown, &hits, &rebuilt.mode);
    match stance {
        Stance::Deny => Decision::Deny(text),
        Stance::Advise => Decision::Advise(text),
        Stance::Observe => Decision::Silent,
    }
}

/// An LSP call that has run: the files it was about go on the session's
/// record under the calling agent, and the call is journalled (`lsp`,
/// `lsp-error` for an `Error performing` answer, `lsp-unparsed` when the
/// response had no text `result`). An error counts as asked: a refusal must
/// never block a Read because the server is broken. Never speaks.
fn on_post_lsp(call: &crate::payload::LspCall) -> Decision {
    let outcome = match call.response.as_deref() {
        None => "lsp-unparsed",
        Some(r) if r.starts_with("Error performing") => "lsp-error",
        Some(_) => "lsp",
    };
    let repo = repo_name(&call.cwd);
    let excerpt = format!("{} {}", call.operation, call.file_path.display());
    let entry = |outcome, excerpt: &str| {
        journal::record(&journal::Entry {
            rule: "-",
            stance: "-",
            outcome,
            session: &call.session,
            repo: &repo,
            mode: &call.permission_mode,
            excerpt,
        })
    };
    entry(outcome, &excerpt);
    let files = crate::lsp::asked_files(call);
    if let Err(why) = crate::lsp::record_asked(&call.session, call.agent_id.as_deref(), &files) {
        entry("unrecorded", why);
    }
    Decision::Silent
}

/// A Read that has run and succeeded: the file is in context now, so this is
/// the moment to remember it. Recording before the call did the wrong thing
/// twice over — a Read refused above, or one that failed on a path that is
/// not there, was still on record, and a retry was told its contents were
/// already in context.
fn on_post_file(op: &crate::payload::FileOp) -> Decision {
    if !op.writes {
        if let Some(agent) = crate::lsp::agent_key(op.agent_id.as_deref()) {
            let window = crate::session_state::read_window(&op.path, &op.window);
            crate::session_state::record(
                &op.session,
                &agent,
                "read",
                &op.path,
                &window,
                !op.persisted_result,
            );
        }
    }
    Decision::Silent
}

/// What `symbol-grep-not-lsp` needs beyond its `confirm`: the facts are in
/// the payload, not the command. The agent must have the LSP tool, and a
/// session is told about one pattern once. An observing rule records nothing,
/// so it never uses up the one telling.
fn grep_gate(
    bash: &Bash,
    parsed: &Parsed,
    finding: &Finding,
    stance: Stance,
) -> Result<(), &'static str> {
    crate::lsp::agent_has_tool(&bash.agent, &bash.cwd)?;
    if stance == Stance::Observe {
        return Ok(());
    }
    let search = rules::symbol_grep_not_lsp::search_at(parsed, finding.span.start)
        .ok_or("the command no longer matches")?;
    let key = format!("grep {}", search.pattern);
    if first_telling(
        &rules::symbol_grep_not_lsp::RULE,
        &bash.session,
        &key,
        &bash.cwd,
        &bash.permission_mode,
    ) {
        Ok(())
    } else {
        Err("this session was told about this pattern")
    }
}

/// A Grep tool call about to run: the tool's half of `symbol-grep-not-lsp`.
/// The pure tests come first; the world is asked only about a symbol search
/// over code.
fn on_grep(op: &crate::payload::GrepOp) -> Decision {
    use rules::symbol_grep_not_lsp as sg;
    if op.output_mode.as_deref() == Some("count") || !sg::symbol(&op.pattern) {
        return Decision::Silent;
    }
    let scope = match &op.path {
        None => sg::GrepScope::Dir,
        Some(p) if p.is_dir() => sg::GrepScope::Dir,
        Some(p) if p.is_file() => sg::GrepScope::File(
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
        Some(_) => sg::GrepScope::Missing,
    };
    if !sg::grep_tool_targets_code(&scope, op.glob.as_deref(), op.kind.as_deref()) {
        return Decision::Silent;
    }
    let target = op.path.clone().unwrap_or_else(|| op.cwd.clone());
    if crate::lsp::serves(&target).is_err()
        || crate::lsp::agent_has_tool(&op.agent, &op.cwd).is_err()
    {
        return Decision::Silent;
    }
    let rule = &sg::RULE;
    let stance = crate::stance::resolve(rule);
    if stance != Stance::Observe
        && !first_telling(
            rule,
            &op.session,
            &format!("grep {}", op.pattern),
            &op.cwd,
            &op.permission_mode,
        )
    {
        return Decision::Silent;
    }
    journal::record(&journal::Entry {
        rule: rule.id,
        stance: stance.as_str(),
        outcome: outcome_of(stance),
        session: &op.session,
        repo: &repo_name(&op.cwd),
        mode: &op.permission_mode,
        excerpt: &format!("Grep {}", op.pattern),
    });
    let (reason, remedy) = sg::phrase(&op.pattern);
    let text = decision::phrase(rule.id, &reason, &remedy);
    match stance {
        Stance::Observe => Decision::Silent,
        Stance::Advise => Decision::Advise(text),
        Stance::Deny => Decision::Deny(text),
    }
}

/// Whether a rule about to speak has not yet told this session about `key`.
/// A record that could not be kept is journalled, and the rule speaks: being
/// told twice costs less than never being told.
fn first_telling(rule: &Rule, session: &str, key: &str, cwd: &std::path::Path, mode: &str) -> bool {
    match crate::lsp::told(session, key) {
        crate::lsp::Told::First => true,
        crate::lsp::Told::Before => false,
        crate::lsp::Told::Unrecorded(why) => {
            journal::record(&journal::Entry {
                rule: rule.id,
                stance: "-",
                outcome: "unrecorded",
                session,
                repo: &repo_name(cwd),
                mode,
                excerpt: why,
            });
            true
        }
    }
}

fn outcome_of(stance: Stance) -> &'static str {
    match stance {
        Stance::Observe => "watched",
        Stance::Advise => "advised",
        Stance::Deny => "denied",
    }
}

/// Whether a Bash result reached the model whole: not an error, shorter than
/// the harness cap (`BASH_MAX_OUTPUT_LENGTH`, else 30,000, counted in
/// characters), and without a persisted-output marker. A payload with no
/// `tool_response` at all is not a harness result and is taken as whole; one
/// whose `tool_response` carries no `stdout` is an unrecognised shape.
fn bash_delivered_whole(bash: &Bash) -> bool {
    if bash.errored {
        return false;
    }
    let Some(out) = &bash.stdout else {
        return !bash.has_response;
    };
    let cap = std::env::var("BASH_MAX_OUTPUT_LENGTH")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(30_000);
    out.chars().count() < cap && !crate::payload::has_persisted_marker(out)
}

/// Lexical clean-up of an absolute path: `.` components dropped, `..` folded.
/// No file is touched, so a path that is not there compares like any other.
fn normalized(path: std::path::PathBuf) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The path a word names when the clause at `at` runs, so that a write and a
/// dump are compared where each one acts, not as spelled.
fn named_path(ctx: &Context, at: usize, text: &str) -> std::path::PathBuf {
    normalized(resolve_path(&ctx.cwd_at(at), text))
}

/// Whether the command also changes the dumped file under the dump, or waits
/// for it to change. Each exception is bound to the dumped path:
/// - an earlier clause redirects (`>`/`>>`) onto the same file;
/// - an earlier interpreter heredoc names the file as a quoted literal and
///   writes (`open(p, 'w')`, `write_text`, ...);
/// - a `while`/`until` loop that runs `sleep` and names the file starts at or
///   before the dump (the dump may sit in the loop or follow it).
///
/// Such a dump shows a file that changes under it, so it is neither a certain
/// read to remember nor a repeat to refuse.
fn writes_or_waits(ctx: &Context, d: &rules::dump::Dump) -> bool {
    let target = named_path(ctx, d.at, &d.path);
    let clauses = || ctx.parsed.judgeable();
    let redirected = clauses().any(|c| {
        c.at < d.at
            && c.redirects
                .iter()
                .any(|(op, w)| op.contains('>') && named_path(ctx, c.at, &w.text) == target)
    });
    if redirected {
        return true;
    }
    // holds-until: the heredoc exception holds for a python, perl, node or
    // ruby heredoc whose body names the dumped path as a quoted literal (or
    // as a variable or first parameter bound to it in the body) and opens
    // that name in a write mode or calls write_text/write_bytes/writeFileSync
    // on it; it ends when the interpreter's write is read from the shell
    // analysis's effects instead of matched in the body text.
    let scripted = rules::dump::heredoc_bodies(ctx.command).iter().any(|h| {
        h.start < d.at
            && h.program
                .as_deref()
                .is_some_and(|p| p.starts_with("python") || matches!(p, "perl" | "node" | "ruby"))
            && body_writes_path(&h.body, &d.path)
    });
    if scripted {
        return true;
    }
    rules::dump::wait_loops(ctx.command)
        .into_iter()
        .filter(|l| l.start <= d.at)
        .any(|l| {
            let inside = || clauses().filter(|c| l.start <= c.at && c.at < l.end);
            inside().any(runs_sleep)
                && inside().any(|c| {
                    c.words
                        .iter()
                        .map(|w| w.text.as_str())
                        .chain(c.redirects.iter().map(|(_, w)| w.text.as_str()))
                        .any(|t| path_like_word(t) && named_path(ctx, c.at, t) == target)
                })
        })
}

/// Whether an interpreter body writes `path`: the quoted literal, or a name
/// the body binds to it, opened in a write mode or given to a path writer.
fn body_writes_path(body: &str, path: &str) -> bool {
    let lits = [format!("'{path}'"), format!("\"{path}\"")];
    let is_ident = |t: &str| {
        !t.is_empty()
            && t.chars().all(|c| c.is_alphanumeric() || c == '_')
            && !t.starts_with(|c: char| c.is_ascii_digit())
    };
    let mut names: Vec<String> = Vec::new();
    for lit in &lits {
        for (i, _) in body.match_indices(lit.as_str()) {
            let before = body[..i].trim_end();
            if let Some(lhs) = before
                .strip_suffix('=')
                .filter(|l| !l.ends_with(['=', '!', '<', '>']))
            {
                let lhs = lhs.trim_end();
                let start = lhs
                    .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .map_or(0, |k| k + 1);
                if is_ident(&lhs[start..]) {
                    names.push(lhs[start..].to_string());
                }
            } else if let Some(call) = before.strip_suffix('(') {
                // The literal is the first argument of `name(`: the name's
                // `def name(first, ...)` binds it.
                let start = call
                    .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .map_or(0, |k| k + 1);
                let name = &call[start..];
                if is_ident(name) {
                    let def = format!("def {name}(");
                    if let Some(k) = body.find(&def) {
                        let rest = &body[k + def.len()..];
                        let first = rest.split([',', ')']).next().unwrap_or("").trim();
                        if is_ident(first) {
                            names.push(first.to_string());
                        }
                    }
                }
            }
        }
    }
    let names_path = |arg: &str| {
        let arg = arg.trim();
        lits.iter().any(|l| l == arg) || names.iter().any(|n| n == arg)
    };
    let write_mode = |m: &str| m.contains(['w', 'a', 'x', '+']);
    let opens = body.match_indices("open(").any(|(i, _)| {
        let rest = &body[i + "open(".len()..];
        let end = rest.find(')').unwrap_or(rest.len());
        let mut args = rest[..end].split(',');
        let target = args.next().unwrap_or("");
        names_path(target)
            && args.any(|a| {
                let a = a.trim();
                let a = a
                    .strip_prefix("mode")
                    .map_or(a, |m| m.trim_start().trim_start_matches('=').trim());
                ["'", "\""].iter().any(|q| {
                    a.strip_prefix(q)
                        .and_then(|m| m.strip_suffix(q))
                        .is_some_and(write_mode)
                })
            })
    });
    if opens {
        return true;
    }
    let writers = [".write_text(", ".write_bytes("].iter().any(|w| {
        body.match_indices(w).any(|(i, _)| {
            let recv = &body[..i];
            match recv.strip_suffix(')') {
                Some(inner) => inner
                    .rfind('(')
                    .is_some_and(|k| names_path(&inner[k + 1..])),
                None => {
                    let start = recv
                        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .map_or(0, |k| k + 1);
                    names_path(&recv[start..])
                }
            }
        })
    });
    writers
        || body.match_indices("writeFileSync(").any(|(i, _)| {
            let rest = &body[i + "writeFileSync(".len()..];
            names_path(rest.split([',', ')']).next().unwrap_or(""))
        })
}

/// Whether the clause runs `sleep`, past the loop keywords the lexer leaves at
/// the front of a clause (`do sleep 1`).
fn runs_sleep(c: &Simple) -> bool {
    c.words
        .iter()
        .find(|w| w.quoted || !matches!(w.text.as_str(), "until" | "while" | "do" | "then" | "!"))
        .is_some_and(|w| !w.quoted && w.text == "sleep")
}

fn path_like_word(text: &str) -> bool {
    !text.is_empty() && !text.starts_with('-')
}

/// The answer to the `file-reread` question.
struct Reread {
    /// The session already has this file: a repeat of a whole read, or of the
    /// same window. True whatever `file-reread`'s stance, so a rule that
    /// yields to it does not start speaking when it observes.
    seen: bool,
    /// What to say, when the rule may speak.
    text: Option<String>,
    /// The stance the verdict was reached under (capped to advise for a read
    /// that is not an exact window).
    stance: Stance,
}

/// The `file-reread` question, asked and journalled the same way for a Read
/// and for a `cat`. `text` is `Some` when the session already has this file
/// and the rule may speak; the journal records the watched case too.
fn reread_verdict(
    session: &str,
    agent_id: Option<&str>,
    path: &std::path::Path,
    window: &str,
    mode: &str,
    cwd: &std::path::Path,
    shown: &str,
) -> Reread {
    let unseen = Reread {
        seen: false,
        text: None,
        stance: Stance::Observe,
    };
    // An agent id that cannot be a key: no record, no deny, no advice.
    let Some(agent) = crate::lsp::agent_key(agent_id) else {
        return unseen;
    };
    let Some(prior) = crate::session_state::prior(session, &agent, path, window) else {
        return unseen;
    };
    // A different window of a file read before is a different read; only a
    // certain record covering this exact window is a re-read, and a read that
    // is not an exact window can only be advised about.
    let (seen, exact) = match prior {
        crate::session_state::Prior::Covered(s) => (s, true),
        crate::session_state::Prior::Partial(s) => (s, false),
    };
    let rule = &rules::file_reread::RULE;
    let stance = crate::stance::resolve(rule);
    let stance = if exact {
        stance
    } else {
        stance.min(Stance::Advise)
    };
    let (reason, remedy) = rules::file_reread::phrase(shown, &seen);
    let outcome = match stance {
        Stance::Observe => "watched",
        Stance::Advise => "advised",
        Stance::Deny => "denied",
    };
    journal::record(&journal::Entry {
        rule: rule.id,
        stance: stance.as_str(),
        outcome,
        session,
        repo: &repo_name(cwd),
        mode,
        excerpt: shown,
    });
    Reread {
        seen: true,
        text: match stance {
            Stance::Observe => None,
            Stance::Advise => Some(decision::phrase(rule.id, &reason, &remedy)),
            Stance::Deny => Some(decision::phrase(rule.id, &reason, &remedy)),
        },
        stance,
    }
}

fn note_file(rule: &Rule, stance: Stance, op: &crate::payload::FileOp, shown: &str) {
    journal::record(&journal::Entry {
        rule: rule.id,
        stance: stance.as_str(),
        outcome: match stance {
            Stance::Observe => "watched",
            Stance::Advise => "advised",
            Stance::Deny => "denied",
        },
        session: &op.session,
        repo: &repo_name(&op.cwd),
        mode: &op.permission_mode,
        excerpt: shown,
    });
}

fn resolve_path(cwd: &std::path::Path, text: &str) -> std::path::PathBuf {
    if text.starts_with('/') {
        std::path::PathBuf::from(text)
    } else if let Some(rest) = text.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h).join(rest))
            .unwrap_or_else(|| cwd.join(text))
    } else {
        cwd.join(text)
    }
}

/// What a confirmed finding carries into the decision: a stance floor, and
/// for a rule that learned its reason by looking, the reason and the journal
/// excerpt to use instead of the finding's.
struct Confirmation {
    floor: Option<Stance>,
    ceiling: Option<Stance>,
    said: Option<(String, String, Option<String>)>,
}

/// Rules whose `confirm` reads nothing from the working directory, so a
/// session left in a removed worktree is judged like any other. Next to the
/// publication gates in [`unknown_directory`], which need one.
const NEEDS_NO_DIRECTORY: [&str; 1] = [rules::publish_without_skill::RULE.id];

fn confirmed(
    rule: &Rule,
    stance: Stance,
    finding: &Finding,
    bash: &Bash,
    parsed: &Parsed,
) -> Result<Confirmation, &'static str> {
    let Some(confirm) = rule.confirm else {
        return Ok(Confirmation {
            floor: None,
            ceiling: None,
            said: None,
        });
    };
    let ctx = Context {
        platform: Some(crate::rules::Platform::host()),
        cwd: &bash.cwd,
        parsed,
        background: bash.background,
        timeout_ms: bash.timeout_ms,
        tool_use_id: &bash.tool_use_id,
        transcript: bash.transcript.as_deref(),
        agent_transcript: bash.agent_transcript.as_deref(),
        command: &bash.command,
        output: None,
        session: &bash.session,
        failed: false,
    };
    // The directory that matters is the one the matched clause runs in,
    // `cd`s followed — not the session's. A session left in a removed
    // worktree still runs `cd /abs/repo && git push …` in a real repository,
    // and that push must be judged (#67).
    if !ctx.cwd_at(finding.span.start).is_dir() && !NEEDS_NO_DIRECTORY.contains(&rule.id) {
        return unknown_directory(rule, stance, finding, bash);
    }
    match confirm(&ctx, finding) {
        Confirmed::Yes => Ok(Confirmation {
            floor: None,
            ceiling: None,
            said: None,
        }),
        Confirmed::YesAt(floor) => Ok(Confirmation {
            floor: Some(floor),
            ceiling: None,
            said: None,
        }),
        Confirmed::YesSaying {
            floor,
            ceiling,
            reason,
            excerpt,
            remedy,
        } => Ok(Confirmation {
            floor,
            ceiling,
            said: Some((reason, excerpt, remedy)),
        }),
        Confirmed::No(why) => Err(why),
    }
}

/// The matched clause runs in a directory that does not exist. Most rules
/// have nothing to look at and decline. The publication gates cannot: when
/// the session's directory is gone the shell does not run there either —
/// Claude Code resets it to the project root — so a push with no `cd` of
/// its own publishes from a repository this hook cannot name. Under deny
/// that is held, like any other push shape the gate cannot read.
fn unknown_directory(
    rule: &Rule,
    stance: Stance,
    finding: &Finding,
    bash: &Bash,
) -> Result<Confirmation, &'static str> {
    const WHY: &str = "the working directory does not exist";
    let gates = [
        crate::push_preview::RULE.id,
        crate::implementation_review_rule::RULE.id,
    ];
    if stance != Stance::Deny || !gates.contains(&rule.id) {
        return Err(WHY);
    }
    let excerpt = crate::backtest::excerpt(&bash.command, finding.span.start, finding.span.end);
    Ok(Confirmation {
        floor: None,
        ceiling: None,
        said: Some((
            "This push runs in a directory that does not exist, so the repository it \
             publishes from cannot be named."
                .to_string(),
            excerpt,
            Some(
                "Start the command with `cd <repository> &&`, or push with \
                 `git -C <repository> push …`."
                    .to_string(),
            ),
        )),
    })
}

fn note(rule: &Rule, stance: &str, outcome: &str, bash: &Bash, finding: &Finding) {
    note_with(rule, stance, outcome, bash, finding, None);
}

/// [`note`], with the journal excerpt a `confirm` chose in place of the
/// command span.
fn note_with(
    rule: &Rule,
    stance: &str,
    outcome: &str,
    bash: &Bash,
    finding: &Finding,
    excerpt: Option<&str>,
) {
    let span = crate::backtest::excerpt(&bash.command, finding.span.start, finding.span.end);
    let excerpt = excerpt.unwrap_or(&span);
    // A rule built on the shell analysis judged the command under a dialect
    // the replay cannot recover from a transcript, so the record carries it:
    // `bypassPermissions+zsh`. Folded into the mode token, which nothing
    // parses, so the journal format is unchanged.
    let mode = match rule.examine {
        rules::Examine::Analysis(_) => format!(
            "{}+{}",
            bash.permission_mode,
            rules::tool_shell::dialect().as_str()
        ),
        rules::Examine::Legacy(_) => bash.permission_mode.clone(),
    };
    journal::record(&journal::Entry {
        rule: rule.id,
        stance,
        outcome,
        session: &bash.session,
        repo: &attributed_repo(rule.id, bash),
        mode: &mode,
        excerpt,
    });
}

/// The repository a journal line names. For the preview gate's push records
/// that is the repository the push runs in — `cd`s and `-C` followed — not
/// the session's: a session in one worktree pushing another must not log
/// the push under the first one's name.
fn attributed_repo(id: &str, bash: &Bash) -> String {
    let pushes = [
        crate::push_preview::RULE.id,
        crate::implementation_review_rule::RULE.id,
        crate::assertions::push_published::ASSERTION.id,
        crate::assertions::land_retry::ASSERTION.id,
    ];
    if !pushes.contains(&id) {
        return repo_name(&bash.cwd);
    }
    let parsed = shell::lex(&bash.command);
    let resolved = crate::push_target::find(&parsed).and_then(|cmd| {
        let ctx = Context {
            platform: Some(crate::rules::Platform::host()),
            cwd: &bash.cwd,
            parsed: &parsed,
            background: bash.background,
            timeout_ms: bash.timeout_ms,
            tool_use_id: &bash.tool_use_id,
            transcript: bash.transcript.as_deref(),
            agent_transcript: bash.agent_transcript.as_deref(),
            command: &bash.command,
            output: None,
            session: &bash.session,
            failed: false,
        };
        crate::push_target::repository(&ctx.cwd_at(cmd.at), cmd)
    });
    match resolved.as_deref().and_then(std::path::Path::file_name) {
        Some(n) => n.to_string_lossy().into_owned(),
        None => repo_name(&bash.cwd),
    }
}

/// The basename of the repository, not its path. Enough to group firings by
/// project without writing `/Users/<name>/…` into a log file.
fn repo_name(cwd: &std::path::Path) -> String {
    let mut dir = cwd;
    loop {
        if dir.join(".git").exists() {
            break;
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => break,
        }
    }
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".to_string())
}

/// One line, rewritten by rename so it is always current and always whole.
/// `doctor` compares it against the newest transcript timestamp: transcripts
/// prove sessions happened, this proves the guard ran in one.
fn heartbeat() {
    let Some(dir) = journal::dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    journal::private(&dir, 0o700);
    let tmp = dir.join("heartbeat.new");
    if std::fs::write(&tmp, format!("{now} {}\n", env!("CARGO_PKG_VERSION"))).is_ok() {
        // Narrowed BEFORE the rename, so no window exists in which the
        // finished file is readable by anyone but its owner. It carries only
        // a timestamp and a version, but it sits in the same directory as the
        // journal and there is no reason for the two to disagree about who
        // may read them.
        journal::private(&tmp, 0o600);
        let _ = std::fs::rename(&tmp, dir.join("heartbeat"));
    }
}

#[cfg(test)]
mod write_tests {
    use super::*;

    fn text(d: Decision) -> (&'static str, String) {
        match d {
            Decision::Silent => ("silent", String::new()),
            Decision::Advise(t) => ("advise", t),
            Decision::Deny(t) => ("deny", t),
            Decision::Ask(t) => ("ask", t),
            Decision::Context(t)
            | Decision::Assert(t)
            | Decision::AssertAfterFailure(t)
            | Decision::Continue(t)
            | Decision::UserNote(t) => ("other", t),
        }
    }

    #[test]
    fn silence_stays_silent() {
        assert_eq!(
            text(combine([Decision::Silent, Decision::Silent])).0,
            "silent"
        );
    }

    #[test]
    fn advice_is_joined_with_a_blank_line() {
        let (kind, t) = text(combine([
            Decision::Advise("one".into()),
            Decision::Silent,
            Decision::Advise("two".into()),
        ]));
        assert_eq!((kind, t.as_str()), ("advise", "one\n\ntwo"));
    }

    #[test]
    fn any_deny_denies_and_carries_every_text() {
        let (kind, t) = text(combine([
            Decision::Advise("one".into()),
            Decision::Deny("two".into()),
            Decision::Advise("three".into()),
        ]));
        assert_eq!((kind, t.as_str()), ("deny", "one\n\ntwo\n\nthree"));
    }

    #[test]
    fn an_ask_beats_advice_and_carries_every_text() {
        let (kind, t) = text(combine([
            Decision::Advise("one".into()),
            Decision::Ask("two".into()),
        ]));
        assert_eq!((kind, t.as_str()), ("ask", "one\n\ntwo"));
    }

    #[test]
    fn a_deny_beats_an_ask() {
        let (kind, t) = text(combine([
            Decision::Ask("one".into()),
            Decision::Deny("two".into()),
        ]));
        assert_eq!((kind, t.as_str()), ("deny", "one\n\ntwo"));
    }

    #[test]
    fn text_from_another_event_is_kept_as_advice() {
        let (kind, t) = text(combine([Decision::Context("one".into())]));
        assert_eq!((kind, t.as_str()), ("advise", "one"));
    }
}
