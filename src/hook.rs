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
use crate::shell::{self, Parsed};

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
    match payload::parse(raw) {
        Event::SessionStart(session) => {
            // The fact that we ran is what `doctor` uses to tell "no rule
            // fired" apart from "the guard is dead". Written first, so a slow
            // fetch below can never cost the heartbeat.
            heartbeat();
            crate::session_state::sweep();
            crate::preview::sweep();
            crate::plan_phases::sweep();
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
            let stance = crate::stance::resolve(&rules::push_preview::RULE);
            match crate::preview::on_pre_ask(&ask, stance) {
                Some(why) => {
                    Decision::Deny(decision::phrase(rules::push_preview::RULE.id, &why, ""))
                }
                None => Decision::Silent,
            }
        }
        Event::PrePlanExit(plan) => {
            let stance = crate::stance::resolve(&rules::plan_review_panel::RULE);
            crate::plan_review::on_plan_exit(&plan, stance)
        }
        Event::PostAsk(ask) => {
            let mut said: Vec<String> = Vec::new();
            if let Some(text) = crate::preview::on_post_ask(&ask) {
                said.push(decision::phrase(rules::push_preview::RULE.id, &text, ""));
            }
            if let Some(text) = crate::implementation_review::on_post_ask(&ask) {
                if crate::stance::resolve(&rules::implementation_review::RULE) != Stance::Observe {
                    said.push(decision::phrase(
                        rules::implementation_review::RULE.id,
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
        Event::PostFile(op) => on_post_file(&op),
        Event::PreBash(bash) => on_bash(&bash),
        Event::PostBash(bash) => on_post_bash(&bash),
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
        let rule = &rules::implementation_review::RULE;
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
    // No early return on `Parsed::Opaque`: `rules::evaluate` owns that
    // policy, so the hook, `check` and the backtester cannot drift apart on
    // it. Legacy rules are skipped on an unreadable command exactly as they
    // were; a rule built on the shell analysis judges it with its own account
    // of what it could not read.
    let input = rules::Input::new(&bash.command, rules::tool_shell::dialect(), &parsed);
    let fired = rules::evaluate(&input);
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
                cwd: &bash.cwd,
                parsed: &parsed,
                background: bash.background,
                timeout_ms: bash.timeout_ms,
                tool_use_id: &bash.tool_use_id,
                transcript: bash.transcript.as_deref(),
                agent_transcript: bash.agent_transcript.as_deref(),
            };
            let path = resolve_path(&ctx.cwd_at(d.at), &d.path);
            let window = dump_window(&d.extent);
            if let Some(text) = reread_verdict(
                &bash.session,
                &path,
                &window,
                &bash.permission_mode,
                &bash.cwd,
                &d.path,
            )
            .text
            {
                match crate::stance::resolve(&rules::file_reread::RULE) {
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
        .map(|t| decision::phrase(rules::push_preview::RULE.id, &t, ""));
    if bash.background {
        // Detached: the tool call returned a task id, not a result. Whatever it
        // claimed has not finished happening.
        return unbound.map_or(Decision::Silent, Decision::Assert);
    }

    let ctx = Context {
        cwd: &bash.cwd,
        parsed: &parsed,
        background: bash.background,
        timeout_ms: bash.timeout_ms,
        tool_use_id: &bash.tool_use_id,
        transcript: bash.transcript.as_deref(),
        agent_transcript: bash.agent_transcript.as_deref(),
    };

    // A `cat`-shaped dump that has now run is a read the session should
    // remember. It needs the whole command: a path recorded from a half-read
    // line would have `file-reread` advise, later, about a file the session
    // may never have read.
    if parsed.fully_read() {
        for d in rules::dump::dumps(&parsed) {
            let path = resolve_path(&ctx.cwd_at(d.at), &d.path);
            crate::session_state::record(&bash.session, "read", &path, &dump_window(&d.extent));
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
            let rule = &rules::implementation_review::RULE;
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
        crate::session_state::record(&op.session, "write", &op.path, "full");
        return lint_suppression(op);
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
        &op.path,
        &op.window,
        &op.permission_mode,
        &op.cwd,
        &shown,
    );
    if let Some(text) = reread.text {
        match crate::stance::resolve(&rules::file_reread::RULE) {
            Stance::Deny => deny.push(text),
            Stance::Advise => advise.push(text),
            Stance::Observe => {}
        }
    }
    // Last, and quiet on a file the session has seen whatever `file-reread`'s
    // stance: its remedy already says to use `offset`/`limit`.
    if !persisted && !reread.seen {
        if let Some(bytes) = rules::read_unbounded_large::applies(&op.path, &op.window) {
            let rule = &rules::read_unbounded_large::RULE;
            let stance = crate::stance::resolve(rule);
            let (reason, remedy) = rules::read_unbounded_large::phrase(&shown, bytes);
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

/// A Read that has run and succeeded: the file is in context now, so this is
/// the moment to remember it. Recording before the call did the wrong thing
/// twice over — a Read refused above, or one that failed on a path that is
/// not there, was still on record, and a retry was told its contents were
/// already in context.
fn on_post_file(op: &crate::payload::FileOp) -> Decision {
    if !op.writes {
        crate::session_state::record(&op.session, "read", &op.path, &op.window);
    }
    Decision::Silent
}

/// The window a `cat`-shaped dump covers, in the Read tool's own spelling.
fn dump_window(extent: &rules::dump::Extent) -> String {
    match extent {
        rules::dump::Extent::Whole => "full".to_string(),
        rules::dump::Extent::Lines(n) => format!("0:{n}"),
        rules::dump::Extent::Bytes(n) => format!("bytes:{n}"),
    }
}

/// The answer to the `file-reread` question.
struct Reread {
    /// The session already has this file: a repeat of a whole read, or of the
    /// same window. True whatever `file-reread`'s stance, so a rule that
    /// yields to it does not start speaking when it observes.
    seen: bool,
    /// What to say, when the rule may speak.
    text: Option<String>,
}

/// The `file-reread` question, asked and journalled the same way for a Read
/// and for a `cat`. `text` is `Some` when the session already has this file
/// and the rule may speak; the journal records the watched case too.
fn reread_verdict(
    session: &str,
    path: &std::path::Path,
    window: &str,
    mode: &str,
    cwd: &std::path::Path,
    shown: &str,
) -> Reread {
    let unseen = Reread {
        seen: false,
        text: None,
    };
    let Some(seen) = crate::session_state::last_read(session, path) else {
        return unseen;
    };
    // A different window of a file read before is a different read; only a
    // repeat of a whole read, or of the same window, is a re-read.
    if seen.window != "full" && seen.window != window {
        return unseen;
    }
    let rule = &rules::file_reread::RULE;
    let stance = crate::stance::resolve(rule);
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
            _ => Some(decision::phrase(rule.id, &reason, &remedy)),
        },
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
        cwd: &bash.cwd,
        parsed,
        background: bash.background,
        timeout_ms: bash.timeout_ms,
        tool_use_id: &bash.tool_use_id,
        transcript: bash.transcript.as_deref(),
        agent_transcript: bash.agent_transcript.as_deref(),
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
        rules::push_preview::RULE.id,
        rules::implementation_review::RULE.id,
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
        rules::push_preview::RULE.id,
        rules::implementation_review::RULE.id,
        crate::assertions::push_published::ASSERTION.id,
    ];
    if !pushes.contains(&id) {
        return repo_name(&bash.cwd);
    }
    let parsed = shell::lex(&bash.command);
    let resolved = crate::push_target::find(&parsed).and_then(|cmd| {
        let ctx = Context {
            cwd: &bash.cwd,
            parsed: &parsed,
            background: bash.background,
            timeout_ms: bash.timeout_ms,
            tool_use_id: &bash.tool_use_id,
            transcript: bash.transcript.as_deref(),
            agent_transcript: bash.agent_transcript.as_deref(),
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
