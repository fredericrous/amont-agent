//! The hook payload, as it arrives on stdin.
//!
//! Claude Code writes one JSON object to this process's stdin before it runs a
//! tool, and reads a decision back from stdout. The shape is defined by
//! somebody else and grows between releases, which decides how it is read here:
//! [`serde_json::Value`] and hand-written accessors, never a
//! `#[derive(Deserialize)]` struct.
//!
//! The difference matters at exactly one moment. A derived struct turns a
//! renamed or retyped field into a hard parse error, and the temptation is then
//! to treat that error as *something* — to block, or to warn. Reading field by
//! field turns the same event into an absent value, which this crate already
//! knows how to answer: no opinion. A guard that starts refusing commands
//! because a payload gained a field is worse than no guard.
//!
//! ## Everything unrecognised is silence
//!
//! Not our event, not our tool, no command, unparseable JSON, a `cwd` that no
//! longer exists — every one of those returns [`Event::NotOurs`], and the
//! caller exits 0 having written nothing.

use std::path::PathBuf;

/// The largest payload worth reading. Commands in the wild reach ~13 KB; a
/// megabyte is a generated blob, and a blob is not a shell command.
const MAX_PAYLOAD: usize = 1024 * 1024;

pub struct Bash {
    pub command: String,
    pub cwd: PathBuf,
    pub session: String,
    /// Journalled, never consulted. A `PreToolUse` hook fires before any
    /// permission check, in every mode including `bypassPermissions`, and a
    /// rule that quietly stopped applying in one mode would be a rule nobody
    /// could reason about. Recording it is how you would notice if that ever
    /// stopped being true.
    pub permission_mode: String,
    /// `tool_input.run_in_background`: the call is detached and the tool's
    /// timeout does not apply. Read by `foreground-poll`'s `confirm`.
    pub background: bool,
    /// `tool_input.timeout`, in milliseconds, when the call sets one. The
    /// tool's own default is two minutes; `foreground-poll` compares a loop's
    /// budget against it.
    pub timeout_ms: Option<u64>,
    /// `tool_use_id`: the one key a `PreToolUse` and its `PostToolUse` share.
    /// `push-published` reads, after the push, the remote state recorded
    /// before it under this id.
    pub tool_use_id: String,
    /// `prompt_id`: which person's prompt this call belongs to. A preview
    /// registration is bound to it, so an approval can be scoped to the turn
    /// that showed the preview.
    pub prompt_id: String,
    /// `tool_response.stdout`, after the call. Only `preview register` output
    /// is ever read from it.
    pub stdout: Option<String>,
    /// `transcript_path`: where this session's completed agents are
    /// recorded. Read by `implementation-review`'s `confirm` for a push;
    /// `None` when the payload carried none.
    pub transcript: Option<PathBuf>,
}

/// A prompt the person typed and submitted.
pub struct Prompt {
    pub text: String,
    pub session: String,
    pub prompt_id: String,
}

/// An `AskUserQuestion` call, before or after the person answered.
pub struct Ask {
    pub session: String,
    pub prompt_id: String,
    pub tool_use_id: String,
    /// The question texts, in order.
    pub questions: Vec<String>,
    /// The call's INPUT already carried `answers`. The tool's schema accepts
    /// them, so a model could pre-answer its own question; an answer that did
    /// not come from the person approves nothing.
    pub prefilled: bool,
    /// After the call: question text → the label the person picked, from
    /// `tool_response.answers`. Empty when nobody answered.
    pub answers: Vec<(String, String)>,
    /// `duration_ms`, raw. NOT the person's answer time: a minutes-long
    /// approval arrived as 0 on 2026-09-28. The latency is measured by the
    /// hook itself (`crate::preview`); this is journalled beside it.
    pub duration_ms: Option<u64>,
    /// `transcript_path`: an `Overrule` of an implementation review is
    /// honoured only when the transcript shows the `rework` it overrules.
    pub transcript: Option<PathBuf>,
}

/// A Read, Edit or Write tool call: one path, and whether it reads or writes.
pub struct FileOp {
    pub path: PathBuf,
    pub writes: bool,
    /// `offset:limit` as the Read tool was given, or `full`.
    pub window: String,
    pub cwd: PathBuf,
    pub session: String,
    pub permission_mode: String,
    /// What a `PreToolUse` Edit, MultiEdit or Write is about to do to the
    /// file. `None` for a Read, after a call, or when the input held no
    /// usable text.
    pub change: Option<Change>,
}

/// The text a write tool was handed, before anything is applied.
pub enum Change {
    /// `Write`: the whole new content.
    Write { content: String },
    /// `Edit` (one entry) or `MultiEdit`, in the order they apply.
    Edits(Vec<EditPart>),
}

pub struct EditPart {
    pub old: String,
    pub new: String,
    pub replace_all: bool,
}

pub struct Session {
    pub cwd: PathBuf,
    pub session: String,
}

/// An `ExitPlanMode` about to run: the plan is about to be shown to the
/// person for approval.
pub struct PlanExit {
    pub session: String,
    pub permission_mode: String,
    /// `transcript_path`: where this session's reviews, if any, are recorded.
    /// `None` when the payload carried none.
    pub transcript: Option<PathBuf>,
    /// `tool_input.planFilePath`: the file the reviewers read. `None` when
    /// the payload carried none.
    pub plan_file: Option<PathBuf>,
}

/// The model finished a turn and is about to hand control back.
// holds-until: the `plan-phases-open` Stop arm reads it (same branch)
#[allow(dead_code)]
pub struct Stop {
    pub session: String,
    /// `None` when the payload's `cwd` is absent or empty: unlike the tool
    /// events there is no fallback to the process cwd, which says nothing
    /// about where the session was.
    pub cwd: Option<PathBuf>,
    pub permission_mode: Option<String>,
    /// `last_assistant_message`: what the model said as it stopped.
    pub last_message: Option<String>,
    /// `background_tasks` is a non-empty array: work is still running.
    pub background_busy: bool,
    /// A previous `Stop` hook already kept this turn going.
    pub stop_hook_active: bool,
}

pub enum Event {
    /// A turn ending. Dispatched, not yet judged.
    #[allow(dead_code)]
    Stop(Stop),
    /// A Bash tool call we can have an opinion about.
    PreBash(Box<Bash>),
    /// A Bash tool call that has already run, and SUCCEEDED.
    ///
    /// Claude Code routes a failed call to `PostToolUseFailure`, a different
    /// event this crate deliberately ignores: a command that returned an error
    /// is already in front of the model, and there is nothing invisible left to
    /// point out. Everything this event carries claimed to work. That is the
    /// whole reason the tier exists — `git push` exiting 0 without the push
    /// landing is not a failure anyone can see.
    PostBash(Box<Bash>),
    /// A session opening. The guard leaves proof that it ran, and — this being
    /// the one moment a fetch is worth its cost — says where the checkout
    /// stands against the remote.
    SessionStart(Session),
    /// A Read, Edit, Write or MultiEdit about to run. The file tier: nothing
    /// here is a shell command, and the only rule that reads it is the one
    /// about reading the same file twice.
    PreFile(Box<FileOp>),
    /// A Read that has run and succeeded — the one moment the file is known
    /// to be in context. Only then is it remembered: a Read that was refused,
    /// or that failed on a path that is not there, is not a read.
    PostFile(Box<FileOp>),
    /// A prompt the person submitted: the one moment a typed approval of a
    /// shown preview can arrive, and the moment every older one lapses.
    Prompt(Box<Prompt>),
    /// An `AskUserQuestion` about to run: record whether its answers were
    /// pre-filled by the model.
    PreAsk(Box<Ask>),
    /// An `AskUserQuestion` the person has answered (or not).
    PostAsk(Box<Ask>),
    /// An `ExitPlanMode` about to run: has the plan been through its review
    /// panel (`plan-review-panel`)?
    PrePlanExit(Box<PlanExit>),
    NotOurs,
}

/// The text an Edit, MultiEdit or Write carries; anything else, or a field of
/// the wrong type, is `None`.
fn change_of(tool: &str, input: Option<&serde_json::Value>) -> Option<Change> {
    let input = input?;
    let text = |v: &serde_json::Value, key: &str| -> Option<String> {
        v.get(key).and_then(|x| x.as_str()).map(str::to_string)
    };
    let part = |v: &serde_json::Value| -> Option<EditPart> {
        Some(EditPart {
            old: text(v, "old_string")?,
            new: text(v, "new_string")?,
            replace_all: v
                .get("replace_all")
                .and_then(|b| b.as_bool())
                .unwrap_or(false),
        })
    };
    match tool {
        "Write" => Some(Change::Write {
            content: text(input, "content")?,
        }),
        "Edit" => Some(Change::Edits(vec![part(input)?])),
        "MultiEdit" => {
            let parts: Option<Vec<EditPart>> =
                input.get("edits")?.as_array()?.iter().map(part).collect();
            parts.map(Change::Edits)
        }
        _ => None,
    }
}

pub fn parse(raw: &str) -> Event {
    if raw.len() > MAX_PAYLOAD {
        return Event::NotOurs;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Event::NotOurs;
    };
    let str_at = |key: &str| -> String {
        v.get(key)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let cwd = {
        let c = str_at("cwd");
        if c.is_empty() {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        } else {
            PathBuf::from(c)
        }
    };
    // A path the payload names, absolute against the session cwd; an empty
    // or missing one is `None`.
    let path_at = |s: Option<&str>| {
        s.filter(|p| !p.trim().is_empty())
            .map(PathBuf::from)
            .map(|p| if p.is_absolute() { p } else { cwd.join(p) })
    };
    let transcript = || path_at(v.get("transcript_path").and_then(|x| x.as_str()));

    match v.get("hook_event_name").and_then(|x| x.as_str()) {
        Some("Stop") => {
            let opt = |key: &str| v.get(key).and_then(|x| x.as_str()).map(str::to_string);
            Event::Stop(Stop {
                session: str_at("session_id"),
                cwd: opt("cwd").filter(|c| !c.is_empty()).map(PathBuf::from),
                permission_mode: opt("permission_mode"),
                last_message: opt("last_assistant_message"),
                background_busy: v
                    .get("background_tasks")
                    .and_then(|b| b.as_array())
                    .is_some_and(|b| !b.is_empty()),
                stop_hook_active: v
                    .get("stop_hook_active")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false),
            })
        }
        Some("SessionStart") => Event::SessionStart(Session {
            cwd,
            session: str_at("session_id"),
        }),
        Some("UserPromptSubmit") => Event::Prompt(Box::new(Prompt {
            text: str_at("prompt"),
            session: str_at("session_id"),
            prompt_id: str_at("prompt_id"),
        })),
        Some("PreToolUse")
            if v.get("tool_name").and_then(|x| x.as_str()) == Some("ExitPlanMode") =>
        {
            Event::PrePlanExit(Box::new(PlanExit {
                session: str_at("session_id"),
                permission_mode: str_at("permission_mode"),
                transcript: transcript(),
                plan_file: path_at(
                    v.get("tool_input")
                        .and_then(|i| i.get("planFilePath"))
                        .and_then(|x| x.as_str()),
                ),
            }))
        }
        Some(stage @ ("PreToolUse" | "PostToolUse"))
            if v.get("tool_name").and_then(|x| x.as_str()) == Some("AskUserQuestion") =>
        {
            let input = v.get("tool_input");
            let questions = input
                .and_then(|i| i.get("questions"))
                .and_then(|q| q.as_array())
                .map(|qs| {
                    qs.iter()
                        .filter_map(|q| q.get("question").and_then(|t| t.as_str()))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            // Present and non-empty in the INPUT is a pre-filled answer. An
            // empty object is what an unanswered call can look like, and is
            // not an answer at all.
            let prefilled = stage == "PreToolUse"
                && input
                    .and_then(|i| i.get("answers"))
                    .and_then(|a| a.as_object())
                    .is_some_and(|a| !a.is_empty());
            let answers = if stage == "PostToolUse" {
                v.get("tool_response")
                    .and_then(|r| r.get("answers"))
                    .and_then(|a| a.as_object())
                    .map(|a| {
                        a.iter()
                            .filter_map(|(q, l)| Some((q.clone(), l.as_str()?.to_string())))
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let ask = Box::new(Ask {
                session: str_at("session_id"),
                prompt_id: str_at("prompt_id"),
                tool_use_id: str_at("tool_use_id"),
                questions,
                prefilled,
                answers,
                duration_ms: v.get("duration_ms").and_then(|d| d.as_u64()),
                transcript: transcript(),
            });
            if stage == "PreToolUse" {
                Event::PreAsk(ask)
            } else {
                Event::PostAsk(ask)
            }
        }
        Some(stage @ ("PreToolUse" | "PostToolUse"))
            if matches!(
                v.get("tool_name").and_then(|x| x.as_str()),
                Some("Read" | "Edit" | "Write" | "MultiEdit")
            ) =>
        {
            let tool = v
                .get("tool_name")
                .and_then(|x| x.as_str())
                .unwrap_or_default();
            let input = v.get("tool_input");
            let Some(path) = input
                .and_then(|i| i.get("file_path"))
                .and_then(|p| p.as_str())
                .filter(|p| !p.trim().is_empty())
            else {
                return Event::NotOurs;
            };
            let path = PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            };
            let num = |key: &str| input.and_then(|i| i.get(key)).and_then(|n| n.as_u64());
            let window = match (num("offset"), num("limit")) {
                (None, None) => "full".to_string(),
                (o, l) => format!(
                    "{}:{}",
                    o.unwrap_or(0),
                    l.map_or("-".to_string(), |l| l.to_string())
                ),
            };
            let change = if stage == "PreToolUse" {
                change_of(tool, input)
            } else {
                None
            };
            let op = Box::new(FileOp {
                path,
                writes: tool != "Read",
                window,
                cwd,
                session: str_at("session_id"),
                permission_mode: str_at("permission_mode"),
                change,
            });
            if stage == "PreToolUse" {
                Event::PreFile(op)
            } else if !op.writes {
                Event::PostFile(op)
            } else {
                // A write is remembered before it runs: recording one that
                // then failed only makes the next read of that file
                // uncommented, which is the safe direction.
                Event::NotOurs
            }
        }
        Some(stage @ ("PreToolUse" | "PostToolUse")) => {
            // Exact, not a prefix. An MCP server may expose a tool whose name
            // merely starts with `Bash`, and that tool is not this one.
            if v.get("tool_name").and_then(|x| x.as_str()) != Some("Bash") {
                return Event::NotOurs;
            }
            let command = v
                .get("tool_input")
                .and_then(|i| i.get("command"))
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                .to_string();
            if command.trim().is_empty() {
                return Event::NotOurs;
            }
            // Two spellings of the same fact, and after a call has run only
            // the second one is reliable: a detached call reports success the
            // moment it is handed a task id, with empty output and the command
            // still going. Asserting anything about it would be asserting about
            // work that has not happened yet.
            let background = v
                .get("tool_input")
                .and_then(|i| i.get("run_in_background"))
                .and_then(|b| b.as_bool())
                .unwrap_or(false)
                || v.get("tool_response")
                    .and_then(|r| r.get("backgroundTaskId"))
                    .is_some();
            let timeout_ms = v
                .get("tool_input")
                .and_then(|i| i.get("timeout"))
                .and_then(|t| t.as_u64());
            let stdout = v
                .get("tool_response")
                .and_then(|r| r.get("stdout"))
                .and_then(|o| o.as_str())
                .map(str::to_string);
            let transcript = transcript();
            let bash = Box::new(Bash {
                command,
                cwd,
                session: str_at("session_id"),
                permission_mode: str_at("permission_mode"),
                background,
                timeout_ms,
                tool_use_id: str_at("tool_use_id"),
                prompt_id: str_at("prompt_id"),
                stdout,
                transcript,
            });
            if stage == "PreToolUse" {
                Event::PreBash(bash)
            } else {
                Event::PostBash(bash)
            }
        }
        _ => Event::NotOurs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pre(command: &str) -> String {
        format!(
            r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":"/tmp",
                 "session_id":"s","tool_use_id":"t","permission_mode":"default",
                 "tool_input":{{"command":{}}}}}"#,
            serde_json::Value::String(command.to_string())
        )
    }

    #[test]
    fn a_bash_call_is_ours() {
        match parse(&pre("git push | tail -1")) {
            Event::PreBash(b) => {
                assert_eq!(b.command, "git push | tail -1");
                assert_eq!(b.cwd, PathBuf::from("/tmp"));
                assert_eq!(b.session, "s");
            }
            _ => panic!("expected a Bash call"),
        }
    }

    /// The list of things that must produce silence rather than an error. Each
    /// of these WILL happen — a payload gains a field, a new tool appears, a
    /// session is starting, a write is truncated mid-flight.
    #[test]
    fn anything_we_do_not_recognise_is_not_an_opinion() {
        for raw in [
            "",
            "{",
            "null",
            "[]",
            // Was `PostToolUse` until that became an event we answer. Kept as a
            // case, with an event that is still none of our business — the list
            // exists to prove an UNKNOWN event is silence, and it would stop
            // proving anything if every entry in it were one we handle.
            r#"{"hook_event_name":"PreCompact","tool_name":"Bash"}"#,
            r#"{"hook_event_name":"PostToolUseFailure","tool_name":"Bash","error":"Exit code 3"}"#,
            r#"{"hook_event_name":"PostToolUse","tool_name":"Bash"}"#,
            r#"{"hook_event_name":"PreToolUse","tool_name":"Read"}"#,
            r#"{"hook_event_name":"PreToolUse","tool_name":"BashOutput"}"#,
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{}}"#,
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"  "}}"#,
        ] {
            assert!(
                matches!(parse(raw), Event::NotOurs),
                "expected silence for {raw:?}"
            );
        }
    }

    /// A successful call is the one we may assert about.
    #[test]
    fn a_finished_bash_call_is_its_own_event() {
        let raw = r#"{"hook_event_name":"PostToolUse","tool_name":"Bash",
                      "tool_input":{"command":"git push"},
                      "tool_response":{"stdout":"","stderr":"","interrupted":false}}"#;
        match parse(raw) {
            Event::PostBash(b) => {
                assert_eq!(b.command, "git push");
                assert!(!b.background);
            }
            _ => panic!("expected a finished Bash call"),
        }
    }

    /// A write carries what it is about to do, before it runs; a Read, and a
    /// write that has run, carry nothing.
    #[test]
    fn a_pre_write_carries_its_change() {
        let pre = |tool: &str, input: &str| {
            let raw = format!(
                r#"{{"hook_event_name":"PreToolUse","tool_name":"{tool}","cwd":"/tmp",
                     "tool_input":{input}}}"#
            );
            match parse(&raw) {
                Event::PreFile(op) => op.change,
                _ => panic!("expected a file call"),
            }
        };
        match pre("Write", r#"{"file_path":"a.py","content":"x"}"#) {
            Some(Change::Write { content }) => assert_eq!(content, "x"),
            _ => panic!("expected a Write"),
        }
        match pre(
            "Edit",
            r#"{"file_path":"a.py","old_string":"a","new_string":"b","replace_all":true}"#,
        ) {
            Some(Change::Edits(e)) => {
                assert_eq!(e.len(), 1);
                assert_eq!((e[0].old.as_str(), e[0].new.as_str()), ("a", "b"));
                assert!(e[0].replace_all);
            }
            _ => panic!("expected an Edit"),
        }
        match pre(
            "MultiEdit",
            r#"{"file_path":"a.py","edits":[{"old_string":"a","new_string":"b"},
                                            {"old_string":"c","new_string":"d"}]}"#,
        ) {
            Some(Change::Edits(e)) => assert_eq!(e.len(), 2),
            _ => panic!("expected a MultiEdit"),
        }
        assert!(pre("Read", r#"{"file_path":"a.py"}"#).is_none());
        assert!(pre("Edit", r#"{"file_path":"a.py","old_string":1}"#).is_none());
    }

    /// A Read that ran is the one moment the file is known to be in context;
    /// a write that ran was already remembered before it did.
    #[test]
    fn a_finished_read_is_its_own_event_and_a_finished_write_is_not() {
        let read = r#"{"hook_event_name":"PostToolUse","tool_name":"Read","cwd":"/tmp",
                       "tool_input":{"file_path":"a.rs","offset":10,"limit":20},
                       "tool_response":{"type":"text","file":{"filePath":"/tmp/a.rs"}}}"#;
        match parse(read) {
            Event::PostFile(op) => {
                assert_eq!(op.path, PathBuf::from("/tmp/a.rs"));
                assert_eq!(op.window, "10:20");
                assert!(!op.writes);
            }
            _ => panic!("expected a finished Read"),
        }
        let write = r#"{"hook_event_name":"PostToolUse","tool_name":"Write","cwd":"/tmp",
                        "tool_input":{"file_path":"a.rs","content":""}}"#;
        assert!(matches!(parse(write), Event::NotOurs));
    }

    /// A failure is already in front of the model. `PostToolUseFailure` carries
    /// no `tool_response` at all — the exit code lives in an `error` string —
    /// and this crate has nothing to add to a command that already said it
    /// failed.
    #[test]
    fn a_failed_call_is_not_ours() {
        let raw = r#"{"hook_event_name":"PostToolUseFailure","tool_name":"Bash",
                      "tool_input":{"command":"git push"},
                      "error":"Exit code 128\nfatal: could not read from remote"}"#;
        assert!(matches!(parse(raw), Event::NotOurs));
    }

    /// A detached call reports success the instant it is handed a task id.
    #[test]
    fn a_backgrounded_call_is_marked_even_without_the_input_flag() {
        let raw = r#"{"hook_event_name":"PostToolUse","tool_name":"Bash",
                      "tool_input":{"command":"git push"},
                      "tool_response":{"stdout":"","backgroundTaskId":"bjkvph22n"}}"#;
        match parse(raw) {
            Event::PostBash(b) => assert!(b.background, "a task id means it is still running"),
            _ => panic!("expected a finished Bash call"),
        }
    }

    /// The shape captured from a real session (2026-09-28): the person's
    /// selection arrives in `tool_response.answers`, keyed by question text.
    #[test]
    fn an_answered_question_carries_the_persons_label() {
        let raw = r#"{"hook_event_name":"PostToolUse","tool_name":"AskUserQuestion",
                      "session_id":"s","prompt_id":"p","tool_use_id":"t","duration_ms":30997,
                      "tool_input":{"questions":[{"question":"[preview a1] ship?","options":[]}],
                                    "answers":{"[preview a1] ship?":"Approve"}},
                      "tool_response":{"questions":[],"answers":{"[preview a1] ship?":"Approve"},
                                       "annotations":{}}}"#;
        match parse(raw) {
            Event::PostAsk(a) => {
                assert_eq!(a.questions, vec!["[preview a1] ship?".to_string()]);
                assert_eq!(
                    a.answers,
                    vec![("[preview a1] ship?".to_string(), "Approve".to_string())]
                );
                assert_eq!(a.duration_ms, Some(30997));
                assert!(!a.prefilled, "prefilled is only judged before the call");
            }
            _ => panic!("expected an answered question"),
        }
    }

    /// The tool's input schema accepts `answers`, so a model can pre-answer.
    #[test]
    fn a_question_that_arrives_with_answers_is_prefilled() {
        let pre = |answers: &str| {
            format!(
                r#"{{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion",
                     "tool_input":{{"questions":[{{"question":"q"}}]{answers}}}}}"#
            )
        };
        match parse(&pre(r#","answers":{"q":"Approve"}"#)) {
            Event::PreAsk(a) => assert!(a.prefilled),
            _ => panic!("expected a question"),
        }
        match parse(&pre(r#","answers":{}"#)) {
            Event::PreAsk(a) => assert!(!a.prefilled, "an empty map is not an answer"),
            _ => panic!("expected a question"),
        }
        match parse(&pre("")) {
            Event::PreAsk(a) => assert!(!a.prefilled),
            _ => panic!("expected a question"),
        }
    }

    #[test]
    fn a_full_stop_payload_is_read_field_by_field() {
        let raw = r#"{"hook_event_name":"Stop","session_id":"s","cwd":"/work",
                      "permission_mode":"plan","last_assistant_message":"done",
                      "background_tasks":[{"id":"b1"}],"stop_hook_active":true}"#;
        match parse(raw) {
            Event::Stop(s) => {
                assert_eq!(s.session, "s");
                assert_eq!(s.cwd, Some(PathBuf::from("/work")));
                assert_eq!(s.permission_mode.as_deref(), Some("plan"));
                assert_eq!(s.last_message.as_deref(), Some("done"));
                assert!(s.background_busy);
                assert!(s.stop_hook_active);
            }
            _ => panic!("expected a Stop"),
        }
    }

    #[test]
    fn a_minimal_stop_payload_leaves_the_rest_absent() {
        let raw = r#"{"hook_event_name":"Stop","session_id":"s","cwd":"/work",
                      "background_tasks":[]}"#;
        match parse(raw) {
            Event::Stop(s) => {
                assert_eq!(s.cwd, Some(PathBuf::from("/work")));
                assert!(s.permission_mode.is_none());
                assert!(s.last_message.is_none());
                assert!(!s.background_busy, "an empty list is nothing running");
                assert!(!s.stop_hook_active);
            }
            _ => panic!("expected a Stop"),
        }
    }

    #[test]
    fn an_empty_stop_cwd_is_none_not_the_process_cwd() {
        for raw in [
            r#"{"hook_event_name":"Stop","session_id":"s","cwd":""}"#,
            r#"{"hook_event_name":"Stop","session_id":"s"}"#,
        ] {
            match parse(raw) {
                Event::Stop(s) => assert!(s.cwd.is_none()),
                _ => panic!("expected a Stop"),
            }
        }
    }

    #[test]
    fn a_submitted_prompt_is_its_own_event() {
        let raw = r#"{"hook_event_name":"UserPromptSubmit","prompt":"approve",
                      "session_id":"s","prompt_id":"p2","cwd":"/tmp"}"#;
        match parse(raw) {
            Event::Prompt(p) => {
                assert_eq!(p.text, "approve");
                assert_eq!(p.prompt_id, "p2");
            }
            _ => panic!("expected a prompt"),
        }
    }

    /// A field arriving with the wrong type is a schema change, not a reason to
    /// start refusing commands.
    #[test]
    fn a_retyped_field_degrades_to_silence() {
        let raw = r#"{"hook_event_name":"PreToolUse","tool_name":"Bash",
                      "tool_input":{"command":{"was":"a string"}}}"#;
        assert!(matches!(parse(raw), Event::NotOurs));
    }

    /// Unknown fields are the normal case, not an error: the payload grows
    /// between Claude Code releases and this crate must not notice.
    #[test]
    fn unknown_fields_are_ignored() {
        let raw = r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","cwd":"/tmp",
                      "tool_input":{"command":"git status","timeout":5,"future":true},
                      "brand_new_field":{"nested":[1,2,3]}}"#;
        assert!(matches!(parse(raw), Event::PreBash(_)));
    }

    #[test]
    fn a_payload_too_large_to_be_a_command_is_ignored() {
        let huge = format!(
            r#"{{"hook_event_name":"PreToolUse","pad":"{}"}}"#,
            "x".repeat(MAX_PAYLOAD)
        );
        assert!(matches!(parse(&huge), Event::NotOurs));
    }
}
