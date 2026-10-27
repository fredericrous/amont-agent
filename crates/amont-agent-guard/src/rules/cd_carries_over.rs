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
//! batch is not, so that is the case nobody is told about. The rule now fires
//! on the call at risk: its `confirm` reads the transcript tail
//! ([`crate::batch_cwd`]) for an earlier Bash call of the same assistant
//! message with a top-level `cd`/`pushd` whose result is not an error, and
//! asks whether the shell's current directory (`Context::cwd`) differs from the
//! `cwd` recorded on the message's line, where the batch started.
//!
//! A `cd` inside `( … )`, `$( … )`, a function body or a `bash -c` string
//! moves nothing outside it and is not a top-level `cd`. Neither is a `cd` into
//! the temp directories: the harness resets the cwd for a path outside the
//! working directories, so nothing carries. A call that itself starts with a
//! top-level `cd` to an absolute path does not depend on where the shell is.
//!
//! Everything unknown is silence: no transcript, an unreadable one, or one
//! that does not show the call.
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
    examine: Examine::Analysis(examine),
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
    program: String,
    /// The clause as written.
    text: String,
}

/// Whether the call may be at risk is only known from the transcript, so
/// `examine` lets every command through to `confirm` except one that names
/// its own absolute directory first.
fn examine(input: &Input) -> Option<Finding> {
    if input.src.trim().is_empty() || starts_with_absolute_cd(input.src, input.legacy) {
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
    if ctx.tool_use_id.is_empty() {
        return Confirmed::No("no tool call id");
    }
    let Some(path) = ctx.transcript else {
        return Confirmed::No("no transcript");
    };
    if starts_with_absolute_cd(ctx.command, ctx.parsed) {
        return Confirmed::No("the call names its own directory");
    }
    let Ok(file) = std::fs::File::open(path) else {
        return Confirmed::No("the transcript could not be opened");
    };
    let Some(batch) = batch_cwd::batch_of(file, ctx.tool_use_id) else {
        return Confirmed::No("the call is not in the transcript");
    };
    if ctx.cwd == std::path::Path::new(&batch.cwd) {
        return Confirmed::No("the shell is where the batch started");
    }
    for sibling in batch.siblings.iter().filter(|s| s.succeeded) {
        let parsed = crate::shell::lex(&sibling.command);
        let Some(cd) = top_level_cd(&sibling.command, &parsed) else {
            continue;
        };
        return Confirmed::YesSaying {
            floor: None,
            ceiling: None,
            reason: format!(
                "This call runs in {} but its batch started in {}: an earlier call of \
                 the same parallel batch ran `{}` (`{}` at the top level of its \
                 command), the Bash tool carried the move, and the harness does not \
                 announce a carry between siblings. Paths and git commands here \
                 resolve against the wrong directory.",
                ctx.cwd.display(),
                batch.cwd,
                cd.text,
                cd.program,
            ),
            excerpt: format!(
                "batch-cwd={} cwd={} {}",
                batch.cwd,
                ctx.cwd.display(),
                cd.text
            ),
            remedy: Some(finding.remedy.clone()),
        };
    }
    Confirmed::No("no earlier sibling carried a cd")
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
            program: program.to_string(),
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
    use serde_json::json;
    use std::path::Path;

    fn assistant(message: &str, cwd: &str, id: &str, command: &str) -> String {
        json!({"type":"assistant","cwd":cwd,"message":{"id":message,"content":[
            {"type":"tool_use","id":id,"name":"Bash","input":{"command":command}}]}})
        .to_string()
    }

    fn result(id: &str, is_error: bool, cwd: &str) -> String {
        json!({"type":"user","cwd":cwd,"message":{"content":[
            {"type":"tool_result","tool_use_id":id,"is_error":is_error,"content":"ok"}]}})
        .to_string()
    }

    fn prompt(cwd: &str) -> String {
        json!({"type":"user","cwd":cwd,"message":{"role":"user","content":"go"}}).to_string()
    }

    /// `confirm` for the call `toolu_b` with the shell in `shell_cwd`.
    fn confirm_with(
        lines: &[String],
        command: &str,
        shell_cwd: &str,
        id: &str,
    ) -> Result<String, &'static str> {
        // One directory per call: tests run in parallel and share fixtures.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("amont-agent-cdco-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join(format!("{}.jsonl", id.replace('/', "_")));
        std::fs::write(&path, lines.join("\n")).expect("write");
        let parsed = lex(command);
        let finding = RULE
            .judge(&Input::new(command, Dialect::Zsh, &parsed))
            .ok_or("examine silent")?;
        let ctx = Context {
            platform: None,
            cwd: Path::new(shell_cwd),
            parsed: &parsed,
            background: false,
            timeout_ms: None,
            tool_use_id: id,
            transcript: Some(&path),
            agent_transcript: None,
            command,
            output: None,
            session: "",
            failed: false,
        };
        let confirmed = (RULE.confirm.expect("confirm"))(&ctx, &finding);
        std::fs::remove_dir_all(&dir).ok();
        match confirmed {
            Confirmed::YesSaying { reason, .. } => Ok(reason),
            Confirmed::No(why) => Err(why),
            _ => Err("unexpected"),
        }
    }

    /// A batch as Claude Code writes it: the prompt before it carries where
    /// the batch started, and every line of the message carries the cwd at
    /// flush time, which is already the moved one (measured 2026-10-27).
    fn batch(a_command: &str, a_error: bool) -> Vec<String> {
        vec![
            prompt("/w"),
            assistant("msg_1", "/w/relais", "toolu_a", a_command),
            assistant("msg_1", "/w/relais", "toolu_b", "pwd"),
            result("toolu_a", a_error, "/w/relais"),
        ]
    }

    #[test]
    fn the_sibling_after_a_carried_cd_is_confirmed() {
        let reason = confirm_with(
            &batch("cd /w/relais && pwd", false),
            "pwd",
            "/w/relais",
            "toolu_b",
        )
        .expect("confirmed");
        assert!(reason.contains("/w/relais"));
        assert!(reason.contains("started in /w"));
        assert!(reason.contains("cd /w/relais"));
    }

    #[test]
    fn the_advice_names_the_remedy() {
        let parsed = lex("pwd");
        let finding = RULE
            .judge(&Input::new("pwd", Dialect::Zsh, &parsed))
            .expect("fires");
        assert!(finding.remedy.contains("(cd X && …)"));
        assert!(finding.remedy.contains("git -C X"));
        assert!(finding.remedy.contains("absolute paths"));
    }

    #[test]
    fn a_subshell_cd_does_not_carry() {
        let lines = batch("(cd /w/relais && pwd)", false);
        assert!(confirm_with(&lines, "pwd", "/w/relais", "toolu_b").is_err());
    }

    #[test]
    fn a_failed_sibling_does_not_carry() {
        let lines = batch("cd /w/relais && pwd", true);
        assert!(confirm_with(&lines, "pwd", "/w/relais", "toolu_b").is_err());
    }

    #[test]
    fn a_cd_from_an_earlier_message_is_announced_by_the_harness() {
        let lines = vec![
            prompt("/w"),
            assistant("msg_0", "/w/relais", "toolu_a", "cd /w/relais && pwd"),
            result("toolu_a", false, "/w/relais"),
            assistant("msg_1", "/w/relais", "toolu_b", "pwd"),
        ];
        assert!(confirm_with(&lines, "pwd", "/w/relais", "toolu_b").is_err());
    }

    #[test]
    fn a_shell_where_the_batch_started_is_silent() {
        let lines = batch("cd /w/relais && pwd", false);
        assert!(confirm_with(&lines, "pwd", "/w", "toolu_b").is_err());
    }

    #[test]
    fn missing_or_unreadable_evidence_is_silence() {
        let parsed = lex("pwd");
        let finding = RULE
            .judge(&Input::new("pwd", Dialect::Zsh, &parsed))
            .expect("fires");
        let mut ctx = Context {
            platform: None,
            cwd: Path::new("/w/relais"),
            parsed: &parsed,
            background: false,
            timeout_ms: None,
            tool_use_id: "toolu_b",
            transcript: None,
            agent_transcript: None,
            command: "pwd",
            output: None,
            session: "",
            failed: false,
        };
        let confirm = RULE.confirm.expect("confirm");
        assert!(matches!(confirm(&ctx, &finding), Confirmed::No(_)));
        ctx.transcript = Some(Path::new("/nonexistent/amont-agent/transcript.jsonl"));
        assert!(matches!(confirm(&ctx, &finding), Confirmed::No(_)));
        let lines = batch("cd /w/relais && pwd", false);
        assert!(confirm_with(&lines, "pwd", "/w/relais", "toolu_other").is_err());
        assert!(confirm_with(&["not json".to_string()], "pwd", "/w/relais", "toolu_b").is_err());
    }

    #[test]
    fn a_call_that_starts_with_an_absolute_cd_is_silent() {
        let lines = batch("cd /w/relais && pwd", false);
        assert!(confirm_with(&lines, "cd /w/other && pwd", "/w/relais", "toolu_b").is_err());
    }

    #[test]
    fn top_level_detection_skips_scoped_and_reset_moves() {
        let cd = |c: &str| top_level_cd(c, &lex(c)).is_some();
        assert!(cd("cd /repo && git status"));
        assert!(cd("pushd /repo && make"));
        assert!(cd("echo hi; cd ~/work && ls"));
        assert!(cd("{ cd /repo; git status; }"));
        assert!(cd("(echo hi) && cd /repo"));
        assert!(cd("cd /tmpfoo && ls"));
        assert!(!cd("(cd /repo && git status)"));
        assert!(!cd("git -C /repo status"));
        assert!(!cd("echo $(cd /repo && pwd)"));
        assert!(!cd("bash -c 'cd /repo && make'"));
        assert!(!cd("f() { cd /repo; }; f"));
        assert!(!cd("function f { cd /repo; }; f"));
        assert!(!cd("cd /tmp && ls"));
        assert!(!cd("cd /private/tmp/x && ls"));
        assert!(!cd("cd"));
    }
}
