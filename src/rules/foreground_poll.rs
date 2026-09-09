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
    // Indexed over ALL clauses so `head` and the `skip`s below stay true to
    // what ran; an unreadable clause is simply never the head of a loop we
    // claim to have understood.
    let clauses = parsed.clauses();
    // `gh run watch` polls on its own, for as long as the run takes.
    for cmd in parsed.judgeable() {
        if cmd.program() == Some("gh")
            && cmd.subcommand() == Some("run")
            && cmd.operands().get(1).is_some_and(|w| w.text == "watch")
        {
            return Some(finding(cmd.at, cmd.end));
        }
    }
    let head = clauses
        .iter()
        .position(|c| c.opaque.is_none() && c.program().is_some_and(|p| LOOPS.contains(&p)))?;
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

/// Seconds a `sleep` word means: `30`, `0.5`, `2m`.
fn seconds(t: &str) -> Option<u64> {
    let (body, unit) = match t.chars().last() {
        Some(u @ ('s' | 'm' | 'h' | 'd')) => (&t[..t.len() - 1], u),
        _ => (t, 's'),
    };
    let n: f64 = body.parse().ok()?;
    let mult = match unit {
        'm' => 60.0,
        'h' => 3600.0,
        'd' => 86400.0,
        _ => 1.0,
    };
    Some((n * mult).ceil() as u64)
}

/// The longest the loop can run on its own terms, in seconds, or `None`
/// when nothing bounds it (`while true`, `until <condition>` with no
/// counter, `gh run watch`).
///
/// Read from the raw text: `$(seq 1 36)` is a substitution the lexer blanks
/// in `text`, and the count is exactly what is wanted here.
pub fn budget(parsed: &Parsed) -> Option<u64> {
    let clauses = parsed.clauses();
    let raw: String = clauses
        .iter()
        .flat_map(|c| c.words.iter().map(|w| w.raw.as_str()))
        .collect::<Vec<_>>()
        .join(" ");
    // A deadline in seconds beats a counter: `end=$((SECONDS+540))`.
    if let Some(i) = raw.find("SECONDS+") {
        let digits: String = raw[i + 8..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(n) = digits.parse::<u64>() {
            return Some(n);
        }
    }
    if let Some(i) = raw.find("--timeout=") {
        let t: String = raw[i + 10..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.')
            .collect();
        if let Some(n) = seconds(&t) {
            return Some(n);
        }
    }
    let sleep = clauses.iter().find(|c| is_sleep(c)).and_then(|c| {
        c.words
            .iter()
            .filter(|w| !w.quoted)
            .map(|w| w.text.as_str())
            .skip_while(|t| *t != "sleep")
            .nth(1)
            .and_then(seconds)
    })?;
    let iterations = count(&raw)?;
    Some(iterations.saturating_mul(sleep))
}

/// How many times the loop can go round: `seq 1 36`, `seq 40`, `{1..20}`,
/// `-lt 40`, `-ge 16`, `-le 30`.
fn count(raw: &str) -> Option<u64> {
    let mut words = raw.split_whitespace().peekable();
    while let Some(w) = words.next() {
        let number = |s: &str| s.trim_end_matches([')', ']', ';']).parse::<u64>().ok();
        match w.trim_start_matches("$(") {
            "seq" => {
                let a = words.next().and_then(number)?;
                return match words.peek().and_then(|b| number(b)) {
                    Some(b) => Some(b.saturating_sub(a) + 1),
                    None => Some(a),
                };
            }
            "-lt" | "-le" | "-ge" | "-gt" => {
                let n = words.next().and_then(number)?;
                return Some(if w == "-le" || w == "-gt" { n + 1 } else { n });
            }
            t if t.starts_with('{') && t.contains("..") => {
                let inner = t.trim_matches(['{', '}']);
                let (a, b) = inner.split_once("..")?;
                return Some(b.parse::<u64>().ok()?.saturating_sub(a.parse().ok()?) + 1);
            }
            _ => {}
        }
    }
    None
}

fn confirm(ctx: &Context, _f: &Finding) -> Confirmed {
    if ctx.background {
        return Confirmed::No("the call already runs in the background");
    }
    match budget(ctx.parsed) {
        Some(secs) if secs * 1000 <= ctx.timeout_ms() => {
            Confirmed::No("the loop's own budget fits inside the call's timeout")
        }
        _ => Confirmed::Yes,
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
            timeout_ms: None,
        };
        assert!(matches!(confirm(&ctx, &f), Confirmed::No(_)));
        let ctx = Context {
            background: false,
            timeout_ms: None,
            ..ctx
        };
        assert!(matches!(confirm(&ctx, &f), Confirmed::Yes));
    }

    #[test]
    fn a_loop_that_fits_its_timeout_is_not_confirmed() {
        let src =
            "for i in $(seq 1 6); do gh pr checks 1 | grep -q pending || break; sleep 10; done";
        let parsed = lex(src);
        assert_eq!(budget(&parsed), Some(60));
        let f = examine(&parsed).expect("fires");
        let ctx = Context {
            cwd: std::path::Path::new("/"),
            parsed: &parsed,
            background: false,
            timeout_ms: None,
        };
        assert!(matches!(confirm(&ctx, &f), Confirmed::No(_)));
        // Thirty-six rounds of fifteen seconds is nine minutes: over the
        // default two, under an explicit ten.
        let parsed = lex("for i in $(seq 1 36); do sleep 15; done");
        assert_eq!(budget(&parsed), Some(540));
        let f = examine(&parsed).unwrap();
        let ctx = Context {
            parsed: &parsed,
            ..ctx
        };
        assert!(matches!(confirm(&ctx, &f), Confirmed::Yes));
        let ctx = Context {
            timeout_ms: Some(600_000),
            ..ctx
        };
        assert!(matches!(confirm(&ctx, &f), Confirmed::No(_)));
    }

    #[test]
    fn the_budget_reads_counters_deadlines_and_nothing() {
        assert_eq!(
            budget(&lex(
                "i=0; until [ $i -ge 16 ]; do sleep 30; i=$((i+1)); done"
            )),
            Some(480)
        );
        assert_eq!(
            budget(&lex("while [ $i -lt 40 ]; do sleep 20; done")),
            Some(800)
        );
        // `{1..20}` is grouping to the lexer and never reaches a word; the
        // loop reads as unbounded, which errs toward speaking.
        assert_eq!(budget(&lex("for i in {1..20}; do sleep 5; done")), None);
        assert_eq!(
            budget(&lex("end=$((SECONDS+540)); until x; do sleep 5; done")),
            Some(540)
        );
        assert_eq!(
            budget(&lex(
                "kubectl wait --for=condition=Ready pod/x --timeout=180s"
            )),
            Some(180)
        );
        assert_eq!(budget(&lex("while true; do sleep 5; done")), None);
        assert_eq!(
            budget(&lex("until grep -q done f; do sleep 30; done")),
            None
        );
        assert_eq!(budget(&lex("gh run watch 1 --exit-status")), None);
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
