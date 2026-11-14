//! `cd-carries-over` — a Bash call that runs in a directory an earlier
//! sibling of its own parallel batch moved the shell to.
//!
//! Claude Code's Bash tool carries a successful top-level `cd` into the next
//! call, including the next call of the same parallel batch. A model that
//! writes `cd /repo && git status` believes the move is local to that command;
//! every later sibling then runs somewhere it did not expect.
//!
//! ## Why the rule fires on the sibling, not on the `cd`
//!
//! The first shape fired on the `cd` itself and matched about 424 per 1,000
//! Bash calls, mostly `cd`s that changed nothing (13,969 to the directory the
//! shell was already in) or that the harness resets (8,299 outside the working
//! directory). A carry into the NEXT TURN is announced to the model by the
//! harness ("Shell cwd was reset" / the new directory); a sibling in the same
//! batch is not, so that is the case nobody is told about.
//!
//! The rule fires on the call at risk: [`crate::batch_cwd`] records every
//! Bash call of a batch, and a later call whose shell directory
//! (`Context::cwd`) differs from the first call's was moved by an earlier
//! sibling. A directory only carries on success, so no result is read. A call
//! that itself starts with a top-level `cd` to an absolute path does not
//! depend on where the shell is.
//!
//! The batch is not read from the transcript: Claude Code writes a response's
//! calls and results there only after the whole batch has run (3.7.0 and
//! 3.7.1 read it, and never confirmed in a live session).
//!
//! Everything unknown is silence: no transcript, no `api-request` line, an
//! unusable session id, a state file that cannot be read.
//!
//! ## Measured 2026-10-27
//!
//! Of 92,286 Bash calls over 60 days, 39,084 had a top-level `cd`, 16,816 of
//! those actually carried, and 636 (about 7 per 1,000 Bash calls) had a later
//! sibling in the same batch.

use crate::batch_cwd;
use crate::rules::{Confirmed, Context, Evidence, Examine, Finding, Input, Rule, Stance, Trend};
use crate::shell::Parsed;

pub const RULE: Rule = Rule {
    id: "cd-carries-over",
    prevents: "A sibling Bash call of a parallel batch that runs in the directory an earlier sibling's `cd` carried the shell to",
    irreversible: false,
    default_stance: Stance::Advise,
    max_stance: Stance::Advise,
    evidence: Evidence {
        // 636 siblings of a carried `cd` in 92,286 Bash calls, 2026-10-27.
        per_1000: 7.0,
        measured: "2026-10-27",
        trend: Trend::Rare,
    },
    examine: Examine::Live(examine),
    confirm: Some(confirm),
};

/// Directories outside every working directory, where the harness resets the
/// cwd after the call.
const RESET_PREFIXES: &[&str] = &[
    "/tmp",
    "/private/tmp",
    "/var/folders",
    "/private/var/folders",
];

const REMEDY: &str = "Scope the move: a subshell `(cd X && …)`, `git -C X …`, or absolute paths.";

/// A top-level directory change in a command.
struct TopLevelCd {
    /// The clause as written.
    text: String,
}

/// Whether the call is at risk is only known from the batch it belongs to, and
/// every call must be recorded in that batch, so `examine` lets every command
/// through to `confirm`.
fn examine(input: &Input) -> Option<Finding> {
    if input.src.trim().is_empty() {
        return None;
    }
    Some(Finding {
        reason: "An earlier call of this parallel batch may have moved the shell's directory."
            .to_string(),
        remedy: REMEDY.to_string(),
        span: 0..input.src.len(),
    })
}

fn confirm(ctx: &Context, finding: &Finding) -> Confirmed {
    let Some(path) = ctx.agent_transcript.or(ctx.transcript) else {
        return Confirmed::No("no transcript");
    };
    let Some(state) = batch_cwd::state_file(ctx.session, path) else {
        return Confirmed::No("no usable session");
    };
    decide(ctx, finding, path, &state)
}

/// [`confirm`] with the transcript and the state file named.
fn decide(
    ctx: &Context,
    finding: &Finding,
    path: &std::path::Path,
    state: &std::path::Path,
) -> Confirmed {
    let Some(key) = std::fs::File::open(path)
        .ok()
        .and_then(batch_cwd::batch_key)
    else {
        return Confirmed::No("no batch key in the transcript");
    };
    let cwd = ctx.cwd.to_string_lossy();
    let Some(start) = batch_cwd::record(state, key, &cwd, ctx.command) else {
        return Confirmed::No("the first call of its batch");
    };
    if ctx.cwd == std::path::Path::new(&start.cwd) {
        return Confirmed::No("the shell is where the batch started");
    }
    if starts_with_absolute_cd(ctx.command, ctx.parsed) {
        return Confirmed::No("the call names its own directory");
    }
    // The move itself is proven by the directory: a cwd only carries on
    // success. The clause is named when an earlier call shows one.
    let mover = start
        .earlier
        .iter()
        .rev()
        .find_map(|c| top_level_cd(c, &crate::shell::lex(c)).map(|cd| cd.text));
    let named = match &mover {
        Some(text) => format!("an earlier call of the same parallel batch ran `{text}`"),
        None => "an earlier call of the same parallel batch moved the shell".to_string(),
    };
    Confirmed::YesSaying {
        floor: None,
        ceiling: None,
        reason: format!(
            "This call runs in {} but its batch started in {}: {named}, the Bash \
             tool carried the move, and the harness does not announce a carry \
             between siblings. Paths and git commands here resolve against the \
             wrong directory.",
            ctx.cwd.display(),
            start.cwd,
        ),
        excerpt: format!(
            "batch-cwd={} cwd={} {}",
            start.cwd,
            ctx.cwd.display(),
            mover.unwrap_or_default()
        ),
        remedy: Some(finding.remedy.clone()),
    }
}

/// Whether the first clause is a top-level `cd`/`pushd` to an absolute path.
fn starts_with_absolute_cd(src: &str, parsed: &Parsed) -> bool {
    let Some(first) = parsed.judgeable().next() else {
        return false;
    };
    if first.nested.is_some() {
        return false;
    }
    let program = first.program();
    if program != Some("cd") && program != Some("pushd") {
        return false;
    }
    let program_at = first
        .words
        .iter()
        .find(|w| Some(w.text.as_str()) == program)
        .map_or(first.at, |w| w.at);
    !is_scoped(src, program_at)
        && first
            .operands()
            .first()
            .is_some_and(|t| t.text.starts_with('/'))
}

/// The first top-level `cd` or `pushd` in `src` that can carry.
fn top_level_cd(src: &str, parsed: &Parsed) -> Option<TopLevelCd> {
    for cmd in parsed.judgeable() {
        if cmd.nested.is_some() {
            continue;
        }
        let Some(program) = cmd.program() else {
            continue;
        };
        if program != "cd" && program != "pushd" {
            continue;
        }
        match cmd.operands().first() {
            // A bare `cd` goes to $HOME, which is not a working directory.
            None if program == "cd" => continue,
            Some(target) if is_reset_path(&target.text) => continue,
            _ => {}
        }
        // The clause's own start can sit before a grouping `(` the lexer
        // consumed, so the scope is read up to the program word itself.
        let program_at = cmd
            .words
            .iter()
            .find(|w| w.text == program)
            .map_or(cmd.at, |w| w.at);
        if is_scoped(src, program_at) {
            continue;
        }
        return Some(TopLevelCd {
            text: src
                .get(cmd.at..cmd.end)
                .unwrap_or(program)
                .trim()
                .to_string(),
        });
    }
    None
}

fn is_reset_path(target: &str) -> bool {
    RESET_PREFIXES
        .iter()
        .any(|p| target == *p || target.starts_with(&format!("{p}/")))
}

/// Whether the command at byte `at` sits inside a `( … )` group or a function
/// body, where its directory change stays.
fn is_scoped(src: &str, at: usize) -> bool {
    let bytes = src.as_bytes();
    let end = at.min(bytes.len());
    let mut parens = 0usize;
    // One entry per open `{` at word start: true for a function body.
    let mut braces: Vec<bool> = Vec::new();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < end {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == b'\\' && q == b'"' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'\\' => i += 1,
                b'\'' | b'"' => quote = Some(c),
                b'(' => parens += 1,
                b')' => parens = parens.saturating_sub(1),
                b'{' if starts_word(bytes, i) && ends_word(bytes, i) => {
                    braces.push(opens_function(&src[..i]));
                }
                b'}' if starts_word(bytes, i) && ends_word(bytes, i) => {
                    braces.pop();
                }
                _ => {}
            },
        }
        i += 1;
    }
    parens > 0 || braces.contains(&true)
}

fn starts_word(bytes: &[u8], i: usize) -> bool {
    i == 0
        || matches!(
            bytes[i - 1],
            b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'('
        )
}

fn ends_word(bytes: &[u8], i: usize) -> bool {
    i + 1 >= bytes.len() || matches!(bytes[i + 1], b' ' | b'\t' | b'\n' | b';')
}

/// `name() {` or `function name {`: the text before the brace says which.
fn opens_function(before: &str) -> bool {
    let trimmed = before.trim_end();
    if trimmed.ends_with(')') {
        return true;
    }
    trimmed
        .rsplit(|c: char| c.is_whitespace() || c == ';')
        .nth(1)
        .is_some_and(|word| word == "function")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Dialect;
    use crate::shell::lex;
    use std::path::{Path, PathBuf};

    /// A session: a transcript whose last `api-request` line is the batch
    /// key, and a state file, in a directory of its own.
    struct Session {
        dir: PathBuf,
        transcript: PathBuf,
        state: PathBuf,
    }

    impl Session {
        fn new() -> Session {
            // One directory per session: tests run in parallel.
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("amont-agent-cdco-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("dir");
            let s = Session {
                transcript: dir.join("t.jsonl"),
                state: dir.join("s.t.batch"),
                dir,
            };
            s.new_response();
            s
        }

        /// The model answers again: Claude Code writes the previous batch,
        /// then the new response's `api-request` line.
        fn new_response(&self) {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.transcript)
                .expect("open");
            writeln!(f, "{{\"type\":\"assistant\"}}").expect("write");
            writeln!(f, "{{\"type\":\"api-request\"}}").expect("write");
        }

        /// The hook's verdict on `command` with the shell in `cwd`.
        fn call(&self, command: &str, cwd: &str) -> Result<String, &'static str> {
            let parsed = lex(command);
            let finding = RULE
                .judge_live(&Input::new(command, Dialect::Zsh, &parsed))
                .ok_or("examine silent")?;
            let ctx = Context {
                platform: None,
                cwd: Path::new(cwd),
                parsed: &parsed,
                background: false,
                timeout_ms: None,
                tool_use_id: "",
                transcript: Some(&self.transcript),
                agent_transcript: None,
                command,
                output: None,
                session: "s",
                failed: false,
            };
            match decide(&ctx, &finding, &self.transcript, &self.state) {
                Confirmed::YesSaying { reason, .. } => Ok(reason),
                Confirmed::No(why) => Err(why),
                _ => Err("unexpected"),
            }
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    #[test]
    fn the_sibling_after_a_carried_cd_is_confirmed() {
        let s = Session::new();
        assert!(s.call("cd /w/relais && pwd", "/w").is_err());
        let reason = s.call("pwd", "/w/relais").expect("confirmed");
        assert!(reason.contains("runs in /w/relais"));
        assert!(reason.contains("started in /w"));
        assert!(reason.contains("cd /w/relais"));
    }

    #[test]
    fn the_next_response_is_a_new_batch() {
        let s = Session::new();
        s.call("cd /w/relais && pwd", "/w").ok();
        s.new_response();
        assert_eq!(
            s.call("pwd", "/w/relais"),
            Err("the first call of its batch")
        );
        assert_eq!(
            s.call("ls", "/w/relais"),
            Err("the shell is where the batch started")
        );
    }

    #[test]
    fn a_batch_that_did_not_move_is_silent() {
        let s = Session::new();
        s.call("git status", "/w").ok();
        assert!(s.call("(cd /w/relais && pwd)", "/w").is_err());
        assert_eq!(
            s.call("pwd", "/w"),
            Err("the shell is where the batch started")
        );
    }

    #[test]
    fn a_call_that_starts_with_an_absolute_cd_is_silent() {
        let s = Session::new();
        s.call("cd /w/relais && pwd", "/w").ok();
        assert_eq!(
            s.call("cd /w/other && pwd", "/w/relais"),
            Err("the call names its own directory")
        );
    }

    #[test]
    fn a_move_with_no_readable_cd_still_names_both_directories() {
        let s = Session::new();
        s.call("source ./env.sh", "/w").ok();
        let reason = s.call("pwd", "/w/x").expect("confirmed");
        assert!(reason.contains("moved the shell"));
    }

    #[test]
    fn missing_evidence_is_silence() {
        let parsed = lex("pwd");
        let finding = RULE
            .judge_live(&Input::new("pwd", Dialect::Zsh, &parsed))
            .expect("examine");
        let ctx = Context {
            platform: None,
            cwd: Path::new("/w/relais"),
            parsed: &parsed,
            background: false,
            timeout_ms: None,
            tool_use_id: "",
            transcript: None,
            agent_transcript: None,
            command: "pwd",
            output: None,
            session: "s",
            failed: false,
        };
        assert!(matches!(confirm(&ctx, &finding), Confirmed::No(_)));
        let s = Session::new();
        std::fs::write(&s.transcript, "{\"type\":\"user\"}\n").expect("write");
        assert_eq!(s.call("pwd", "/w"), Err("no batch key in the transcript"));
    }

    #[test]
    fn a_replay_never_fires_it() {
        for command in ["pwd", "git status", "cd relative/dir && pwd"] {
            let parsed = lex(command);
            let input = Input::new(command, Dialect::Zsh, &parsed);
            assert!(RULE.judge(&input).is_none(), "{command}");
            assert!(RULE.judge_live(&input).is_some(), "{command}");
        }
    }

    #[test]
    fn the_advice_names_the_remedy() {
        let parsed = lex("pwd");
        let finding = RULE
            .judge_live(&Input::new("pwd", Dialect::Zsh, &parsed))
            .expect("fires");
        assert!(finding.remedy.contains("(cd X && …)"));
        assert!(finding.remedy.contains("git -C X"));
        assert!(finding.remedy.contains("absolute paths"));
    }

    #[test]
    fn top_level_detection_skips_scoped_and_reset_moves() {
        let cd = |c: &str| top_level_cd(c, &lex(c)).is_some();
        assert!(cd("cd /repo && git status"));
        assert!(cd("pushd /repo && make"));
        assert!(!cd("(cd /repo && git status)"));
        assert!(!cd("echo $(cd /repo && pwd)"));
        assert!(!cd("bash -c 'cd /repo && make'"));
        assert!(!cd("cd /tmp && ls"));
    }
}
