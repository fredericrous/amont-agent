//! Preview approval (ADR-0023): a UI-changing commit is published only after
//! the person approves that exact commit, seen on localhost.
//!
//! ## The flow this module keeps honest
//!
//! 1. The agent verifies the final commit itself, then runs
//!    `amont-agent preview register --url … --guide …` as a standalone
//!    command. That command VALIDATES (clean tree, real commit, a complete
//!    guide outside the worktree), renders the guide to `index.html` beside
//!    it ([`crate::guide`]) and prints JSON; the PostToolUse hook binds that
//!    output to the session and the person's prompt ([`bind`]).
//! 2. In the same turn the agent asks a *marked* question —
//!    `[preview <id>,…]` in its text, every `repo@sha` listed, options exactly
//!    `Approve` / `Request changes` / `Hold`.
//! 3. The person answers. Only their selection, read from `tool_response`
//!    ([`on_post_ask`]), or a typed `approve|ship|lgtm|looks good` in the very
//!    next prompt ([`on_prompt`]), approves — and only the commits the
//!    question listed.
//!
//! What this is NOT: proof. A registration is the agent's attestation of
//! worktree, commit and URL; nothing here can show the browser served that
//! commit or that the person looked. It is a workflow aid over Claude Code's
//! hooks, not a Git enforcement boundary.
//!
//! ## Why the answer has to be checked twice
//!
//! `AskUserQuestion`'s input schema accepts `answers`. A model could
//! pre-answer its own question, and the PostToolUse payload would then carry
//! an "Approve" nobody chose. [`on_pre_ask`] records, per `tool_use_id`,
//! whether the call arrived pre-answered; a pre-answered marked question never
//! approves, and under `deny` it is refused before it runs.

use std::path::{Path, PathBuf};

use crate::journal;
use crate::push_target::{self, Kind, Push, Target};
use crate::rules::Stance;

/// A registration nobody answered lapses after a day: the commit pins the
/// content, and any other prompt drops it long before this.
const REGISTRATION_TTL: u64 = 24 * 3600;
/// An approval unused for a day lapses too.
const APPROVAL_TTL: u64 = 24 * 3600;
/// Per-call scratch (`pushes/`, `asks/`) older than this is nobody's call.
const SCRATCH_TTL: u64 = 24 * 3600;

pub const RULE_ID: &str = "push-preview";

/// The labels of a marked question. Exact, with no "(Recommended)" suffix.
pub const APPROVE: &str = "Approve";
pub const REQUEST_CHANGES: &str = "Request changes";
pub const HOLD: &str = "Hold";

fn now() -> u64 {
    now_ms() / 1000
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// How long the person took to answer, as the journal spells it:
/// `<secs>s,dur=<duration_ms>ms`. The seconds are ours — PostToolUse time
/// minus the PreToolUse time recorded in `asks/<tool_use_id>` — because the
/// payload's `duration_ms` is not the person's answer time (a minutes-long
/// approval arrived as 0). The raw `duration_ms` rides along for comparison.
fn latency(asked_ms: Option<u64>, answered_ms: u64, duration_ms: Option<u64>) -> String {
    let secs = asked_ms
        .filter(|&a| a > 0 && a <= answered_ms)
        .map(|a| format!("{}s", (answered_ms - a) / 1000))
        .unwrap_or_else(|| "-".to_string());
    let dur = duration_ms
        .map(|d| format!("{d}ms"))
        .unwrap_or_else(|| "-".to_string());
    format!("{secs},dur={dur}")
}

fn dir() -> Option<PathBuf> {
    Some(journal::dir()?.join("previews"))
}

fn ensure(dir: &Path) -> Option<()> {
    std::fs::create_dir_all(dir).ok()?;
    journal::private(dir, 0o700);
    Some(())
}

// --- gating ---------------------------------------------------------------

/// Whether a repository has a user interface a person can preview.
///
/// `git config amont.agent.push-preview.ui true|false` decides when set (the
/// repository's own config, which a clone never carries). Otherwise: a `dev`
/// script in `package.json` at the root or in `web/`.
pub fn gated(repo: &Path) -> bool {
    match crate::git::stdout_in(repo, &["config", "--get", "amont.agent.push-preview.ui"])
        .as_deref()
    {
        Some("true") => return true,
        Some("false") => return false,
        _ => {}
    }
    ["package.json", "web/package.json"]
        .iter()
        .any(|p| has_dev_script(&repo.join(p)))
}

fn has_dev_script(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("scripts")?.get("dev").cloned())
        .is_some_and(|d| d.as_str().is_some_and(|s| !s.trim().is_empty()))
}

/// A path whose change a person could see.
pub fn is_ui_path(path: &str) -> bool {
    let top = ["app/", "src/", "web/"];
    let ext = [".tsx", ".jsx", ".css", ".html"];
    top.iter()
        .any(|t| path.starts_with(t) || path.contains(&format!("/{t}")))
        || ext.iter().any(|e| path.ends_with(e))
}

/// What a branch push would publish: the files, and the base they were
/// diffed against (`None` when they were listed from `git log` instead).
struct Published {
    base: Option<String>,
    files: Vec<String>,
}

/// The files a branch push would publish, or `None` when that cannot be
/// established without guessing.
fn published(repo: &Path, t: &Target) -> Option<Published> {
    let short = t.dst.strip_prefix("refs/heads/")?;
    let tracking = format!("refs/remotes/{}/{}", t.remote, short);
    let base = if git(repo, &["rev-parse", "--verify", "--quiet", &tracking]).is_some() {
        Some(tracking)
    } else {
        git(
            repo,
            &[
                "merge-base",
                &t.src,
                &format!("refs/remotes/{}/HEAD", t.remote),
            ],
        )
    };
    let out = match &base {
        Some(b) => {
            crate::git::stdout_in(repo, &["diff", "--name-only", &format!("{b}..{}", t.src)])?
        }
        None => crate::git::stdout_in(
            repo,
            &[
                "log",
                "--format=",
                "--name-only",
                &t.src,
                "--not",
                &format!("--remotes={}", t.remote),
            ],
        )?,
    };
    Some(Published {
        base,
        files: out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    })
}

/// Whether a branch push carries an interface change, or `None` when its
/// files cannot be listed. Stops at the first file that counts.
fn carries_ui(repo: &Path, t: &Target) -> Option<bool> {
    let p = published(repo, t)?;
    let mut packages = std::collections::HashMap::new();
    Some(
        p.files
            .iter()
            .any(|f| counts_as_ui(repo, p.base.as_deref(), t, f, &mut packages)),
    )
}

// --- what counts as an interface change -----------------------------------

/// One changed file counts when all hold: its path looks like an interface
/// ([`is_ui_path`]); its nearest `package.json` in the pushed commit looks
/// like one ([`package_is_ui`]); and its diff is not comments only
/// ([`comment_only`]). Every doubt — an unreadable manifest, a diff that
/// cannot be taken — counts.
fn counts_as_ui(
    repo: &Path,
    base: Option<&str>,
    t: &Target,
    file: &str,
    packages: &mut std::collections::HashMap<String, bool>,
) -> bool {
    let commit = t.src.as_str();
    if !is_ui_path(file) {
        return false;
    }
    if !nearest_package_is_ui(repo, commit, file, packages) {
        return false;
    }
    let commentable = [".ts", ".tsx", ".js", ".jsx", ".css"]
        .iter()
        .any(|e| file.ends_with(e));
    if !commentable {
        return true;
    }
    let range = base.map(|b| format!("{b}..{commit}"));
    let not_remote = format!("--remotes={}", t.remote);
    let args: Vec<&str> = match &range {
        Some(r) => vec!["diff", "--no-ext-diff", "--no-color", "-U0", r, "--", file],
        // No single base: every commit the remote lacks, each against its
        // first parent. Comments-only in each is comments-only in all.
        None => vec![
            "log",
            "-p",
            "-U0",
            "--format=",
            "--no-ext-diff",
            "--no-color",
            "--first-parent",
            "--diff-merges=first-parent",
            commit,
            "--not",
            &not_remote,
            "--",
            file,
        ],
    };
    match crate::git::stdout_in(repo, &args) {
        Some(diff) => !comment_only(&diff),
        None => true,
    }
}

/// Walk up from the file to the repository root, in the pushed commit, and
/// judge the first `package.json` met. None at all: the path decides.
fn nearest_package_is_ui(
    repo: &Path,
    commit: &str,
    file: &str,
    cache: &mut std::collections::HashMap<String, bool>,
) -> bool {
    let mut dir = Path::new(file).parent();
    loop {
        let d = dir
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(&known) = cache.get(&d) {
            return known;
        }
        let manifest = if d.is_empty() {
            "package.json".to_string()
        } else {
            format!("{d}/package.json")
        };
        if let Some(text) = crate::git::stdout_in(repo, &["show", &format!("{commit}:{manifest}")])
        {
            let ui = package_is_ui(&text);
            cache.insert(d, ui);
            return ui;
        }
        if d.is_empty() {
            cache.insert(d, true);
            return true;
        }
        dir = dir.and_then(Path::parent);
    }
}

/// Packages whose presence says "this renders something".
const UI_PACKAGES: &[&str] = &[
    "react",
    "react-dom",
    "react-native",
    "react-strict-dom",
    "@duro-app/ui",
];

/// A `package.json` that looks like an interface: a `dev` script, or a UI
/// package among its `dependencies` or its non-optional `peerDependencies`.
///
/// `devDependencies` and optional peers do not count: a CLI that reads the
/// design system's metadata (duro-design-system's `packages/cli`) carries
/// `@duro-app/ui` exactly there, and renders nothing. Unparseable counts.
pub fn package_is_ui(text: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return true;
    };
    let dev = v
        .get("scripts")
        .and_then(|s| s.get("dev"))
        .and_then(|d| d.as_str())
        .is_some_and(|s| !s.trim().is_empty());
    if dev {
        return true;
    }
    let has = |section: &str, name: &str| {
        v.get(section)
            .and_then(|d| d.as_object())
            .is_some_and(|d| d.contains_key(name))
    };
    let optional = |name: &str| {
        v.get("peerDependenciesMeta")
            .and_then(|m| m.get(name))
            .and_then(|m| m.get("optional"))
            .and_then(|o| o.as_bool())
            .unwrap_or(false)
    };
    UI_PACKAGES
        .iter()
        .any(|name| has("dependencies", name) || (has("peerDependencies", name) && !optional(name)))
}

/// Whether a `git diff -U0` of one file changes comments and nothing else:
/// at least one hunk, and every added or removed line that is not blank is
/// a comment line ([`is_comment_line`]). No hunk at all (a binary file, a
/// mode change) is not "comments only".
pub fn comment_only(diff: &str) -> bool {
    let mut hunks = false;
    let mut header = true;
    for line in diff.lines() {
        if line.starts_with("diff ") {
            // A file's header — `index`, `---`, `+++` — until its first hunk.
            header = true;
            continue;
        }
        if line.starts_with("@@") {
            hunks = true;
            header = false;
            continue;
        }
        if header {
            continue;
        }
        let body = match line.as_bytes().first() {
            Some(b'+') | Some(b'-') => &line[1..],
            _ => continue,
        };
        if body.trim().is_empty() {
            continue;
        }
        if !is_comment_line(body) {
            return false;
        }
    }
    hunks
}

/// A line that, on its own, is nothing but a comment. Conservative: a line
/// that also carries code — `/* x */ foo()`, `*/ bar` — is not, and neither
/// is a `*` line that reads like code (a CSS `* {` rule, a continued
/// multiplication ending in `;`).
pub fn is_comment_line(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with("//") {
        return true;
    }
    if let Some(rest) = t.strip_prefix("{/*") {
        // A JSX comment: closed on this line with `*/}`, or not closed yet.
        return match rest.find("*/") {
            None => !rest.contains('}'),
            Some(i) => rest[i..] == *"*/}",
        };
    }
    if let Some(rest) = t.strip_prefix("/*") {
        return match rest.find("*/") {
            None => true,
            Some(i) => rest[i..] == *"*/",
        };
    }
    if let Some(rest) = t.strip_prefix("*/") {
        // The end of a block comment — `*/`, or a JSX comment's `*/}`.
        return rest.is_empty() || rest == "}";
    }
    if let Some(rest) = t.strip_prefix('*') {
        // Inside a block comment: `*`, `* text`, `* text */`.
        if !(rest.is_empty() || rest.starts_with([' ', '\t']) || rest.starts_with("*/")) {
            return false;
        }
        let body = rest.trim();
        if let Some(i) = body.find("*/") {
            return &body[i..] == "*/" && !body.contains("/*");
        }
        if body.starts_with('@') {
            // A JSDoc tag: `* @param {string} name`.
            return true;
        }
        // What reads like code: a CSS `* {` rule, a statement's end.
        return !body.ends_with(['{', '}', ';']) && !body.contains('{');
    }
    false
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    crate::git::stdout_in(dir, args).filter(|s| !s.is_empty())
}

/// The rule's question: would this push publish an interface change that no
/// approved preview covers? `Ok` fires the rule; `Err` names why it did not.
/// `Ok(held)` when the push needs a preview it does not have; `held` when
/// the unapproved commit carries a picked mockup (or cannot be told apart
/// from one), which the rule holds at deny.
pub fn needs_preview(
    cwd: &Path,
    cmd: &crate::shell::Simple,
    stance: Stance,
) -> Result<bool, &'static str> {
    match push_target::resolve(cwd, cmd) {
        Push::DryRun => Err("a dry run publishes nothing"),
        Push::Unresolvable { repo, shape } => {
            if repo.as_deref().is_some_and(gated) && stance == Stance::Deny {
                // Under deny a shape nobody can read is held: `--all` must
                // not be the way around the gate.
                Ok(false)
            } else {
                Err(shape)
            }
        }
        Push::Resolved { repo, targets } => {
            if !gated(&repo) {
                return Err("not a repository with a user interface");
            }
            let mut any_ui = false;
            for t in targets.iter().filter(|t| t.kind == Kind::Branch) {
                let Some(ui) = carries_ui(&repo, t) else {
                    return if stance == Stance::Deny {
                        Ok(false)
                    } else {
                        Err("the published files cannot be listed")
                    };
                };
                if ui {
                    any_ui = true;
                    if !approved(&repo, &t.src) {
                        return Ok(mockup_screens(&repo, &t.src).map_or(true, |s| !s.is_empty()));
                    }
                }
            }
            if any_ui {
                Err("every interface change is approved")
            } else {
                Err("no interface change")
            }
        }
    }
}

// --- mockup mode -----------------------------------------------------------

/// Where a repository keeps its picked mockups: `git config
/// amont.agent.preview.mockups`, else `docs/mockups` — the same key the
/// application-landscape `ui-handoff` check reads.
fn mockups_dir(repo: &Path) -> String {
    git(repo, &["config", "--get", "amont.agent.preview.mockups"])
        .filter(|d| !d.trim().is_empty())
        .map(|d| d.trim().trim_end_matches('/').to_string())
        .unwrap_or_else(|| "docs/mockups".to_string())
}

/// The screens a commit's branch carries picked mockups for: folders under
/// the mockups directory that `merge-base(<default branch>, commit)..commit`
/// changes and that hold a `*.dc.html` artboard at `commit`.
///
/// Always from the merge base, never from the upstream ref: after the first
/// push the upstream already holds the artboards, and a range that starts
/// there would find no screen and switch the check off (the PR #302 case).
/// `Err` when the range cannot be established while the commit does carry
/// artboards: a check that cannot look must not pass.
pub fn mockup_screens(repo: &Path, commit: &str) -> Result<Vec<String>, String> {
    let dir = mockups_dir(repo);
    let has_artboards = |tree_path: &str| {
        git(
            repo,
            &["ls-tree", "-r", "--name-only", commit, "--", tree_path],
        )
        .is_some_and(|l| l.lines().any(|f| f.ends_with(".dc.html")))
    };
    if !has_artboards(&dir) {
        return Ok(Vec::new());
    }
    let base = crate::stale::remote_of(repo)
        .and_then(|r| crate::stale::default_base(repo, &r))
        .ok_or_else(|| {
            format!(
                "{dir} holds artboards, and the commits this preview adds cannot be told apart from the default branch's: no default remote branch (set `checkout.defaultRemote`, or fetch `origin`)"
            )
        })?;
    let from = git(repo, &["merge-base", &base, commit]).ok_or_else(|| {
        format!(
            "{dir} holds artboards, and {base} shares no history with {}",
            short(commit)
        )
    })?;
    let changed = git(repo, &["diff", "--name-only", &format!("{from}..{commit}")])
        .ok_or_else(|| format!("`git diff {}..{}` failed", short(&from), short(commit)))?;
    let prefix = format!("{dir}/");
    let mut screens: Vec<String> = Vec::new();
    for f in changed.lines() {
        let Some(rest) = f.strip_prefix(&prefix) else {
            continue;
        };
        let Some((name, _)) = rest.split_once('/') else {
            continue;
        };
        let screen = format!("{dir}/{name}");
        if !screens.contains(&screen) && has_artboards(&screen) {
            screens.push(screen);
        }
    }
    Ok(screens)
}

// --- the store ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Registration {
    id: String,
    session: String,
    prompt: String,
    repo: String,
    commit: String,
    url: String,
    /// The guide's path (the attestation's, before 2.21.0).
    guide: String,
    at: u64,
    /// The `tool_use_id` of the marked question that listed it, or `-`.
    asked: String,
}

impl Registration {
    fn line(&self) -> String {
        [
            self.id.as_str(),
            &self.session,
            &self.prompt,
            &self.repo,
            &self.commit,
            &self.url,
            &self.guide,
            &self.at.to_string(),
            &self.asked,
        ]
        .map(clean)
        .join("\t")
    }
    fn parse(line: &str) -> Option<Registration> {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 9 {
            return None;
        }
        Some(Registration {
            id: f[0].into(),
            session: f[1].into(),
            prompt: f[2].into(),
            repo: f[3].into(),
            commit: f[4].into(),
            url: f[5].into(),
            guide: f[6].into(),
            at: f[7].parse().ok()?,
            asked: f[8].into(),
        })
    }
    fn label(&self) -> String {
        let name = Path::new(&self.repo)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("{name}@{}", short(&self.commit))
    }
    /// Every label a question may show for this commit: the checkout's own
    /// directory, then the aliases `register` worked out (the main worktree's
    /// directory, the origin repository's name), each `@<sha7>`.
    fn labels(&self) -> Vec<String> {
        let mut all = vec![self.label()];
        for a in aliases_of(&self.id) {
            if !all.contains(&a) {
                all.push(a);
            }
        }
        all
    }
}

/// The names a person knows a checkout by, most specific first: its
/// directory, the main worktree's directory (a task worktree is
/// `app-wt-x`; the person calls it `app`), and the origin repository's name.
fn checkout_names(repo: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let s = s.trim();
        if !s.is_empty() && !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    };
    if let Some(n) = repo.file_name() {
        push(&n.to_string_lossy());
    }
    if let Some(common) = git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) {
        let common = PathBuf::from(common);
        if common.file_name().is_some_and(|n| n == ".git") {
            if let Some(n) = common.parent().and_then(Path::file_name) {
                push(&n.to_string_lossy());
            }
        }
    }
    if let Some(url) = git(repo, &["remote", "get-url", "origin"]) {
        let last = url
            .trim_end_matches('/')
            .rsplit(['/', ':'])
            .next()
            .unwrap_or("");
        push(last.trim_end_matches(".git"));
    }
    out
}

/// The alias labels bound with registration `id`, from the sidecar that
/// keeps the registrations line in the format older releases read.
fn aliases_of(id: &str) -> Vec<String> {
    let Some(path) = dir().map(|d| d.join("registrations.labels")) else {
        return Vec::new();
    };
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .find_map(|l| {
            let (i, rest) = l.split_once('\t')?;
            (i == id).then(|| {
                rest.split(' ')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Record `id`'s alias labels, keeping only the lines of live registrations.
fn save_aliases(id: &str, labels: &[String], live: &[Registration]) {
    let Some(d) = dir() else { return };
    if ensure(&d).is_none() {
        return;
    }
    let path = d.join("registrations.labels");
    let mut body: String = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|l| {
            l.split_once('\t')
                .is_some_and(|(i, _)| i != id && live.iter().any(|r| r.id == i))
        })
        .map(|l| format!("{l}\n"))
        .collect();
    body.push_str(&format!(
        "{}\t{}\n",
        clean(id),
        labels
            .iter()
            .map(|l| clean(l))
            .collect::<Vec<_>>()
            .join(" ")
    ));
    let _ = crate::atomic::write_atomic(&path, &body);
    journal::private(&path, 0o600);
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// Live registrations; lapsed ones are journalled and dropped here, on read,
/// so expiry holds in a session that never restarts.
fn registrations() -> Vec<Registration> {
    let Some(path) = dir().map(|d| d.join("registrations")) else {
        return Vec::new();
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let all: Vec<Registration> = text.lines().filter_map(Registration::parse).collect();
    let cutoff = now().saturating_sub(REGISTRATION_TTL);
    let (live, lapsed): (Vec<_>, Vec<_>) = all.into_iter().partition(|r| r.at >= cutoff);
    if !lapsed.is_empty() {
        for r in &lapsed {
            note(
                "expired",
                &r.session,
                &r.repo,
                &format!("registration {} {}", r.id, r.label()),
            );
        }
        save_registrations(&live);
    }
    live
}

fn save_registrations(regs: &[Registration]) {
    let Some(d) = dir() else { return };
    if ensure(&d).is_none() {
        return;
    }
    let mut body: String = regs.iter().map(|r| r.line() + "\n").collect();
    if body.is_empty() {
        body.push('\n');
    }
    let path = d.join("registrations");
    let _ = crate::atomic::write_atomic(&path, &body);
    journal::private(&path, 0o600);
}

fn approvals() -> Vec<(String, String, u64, String)> {
    let Some(path) = dir().map(|d| d.join("approvals")) else {
        return Vec::new();
    };
    let cutoff = now().saturating_sub(APPROVAL_TTL);
    std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let at: u64 = f.get(2)?.parse().ok()?;
            Some((
                f.first()?.to_string(),
                f.get(1)?.to_string(),
                at,
                f.get(3).unwrap_or(&"-").to_string(),
            ))
        })
        .filter(|(_, _, at, _)| *at >= cutoff)
        .collect()
}

/// Whether the person approved exactly this commit of this repository.
pub fn approved(repo: &Path, commit: &str) -> bool {
    let repo = repo.to_string_lossy();
    approvals()
        .iter()
        .any(|(r, c, _, _)| *r == repo && c == commit)
}

fn approve(regs: &[Registration], channel: &str) {
    let Some(d) = dir() else { return };
    if ensure(&d).is_none() {
        return;
    }
    let mut kept = approvals();
    for r in regs {
        kept.retain(|(repo, c, _, _)| !(repo == &r.repo && c == &r.commit));
        kept.push((r.repo.clone(), r.commit.clone(), now(), channel.to_string()));
        note(
            "approved",
            &r.session,
            &r.repo,
            &format!("{} by {channel}", r.label()),
        );
    }
    let body: String = kept
        .iter()
        .map(|(r, c, at, ch)| format!("{}\t{}\t{at}\t{}\n", clean(r), clean(c), clean(ch)))
        .collect();
    let path = d.join("approvals");
    let _ = crate::atomic::write_atomic(&path, &body);
    journal::private(&path, 0o600);
}

/// One journal line under the rule's id. The soak reads these back.
fn note(outcome: &str, session: &str, repo: &str, excerpt: &str) {
    let name = Path::new(repo)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "-".to_string());
    journal::record(&journal::Entry {
        rule: RULE_ID,
        stance: "-",
        outcome,
        session,
        repo: &name,
        mode: "-",
        excerpt,
    });
}

// --- registration ---------------------------------------------------------

/// What `preview register` validated and printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    pub id: String,
    pub repo: String,
    pub commit: String,
    pub url: String,
    /// The guide, absolute.
    pub guide: String,
    /// `index.html` beside the guide, absolute; empty when read from a
    /// release that rendered none.
    pub page: String,
    /// The page as a `file://` URL.
    pub page_url: String,
    /// What the marked question must show: `<checkout>@<sha7>`.
    pub label: String,
    /// Other labels that name the same commit: the main worktree's
    /// directory and the origin repository's name, each `@<sha7>`.
    pub aliases: Vec<String>,
}

impl Registered {
    /// `attestation` repeats `guide` for one release, so a hook older than
    /// the CLI that printed this still binds it.
    pub fn to_json(&self) -> String {
        crate::json::object(&[
            crate::json::string_field("id", &self.id),
            crate::json::string_field("repo", &self.repo),
            crate::json::string_field("commit", &self.commit),
            crate::json::string_field("url", &self.url),
            crate::json::string_field("guide", &self.guide),
            crate::json::string_field("page", &self.page),
            crate::json::string_field("page_url", &self.page_url),
            crate::json::string_field("label", &self.label),
            crate::json::string_array_field("aliases", &self.aliases),
            crate::json::string_field("attestation", &self.guide),
        ])
    }
    /// Reads 2.21.0's shape, and the older one that carried only
    /// `attestation`.
    fn from_json(text: &str) -> Option<Registered> {
        let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
        let s = |k: &str| v.get(k)?.as_str().map(str::to_string);
        Some(Registered {
            id: s("id")?,
            repo: s("repo")?,
            commit: s("commit")?,
            url: s("url")?,
            guide: s("guide").or_else(|| s("attestation"))?,
            page: s("page").unwrap_or_default(),
            page_url: s("page_url").unwrap_or_default(),
            label: s("label").unwrap_or_default(),
            aliases: v
                .get("aliases")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

/// Validate a preview for registration and render its guide to `index.html`
/// beside it. Touches no store: the hook binds.
pub fn register(repo_dir: &Path, url: &str, guide: &Path) -> Result<Registered, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("`{url}` is not an http(s) URL"));
    }
    let repo = push_target::toplevel(repo_dir)
        .ok_or_else(|| format!("{} is not inside a git repository", repo_dir.display()))?;
    let commit = git(
        &repo,
        &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
    )
    .ok_or_else(|| "HEAD is not a commit".to_string())?;
    let status = crate::git::stdout_in(&repo, &["status", "--porcelain"])
        .ok_or_else(|| "`git status` failed".to_string())?;
    if !status.trim().is_empty() {
        return Err(format!(
            "the worktree is not clean, so the preview is not of commit {}:\n{status}",
            short(&commit)
        ));
    }
    let record = std::fs::canonicalize(guide)
        .map_err(|_| format!("the guide {} does not exist", guide.display()))?;
    let size = std::fs::metadata(&record).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return Err(format!("the guide {} is empty", record.display()));
    }
    if size > crate::guide::MAX_BYTES {
        return Err(format!(
            "the guide {} is {size} bytes; a guide is a page, not a log",
            record.display()
        ));
    }
    let canon_repo = std::fs::canonicalize(&repo).unwrap_or_else(|_| repo.clone());
    if record.starts_with(&canon_repo) {
        return Err(format!(
            "the guide {} is inside the worktree; writing it there (and its page beside it) dirties the commit it describes. Keep it under {}",
            record.display(),
            dir()
                .and_then(|d| d.parent().map(|p| p.join("attestations")))
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "the amont-agent state directory".to_string())
        ));
    }
    let text = std::fs::read_to_string(&record)
        .map_err(|_| format!("the guide {} is not UTF-8 text", record.display()))?;
    let folder = record.parent().unwrap_or(Path::new("/")).to_path_buf();
    let screens = mockup_screens(&repo, &commit)?;
    let mut missing = crate::guide::check(&text);
    if !screens.is_empty() {
        missing.extend(crate::guide::check_mockup(&text, &screens, &folder));
    }
    if !missing.is_empty() {
        let mut sections: Vec<String> = crate::guide::SECTIONS
            .iter()
            .map(|s| format!("`## {s}`"))
            .collect();
        let mut mode = String::new();
        if !screens.is_empty() {
            sections.push(format!("`## {}`", crate::guide::DIFFERENCES));
            let screen = &screens[0];
            mode = format!(
                "mockup mode: this branch commits the picked artboards of {} (handoff.prove-fidelity), so the guide shows the artboard beside the built screen, at one viewport, and says how they differ.\n",
                screens.iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", ")
            );
            let paste = format!(
                "\nPaste and fill, with artboard.png and after.png copied beside the guide first (its own step):\n\n## Reference\nArtboards: {screen}\nViewport: 1120px · light\n\n![Mockup, direction B](artboard.png)\n![Built screen](after.png)\n\n## {}\n- fixed: <a difference found and removed>\n- deliberate: <a difference kept, and why>\n",
                crate::guide::DIFFERENCES
            );
            missing.push(paste.trim_end().to_string());
        }
        let (paste, items): (Vec<&String>, Vec<&String>) =
            missing.iter().partition(|m| m.starts_with("\nPaste"));
        return Err(format!(
            "the guide {} lacks what the person needs to decide with (work.preview-is-guided):\n{mode}{}\nRequired H2 sections, in order: {}.{}",
            record.display(),
            items
                .iter()
                .map(|m| format!("  - {m}"))
                .collect::<Vec<_>>()
                .join("\n"),
            sections.join(", "),
            paste.first().map(|s| s.as_str()).unwrap_or("")
        ));
    }
    if !screens.is_empty() {
        for w in crate::guide::width_warnings(&text, &folder) {
            eprintln!("amont-agent: warning: {w}");
        }
    }
    let names = checkout_names(&repo);
    let label = format!(
        "{}@{}",
        names.first().map(String::as_str).unwrap_or_default(),
        short(&commit)
    );
    let aliases: Vec<String> = names
        .iter()
        .skip(1)
        .map(|n| format!("{n}@{}", short(&commit)))
        .collect();
    let html = crate::guide::render(
        &text,
        &crate::guide::Page {
            label: &label,
            commit: &commit,
            url,
        },
    );
    for img in crate::guide::missing_images(&text, &folder) {
        eprintln!(
            "amont-agent: warning: the guide references {img}, which is not beside it in {}",
            folder.display()
        );
    }
    let page = folder.join("index.html");
    crate::atomic::write_atomic(&page, &html)
        .map_err(|e| format!("the page {} could not be written: {e}", page.display()))?;
    let id = format!("{}{:x}", short(&commit), now() & 0xfff_ffff);
    Ok(Registered {
        id,
        repo: repo.to_string_lossy().into_owned(),
        commit,
        url: url.to_string(),
        guide: record.to_string_lossy().into_owned(),
        page_url: crate::guide::file_url(&page),
        page: page.to_string_lossy().into_owned(),
        label,
        aliases,
    })
}

/// After a Bash call: bind a `preview register` that ran as its own
/// foreground command to this session and prompt.
///
/// Returns what the session must hear: a register that did not bind cannot
/// be approved by any answer, and in PR #302 that failure was silent — the
/// journal knew, the agent asked anyway, and the approval bound nothing.
pub fn bind(bash: &crate::payload::Bash, parsed: &crate::shell::Parsed) -> Option<String> {
    let clauses = parsed.clauses();
    let cmd = clauses.iter().find(|c| is_register(c))?;
    // Where the leading `cd`s left the shell, with `cwd_at`'s semantics.
    let ctx = crate::rules::Context {
        cwd: &bash.cwd,
        parsed,
        background: bash.background,
        timeout_ms: bash.timeout_ms,
        tool_use_id: &bash.tool_use_id,
        transcript: bash.transcript.as_deref(),
    };
    let here = ctx.cwd_at(cmd.at);
    let dir = match cmd.flag_value("--repo") {
        Some(d) if d.starts_with('/') => PathBuf::from(d),
        Some(d) => here.join(d),
        None => here.clone(),
    };
    let named = push_target::toplevel(&dir);
    // Even a refusal is journalled under the repository the command named.
    let shown = named.clone().unwrap_or_else(|| dir.clone());
    let excerpt = "preview register";
    let printed = bash.stdout.as_deref().and_then(Registered::from_json);
    // The same command, alone: what the session runs to bind it.
    let again = format!(
        "cd {} && {}",
        shell_word(&here.to_string_lossy()),
        cmd.words
            .iter()
            .map(|w| shell_word(&w.text))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let speaks = crate::stance::resolve(&crate::rules::push_preview::RULE) != Stance::Observe;
    let refuse = |why: &str| -> Option<String> {
        note(
            "unbound",
            &bash.session,
            &shown.to_string_lossy(),
            &format!("{excerpt}: {why}"),
        );
        speaks.then(|| {
            let label = printed
                .as_ref()
                .filter(|p| !p.label.is_empty())
                .map(|p| format!(" and the label `{}`", p.label))
                .unwrap_or_default();
            format!(
                "`preview register` ran, but the preview is NOT bound to this session ({why}), so no answer to a marked question can approve it. Run it again as its own foreground command, with nothing chained before it but `cd`:\n  {again}\nthen ask the marked question with `[preview <id>]`{label} from its output."
            )
        })
    };
    if bash.background {
        return refuse("it ran in the background");
    }
    if !parsed.fully_read() || !standalone(clauses, cmd.at) {
        return refuse("it was not a standalone command");
    }
    let Some(printed) = printed.clone() else {
        return refuse("its output is not the expected JSON");
    };
    let Some(repo) = named else {
        return refuse("the repository it names is gone");
    };
    let head = git(
        &repo,
        &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
    );
    if printed.repo != repo.to_string_lossy() || head.as_deref() != Some(printed.commit.as_str()) {
        return refuse("its output does not match the repository's HEAD");
    }
    let mut regs = registrations();
    let before = regs.len();
    regs.retain(|r| !(r.session == bash.session && r.repo == printed.repo));
    let outcome = if regs.len() < before {
        "replaced"
    } else {
        "registered"
    };
    let reg = Registration {
        id: printed.id.clone(),
        session: bash.session.clone(),
        prompt: bash.prompt_id.clone(),
        repo: printed.repo.clone(),
        commit: printed.commit.clone(),
        url: printed.url.clone(),
        guide: printed.guide.clone(),
        at: now(),
        asked: "-".to_string(),
    };
    note(
        outcome,
        &bash.session,
        &reg.repo,
        &format!(
            "{} {} {} page={}",
            reg.id,
            reg.label(),
            reg.url,
            if printed.page.is_empty() {
                "-"
            } else {
                &printed.page
            }
        ),
    );
    let id = reg.id.clone();
    regs.push(reg);
    save_registrations(&regs);
    save_aliases(&id, &printed.aliases, &regs);
    None
}

/// A word as a shell reads it back: bare when it is plain, single-quoted
/// otherwise.
fn shell_word(w: &str) -> String {
    if !w.is_empty()
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c))
    {
        w.to_string()
    } else {
        format!("'{}'", w.replace('\'', "'\\''"))
    }
}

/// The register clause at `at` is the last clause of the line, and every
/// clause before it is a `cd <literal path>` joined by `&&`: no pipe, no
/// `;`- or `||`-sequenced program, no background, no substitution.
fn standalone(clauses: &[crate::shell::Simple], at: usize) -> bool {
    use crate::shell::Connector;
    let Some(last) = clauses.last() else {
        return false;
    };
    if last.at != at || last.next.is_some() || last.nested.is_some() {
        return false;
    }
    let lead = &clauses[..clauses.len() - 1];
    if lead.is_empty() {
        return last.prev.is_none();
    }
    if last.prev != Some(Connector::AndAnd) {
        return false;
    }
    lead.iter().enumerate().all(|(i, c)| {
        let literal = c.words.len() == 2
            && c.words[0].text == "cd"
            && !c.words[0].quoted
            && !c.words[1].expanded
            && !c.words[1].text.trim().is_empty()
            && !c.words[1].text.starts_with('-');
        literal
            && c.nested.is_none()
            && c.redirects.is_empty()
            && !c.heredoc
            && c.next == Some(Connector::AndAnd)
            && (if i == 0 {
                c.prev.is_none()
            } else {
                c.prev == Some(Connector::AndAnd)
            })
    })
}

fn is_register(cmd: &crate::shell::Simple) -> bool {
    let is_us = cmd
        .program()
        .is_some_and(|p| p == "amont-agent" || p.ends_with("/amont-agent"));
    let ops: Vec<&str> = cmd.operands().iter().map(|w| w.text.as_str()).collect();
    // `--help` prints usage and registers nothing: telling the session it
    // "did not bind" was a false alarm (seen 2026-09-29, the first session
    // on 2.23.0).
    let help = cmd
        .words
        .iter()
        .any(|w| w.text == "--help" || w.text == "-h");
    is_us && !help && ops.first() == Some(&"preview") && ops.get(1) == Some(&"register")
}

// --- marked questions and approval ----------------------------------------

/// The ids in a `[preview a,b]` marker, if the question carries one.
pub fn marker(question: &str) -> Option<Vec<String>> {
    let start = question.find("[preview ")? + "[preview ".len();
    let end = start + question[start..].find(']')?;
    let ids: Vec<String> = question[start..end]
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    (!ids.is_empty()).then_some(ids)
}

/// Before an `AskUserQuestion` runs. Returns the refusal text when a marked
/// question arrives pre-answered and the rule is denying.
pub fn on_pre_ask(ask: &crate::payload::Ask, stance: Stance) -> Option<String> {
    let marked: Vec<Vec<String>> = ask.questions.iter().filter_map(|q| marker(q)).collect();
    if marked.is_empty() {
        return None;
    }
    let ids: Vec<String> = marked.concat();
    if let Some(d) = dir().map(|d| d.join("asks")) {
        if ensure(&d).is_some() && !ask.tool_use_id.is_empty() && !ask.tool_use_id.contains('/') {
            let _ = crate::atomic::write_atomic(
                &d.join(&ask.tool_use_id),
                &format!(
                    "{}\t{}\t{}\t{}\t{}\n",
                    clean(&ask.session),
                    clean(&ask.prompt_id),
                    ask.prefilled,
                    ids.join(","),
                    now_ms()
                ),
            );
        }
    }
    if ask.prefilled {
        note(
            "prefilled",
            &ask.session,
            &repo_of(&ask.session, &ids),
            &format!("marked question {}", ids.join(",")),
        );
        if stance == Stance::Deny {
            return Some(
                "A preview question may not arrive with `answers` already filled in: only the person's own selection approves a preview. Ask it again without `answers`."
                    .to_string(),
            );
        }
        return None;
    }
    // Mark the listed registrations as asked in this turn.
    let mut regs = registrations();
    let mut changed = false;
    for r in regs.iter_mut() {
        if r.session == ask.session && r.prompt == ask.prompt_id && ids.contains(&r.id) {
            r.asked = ask.tool_use_id.clone();
            changed = true;
        }
    }
    if changed {
        save_registrations(&regs);
    }
    None
}

/// After an `AskUserQuestion`: an answer to a marked question approves,
/// drops, or — unanswered — leaves its registrations pending. Returns what
/// the session must hear: an approval that bound nothing because the
/// question did not name the commit.
pub fn on_post_ask(ask: &crate::payload::Ask) -> Option<String> {
    let recorded = dir()
        .map(|d| d.join("asks").join(&ask.tool_use_id))
        .filter(|_| !ask.tool_use_id.is_empty() && !ask.tool_use_id.contains('/'))
        .and_then(|p| {
            let t = std::fs::read_to_string(&p).ok();
            let _ = std::fs::remove_file(&p);
            t
        });
    let Some(recorded) = recorded else {
        // No PreToolUse record: the question was never seen before it ran,
        // so nothing proves its answers were not pre-filled.
        if ask.questions.iter().any(|q| marker(q).is_some()) {
            let ids: Vec<String> = ask
                .questions
                .iter()
                .filter_map(|q| marker(q))
                .flatten()
                .collect();
            note(
                "unrecorded",
                &ask.session,
                &repo_of(&ask.session, &ids),
                "marked question with no PreToolUse record",
            );
        }
        return None;
    };
    let answered_ms = now_ms();
    let f: Vec<&str> = recorded.trim_end().split('\t').collect();
    // Four fields: written by a release that did not yet time the ask.
    if !(f.len() == 4 || f.len() == 5) || f[0] != ask.session || f[2] != "false" {
        return None;
    }
    let mut said: Vec<String> = Vec::new();
    let asked_ms = f.get(4).and_then(|t| t.parse::<u64>().ok());
    for question in &ask.questions {
        let Some(ids) = marker(question) else {
            continue;
        };
        let label = ask
            .answers
            .iter()
            .find(|(q, _)| q == question)
            .map(|(_, l)| l.as_str())
            .unwrap_or("");
        let latency = latency(asked_ms, answered_ms, ask.duration_ms);
        let mut regs = registrations();
        let (mine, rest): (Vec<Registration>, Vec<Registration>) = regs.drain(..).partition(|r| {
            r.session == ask.session && r.asked == ask.tool_use_id && ids.contains(&r.id)
        });
        if mine.is_empty() {
            continue;
        }
        match label {
            APPROVE => {
                // Every approved commit must be visible in the question itself,
                // under any name the person knows the checkout by.
                let (listed, unlisted): (Vec<_>, Vec<_>) = mine
                    .into_iter()
                    .partition(|r| r.labels().iter().any(|l| question.contains(l.as_str())));
                for r in &unlisted {
                    note(
                        "unlisted",
                        &r.session,
                        &r.repo,
                        &format!("{} not shown in the question", r.label()),
                    );
                    said.push(format!(
                        "The answer Approve bound NOTHING for preview {}: the question did not name its commit. Ask again with `[preview {}]` and one of {} in the question text; the registration is still pending.",
                        r.id,
                        r.id,
                        r.labels()
                            .iter()
                            .map(|l| format!("`{l}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                approve(&listed, &format!("option,{latency}"));
                // An unlisted registration stays pending: dropping it made the
                // corrected question fail too.
                let mut back = rest;
                back.extend(unlisted.into_iter().map(|mut r| {
                    r.asked = "-".to_string();
                    r
                }));
                save_registrations(&back);
            }
            REQUEST_CHANGES | HOLD => {
                for r in &mine {
                    note(
                        "dropped",
                        &r.session,
                        &r.repo,
                        &format!("{} {label} after {latency}", r.label()),
                    );
                }
                save_registrations(&rest);
            }
            _ => {
                // Unanswered — a timeout, or an empty answer. Still pending, so
                // the typed fallback works for a person who was not watching.
                for r in &mine {
                    note(
                        "unanswered",
                        &r.session,
                        &r.repo,
                        &format!("{} after {latency}", r.label()),
                    );
                }
                let mut back = rest;
                back.extend(mine);
                save_registrations(&back);
            }
        }
    }
    (!said.is_empty()
        && crate::stance::resolve(&crate::rules::push_preview::RULE) != Stance::Observe)
        .then(|| said.join("\n\n"))
}

/// The repository of this session's registrations a marked question names,
/// for the journal; `-` when it names none (or several repositories).
fn repo_of(session: &str, ids: &[String]) -> String {
    let mut repos: Vec<String> = registrations()
        .into_iter()
        .filter(|r| r.session == session && ids.contains(&r.id))
        .map(|r| r.repo)
        .collect();
    repos.sort();
    repos.dedup();
    match repos.as_slice() {
        [one] => one.clone(),
        _ => "-".to_string(),
    }
}

/// A typed approval: the whole prompt, trimmed, is one of these words.
pub fn is_typed_approval(text: &str) -> bool {
    let t = text
        .trim()
        .trim_end_matches(['.', '!', ' '])
        .to_ascii_lowercase();
    let t = t.strip_suffix(" it").unwrap_or(&t);
    matches!(t, "approve" | "approved" | "ship" | "lgtm" | "looks good")
}

/// A new prompt: previews shown in an earlier turn either get the typed
/// approval right now, or lapse.
pub fn on_prompt(prompt: &crate::payload::Prompt) {
    let regs = registrations();
    let (older, rest): (Vec<Registration>, Vec<Registration>) = regs
        .into_iter()
        .partition(|r| r.session == prompt.session && r.prompt != prompt.prompt_id);
    if older.is_empty() {
        return;
    }
    let typed = is_typed_approval(&prompt.text);
    let (asked, never): (Vec<_>, Vec<_>) = older.into_iter().partition(|r| r.asked != "-");
    for r in &never {
        note(
            "dropped",
            &r.session,
            &r.repo,
            &format!("{} was never asked about", r.label()),
        );
    }
    if typed {
        approve(&asked, "typed");
    } else {
        for r in &asked {
            note(
                "dropped",
                &r.session,
                &r.repo,
                &format!("{} the next prompt was not an approval", r.label()),
            );
        }
    }
    save_registrations(&rest);
}

// --- publication ----------------------------------------------------------

/// Before a push runs: remember, per `tool_use_id`, where each branch
/// destination stood, and whether the commit was an approved UI change.
/// `push-published` reads it back after the push.
pub fn record_before(bash: &crate::payload::Bash, parsed: &crate::shell::Parsed) {
    if bash.tool_use_id.is_empty() || bash.tool_use_id.contains('/') {
        return;
    }
    let Some(cmd) = push_target::find(parsed) else {
        return;
    };
    let ctx = crate::rules::Context {
        cwd: &bash.cwd,
        parsed,
        background: bash.background,
        timeout_ms: bash.timeout_ms,
        tool_use_id: &bash.tool_use_id,
        transcript: bash.transcript.as_deref(),
    };
    let Push::Resolved { repo, targets } = push_target::resolve(&ctx.cwd_at(cmd.at), cmd) else {
        return;
    };
    if !gated(&repo) {
        return;
    }
    let mut body = String::new();
    for t in targets.iter().filter(|t| t.kind == Kind::Branch) {
        let short = t.dst.trim_start_matches("refs/heads/");
        let before = git(
            &repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/remotes/{}/{short}", t.remote),
            ],
        )
        .unwrap_or_else(|| "-".to_string());
        let ui = carries_ui(&repo, t).unwrap_or(false);
        body.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            clean(&repo.to_string_lossy()),
            clean(&t.remote),
            clean(&t.dst),
            before,
            t.src,
            ui,
            approved(&repo, &t.src)
        ));
    }
    if body.is_empty() {
        return;
    }
    if let Some(d) = dir().map(|d| d.join("pushes")) {
        if ensure(&d).is_some() {
            let _ = crate::atomic::write_atomic(&d.join(&bash.tool_use_id), &body);
        }
    }
}

/// One branch destination as recorded before the push.
pub struct Before {
    pub repo: PathBuf,
    pub remote: String,
    pub dst: String,
    pub before: Option<String>,
    pub src: String,
    pub ui: bool,
    pub approved: bool,
}

/// Take (read and remove) what [`record_before`] wrote for this call.
pub fn take_before(tool_use_id: &str) -> Vec<Before> {
    if tool_use_id.is_empty() || tool_use_id.contains('/') {
        return Vec::new();
    }
    let Some(path) = dir().map(|d| d.join("pushes").join(tool_use_id)) else {
        return Vec::new();
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() != 7 {
                return None;
            }
            Some(Before {
                repo: PathBuf::from(f[0]),
                remote: f[1].to_string(),
                dst: f[2].to_string(),
                before: (f[3] != "-").then(|| f[3].to_string()),
                src: f[4].to_string(),
                ui: f[5] == "true",
                approved: f[6] == "true",
            })
        })
        .collect()
}

/// Delete per-call scratch nobody came back for.
pub fn sweep() {
    let Some(d) = dir() else { return };
    for sub in ["pushes", "asks"] {
        let Ok(entries) = std::fs::read_dir(d.join(sub)) else {
            continue;
        };
        for e in entries.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() > SCRATCH_TTL);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    // Reading prunes lapsed registrations and journals them.
    let _ = registrations();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marker_lists_its_ids() {
        assert_eq!(
            marker("[preview a1b2, c3] Ship duro-app@abc1234?"),
            Some(vec!["a1b2".to_string(), "c3".to_string()])
        );
        assert_eq!(marker("Ship it?"), None);
        assert_eq!(marker("[preview ] empty"), None);
        assert_eq!(marker("[preview unterminated"), None);
    }

    #[test]
    fn only_explicit_words_are_a_typed_approval() {
        for yes in [
            "approve",
            "Approve.",
            "ship it",
            "Ship it!",
            "lgtm",
            "looks good",
            "  LGTM  ",
        ] {
            assert!(is_typed_approval(yes), "{yes}");
        }
        for no in [
            "yes",
            "go",
            "ok",
            "continue",
            "approve but fix the header",
            "ship it later",
            "",
        ] {
            assert!(!is_typed_approval(no), "{no}");
        }
    }

    #[test]
    fn ui_paths_are_recognised_and_docs_are_not() {
        for ui in [
            "app/routes/home.tsx",
            "src/main.ts",
            "web/src/x.ts",
            "packages/ui/src/a.ts",
            "styles/site.css",
            "public/index.html",
        ] {
            assert!(is_ui_path(ui), "{ui}");
        }
        for not in [
            "docs/plans/x.md",
            ".github/workflows/ci.yaml",
            "README.md",
            "Cargo.toml",
            "package.json",
        ] {
            assert!(!is_ui_path(not), "{not}");
        }
    }

    /// application-landscape #301 (2026-09-28), which was advised although
    /// it changed only comments under app/.
    const PR_301: &str = "\
diff --git a/app/.server/observedPipeline.integration.test.ts b/app/.server/observedPipeline.integration.test.ts
index 15750da..2770014 100644
--- a/app/.server/observedPipeline.integration.test.ts
+++ b/app/.server/observedPipeline.integration.test.ts
@@ -8 +8 @@
- * docs/plan-e4-e5-cluster-connector.md; it needs an authed Pro-org Playwright
+ * docs/plans/2026-07-07-e4-e5-cluster-connector.md; it needs an authed Pro-org Playwright
";

    #[test]
    fn comment_only_diffs_from_real_pushes_are_recognised() {
        assert!(comment_only(PR_301));
        // Two commits' diffs back to back, as `git log -p` prints them: the
        // second header is not a changed line.
        assert!(comment_only(&format!("{PR_301}{PR_301}")));
        for diff in [
            "--- a/app/components/board/BoardToolbar.tsx\n+++ b/app/components/board/BoardToolbar.tsx\n\
             @@ -88 +88 @@ export interface BoardToolbarProps {\n\
             -  /** Photo mode (plan-90d A): when set, a camera button joins the controls\n\
             +  /** Photo mode (90-day-table-stakes-plan A): when set, a camera button joins the controls\n",
            "--- a/app/styles/board.css\n+++ b/app/styles/board.css\n@@ -40 +40 @@\n\
             -/* ---- PHOTO MODE (plan-90d A) ---- */\n+/* ---- PHOTO MODE (90-day-table-stakes-plan A) ---- */\n",
            "--- a/app/routes/import.tsx\n+++ b/app/routes/import.tsx\n@@ -3 +3 @@\n\
             -// Import from a spreadsheet (plan-90d D): an onboarding path\n\
             +// Import from a spreadsheet (90-day-table-stakes-plan D): an onboarding path\n",
            "@@ -12,0 +13,2 @@\n+        {/* the header row */}\n+\n",
            "@@ -5,3 +5,3 @@\n- * Old text.\n- * @param {string} name the name\n-*/\n+ * New text.\n+ * @returns {JSX.Element}\n+ */\n",
        ] {
            assert!(comment_only(diff), "{diff}");
        }
    }

    #[test]
    fn any_code_in_the_diff_counts_as_an_interface_change() {
        for diff in [
            // A real JSX change.
            "@@ -20 +20 @@ export function Toolbar() {\n-      <Button>Save</Button>\n+      <Button>Save all</Button>\n",
            // A comment and a line of code together.
            "@@ -1,2 +1,2 @@\n-// old\n+// new\n-const gap = 4\n+const gap = 8\n",
            // Code after a closed block comment, on the same line.
            "@@ -1 +1 @@\n-/* a */ foo()\n+/* b */ foo()\n",
            "@@ -3 +3 @@\n- */ export const x = 1\n+ */ export const x = 2\n",
            // CSS's universal selector starts with `*` and is not a comment.
            "@@ -1 +1 @@\n-* { margin: 0 }\n+* { margin: 4px }\n",
            "@@ -1,0 +1 @@\n+* {\n",
            // No hunk: a binary file or a mode change.
            "diff --git a/app/logo.png b/app/logo.png\nBinary files a/app/logo.png and b/app/logo.png differ\n",
            "",
        ] {
            assert!(!comment_only(diff), "{diff}");
        }
    }

    #[test]
    fn a_package_json_looks_like_an_interface_by_dev_script_or_ui_dependency() {
        // duro-design-system/packages/cli: `@duro-app/ui` only as a dev and
        // an optional peer dependency. A CLI; renders nothing.
        let cli = r#"{"name":"@duro-app/cli","bin":{"duro":"./dist/bin.js"},
            "scripts":{"build":"tsc -p tsconfig.build.json"},
            "peerDependencies":{"@duro-app/ui":"workspace:^"},
            "peerDependenciesMeta":{"@duro-app/ui":{"optional":true}},
            "devDependencies":{"@duro-app/ui":"workspace:^","typescript":"^5.7.0"}}"#;
        assert!(!package_is_ui(cli));
        // duro-design-system/packages/ui: a required react peer.
        assert!(package_is_ui(
            r#"{"peerDependencies":{"react":"^19","react-strict-dom":"*"}}"#
        ));
        // An app: react in dependencies.
        assert!(package_is_ui(r#"{"dependencies":{"react-dom":"^19"}}"#));
        assert!(package_is_ui(r#"{"dependencies":{"@duro-app/ui":"^3"}}"#));
        assert!(package_is_ui(r#"{"scripts":{"dev":"vite"}}"#));
        assert!(!package_is_ui(r#"{"scripts":{"dev":"  "}}"#));
        assert!(!package_is_ui(r#"{"name":"eslint-plugin"}"#));
        // Unreadable: doubt counts.
        assert!(package_is_ui("{not json"));
    }

    #[test]
    fn latency_is_measured_by_the_hook_and_duration_ms_rides_along() {
        assert_eq!(latency(Some(1_000), 185_400, Some(0)), "184s,dur=0ms");
        assert_eq!(latency(None, 185_400, Some(30_997)), "-,dur=30997ms");
        assert_eq!(latency(Some(9_000), 5_000, None), "-,dur=-");
    }

    #[test]
    fn a_registration_round_trips_through_its_line() {
        let r = Registration {
            id: "abc123".into(),
            session: "s".into(),
            prompt: "p".into(),
            repo: "/w/duro-app".into(),
            commit: "abcdef0123".into(),
            url: "http://localhost:5173/x".into(),
            guide: "/h/a.md".into(),
            at: 42,
            asked: "-".into(),
        };
        assert_eq!(Registration::parse(&r.line()), Some(r.clone()));
        assert_eq!(r.label(), "duro-app@abcdef0");
    }

    #[test]
    fn registered_json_round_trips() {
        let r = Registered {
            id: "i".into(),
            repo: "/r".into(),
            commit: "c".into(),
            url: "http://localhost:1".into(),
            guide: "/g/guide.md".into(),
            page: "/g/index.html".into(),
            page_url: "file:///g/index.html".into(),
            label: "r-wt-x@c".into(),
            aliases: vec!["r@c".into(), "repo@c".into()],
        };
        assert_eq!(Registered::from_json(&r.to_json()), Some(r));
    }

    #[test]
    fn a_registration_printed_before_labels_still_reads() {
        let old = r#"{"id":"i","repo":"/r","commit":"c","url":"http://l/","guide":"/g.md","page":"","page_url":""}"#;
        let r = Registered::from_json(old).expect("the 2.22 shape");
        assert_eq!(r.label, "");
        assert!(r.aliases.is_empty());
    }

    #[test]
    fn a_word_is_quoted_only_when_it_must_be() {
        assert_eq!(shell_word("/tmp/g.md"), "/tmp/g.md");
        assert_eq!(
            shell_word("http://localhost:5173/"),
            "http://localhost:5173/"
        );
        assert_eq!(shell_word("a b"), "'a b'");
        assert_eq!(shell_word("it's"), "'it'\\''s'");
    }

    #[test]
    fn a_registration_printed_before_guides_still_reads() {
        let old = r#"{"id":"i","repo":"/r","commit":"c","url":"http://l/","attestation":"/a.md"}"#;
        let r = Registered::from_json(old).expect("the 2.20.0 shape");
        assert_eq!(r.guide, "/a.md");
        assert_eq!(r.page, "");
    }
}
