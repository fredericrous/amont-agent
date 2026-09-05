//! `foreground-poll` — a wait that the tool's own clock will cut short.
//!
//! The Bash tool kills a foreground command at its timeout — ten minutes by
//! default — and reports nothing but the kill. A polling loop (`until … do
//! sleep 30; done`, `gh run watch`) written to outlast a CI run is exactly
//! the shape that runs into it: the loop pays the whole wait, is killed one
//! poll short of the answer, and the model starts another. Measured over
//! forty-two sessions: 552 polling loops, 265 of them in the foreground, 96
//! commands killed at the ten-minute cap — sixteen hours of waiting for a
//! kill.
//!
//! The failure is quiet in the sense that matters: nothing in the kill says
//! "this should have run in the background", so no correcting loop forms.
//! The remedy costs nothing — the same loop with `run_in_background: true`
//! delivers one notification when it exits, and the tool's clock stops
//! applying.
//!
//! ## `confirm` reads the flag, not the world
//!
//! `examine` sees only the command text; whether the call runs in the
//! background is a sibling field of the payload. `confirm` reads it from the
//! [`Context`] and stays silent for a loop that is already detached. The
//! backtester never runs `confirm`, so a replayed rate counts background
//! loops too — an overcount, documented here rather than hidden.

use crate::rules::{Confirmed, Context, Evidence, Finding, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "foreground-poll",
    // Advises from the start: it refuses nothing, and the failure it names
    // is a ten-minute wait that ends in a kill nobody explains.
    default_stance: Stance::Advise,
    evidence: Evidence {
        per_1000: 13.6,
        measured: "2026-09-05",
        trend: Trend::Flat(8),
    },
    examine,
    confirm: Some(confirm),
};

/// Shell loop keywords that open a polling loop.
const LOOPS: &[&str] = &["until", "while", "for"];

fn is_sleep(cmd: &Simple) -> bool {
    // `do sleep 30` lexes as a clause whose first word is `do`; a bare
    // `sleep 30` is its own clause.
    let mut words = cmd
        .words
        .iter()
        .filter(|w| !w.quoted)
        .map(|w| w.text.as_str());
    match words.next() {
        Some("sleep") => true,
        Some("do") => words.next() == Some("sleep"),
        _ => false,
    }
}

fn examine(parsed: &Parsed) -> Option<Finding> {
    let clauses = parsed.clauses();
    // `gh run watch` polls on its own, for as long as the run takes.
    for cmd in clauses {
        if cmd.program() == Some("gh")
            && cmd.subcommand() == Some("run")
            && cmd.operands().get(1).is_some_and(|w| w.text == "watch")
        {
            return Some(finding(cmd.at, cmd.end));
        }
    }
    let head = clauses
        .iter()
        .position(|c| c.program().is_some_and(|p| LOOPS.contains(&p)))?;
    // The loop body must actually sleep: `for f in *; do echo $f; done` is
    // not a wait, it is a loop.
    let sleep = clauses.iter().skip(head + 1).find(|c| is_sleep(c))?;
    let end = clauses
        .iter()
        .skip(head + 1)
        .find(|c| {
            c.words
                .first()
                .is_some_and(|w| !w.quoted && w.text == "done")
        })
        .map(|c| c.end)
        .unwrap_or(sleep.end);
    Some(finding(clauses[head].at, end))
}

fn finding(at: usize, end: usize) -> Finding {
    Finding {
        reason: "the Bash tool kills a foreground command at its timeout (ten minutes by \
                 default) and reports only the kill; a polling loop or `gh run watch` \
                 written to outlast a CI run pays the whole wait and is cut off one poll \
                 short of the answer — measured: 96 commands killed at the cap."
            .to_string(),
        remedy: "Run the same wait with `run_in_background: true` (one notification when \
                 it exits, no clock), or use the harness's own completion notice for work \
                 it started. Keep a foreground command well inside the timeout."
            .to_string(),
        span: at..end,
    }
}

fn confirm(ctx: &Context, _f: &Finding) -> Confirmed {
    if ctx.background {
        Confirmed::No("the call already runs in the background")
    } else {
        Confirmed::Yes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::lex;

    fn fires(command: &str) -> bool {
        examine(&lex(command)).is_some()
    }

    #[test]
    fn a_polling_loop_has_the_shape() {
        assert!(fires(
            "until [ \"$(gh pr checks 247 2>/dev/null | grep -c pending)\" = \"0\" ]; do sleep 30; done; echo done"
        ));
        assert!(fires("while true; do sleep 5; done"));
        assert!(fires(
            "for i in $(seq 1 18); do n=$(gh pr checks 247 | grep -c pending); if [ \"$n\" = \"0\" ]; then break; fi; sleep 30; done"
        ));
        assert!(fires(
            "cd /repo && until grep -q '^exit=' push.log; do sleep 10; done; tail -3 push.log"
        ));
    }

    #[test]
    fn gh_run_watch_is_a_poll_too() {
        assert!(fires("gh run watch 33107191361 --exit-status"));
        assert!(fires(
            "cd ~/x && gh run watch 1 > /dev/null 2>&1; gh run view 1 --json conclusion"
        ));
    }

    #[test]
    fn a_loop_that_does_not_sleep_is_a_loop() {
        assert!(!fires("for f in a b c; do echo $f; done"));
        assert!(!fires("for w in x y; do kubectl get workflow $w; done"));
    }

    #[test]
    fn a_single_sleep_is_not_a_poll() {
        assert!(!fires("sleep 2; gh pr checks 229"));
        assert!(!fires("sleep 20 && gh run list --limit 1"));
    }

    #[test]
    fn a_background_call_is_not_confirmed() {
        let parsed = lex("while true; do sleep 5; done");
        let f = examine(&parsed).expect("fires");
        let ctx = Context {
            cwd: std::path::Path::new("/"),
            parsed: &parsed,
            background: true,
        };
        assert!(matches!(confirm(&ctx, &f), Confirmed::No(_)));
        let ctx = Context {
            background: false,
            ..ctx
        };
        assert!(matches!(confirm(&ctx, &f), Confirmed::Yes));
    }

    #[test]
    fn the_span_covers_the_loop() {
        let src = "git fetch -q; until [ -f done ]; do sleep 1; done; echo ok";
        let f = examine(&lex(src)).expect("fires");
        // A clause's end includes the separator that closes it.
        assert_eq!(
            src[f.span.clone()].trim().trim_end_matches(';'),
            "until [ -f done ]; do sleep 1; done"
        );
    }
}
