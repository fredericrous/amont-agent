//! `test-output-trimmed` — a test, lint or typecheck runner piped into
//! `tail`/`head`/`grep` with `pipefail` off.
//!
//! ```sh
//! cargo test 2>&1 | tail -20      # exits 0 when a test failed
//! ```
//!
//! A pipeline's status is its last command's, so the trimmed run reports
//! success whatever the runner did, which breaks any chain such as
//! `cargo test | tail && git push`, and `tail` may cut the failure off.
//! Measured over 30 days of transcripts: 76% of test/lint runs were trimmed
//! this way, and a quarter of those printed failure text yet returned success.
//!
//! Whether `pipefail` is on comes from the shell analysis as a yes/no/maybe
//! state, so `set -o pipefail; set +o pipefail`, a conditional
//! `true || set -o pipefail`, and zsh `setopt pipefail` are all read as the
//! shell would. Only a definite yes exempts.

use std::ops::Range;

use crate::rules::pipe_to_tail::{pipefail_is_on, pipeline_sink};
use crate::rules::{Evidence, Examine, Finding, Input, Rule, Stance, Trend};
use crate::shell::{Parsed, Simple};

pub const RULE: Rule = Rule {
    id: "test-output-trimmed",
    default_stance: Stance::Advise,
    max_stance: Stance::Deny,
    evidence: Evidence {
        per_1000: EVIDENCE_PER_1000,
        measured: EVIDENCE_MEASURED,
        trend: Trend::Flat(4),
    },
    examine: Examine::Analysis(examine),
    confirm: None,
};

const EVIDENCE_PER_1000: f32 = 33.7;
const EVIDENCE_MEASURED: &str = "2026-10-11";

const TRIMMING: &[&str] = &["tail", "head", "grep"];

/// Arguments that make the run not a real test run.
const EXEMPT_ARGS: &[&str] = &["--help", "-h", "--version", "--list", "--no-run"];

/// Package-manager scripts that run a check.
const SCRIPTS: &[&str] = &["test", "lint", "check", "typecheck", "e2e"];

fn is_runner(cmd: &Simple) -> bool {
    let Some(program) = cmd.program() else {
        return false;
    };
    let args: Vec<&str> = cmd
        .args()
        .iter()
        .filter(|w| !w.quoted)
        .map(|w| w.text.as_str())
        .collect();
    let first_operand = args.iter().copied().find(|a| !a.starts_with('-'));
    match program {
        "cargo" => matches!(first_operand, Some("test" | "nextest" | "clippy")),
        "go" => matches!(first_operand, Some("test" | "vet")),
        "pytest" | "vitest" | "jest" | "tsc" | "ruff" | "mypy" | "pyright" | "eslint" | "tox" => {
            true
        }
        "python" | "python3" => args.windows(2).any(|w| w[0] == "-m" && w[1] == "pytest"),
        "playwright" => first_operand == Some("test"),
        "npm" | "pnpm" | "yarn" | "bun" => {
            let mut it = args.iter().copied().filter(|a| !a.starts_with('-'));
            let first = it.next();
            let script = if first == Some("run") {
                it.next()
            } else {
                first
            };
            script.is_some_and(|s| SCRIPTS.contains(&s))
        }
        "make" => matches!(first_operand, Some("test" | "check" | "lint")),
        "just" => matches!(first_operand, Some("test" | "check")),
        _ => false,
    }
}

fn exempt(cmd: &Simple) -> bool {
    cmd.args()
        .iter()
        .any(|w| !w.quoted && EXEMPT_ARGS.contains(&w.text.as_str()))
}

fn describe(cmd: &Simple) -> String {
    let mut words: Vec<&str> = Vec::new();
    if let Some(p) = cmd.program() {
        words.push(p);
    }
    for w in cmd.args().iter().filter(|w| !w.quoted).take(2) {
        if w.text.starts_with('-') {
            break;
        }
        words.push(&w.text);
    }
    words.join(" ")
}

fn examine(input: &Input) -> Option<Finding> {
    let parsed = match input.legacy {
        Parsed::Opaque(_) => return None,
        parsed => parsed,
    };
    let clauses = parsed.clauses();
    for (i, cmd) in clauses.iter().enumerate() {
        if cmd.opaque.is_some() || !cmd.next.is_some_and(|c| c.is_pipe()) {
            continue;
        }
        if !is_runner(cmd) || exempt(cmd) {
            continue;
        }
        let Some(sink) = pipeline_sink(clauses, i) else {
            continue;
        };
        let Some(sink_program) = sink.program() else {
            continue;
        };
        if !TRIMMING.contains(&sink_program) {
            continue;
        }
        if pipefail_is_on(input, cmd) {
            continue;
        }
        let verb = describe(cmd);
        return Some(Finding {
            reason: format!(
                "`{verb}` pipes into `{sink_program}`, so the pipeline reports \
                 {sink_program}'s exit status, not {verb}'s: a failing run exits 0, \
                 which breaks any `&&` chain after it, and the trimming can cut the \
                 failure off."
            ),
            remedy: format!(
                "Run a declared check as `amont run <check>`, or any other command as \
                 `amont-agent quiet -- {verb}`: one line on a pass, only the failure \
                 on a fail, the real exit code. Minimal fix: `set -o pipefail; {verb} \
                 | {sink_program}`."
            ),
            span: Range {
                start: cmd.at,
                end: sink.end,
            },
        });
    }
    None
}
