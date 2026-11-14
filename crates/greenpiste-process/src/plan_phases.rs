//! The judgement behind `plan-phases-open` (ADR-0022, `work.commits-follow-phases`).
//!
//! A `Stop` hook: when the agent tries to end its turn while the plan its
//! branch carries still has an open phase that is not a `🧑 decision:`, it is
//! sent on to that phase. A guard that cannot establish the fact never keeps
//! the agent running, so every failure here — no repository, git error, no
//! readable plan, a counter that cannot be written — is silence.
//!
//! The cap is a file per session, `<journal dir>/plan-phases-open/<session>`:
//! the plan, the phase label and the number of continuations so far. It is
//! reset when the open phase changes and when the person submits a prompt.

use greenpiste_guard::names::BIN;
use std::path::{Path, PathBuf};
use std::time::Instant;

use greenpiste_guard::decision::{self, Decision};
use greenpiste_guard::journal;
use greenpiste_guard::payload::{Prompt, Stop};
use greenpiste_guard::rules::{plan_phases_open::RULE, Stance};

/// Continuations before the stop is let through.
const CAP: u32 = 3;
/// Longest label quoted back, `…` included.
const LABEL_MAX: usize = 80;

pub fn on_stop(stop: &Stop, stance: impl FnOnce() -> Stance) -> Decision {
    judge(stop, stance).unwrap_or(Decision::Silent)
}

/// What the branch's plan says about the work, for the rules that share the
/// Stop hook (`clear-at-boundary` judges before `plan-phases-open`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseFact {
    /// No open phase, or nothing could be established.
    NoOpenPhase,
    /// An open, non-decision phase, and no commit after the plan's own
    /// `docs(plan): ` commit: the plan is the whole state.
    PlanOnly,
    /// An open, non-decision phase, and commits after the plan commit.
    Working,
    /// A git child did not answer before the caller's deadline.
    TimedOut,
}

/// The commit subject that opens a plan's branch.
const PLAN_COMMIT_PREFIX: &str = "docs(plan): ";

/// Why the plan's state could not be established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unknown {
    /// A git child did not answer before the deadline.
    TimedOut,
    /// No plan, no repository, or git refused a command.
    Unestablished,
}

impl From<greenpiste_guard::git::BoundedFailure> for Unknown {
    fn from(failure: greenpiste_guard::git::BoundedFailure) -> Self {
        match failure {
            greenpiste_guard::git::BoundedFailure::TimedOut => Unknown::TimedOut,
            greenpiste_guard::git::BoundedFailure::Exited { .. } => Unknown::Unestablished,
        }
    }
}

fn bounded(root: &Path, args: &[&str], deadline: Instant) -> Result<Vec<u8>, Unknown> {
    Ok(greenpiste_guard::git::bytes_bounded_detail(
        root,
        args,
        None,
        &[],
        deadline,
    )?)
}

fn bounded_text(root: &Path, args: &[&str], deadline: Instant) -> Result<String, Unknown> {
    bounded(root, args, deadline).map(|b| String::from_utf8_lossy(&b).trim().to_string())
}

/// [`PhaseFact`] of the plan the branch of `stop.cwd` carries, every git
/// child bounded by `deadline`.
pub fn open_phase(stop: &Stop, deadline: Instant) -> PhaseFact {
    match phase_fact(stop, deadline) {
        Ok(fact) => fact,
        Err(Unknown::TimedOut) => PhaseFact::TimedOut,
        Err(Unknown::Unestablished) => PhaseFact::NoOpenPhase,
    }
}

fn phase_fact(stop: &Stop, deadline: Instant) -> Result<PhaseFact, Unknown> {
    use Unknown::Unestablished;
    if !greenpiste_guard::session_state::usable_id(&stop.session) {
        return Err(Unestablished);
    }
    let cwd = stop.cwd.as_deref().ok_or(Unestablished)?;
    if !cwd.is_dir() {
        return Err(Unestablished);
    }
    let root = repo_root(cwd).ok_or(Unestablished)?;
    if !root.join("docs/plans").is_dir() {
        return Err(Unestablished);
    }
    let (branch, base) = branch_and_base_bounded(&root, deadline)?;
    if base.split_once('/').map(|(_, b)| b) == Some(branch.as_str()) {
        return Err(Unestablished);
    }
    let changed = bounded(
        &root,
        &[
            "diff",
            "-z",
            "--name-only",
            "--diff-filter=d",
            "--merge-base",
            &base,
            "--",
            "docs/plans/",
        ],
        deadline,
    )?;
    let untracked = bounded(
        &root,
        &[
            "ls-files",
            "-z",
            "--others",
            "--exclude-standard",
            "docs/plans/",
        ],
        deadline,
    )?;
    let names = plan_names(&changed, &untracked);
    let plan = choose(&root, &names, &branch).ok_or(Unestablished)?;
    let line = next_open(&plan.body).ok_or(Unestablished)?;
    if line.contains('🧑') {
        return Err(Unestablished);
    }
    let range = format!("{base}..HEAD");
    let subjects = bounded(&root, &["log", "-z", "--format=%s", &range], deadline)?;
    let subjects: Vec<&[u8]> = subjects
        .split(|c| *c == 0)
        .filter(|s| !s.is_empty())
        .collect();
    // Newest first: commits ahead of the newest plan commit are work.
    let plan_commit = subjects
        .iter()
        .position(|s| s.starts_with(PLAN_COMMIT_PREFIX.as_bytes()));
    let working = match plan_commit {
        Some(position) => position > 0,
        None => !subjects.is_empty(),
    };
    Ok(if working {
        PhaseFact::Working
    } else {
        PhaseFact::PlanOnly
    })
}

/// [`branch_and_base`] with every git child under `deadline`.
fn branch_and_base_bounded(root: &Path, deadline: Instant) -> Result<(String, String), Unknown> {
    let out = bounded_text(
        root,
        &["rev-parse", "--abbrev-ref", "HEAD", "origin/HEAD"],
        deadline,
    );
    let (branch, base) = match out {
        Ok(text) => {
            let mut lines = text.lines();
            let branch = lines.next().unwrap_or("").to_string();
            let base = lines
                .next()
                .filter(|b| !b.is_empty() && *b != "origin/HEAD")
                .map(str::to_string);
            (branch, base)
        }
        Err(Unknown::TimedOut) => return Err(Unknown::TimedOut),
        // rev-parse fails as a whole when `origin/HEAD` is missing.
        Err(Unknown::Unestablished) => (
            bounded_text(root, &["symbolic-ref", "-q", "--short", "HEAD"], deadline)?,
            None,
        ),
    };
    if branch.is_empty() || branch == "HEAD" {
        return Err(Unknown::Unestablished);
    }
    if let Some(base) = base {
        return Ok((branch, base));
    }
    match greenpiste_guard::git::default_ref(root, deadline) {
        Some((base, _)) => Ok((branch, base)),
        // `default_ref` folds a deadline into `None`.
        None if Instant::now() >= deadline => Err(Unknown::TimedOut),
        None => Err(Unknown::Unestablished),
    }
}

/// The plan names in two NUL-separated listings.
fn plan_names(changed: &[u8], untracked: &[u8]) -> Vec<String> {
    let mut names: Vec<String> = changed
        .split(|c| *c == 0)
        .chain(untracked.split(|c| *c == 0))
        .filter_map(|p| std::str::from_utf8(p).ok())
        .filter_map(|p| p.strip_prefix("docs/plans/"))
        .filter(|n| is_plan_name(n))
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// `None` is silence.
fn judge(stop: &Stop, stance: impl FnOnce() -> Stance) -> Option<Decision> {
    if !greenpiste_guard::session_state::usable_id(&stop.session) {
        return None;
    }
    let cwd = stop.cwd.as_deref()?;
    if !cwd.is_dir() {
        return None;
    }
    let root = repo_root(cwd)?;
    if !root.join("docs/plans").is_dir() {
        return None;
    }
    // This runs at the end of every turn, and each git spawn costs 15–30 ms on
    // a busy machine: the branch and the usual default base come from one.
    let (branch, base) = branch_and_base(&root)?;
    if base.split_once('/').map(|(_, b)| b) == Some(branch.as_str()) {
        return None;
    }

    let plan = choose(&root, &candidates(&root, &base)?, &branch)?;
    let line = next_open(&plan.body)?;
    let label = label_of(&line);

    let repo = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".to_string());
    let mode = stop.permission_mode.as_deref().unwrap_or("-");
    let excerpt = |what: &str| {
        format!(
            "{}: {label} ({what}, stop_hook_active={})",
            plan.name, stop.stop_hook_active
        )
    };
    let record = |stance: &str, outcome: &str, what: &str| {
        journal::record(&journal::Entry {
            rule: RULE.id,
            stance,
            outcome,
            session: &stop.session,
            repo: &repo,
            mode,
            excerpt: &excerpt(what),
        });
    };

    let stance = stance();
    let stance_name = stance.as_str();
    let allowed = if line.contains('🧑') {
        Some("decision phase")
    } else if stop.permission_mode.as_deref() == Some("plan") {
        Some("plan mode")
    } else if stop.background_busy {
        Some("background tasks")
    } else if stop.last_message.as_deref().is_some_and(waiting) {
        Some("WAITING")
    } else {
        None
    };
    if let Some(why) = allowed {
        record(stance_name, "watched", why);
        return None;
    }

    match stance {
        Stance::Observe => {
            record(stance_name, "watched", "open phase");
            None
        }
        Stance::Advise => {
            record(stance_name, "advised", "open phase");
            Some(Decision::UserNote(format!(
                "{BIN}/{}: the agent stopped with \"{label}\" open in {}; say \"continue\" to run it.",
                RULE.id, plan.name
            )))
        }
        Stance::Deny => {
            let file = counter_file(&stop.session)?;
            let done = count(&file, &plan.name, &label);
            if done < CAP {
                // A counter that cannot be written means no loop is kept.
                bump(&file, &plan.name, &label, done + 1)?;
                record(stance_name, "denied", &format!("continuation {}", done + 1));
                let fact = format!(
                    "{} has \"{label}\" open, and it is not a 🧑 decision (ADR-0022, work.commits-follow-phases).",
                    plan.name
                );
                let remedy = "Ticking it and committing moves on to the next phase; a line starting with \"WAITING: <reason>\" ends the turn (preview approval, a deferred phase, a blocker).";
                Some(Decision::Continue(decision::phrase(RULE.id, &fact, remedy)))
            } else if done == CAP {
                bump(&file, &plan.name, &label, done + 1)?;
                record(stance_name, "released", "cap reached");
                Some(Decision::UserNote(format!(
                    "{BIN}/{}: stopped with \"{label}\" still open in {} after {CAP} continuations; say \"continue\", or tick it.",
                    RULE.id, plan.name
                )))
            } else {
                record(stance_name, "watched", "after the cap");
                None
            }
        }
    }
}

/// A prompt from the person: three fresh continuations.
pub fn on_prompt(prompt: &Prompt) {
    let Some(file) = counter_file(&prompt.session) else {
        return;
    };
    match std::fs::remove_file(&file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            // The person's "continue" bought no fresh continuations: say so
            // where `status` will count it, rather than nowhere.
            journal::record(&journal::Entry {
                rule: RULE.id,
                stance: "-",
                outcome: "error",
                session: &prompt.session,
                repo: "-",
                mode: "-",
                excerpt: &format!("counter not reset: {e}"),
            });
        }
        _ => {}
    }
}

/// Counter files older than a week are nobody's session any more.
pub fn sweep() {
    let Some(dir) = counter_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let week = std::time::Duration::from_secs(7 * 24 * 3600);
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > week);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn counter_dir() -> Option<PathBuf> {
    Some(journal::dir()?.join("plan-phases-open"))
}

fn counter_file(session: &str) -> Option<PathBuf> {
    if !greenpiste_guard::session_state::usable_id(session) {
        return None;
    }
    Some(counter_dir()?.join(session))
}

/// Continuations so far for this (plan, phase); anything unreadable or for
/// another phase is zero.
fn count(file: &Path, plan: &str, label: &str) -> u32 {
    let Ok(text) = std::fs::read_to_string(file) else {
        return 0;
    };
    let mut lines = text.lines();
    match (lines.next(), lines.next(), lines.next()) {
        (Some(p), Some(l), Some(n)) if p == plan && l == label => n.parse().unwrap_or(0),
        _ => 0,
    }
}

fn bump(file: &Path, plan: &str, label: &str, n: u32) -> Option<()> {
    let dir = file.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    journal::private(dir, 0o700);
    greenpiste_guard::atomic::write_atomic(file, &format!("{plan}\n{label}\n{n}\n")).ok()
}

/// The directory holding the first `.git` entry at or above `cwd`.
fn repo_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The checked-out branch and the remote default branch (`origin/main`).
/// `None` on a detached HEAD, which `--abbrev-ref` prints as `HEAD`.
fn branch_and_base(root: &Path) -> Option<(String, String)> {
    let out = greenpiste_guard::git::stdout_in(
        root,
        &["rev-parse", "--abbrev-ref", "HEAD", "origin/HEAD"],
    );
    let mut lines = out.as_deref().unwrap_or("").lines();
    let branch = match lines.next() {
        Some(b) if !b.is_empty() && b != "HEAD" => b.to_string(),
        // rev-parse fails as a whole when `origin/HEAD` is missing, so the
        // branch is asked again on its own before giving up.
        None => greenpiste_guard::git::stdout_in(root, &["symbolic-ref", "-q", "--short", "HEAD"])
            .filter(|b| !b.is_empty())?,
        Some(_) => return None,
    };
    if let Some(base) = lines
        .next()
        .filter(|b| !b.is_empty() && *b != "origin/HEAD")
    {
        return Some((branch, base.to_string()));
    }
    // A remote not named origin, or no `origin/HEAD`: the slow, general path.
    let remote = greenpiste_guard::stale::remote_of(root)?;
    let base = greenpiste_guard::stale::default_base(root, &remote)?;
    Some((branch, base))
}

/// The plan files that differ between the merge base with `base` and the
/// working tree, plus the ones not yet added. `None` when git cannot be asked.
fn candidates(root: &Path, base: &str) -> Option<Vec<String>> {
    let changed = greenpiste_guard::git::bytes_in(
        root,
        &[
            "diff",
            "-z",
            "--name-only",
            "--diff-filter=d",
            "--merge-base",
            base,
            "--",
            "docs/plans/",
        ],
        None,
    )
    .ok()??;
    let untracked = greenpiste_guard::git::bytes_in(
        root,
        &[
            "ls-files",
            "-z",
            "--others",
            "--exclude-standard",
            "docs/plans/",
        ],
        None,
    )
    .ok()??;
    let names = plan_names(&changed, &untracked);
    Some(names)
}

/// A plan's file name under `docs/plans/`, that prefix stripped.
pub(crate) fn is_plan_name(n: &str) -> bool {
    n.ends_with(".md")
        && n != "README.md"
        && !n.ends_with(".reviews.md")
        && !n.contains('/')
        && !n.contains(|c: char| c.is_control())
}

struct Plan {
    name: String,
    branch: String,
    body: String,
}

/// The active, non-pointer plan this branch is working: the one whose
/// `branch:` is the current branch, else the oldest by file name. A
/// candidate that cannot be read is skipped alone.
fn choose(root: &Path, names: &[String], branch: &str) -> Option<Plan> {
    let mut plans: Vec<Plan> = Vec::new();
    for name in names {
        let Ok(text) = std::fs::read_to_string(root.join("docs/plans").join(name)) else {
            continue;
        };
        let (front, body) = crate::plans::split_front_matter(&text);
        if crate::plans::field(front, "status").as_deref() != Some("active") {
            continue;
        }
        if crate::plans::field(front, "canonical").is_some_and(|c| !c.is_empty()) {
            continue;
        }
        plans.push(Plan {
            name: name.clone(),
            branch: crate::plans::field(front, "branch").unwrap_or_default(),
            body: body.to_string(),
        });
    }
    let at = plans.iter().position(|p| p.branch == branch).unwrap_or(0);
    (!plans.is_empty()).then(|| plans.swap_remove(at))
}

/// The first unindented `- [ ] ` line inside `## Phases`, without its marker.
fn next_open(body: &str) -> Option<String> {
    let mut in_phases = false;
    let mut fenced = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if line.starts_with("## ") {
            in_phases = line.trim_end() == "## Phases";
            continue;
        }
        if in_phases {
            if let Some(rest) = line.strip_prefix("- [ ] ") {
                return Some(rest.trim().to_string());
            }
        }
    }
    None
}

/// The text before the first ` — `, capped at [`LABEL_MAX`] characters.
fn label_of(line: &str) -> String {
    let head = line.split(" — ").next().unwrap_or(line).trim();
    if head.chars().count() <= LABEL_MAX {
        return head.to_string();
    }
    let kept: String = head.chars().take(LABEL_MAX - 1).collect();
    format!("{kept}…")
}

/// A line that starts with `WAITING:` once trimmed.
fn waiting(message: &str) -> bool {
    message.lines().any(|l| l.trim().starts_with("WAITING:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = "# P\n\n## Phases\n- [x] Phase 1 — a\n- [ ] Phase 2 — b\n- [ ] Phase 3\n\n## Verification\n- [ ] check\n";

    #[test]
    fn the_first_open_box_in_phases_is_next() {
        assert_eq!(next_open(PLAN).as_deref(), Some("Phase 2 — b"));
    }

    #[test]
    fn a_box_inside_a_fence_is_ignored() {
        let body = "## Phases\n```\n- [ ] fenced\n```\n- [ ] real\n";
        assert_eq!(next_open(body).as_deref(), Some("real"));
        assert_eq!(next_open("## Phases\n```\n- [ ] fenced\n```\n"), None);
    }

    #[test]
    fn an_indented_box_is_ignored() {
        assert_eq!(next_open("## Phases\n  - [ ] nested\n"), None);
        assert_eq!(
            next_open("## Phases\n\t- [ ] nested\n- [ ] top\n").as_deref(),
            Some("top")
        );
    }

    #[test]
    fn a_box_only_in_verification_is_ignored() {
        assert_eq!(
            next_open("## Phases\n- [x] done\n\n## Verification\n- [ ] later\n"),
            None
        );
    }

    #[test]
    fn the_label_stops_at_the_dash_and_is_capped_at_eighty() {
        assert_eq!(label_of("Phase 2 — the rule: x"), "Phase 2");
        assert_eq!(label_of("no dash here"), "no dash here");
        let long = "x".repeat(200);
        let capped = label_of(&long);
        assert_eq!(capped.chars().count(), 80);
        assert!(capped.ends_with('…'));
        assert_eq!(label_of(&"y".repeat(80)).chars().count(), 80);
        assert!(!label_of(&"y".repeat(80)).ends_with('…'));
    }

    #[test]
    fn waiting_may_be_indented_but_must_start_the_line() {
        assert!(waiting("done\n   WAITING: preview at :5173"));
        assert!(waiting("WAITING: x"));
        assert!(!waiting("I am WAITING: for nothing"));
        assert!(!waiting("waiting: lower case"));
    }

    #[test]
    fn plan_names_exclude_readmes_and_reviews() {
        assert!(is_plan_name("2026-10-07-x.md"));
        assert!(!is_plan_name("README.md"));
        assert!(!is_plan_name("2026-10-07-x.reviews.md"));
        assert!(!is_plan_name("sub/x.md"));
        assert!(!is_plan_name("x.txt"));
    }

    mod open_phase {
        use super::super::*;
        use crate::clear_boundary::testrepo::TestRepo;
        use std::time::Duration;

        const OPEN_PLAN: &str =
            "---\nstatus: active\nbranch: feature\n---\n# P\n\n## Phases\n- [ ] Phase 1 — a\n";
        const DONE_PLAN: &str =
            "---\nstatus: active\nbranch: feature\n---\n# P\n\n## Phases\n- [x] Phase 1 — a\n";

        fn stop_in(repo: &TestRepo) -> Stop {
            Stop {
                session: "s1".to_string(),
                cwd: Some(repo.path().to_path_buf()),
                permission_mode: None,
                last_message: None,
                background_busy: false,
                stop_hook_active: false,
                transcript: None,
            }
        }

        fn repo_with_plan(plan: &str) -> TestRepo {
            let repo = TestRepo::new();
            repo.commit_file("README.md", "x", "init");
            repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
            repo.git(&["checkout", "-q", "-b", "feature"]);
            repo.commit_file("docs/plans/p.md", plan, "docs(plan): p");
            repo
        }

        fn fact(repo: &TestRepo) -> PhaseFact {
            open_phase(&stop_in(repo), Instant::now() + Duration::from_secs(20))
        }

        #[test]
        fn a_lone_plan_commit_is_plan_only() {
            assert_eq!(fact(&repo_with_plan(OPEN_PLAN)), PhaseFact::PlanOnly);
        }

        #[test]
        fn a_commit_after_the_plan_commit_is_working() {
            let repo = repo_with_plan(OPEN_PLAN);
            repo.commit_file("src.rs", "fn main() {}", "feat: a");
            assert_eq!(fact(&repo), PhaseFact::Working);
        }

        #[test]
        fn no_open_phase_is_none() {
            assert_eq!(fact(&repo_with_plan(DONE_PLAN)), PhaseFact::NoOpenPhase);
        }

        #[test]
        fn a_decision_phase_is_none() {
            let plan = OPEN_PLAN.replace("Phase 1", "🧑 decision: pick");
            assert_eq!(fact(&repo_with_plan(&plan)), PhaseFact::NoOpenPhase);
        }

        #[test]
        fn a_passed_deadline_is_timed_out() {
            let repo = repo_with_plan(OPEN_PLAN);
            assert_eq!(
                open_phase(&stop_in(&repo), Instant::now()),
                PhaseFact::TimedOut
            );
        }
    }
}
