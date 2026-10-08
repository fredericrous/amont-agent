//! Every plan passes its review panel before the person sees it
//! (ADR-0022, `work.plan-review-panel`).
//!
//! The panel follows what the plan touches: an expert in each language and a
//! staff backend engineer always; a platform engineer for ops; Unix and TUI
//! experts for a command line; a product owner and an architect for a large
//! repository; four interface roles for a UI. The skill `/plan-review` runs
//! it. This module is the check at `ExitPlanMode` that it did.
//!
//! ## What is compared
//!
//! * **The canonical body** ([`canonical`]): the plan without its review
//!   section, its full reviews and its machine comment. Writing the review's
//!   results into the plan therefore never makes the review stale.
//! * **The areas**, read from each repository named in the machine comment
//!   (`repos=`) on its default tree — never from keywords in the plan, which
//!   matched 74% of plans for a UI regex against about 8 real UI plans — plus
//!   what the plan declares it adds (`adds=`).
//! * **The bindings** ([`bindings`]): each `plan-review-<role>` agent this
//!   session launched with a review block for this plan (`<<<PLAN path=…
//!   sha=…>>>`, printed by `amont-agent plan-sha --block`) and that
//!   COMPLETED, read only from the structured fields Claude Code writes. The
//!   text of a notification echoed inside a tool result is never read: that
//!   is a forgery path, and this repository's author found four in one
//!   session.
//!
//! The declared sha stops the forgetful model — a review skipped, or run on
//! an older body. It does not stop a model set on declaring a sha for a body
//! it did not have reviewed; the person accepted that (2026-09-29).
//!
//! ## Who is answered
//!
//! A cause the model controls (no `repos=`, no review section, missing
//! reviews) is refused, and counted: after two refusals of the same plan the
//! call goes to the PERSON as `ask`, so a model cannot loop and cannot slip
//! through. A cause outside its control (no transcript, a git read that
//! failed) goes to the person at once. A pass is remembered as the plan's
//! baseline, so a change the person asks for needs only its delta: the
//! backend reviewer plus the reviewers of any area that is new.

use std::collections::{BTreeSet, HashMap};
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::decision::{self, Decision};
use crate::journal;
use crate::payload::PlanExit;
use crate::rules::{plan_review_panel::RULE, Stance};

/// Opens a review block in a reviewer's prompt.
pub const BLOCK_OPEN: &str = "<<<PLAN ";
const BLOCK_CLOSE: &str = ">>>";
const REVIEW_SECTION: &str = "Review panel";
const FULL_REVIEWS: &str = "Full reviews";
const AGENT_PREFIX: &str = "plan-review-";
/// Refusals of one plan before the call goes to the person instead.
const REFUSALS_BEFORE_ASK: u32 = 2;
/// Repository size, in files or in own commits, that adds a product owner
/// and an architect.
const LARGE: usize = 500;
/// Blobs read to classify one repository.
const MAX_BLOBS: usize = 200;
/// Where `repos=` names resolve: `git config amont.agent.plan-review.<name>.path`
/// for a repository kept elsewhere (dotfiles under `~/.local/share/chezmoi`),
/// else `$AMONT_AGENT_PLAN_ROOT/<name>`, else `~/Developer/Perso/<name>`.
pub const ENV_ROOT: &str = "AMONT_AGENT_PLAN_ROOT";

// ---------------------------------------------------------------- the body

/// The plan as reviewed: CRLF → LF; YAML front matter (a `---` first line
/// through the next `---`) and the blank lines after it removed; the
/// `## Review panel` section, the `## Full reviews …` section to the end,
/// and a machine comment on the last non-empty line removed; trailing space
/// and tab trimmed per line; one final newline. Headings inside code fences
/// are text. Idempotent.
///
/// Front matter is skipped because a plan gains it when it lands in a
/// repository (`docs/plans/`, ADR-0022). The landed copy then hashes like
/// the approved one the reviewers read, so landing can be checked on the
/// file it writes.
pub fn canonical(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let comment = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .filter(|&i| is_machine_comment(lines[i]));
    let mut start = front_matter_end(&lines);
    while start < lines.len() && lines[start].trim().is_empty() {
        start += 1;
    }
    let mut out: Vec<&str> = Vec::new();
    let mut fence = Fence::default();
    let mut dropping = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        if Some(i) == comment {
            continue;
        }
        if let Some(title) = fence.h2(line) {
            if title.starts_with(FULL_REVIEWS) {
                break;
            }
            dropping = title == REVIEW_SECTION;
        }
        if !dropping {
            out.push(line.trim_end_matches([' ', '\t']));
        }
    }
    let mut body = out.join("\n");
    body.truncate(body.trim_end_matches(['\n', ' ', '\t']).len());
    body.push('\n');
    body
}

/// The index of the first line after YAML front matter, or 0 when there is
/// none: a `---` first line with no closing `---` is text, not front matter.
fn front_matter_end(lines: &[&str]) -> usize {
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return 0;
    }
    lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| l.trim_end() == "---")
        .map_or(0, |(i, _)| i + 1)
}

/// sha256 of what the canonical body says, read as CommonMark
/// ([`crate::plan_words::words`]), 64 lowercase hex: a formatter that only
/// moves blank lines, list markers or table padding keeps it.
pub fn body_sha(text: &str) -> String {
    #[cfg(test)]
    if tests::PARSER_PANICS.with(|p| p.get()) {
        panic!("injected parser panic");
    }
    hex(&sha256(
        crate::plan_words::words(&canonical(text)).as_bytes(),
    ))
}

// LEGACY: the byte sha every review was bound to before 2.25. Accepted
// alongside `body_sha` until the journal shows no `match=legacy` for 14 days.
/// sha256 of the canonical body's bytes, 64 lowercase hex.
pub fn legacy_sha(text: &str) -> String {
    hex(&sha256(canonical(text).as_bytes()))
}

/// Both shas of a plan.
pub struct Shas {
    pub sha: String,
    pub legacy: String,
}

/// [`body_sha`] and [`legacy_sha`], with a panic in the Markdown parser
/// caught here: the hook's own guard turns a panic into silence, which for
/// this rule would let a plan through unreviewed.
pub fn shas(text: &str) -> Result<Shas, ShaError> {
    guarded(|| body_sha(text)).map(|sha| Shas {
        sha,
        legacy: legacy_sha(text),
    })
}

fn guarded(f: impl FnOnce() -> String) -> Result<String, ShaError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|_| ShaError::ParserPanicked)
}

/// Why a plan's sha could not be computed.
#[derive(Debug, PartialEq)]
pub enum ShaError {
    ParserPanicked,
}

impl std::fmt::Display for ShaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShaError::ParserPanicked => {
                f.write_str("the plan's sha cannot be computed (the Markdown parser panicked)")
            }
        }
    }
}

/// Whether a review or a baseline at `seen` is of this body.
fn of_body(seen: &str, sha: &str, legacy: &str) -> bool {
    seen == sha || seen == legacy
}

/// The plan's H1 outside any fence, which names it across sessions: the
/// same plan presented from a new session gets a new random file name.
fn title(text: &str) -> Option<String> {
    let mut fence = Fence::default();
    canonical(text).lines().find_map(|l| {
        fence.h2(l);
        if fence.open.is_some() {
            return None;
        }
        l.strip_prefix("# ").map(|t| t.trim().to_string())
    })
}

/// The review block a reviewer's prompt carries.
pub fn block(path: &Path, sha: &str, lang: Option<&str>) -> String {
    let lang = lang.map(|l| format!(" lang={l}")).unwrap_or_default();
    format!(
        "{BLOCK_OPEN}path={} sha={sha}{lang}{BLOCK_CLOSE}",
        path.display()
    )
}

/// The canonical path of a plan: symlinks resolved (`~/.claude` is often a
/// dotfiles link), or the path as given when it cannot be resolved.
pub fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn is_machine_comment(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("<!-- panel:") && t.ends_with("-->")
}

/// Tracks fenced code so that a `## Review panel` inside an example is text.
#[derive(Default)]
struct Fence {
    open: Option<(char, usize)>,
}

impl Fence {
    /// The title of an ATX H2 outside any fence; updates the fence state.
    fn h2<'a>(&mut self, line: &'a str) -> Option<&'a str> {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let t = line.trim_start_matches(' ');
        if indent <= 3 {
            for c in ['`', '~'] {
                let run = t.len() - t.trim_start_matches(c).len();
                if run >= 3 {
                    match self.open {
                        None => self.open = Some((c, run)),
                        Some((o, n)) if o == c && run >= n && t[run..].trim().is_empty() => {
                            self.open = None
                        }
                        _ => {}
                    }
                    return None;
                }
            }
        }
        if self.open.is_some() || indent > 3 {
            return None;
        }
        let title = t.strip_prefix("## ")?;
        Some(title.trim().trim_end_matches('#').trim_end())
    }
}

// ---------------------------------------------------------------- the plan's own claims

/// What the machine comment says.
#[derive(Debug, Default, PartialEq)]
pub struct Meta {
    pub repos: Vec<String>,
    pub adds: Vec<String>,
}

/// The machine comment on the plan's last non-empty line. `Err` names what
/// is wrong with it, in words the model can act on.
pub fn meta(text: &str) -> Result<Meta, String> {
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .filter(|l| is_machine_comment(l))
        .ok_or(
            "the plan's last line is not its machine comment (`<!-- panel: repos=<repo>,… -->`)",
        )?;
    let inner = line
        .trim()
        .trim_start_matches("<!-- panel:")
        .trim_end_matches("-->");
    let mut m = Meta::default();
    for tok in inner.split_whitespace() {
        let list = |v: &str| -> Vec<String> {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        };
        if let Some(v) = tok.strip_prefix("repos=") {
            m.repos = list(v);
        } else if let Some(v) = tok.strip_prefix("adds=") {
            m.adds = list(v);
        }
    }
    if m.repos.is_empty() {
        return Err("the machine comment names no repository (`repos=<repo>,…`)".into());
    }
    if let Some(bad) = m.repos.iter().find(|n| !valid_name(n)) {
        return Err(format!(
            "`repos=` names `{bad}`, which is not a repository name"
        ));
    }
    if let Some(bad) = m.adds.iter().find(|a| !valid_class(a)) {
        return Err(format!(
            "`adds=` names `{bad}`; it takes cli, ui, ops, large or lang:<language>"
        ));
    }
    Ok(m)
}

/// A directory name under the root, and nothing that could leave it.
fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n != "."
        && !n.contains("..")
        && !n.starts_with('-')
        && !n.contains(['/', '\\', '\0', '\n', '\r'])
}

fn valid_class(c: &str) -> bool {
    match c.strip_prefix("lang:") {
        Some(l) => {
            !l.is_empty()
                && l.chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || "+#-".contains(ch))
        }
        None => matches!(c, "cli" | "ui" | "ops" | "large"),
    }
}

/// The review section: is it there, and does it carry a skip the person
/// asked for (`⚠ unreviewed: …`)?
#[derive(Debug, PartialEq)]
pub struct Section {
    pub present: bool,
    pub unreviewed: bool,
}

pub fn section(text: &str) -> Section {
    let mut fence = Fence::default();
    let mut inside = false;
    let mut s = Section {
        present: false,
        unreviewed: false,
    };
    for line in text.lines() {
        if let Some(title) = fence.h2(line) {
            inside = title == REVIEW_SECTION;
            s.present |= inside;
            continue;
        }
        if inside {
            let t = line.trim_start().trim_start_matches(['-', '*', ' ']);
            if t.starts_with('⚠') && t.to_lowercase().contains("unreviewed") {
                s.unreviewed = true;
            }
        }
    }
    s
}

// ---------------------------------------------------------------- areas and roles

/// The reviewers an area set requires. Always the staff backend engineer.
pub fn roles(classes: &BTreeSet<String>) -> BTreeSet<String> {
    let mut r = BTreeSet::from(["backend".to_string()]);
    for c in classes {
        let add: &[&str] = match c.as_str() {
            "ops" => &["platform"],
            "cli" => &["unix", "tui"],
            "large" => &["po", "architect"],
            "ui" => &["react", "ui-design", "ux-research", "game-ux"],
            _ => &[],
        };
        r.extend(add.iter().map(|s| s.to_string()));
        if c.starts_with("lang:") {
            r.insert(c.clone());
        }
    }
    r
}

/// Where one `repos=` name lives. The config key's subsection keeps the
/// name's case, so any valid name can be configured.
pub fn locate(root: &Path, name: &str) -> PathBuf {
    let key = format!("amont.agent.plan-review.{name}.path");
    match crate::git::output(&["config", "--get", "--type=path", &key]) {
        Some(o) if o.code == 0 && !o.stdout.is_empty() => PathBuf::from(o.stdout),
        _ => root.join(name),
    }
}

/// The directory `repos=` names resolve under.
pub fn root() -> Option<PathBuf> {
    if let Some(r) = std::env::var_os(ENV_ROOT).filter(|r| !r.is_empty()) {
        return Some(PathBuf::from(r));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join("Developer").join("Perso"))
}

/// One repository's areas. `Ok(None)`: no repository there yet — a plan
/// that creates one says what it adds. `Err`: git could not be asked.
pub fn repo_areas(dir: &Path) -> Result<Option<BTreeSet<String>>, String> {
    use crate::git;
    let io = |e: std::io::Error| format!("git could not be run in {}: {e}", dir.display());
    if !dir.is_dir() {
        return Ok(None);
    }
    // Fails outside a repository, so it answers both questions in one
    // process: each git call costs 30–120 ms on a machine running agents.
    let Some(shallow) =
        git::bytes_in(dir, &["rev-parse", "--is-shallow-repository"], None).map_err(io)?
    else {
        return Ok(None);
    };
    let shallow = String::from_utf8_lossy(&shallow).trim() == "true";
    // The default branch as last fetched, whichever checkout the session
    // sits in. Never a fetch: this runs while the person waits.
    let mut rev = None;
    for r in [
        "refs/remotes/origin/HEAD",
        "origin/main",
        "origin/master",
        "HEAD",
    ] {
        let spec = format!("{r}^{{tree}}");
        if let Some(out) = git::bytes_in(
            dir,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &spec,
            ],
            None,
        )
        .map_err(io)?
        {
            rev = Some((r, String::from_utf8_lossy(&out).trim().to_string()));
            break;
        }
    }
    let Some((commit, tree)) = rev else {
        // Unborn: an empty tree, whose only area is what the plan adds.
        return Ok(Some(BTreeSet::new()));
    };
    let paths: Vec<String> = git::ls_tree_z(dir, &tree)
        .map_err(io)?
        .ok_or_else(|| format!("git ls-tree failed in {}", dir.display()))?
        .into_iter()
        .map(|p| String::from_utf8_lossy(&p).into_owned())
        .collect();

    let wanted = candidates(&paths);
    let names: Vec<String> = wanted.iter().map(|p| format!("{tree}:{p}")).collect();
    let blobs: HashMap<String, String> = git::cat_file_batch(dir, &names)
        .map_err(io)?
        .ok_or_else(|| format!("git cat-file failed in {}", dir.display()))?
        .into_iter()
        .map(|(n, b)| {
            let p = n.split_once(':').map(|(_, p)| p.to_string()).unwrap_or(n);
            (p, String::from_utf8_lossy(&b).into_owned())
        })
        .collect();

    let commits = if shallow {
        None
    } else {
        own_commits(dir, commit)
    };
    Ok(Some(classify(&paths, &blobs, commits)))
}

/// Commits of this repository's own (a shallow clone's are not counted at
/// all). A fork's are counted past its upstream, whose history it inherits.
fn own_commits(dir: &Path, rev: &str) -> Option<usize> {
    let ask = |args: &[&str]| {
        crate::git::bytes_in(dir, args, None)
            .ok()
            .flatten()
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
    };
    let refs = ask(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/remotes/upstream/",
    ])
    .unwrap_or_default();
    let upstream = [
        "refs/remotes/upstream/HEAD",
        "refs/remotes/upstream/main",
        "refs/remotes/upstream/master",
    ]
    .into_iter()
    .find(|u| refs.lines().any(|l| l == *u));
    let mut args = vec!["rev-list", "--count", rev];
    if let Some(u) = upstream {
        args.extend(["--not", u]);
    }
    ask(&args)?.parse().ok()
}

/// Vendored, generated and lock files: not the repository's own size, and
/// not evidence of its languages.
fn denied(path: &str) -> bool {
    let dirs = ["vendor", "node_modules", "third_party", "dist"];
    let mut parts = path.split('/').peekable();
    while let Some(p) = parts.next() {
        if parts.peek().is_some() && dirs.contains(&p) {
            return true;
        }
    }
    let base = basename(path);
    base.ends_with(".lock") || base.contains("-lock.")
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The files worth opening, most telling first, at most [`MAX_BLOBS`].
fn candidates(paths: &[String]) -> Vec<String> {
    let own: Vec<&String> = paths.iter().filter(|p| !denied(p)).collect();
    let named = |n: &str| -> Vec<String> {
        own.iter()
            .filter(|p| basename(p) == n)
            .map(|p| (*p).clone())
            .collect()
    };
    let mut out: Vec<String> = Vec::new();
    out.extend(
        own.iter()
            .filter(|p| p.as_str() == ".adr.yaml")
            .map(|p| (*p).clone()),
    );
    out.extend(named("package.json"));
    out.extend(named("Cargo.toml"));
    let ops_by_name = own.iter().any(|p| ops_name(p));
    if own.iter().any(|p| basename(p) == "go.mod") {
        let mut go: Vec<String> = own
            .iter()
            .filter(|p| p.ends_with(".go") && !p.ends_with("_test.go"))
            .map(|p| (*p).clone())
            .collect();
        go.sort_by_key(|p| (basename(p) != "main.go", !p.contains("cmd/")));
        out.extend(go);
    }
    if !ops_by_name {
        out.extend(
            own.iter()
                .filter(|p| p.ends_with(".yaml") || p.ends_with(".yml"))
                .map(|p| (*p).clone()),
        );
    }
    out.truncate(MAX_BLOBS);
    out
}

fn ops_name(path: &str) -> bool {
    matches!(
        basename(path),
        "kustomization.yaml" | "kustomization.yml" | "Kustomization" | "Chart.yaml"
    ) || path.ends_with(".tf")
}

/// The areas of one repository, from its tree and the files read from it.
/// Pure, so every signal is tested without a repository.
pub fn classify(
    paths: &[String],
    blobs: &HashMap<String, String>,
    commits: Option<usize>,
) -> BTreeSet<String> {
    let own: Vec<&String> = paths.iter().filter(|p| !denied(p)).collect();
    let mut out = BTreeSet::new();
    for p in &own {
        let lang = match basename(p) {
            "Cargo.toml" => "rust",
            "go.mod" => "go",
            "package.json" | "tsconfig.json" => "typescript",
            "pyproject.toml" => "python",
            _ => continue,
        };
        out.insert(format!("lang:{lang}"));
    }
    let traits = blobs
        .get(".adr.yaml")
        .map(|t| adr_traits(t))
        .unwrap_or_default();
    let blob = |p: &str| blobs.get(p).map(String::as_str);

    let ui = traits.contains("ui")
        || own
            .iter()
            .filter(|p| basename(p) == "package.json")
            .filter_map(|p| blob(p))
            .any(crate::preview::package_is_ui);
    let ops = own.iter().any(|p| ops_name(p))
        || own
            .iter()
            .filter(|p| p.ends_with(".yaml") || p.ends_with(".yml"))
            .filter_map(|p| blob(p))
            .any(|t| {
                t.lines()
                    .any(|l| matches!(l.trim(), "kind: HelmRelease" | "kind: Kustomization"))
            });
    let cli = traits.contains("cli")
        || own
            .iter()
            .any(|p| p.as_str() == "src/main.rs" || p.ends_with("/src/main.rs"))
        || own.iter().any(|p| {
            let t = blob(p).unwrap_or("");
            match basename(p) {
                "Cargo.toml" => t.contains("[[bin]]"),
                "package.json" => serde_json::from_str::<serde_json::Value>(t)
                    .ok()
                    .is_some_and(|v| v.get("bin").is_some()),
                b if b.ends_with(".go") => t.lines().any(|l| l.trim() == "package main"),
                _ => false,
            }
        });
    let large = own.len() >= LARGE || commits.is_some_and(|c| c >= LARGE);
    for (on, class) in [(ui, "ui"), (ops, "ops"), (cli, "cli"), (large, "large")] {
        if on {
            out.insert(class.to_string());
        }
    }
    out
}

/// The traits `.adr.yaml` declares under `areas:` (`"glob": [cli, ui]`).
fn adr_traits(text: &str) -> BTreeSet<String> {
    let mut inside = false;
    let mut out = BTreeSet::new();
    for line in text.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            inside = line.trim_end() == "areas:";
            continue;
        }
        if !inside {
            continue;
        }
        if let (Some(a), Some(b)) = (line.rfind('['), line.rfind(']')) {
            if a < b {
                for t in line[a + 1..b].split(',') {
                    let t = t.trim().trim_matches(['"', '\'']);
                    if !t.is_empty() {
                        out.insert(t.to_string());
                    }
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------- the transcript

/// One completed review of one body.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub role: String,
    pub path: String,
    pub sha: String,
}

/// The completed reviews in a transcript, streamed: the review blocks of
/// every `plan-review-*` launch that completed. See [`completed_agents`].
pub fn bindings(reader: impl BufRead) -> Vec<Binding> {
    completed_agents(reader, AGENT_PREFIX)
        .into_iter()
        .flat_map(|c| parse_blocks(&c.agent, &c.prompt))
        .collect()
}

/// One agent launch that Claude Code recorded as completed: the agent type,
/// the prompt it was given, and the text of its result (a foreground
/// result's text parts; a background notification's `<result>` body, or the
/// whole notification when it has none).
pub struct Completed {
    pub agent: String,
    pub prompt: String,
    pub result: String,
}

/// The completed launches of agents whose type starts with `prefix`, in the
/// order they were launched.
///
/// A line is parsed only when it could matter: it names such an agent, or
/// it is a task notification. A launch counts only when it completed, read
/// from structured fields: a foreground result's `toolUseResult` (`status:
/// completed`, `agentType` the agent launched), or a background
/// notification Claude Code itself queued (a `user` entry with
/// `origin.kind: task-notification`, or an `attachment` whose
/// `queued_command` has `commandMode: task-notification`). Nothing inside a
/// tool result or assistant text is read as a notification.
pub fn completed_agents(reader: impl BufRead, prefix: &str) -> Vec<Completed> {
    scan(reader, prefix, false)
}

/// [`completed_agents`], plus every round a launched agent was RESUMED for
/// with `SendMessage`, each as a completion of its own: the agent launched,
/// the message as its prompt, the round's notification as its result.
///
/// The shape, from a real session (2026-10-08,
/// `tests/fixtures/sendmessage-resume.jsonl`): the launch's result carries
/// `toolUseResult.agentId`; a `SendMessage` whose `input.to` is that id is
/// answered with `toolUseResult.resumedAgentId` = the same id; and the
/// round's task notification carries `<tool-use-id>` = THAT SendMessage's id
/// and `<task-id>` = the agent id, so the launch's own result is never
/// overwritten. A round counts only when all three agree, and only on a
/// structured notification, as a launch does.
///
/// Only the implementation review reads this: the plan-review panel counts
/// fresh launches alone.
pub fn completed_agents_resumable(reader: impl BufRead, prefix: &str) -> Vec<Completed> {
    scan(reader, prefix, true)
}

fn scan(reader: impl BufRead, prefix: &str, resumable: bool) -> Vec<Completed> {
    // id → (agent, prompt), and the launch order, since a HashMap has none.
    // A resumed round is keyed by its SendMessage's id.
    let mut launched: HashMap<String, (String, String)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut done: HashMap<String, String> = HashMap::new();
    // agentId → the agent type it was launched as.
    let mut agents: HashMap<String, String> = HashMap::new();
    // SendMessage id → (to, message), until its result says it resumed.
    let mut sends: HashMap<String, (String, String)> = HashMap::new();
    // A resumed round's id → the agentId its notification must carry.
    let mut rounds: HashMap<String, String> = HashMap::new();
    for line in reader.split(b'\n') {
        let Ok(raw) = line else { break };
        let Ok(text) = std::str::from_utf8(&raw) else {
            continue;
        };
        let review = text.contains(prefix)
            || (resumable
                && (text.contains("SendMessage")
                    || text.contains("agentId")
                    || text.contains("AgentId")));
        let notice = text.contains("task-notification");
        if !review && !notice {
            continue;
        }
        // A truncated last line is a session being written right now.
        let Ok(e) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        let content = e
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array());
        let mut notified: Vec<Notification> = Vec::new();
        match e.get("type").and_then(|t| t.as_str()) {
            Some("assistant") if review => {
                for c in content.into_iter().flatten() {
                    if c.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                        continue;
                    }
                    let Some(id) = c.get("id").and_then(|i| i.as_str()) else {
                        continue;
                    };
                    let input = c.get("input");
                    let field = |k: &str| input.and_then(|i| i.get(k)).and_then(|v| v.as_str());
                    match c.get("name").and_then(|n| n.as_str()) {
                        Some("Agent" | "Task") => {
                            let Some(agent) =
                                field("subagent_type").filter(|s| s.starts_with(prefix))
                            else {
                                continue;
                            };
                            let prompt = field("prompt").unwrap_or("");
                            if !launched.contains_key(id) {
                                order.push(id.to_string());
                                launched.insert(
                                    id.to_string(),
                                    (agent.to_string(), prompt.to_string()),
                                );
                            }
                        }
                        Some("SendMessage") if resumable => {
                            if let (Some(to), Some(message)) = (field("to"), field("message")) {
                                sends.insert(id.to_string(), (to.to_string(), message.to_string()));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                if review {
                    let result = e.get("toolUseResult");
                    let field = |k: &str| result.and_then(|r| r.get(k)).and_then(|s| s.as_str());
                    let completed = field("status") == Some("completed");
                    let agent_type = field("agentType");
                    for c in content.into_iter().flatten() {
                        if c.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                            continue;
                        }
                        let Some(id) = c.get("tool_use_id").and_then(|i| i.as_str()) else {
                            continue;
                        };
                        if let Some((agent, _)) = launched.get(id) {
                            if completed && agent_type == Some(agent.as_str()) {
                                done.insert(id.to_string(), text_of(c.get("content")));
                            }
                            if resumable {
                                if let Some(agent_id) = field("agentId") {
                                    agents.insert(agent_id.to_string(), agent.clone());
                                }
                            }
                        } else if let Some((to, message)) = sends.remove(id) {
                            let Some(agent) = agents.get(&to) else {
                                continue;
                            };
                            if field("resumedAgentId") != Some(to.as_str()) {
                                continue;
                            }
                            order.push(id.to_string());
                            launched.insert(id.to_string(), (agent.clone(), message));
                            rounds.insert(id.to_string(), to);
                        }
                    }
                }
                let origin = e
                    .get("origin")
                    .and_then(|o| o.get("kind"))
                    .and_then(|k| k.as_str());
                if notice && origin == Some("task-notification") {
                    let body = match e.get("message").and_then(|m| m.get("content")) {
                        Some(serde_json::Value::String(s)) => s.clone(),
                        Some(serde_json::Value::Array(a)) => a
                            .iter()
                            .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n"),
                        _ => String::new(),
                    };
                    notified = notifications(&body);
                }
            }
            Some("attachment") if notice => {
                let a = e.get("attachment");
                let field = |k: &str| a.and_then(|a| a.get(k)).and_then(|v| v.as_str());
                if field("type") == Some("queued_command")
                    && field("commandMode") == Some("task-notification")
                {
                    notified = notifications(field("prompt").unwrap_or(""));
                }
            }
            _ => {}
        }
        for n in notified {
            // A resumed round completes only on its own agent's notification.
            if rounds
                .get(&n.tool_use_id)
                .is_some_and(|agent_id| *agent_id != n.task_id)
            {
                continue;
            }
            done.insert(n.tool_use_id, n.result);
        }
    }
    order
        .into_iter()
        .filter_map(|id| {
            let result = done.remove(&id)?;
            let (agent, prompt) = launched.remove(&id)?;
            Some(Completed {
                agent,
                prompt,
                result,
            })
        })
        .collect()
}

/// The text of a tool result's content: a string, or its text parts joined.
fn text_of(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The tool-use ids a task notification reports `completed`.
#[cfg(test)]
fn completed_ids(text: &str) -> Vec<String> {
    completed_results(text)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// One `completed` entry of a task notification.
pub struct Notification {
    /// `<tool-use-id>`: the launch, or the `SendMessage` that resumed it.
    pub tool_use_id: String,
    /// `<task-id>`: the agent's id; empty when the notification has none.
    pub task_id: String,
    /// The `<result>` text (the whole chunk when it carries none).
    pub result: String,
}

/// The `completed` entries of a task notification.
pub fn notifications(text: &str) -> Vec<Notification> {
    let tag = |chunk: &str, name: &str| -> Option<String> {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let a = chunk.find(&open)? + open.len();
        let b = chunk[a..].find(&close)? + a;
        Some(chunk[a..b].trim().to_string())
    };
    text.split("<task-notification>")
        .skip(1)
        .filter(|c| tag(c, "status").as_deref() == Some("completed"))
        .filter_map(|c| {
            Some(Notification {
                tool_use_id: tag(c, "tool-use-id")?,
                task_id: tag(c, "task-id").unwrap_or_default(),
                result: tag(c, "result").unwrap_or_else(|| c.trim().to_string()),
            })
        })
        .collect()
}

/// The tool-use ids a task notification reports `completed`, each with the
/// text of its `<result>` (the whole chunk when it carries none).
#[cfg(test)]
fn completed_results(text: &str) -> Vec<(String, String)> {
    notifications(text)
        .into_iter()
        .map(|n| (n.tool_use_id, n.result))
        .collect()
}

/// The review blocks in one reviewer's prompt, as bindings of its role.
fn parse_blocks(agent: &str, prompt: &str) -> Vec<Binding> {
    let role = agent.trim_start_matches(AGENT_PREFIX);
    let mut out = Vec::new();
    let mut rest = prompt;
    while let Some(at) = rest.find(BLOCK_OPEN) {
        rest = &rest[at + BLOCK_OPEN.len()..];
        let Some(end) = rest.find(BLOCK_CLOSE) else {
            break;
        };
        let inner = &rest[..end];
        rest = &rest[end..];
        let Some(body) = inner.strip_prefix("path=") else {
            continue;
        };
        let Some(split) = body.rfind(" sha=") else {
            continue;
        };
        let path = body[..split].to_string();
        let mut tail = body[split + 5..].split_whitespace();
        let Some(sha) = tail.next().filter(|s| s.len() == 64) else {
            continue;
        };
        let lang = tail.find_map(|t| t.strip_prefix("lang="));
        let role = match (role, lang) {
            ("language", Some(l)) => format!("lang:{l}"),
            ("language", None) => continue,
            (r, _) => r.to_string(),
        };
        out.push(Binding {
            role,
            path,
            sha: sha.to_ascii_lowercase(),
        });
    }
    out
}

// ---------------------------------------------------------------- the judgement

/// What the plan was last accepted with.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Baseline {
    pub sha: String,
    pub roles: BTreeSet<String>,
    pub classes: BTreeSet<String>,
}

pub struct Facts<'a> {
    pub sha: &'a str,
    /// LEGACY: the byte sha, accepted alongside `sha`.
    pub legacy: &'a str,
    pub path: &'a str,
    pub classes: &'a BTreeSet<String>,
    pub bindings: &'a [Binding],
    pub baseline: Option<&'a Baseline>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Every required review bound; the roles that did.
    Pass(BTreeSet<String>),
    /// Roles never reviewed for this plan, and roles whose review is of an
    /// older body.
    Short {
        missing: BTreeSet<String>,
        stale: BTreeSet<String>,
    },
}

/// Pure: whether the reviews in hand cover this plan.
///
/// * No baseline: the whole panel, each role bound to this plan's path, and
///   backend bound to THIS body — round-1 edits change the body, so backend
///   always re-runs on the final one.
/// * A baseline with this body, and no area it lacks: a re-presented plan or
///   a metadata-only write. Passes.
/// * Otherwise a delta: backend, plus every role of an area new since the
///   baseline, bound to this body. The floor is computed here, never taken
///   from the plan's own listing.
pub fn judge(f: &Facts) -> Outcome {
    let of_plan = |r: &str| -> Vec<&Binding> {
        f.bindings
            .iter()
            .filter(|b| b.role == r && b.path == f.path)
            .collect()
    };
    let now = |r: &str| of_plan(r).iter().any(|b| of_body(&b.sha, f.sha, f.legacy));
    let ever = |r: &str| !of_plan(r).is_empty();
    let need_now: BTreeSet<String>;
    let need_ever: BTreeSet<String>;
    match f.baseline {
        None => {
            need_ever = roles(f.classes);
            need_now = BTreeSet::from(["backend".to_string()]);
        }
        Some(b) if is_current(b, f.sha, f.legacy, f.classes) => {
            return Outcome::Pass(b.roles.clone());
        }
        Some(b) => {
            let new: BTreeSet<String> = f.classes.difference(&b.classes).cloned().collect();
            let mut need = roles(&new);
            need.insert("backend".into());
            need_now = need;
            need_ever = BTreeSet::new();
        }
    }
    let mut missing = BTreeSet::new();
    let mut stale = BTreeSet::new();
    for r in need_ever.iter().chain(need_now.iter()) {
        if !ever(r) {
            missing.insert(r.clone());
        } else if need_now.contains(r) && !now(r) {
            stale.insert(r.clone());
        }
    }
    if missing.is_empty() && stale.is_empty() {
        let mut bound: BTreeSet<String> = need_ever.union(&need_now).cloned().collect();
        if let Some(b) = f.baseline {
            bound.extend(b.roles.iter().cloned());
        }
        Outcome::Pass(bound)
    } else {
        Outcome::Short { missing, stale }
    }
}

/// A baseline of this body with no area it lacks: nothing to review. Shared
/// by [`judge`] and [`panel`], so `plan-panel` and the hook never disagree.
fn is_current(b: &Baseline, sha: &str, legacy: &str, classes: &BTreeSet<String>) -> bool {
    of_body(&b.sha, sha, legacy) && classes.is_subset(&b.classes)
}

// ---------------------------------------------------------------- the store

fn store() -> Option<PathBuf> {
    Some(journal::dir()?.join("plan-review"))
}

fn key(text: &str) -> String {
    hex(&sha256(text.as_bytes()))
}

fn load(file: &Path) -> Option<Baseline> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    let set = |k: &str| -> BTreeSet<String> {
        v.get(k)
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(Baseline {
        sha: v.get("sha")?.as_str()?.to_string(),
        roles: set("roles"),
        classes: set("classes"),
    })
}

/// The key that names a plan across sessions: its H1 and its repositories.
fn title_key(text: &str, repos: &[String]) -> Option<String> {
    let mut repos = repos.to_vec();
    repos.sort();
    Some(key(&format!("{}\n{}", title(text)?, repos.join(","))))
}

/// The plan's baseline, most specific first: by its path at this body; by
/// this body under any path (the same plan from a new session gets a new
/// random file name); then, for a delta, by its path at an older body, and
/// by its title at an older body. An older baseline is what [`judge`] needs
/// to ask for backend plus the reviewers of new areas instead of the whole
/// panel. Two plans with the same H1 and repositories share a title
/// baseline; backend still reviews the body.
pub fn baseline(path: &str, sha: &str, legacy: &str, title: Option<&str>) -> Option<Baseline> {
    let s = store()?;
    let by_path = load(&s.join("by-path").join(format!("{}.json", key(path))));
    let by_body = |b: &str| load(&s.join("by-body").join(format!("{b}.json")));
    by_path
        .clone()
        .filter(|b| of_body(&b.sha, sha, legacy))
        .or_else(|| by_body(sha))
        .or_else(|| by_body(legacy)) // LEGACY
        .or(by_path)
        .or_else(|| load(&s.join("by-title").join(format!("{}.json", title?))))
}

fn private_dir(dir: &Path) -> Option<()> {
    std::fs::create_dir_all(dir).ok()?;
    journal::private(dir, 0o700);
    Some(())
}

/// Written only on a verified pass: after an `ask` the hook cannot see what
/// the person answered.
fn save(path: &str, title: Option<&str>, b: &Baseline) -> Option<()> {
    let s = store()?;
    private_dir(&s)?;
    let body = serde_json::json!({
        "sha": b.sha,
        "roles": b.roles,
        "classes": b.classes,
        "path": path,
    })
    .to_string();
    let mut places = vec![
        ("by-path", format!("{}.json", key(path))),
        ("by-body", format!("{}.json", b.sha)),
    ];
    if let Some(t) = title {
        places.push(("by-title", format!("{t}.json")));
    }
    for (dir, name) in places {
        let d = s.join(dir);
        private_dir(&d)?;
        let f = d.join(name);
        crate::atomic::write_atomic(&f, &body).ok()?;
        journal::private(&f, 0o600);
    }
    Some(())
}

fn refusals_file(path: &str) -> Option<PathBuf> {
    Some(store()?.join("refusals").join(key(path)))
}

fn refusals(path: &str) -> u32 {
    refusals_file(path)
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0)
}

fn set_refusals(path: &str, n: u32) {
    let Some(f) = refusals_file(path) else { return };
    if n == 0 {
        let _ = std::fs::remove_file(f);
        return;
    }
    if let Some(d) = f.parent() {
        if private_dir(d).is_none() {
            return;
        }
    }
    let _ = crate::atomic::write_atomic(&f, &format!("{n}\n"));
    journal::private(&f, 0o600);
}

// ---------------------------------------------------------------- the hook

/// What the hook says, before the stance is applied.
enum Said {
    Pass,
    /// The model can fix it: refused, and counted.
    Refuse(String),
    /// Only the person can: asked, and not counted.
    Ask(String),
}

const REMEDY: &str = "run /plan-review then ExitPlanMode again";

pub fn on_plan_exit(ev: &PlanExit, stance: Stance) -> Decision {
    let (said, repos, excerpt) = examine(ev);
    let path_key = ev
        .plan_file
        .as_deref()
        .map(|p| real(p).display().to_string());

    // The anti-trap. A refusal the model could not fix twice is the
    // person's to decide, and the model cannot answer an `ask`.
    let said = match (said, &path_key) {
        (Said::Refuse(why), Some(p)) => {
            let before = refusals(p);
            set_refusals(p, before + 1);
            if before >= REFUSALS_BEFORE_ASK {
                Said::Ask(format!(
                    "UNREVIEWED: refused {before}×; {why} Approve only to accept an unreviewed plan."
                ))
            } else {
                Said::Refuse(why)
            }
        }
        (Said::Pass, Some(p)) => {
            set_refusals(p, 0);
            Said::Pass
        }
        (other, _) => other,
    };

    let outcome = match (&said, stance) {
        (_, Stance::Observe) => "watched",
        (Said::Pass, _) => "passed",
        (Said::Refuse(_), Stance::Deny) => "denied",
        (Said::Ask(_), Stance::Deny) => "asked",
        (_, Stance::Advise) => "advised",
    };
    journal::record(&journal::Entry {
        rule: RULE.id,
        stance: stance.as_str(),
        outcome,
        session: &ev.session,
        repo: &repos,
        mode: &ev.permission_mode,
        excerpt: &excerpt,
    });
    let phrase = |why: &str, remedy: &str| decision::phrase(RULE.id, why, remedy);
    match (said, stance) {
        (_, Stance::Observe) | (Said::Pass, _) => Decision::Silent,
        (Said::Refuse(why), Stance::Deny) => Decision::Deny(phrase(&why, REMEDY)),
        (Said::Ask(why), Stance::Deny) => Decision::Ask(phrase(&why, "")),
        (Said::Refuse(why) | Said::Ask(why), Stance::Advise) => Decision::Advise(phrase(&why, "")),
    }
}

/// The verdict, the repository names for the journal, and an excerpt.
fn examine(ev: &PlanExit) -> (Said, String, String) {
    let Some(plan_file) = ev.plan_file.as_deref() else {
        return (
            Said::Ask(
                "ExitPlanMode carried no planFilePath, so the plan's review cannot be checked."
                    .into(),
            ),
            "-".into(),
            "no planFilePath".into(),
        );
    };
    let text = match std::fs::read_to_string(plan_file) {
        Ok(t) => t,
        Err(e) => {
            return (
                Said::Ask(format!(
                    "the plan file {} cannot be read ({e}), so its review cannot be checked.",
                    plan_file.display()
                )),
                "-".into(),
                "plan unreadable".into(),
            )
        }
    };
    let path = real(plan_file).display().to_string();
    let Shas { sha, legacy } = match shas(&text) {
        Ok(s) => s,
        Err(why) => {
            return (
                Said::Ask(format!(
                    "{why}; approve only to accept the plan unreviewed."
                )),
                "-".into(),
                "sha unavailable".into(),
            )
        }
    };
    let short = &sha[..12];

    let m = match meta(&text) {
        Ok(m) => m,
        Err(why) => {
            return (
                Said::Refuse(format!("{why};")),
                "-".into(),
                format!("{short} no meta"),
            )
        }
    };
    let repos = m.repos.join(",");
    let sec = section(&text);
    if !sec.present {
        return (
            Said::Refuse(format!(
                "the plan has no `## Review panel` section for repos={repos};"
            )),
            repos,
            format!("{short} no section"),
        );
    }

    let Some(transcript) = ev.transcript.as_deref() else {
        return (
            Said::Ask(
                "the hook payload carried no transcript_path, so the reviews cannot be checked."
                    .into(),
            ),
            repos,
            format!("{short} no transcript"),
        );
    };
    let found = match std::fs::File::open(transcript) {
        Ok(f) => bindings(std::io::BufReader::new(f)),
        Err(e) => {
            return (
                Said::Ask(format!(
                "the session transcript cannot be read ({e}), so the reviews cannot be checked."
            )),
                repos,
                format!("{short} transcript unreadable"),
            )
        }
    };

    let classes = match classes_of(&m) {
        Ok(c) => c,
        Err(said) => {
            let what = match &said {
                Said::Ask(_) => "cannot resolve",
                _ => "unknown repo",
            };
            return (said, repos, format!("{short} {what}"));
        }
    };

    let title = title_key(&text, &m.repos);
    let base = baseline(&path, &sha, &legacy, title.as_deref());
    let facts = Facts {
        sha: &sha,
        legacy: &legacy,
        path: &path,
        classes: &classes,
        bindings: &found,
        baseline: base.as_ref(),
    };
    match judge(&facts) {
        Outcome::Pass(bound) => {
            if sec.unreviewed {
                return (
                    Said::Ask(format!(
                        "the plan for repos={repos} is marked unreviewed at the person's request (body {short}); approve only if you asked to skip the review."
                    )),
                    repos,
                    format!("{short} unreviewed"),
                );
            }
            // LEGACY: whether the pass needed the byte sha — the same facts
            // without it fall short. Leads the excerpt, which is cut at
            // MAX_EXCERPT, so it is never the part that is lost.
            let on_legacy = !matches!(
                judge(&Facts {
                    legacy: "-",
                    ..facts
                }),
                Outcome::Pass(_)
            );
            let matched = if on_legacy { "legacy" } else { "new" };
            let _ = save(
                &path,
                title.as_deref(),
                &Baseline {
                    sha: sha.clone(),
                    roles: bound.clone(),
                    classes: classes
                        .union(&base.map(|b| b.classes).unwrap_or_default())
                        .cloned()
                        .collect(),
                },
            );
            let listed = bound.into_iter().collect::<Vec<_>>().join(",");
            (
                Said::Pass,
                repos,
                format!("match={matched} {short} {listed}"),
            )
        }
        Outcome::Short { missing, stale } => {
            if sec.unreviewed {
                return (
                    Said::Ask(format!(
                        "the plan for repos={repos} is marked unreviewed at the person's request; missing {} for body {short}. Approve only if you asked to skip the review.",
                        list(&missing.union(&stale).cloned().collect())
                    )),
                    repos,
                    format!("{short} unreviewed"),
                );
            }
            let mut parts = Vec::new();
            if !missing.is_empty() {
                parts.push(format!("missing for repos={repos}: {}", list(&missing)));
            }
            if !stale.is_empty() {
                parts.push(format!(
                    "stale (reviewed an older body) for repos={repos}: {}",
                    list(&stale)
                ));
            }
            let excerpt = format!(
                "{short} short {}",
                list(&missing.union(&stale).cloned().collect())
            );
            (
                Said::Refuse(format!("{} (body {short});", parts.join("; "))),
                repos,
                excerpt,
            )
        }
    }
}

fn list(s: &BTreeSet<String>) -> String {
    s.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// The areas of every repository the plan names, plus what it declares.
/// `Err` carries the answer the hook gives instead.
fn classes_of(m: &Meta) -> Result<BTreeSet<String>, Said> {
    let Some(root) = root() else {
        return Err(Said::Ask(
            "no home directory, so repos= cannot be resolved.".into(),
        ));
    };
    let mut classes: BTreeSet<String> = m.adds.iter().cloned().collect();
    for name in &m.repos {
        match repo_areas(&locate(&root, name)) {
            Ok(Some(a)) => classes.extend(a),
            Ok(None) if m.adds.iter().any(|a| a.starts_with("lang:")) => {}
            Ok(None) => {
                return Err(Said::Refuse(format!(
                    "repos= names `{name}`, which is not a repository at {} (set `git config --global amont.agent.plan-review.{name}.path <dir>` for one kept elsewhere); a plan that creates one declares its language in adds=lang:<language>;",
                    locate(&root, name).display()
                )))
            }
            Err(why) => {
                return Err(Said::Ask(format!(
                    "{why}, so the panel cannot be computed."
                )))
            }
        }
    }
    Ok(classes)
}

/// What `amont-agent plan-panel` reports: the reviews this plan still
/// needs, computed exactly as the hook will.
pub struct Panel {
    pub repos: String,
    pub classes: BTreeSet<String>,
    pub sha: String,
    /// `full` (no baseline), `delta` (the body or the areas changed since
    /// the last pass) or `current` (nothing to review).
    pub mode: &'static str,
    pub needed: BTreeSet<String>,
}

pub fn panel(plan_file: &Path) -> Result<Panel, String> {
    let text = std::fs::read_to_string(plan_file)
        .map_err(|e| format!("cannot read {}: {e}", plan_file.display()))?;
    let m = meta(&text)?;
    let classes = classes_of(&m).map_err(|said| match said {
        Said::Refuse(w) | Said::Ask(w) => w.trim_end_matches(';').to_string(),
        Said::Pass => String::new(),
    })?;
    let path = real(plan_file).display().to_string();
    let Shas { sha, legacy } = shas(&text).map_err(|e| e.to_string())?;
    let title = title_key(&text, &m.repos);
    let (mode, needed) = match baseline(&path, &sha, &legacy, title.as_deref()) {
        None => ("full", roles(&classes)),
        Some(b) if is_current(&b, &sha, &legacy, &classes) => ("current", BTreeSet::new()),
        Some(b) => {
            let new: BTreeSet<String> = classes.difference(&b.classes).cloned().collect();
            let mut need = roles(&new);
            need.insert("backend".into());
            ("delta", need)
        }
    };
    Ok(Panel {
        repos: m.repos.join(","),
        classes,
        sha,
        mode,
        needed,
    })
}

/// The agent to launch for a role, and the `--lang` its block takes.
pub fn agent_for(role: &str) -> (String, Option<&str>) {
    match role.strip_prefix("lang:") {
        Some(l) => (format!("{AGENT_PREFIX}language"), Some(l)),
        None => (format!("{AGENT_PREFIX}{role}"), None),
    }
}

// ---------------------------------------------------------------- sha256

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// FIPS 180-4 SHA-256. Hand-written because the crate carries one
/// dependency on purpose; pinned by the NIST vectors below.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn sha256_matches_the_nist_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hex(&sha256(&vec![b'a'; 1_000_000])),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    const PLAN: &str = "# Title\n\n## Review panel\n\n👉 Decide: none\n\n## Context\n\nBody.  \n\n```md\n## Review panel\nkept\n```\n\n## Full reviews (reference)\n\n- rust: ok\n\n<!-- panel: repos=amont-agent adds=cli body-sha=abc -->\n";

    #[test]
    fn the_canonical_body_drops_review_metadata_and_nothing_else() {
        let c = canonical(PLAN);
        assert_eq!(
            c,
            "# Title\n\n## Context\n\nBody.\n\n```md\n## Review panel\nkept\n```\n"
        );
    }

    #[test]
    fn canonicalising_is_idempotent_and_line_endings_do_not_count() {
        let once = canonical(PLAN);
        assert_eq!(canonical(&once), once);
        assert_eq!(body_sha(&PLAN.replace('\n', "\r\n")), body_sha(PLAN));
    }

    #[test]
    fn front_matter_added_on_landing_keeps_the_sha() {
        let landed = format!(
            "---\nstatus: active\nbranch: feat/x\nrepos: [x]\n---\n\n{}",
            PLAN.replace(
                "👉 Decide: none",
                "👉 Decide: none\n📄 Full reviews: [x](x.reviews.md)"
            )
            .split("\n## Full reviews")
            .next()
            .unwrap()
        ) + "\n<!-- panel: repos=amont-agent -->\n";
        assert_eq!(body_sha(&landed), body_sha(PLAN));
        assert_eq!(canonical(&canonical(&landed)), canonical(&landed));
        // Not front matter: no closing fence, or not on the first line.
        // (Its `---` is then a thematic break, which says nothing, so the
        // sha over the CommonMark reading does not see it; the bytes do.)
        let open = format!("---\n{PLAN}");
        assert_ne!(legacy_sha(&open), legacy_sha(PLAN));
        let later = PLAN.replace("Body.", "Body.\n\n---\n\nMore.\n\n---");
        assert!(canonical(&later).contains("---\n\nMore."));
    }

    #[test]
    fn writing_review_metadata_keeps_the_sha() {
        let edited = PLAN
            .replace("👉 Decide: none", "👉 Decide: none left\n📍 amont-agent")
            .replace("- rust: ok", "- rust: ok\n- backend: ok")
            .replace("body-sha=abc", "body-sha=def reviewers=backend");
        assert_eq!(body_sha(&edited), body_sha(PLAN));
        assert_ne!(body_sha(&PLAN.replace("Body.", "Body!")), body_sha(PLAN));
    }

    /// Both values are pinned: a `pulldown-cmark` bump that re-hashes plans
    /// fails here, and the legacy value is what 2.24.0's `plan-sha` printed
    /// for this plan, so every review bound before 2.25 still matches.
    #[test]
    fn the_shas_of_a_fixed_plan_do_not_move() {
        assert_eq!(
            body_sha(PLAN),
            "ab25150b6b847cc4ed344aad0d38ae282422b111a37f8f48c14863dff74ed788"
        );
        assert_eq!(
            legacy_sha(PLAN),
            "db6b774afd48bd8c7bd4edb3b8758a4417beae2c3ce26d4245cba7bc36e396dd"
        );
    }

    #[test]
    fn formatting_the_plan_keeps_the_sha_and_the_legacy_sha_does_not() {
        let formatted = PLAN.replace("## Context\n\nBody.", "## Context\n\n\nBody.");
        assert_eq!(body_sha(&formatted), body_sha(PLAN));
        assert_ne!(legacy_sha(&formatted), legacy_sha(PLAN));
    }

    thread_local! {
        /// Makes [`super::body_sha`] panic, as a parser bug would.
        pub(crate) static PARSER_PANICS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    #[test]
    fn a_panic_computing_the_sha_is_an_error_not_silence() {
        assert_eq!(
            guarded(|| panic!("parser bug")),
            Err(ShaError::ParserPanicked)
        );
        assert!(shas(PLAN).is_ok());
    }

    #[test]
    fn a_parser_panic_in_the_hook_goes_to_the_person() {
        let dir =
            std::env::temp_dir().join(format!("amont-agent-sha-panic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = dir.join("p.md");
        std::fs::write(&plan, PLAN).unwrap();
        let ev = PlanExit {
            session: "s".into(),
            permission_mode: "plan".into(),
            transcript: None,
            plan_file: Some(plan),
        };
        PARSER_PANICS.with(|p| p.set(true));
        let (said, _, excerpt) = examine(&ev);
        PARSER_PANICS.with(|p| p.set(false));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            matches!(said, Said::Ask(ref why) if why.contains("cannot be computed")),
            "{excerpt}"
        );
        assert_eq!(excerpt, "sha unavailable");
    }

    #[test]
    fn the_title_is_the_first_h1_outside_a_fence() {
        assert_eq!(title(PLAN).as_deref(), Some("Title"));
        let fenced = "```md\n# Not this\n```\n\n# This one\n";
        assert_eq!(title(fenced).as_deref(), Some("This one"));
        assert_eq!(title("no heading\n"), None);
    }

    #[test]
    fn the_machine_comment_is_read_and_checked() {
        let m = meta(PLAN).unwrap();
        assert_eq!(m.repos, vec!["amont-agent"]);
        assert_eq!(m.adds, vec!["cli"]);
        assert!(meta("# t\n").is_err());
        assert!(meta("<!-- panel: adds=ui -->").is_err());
        for bad in ["..", "a/b", "-x", "."] {
            assert!(
                meta(&format!("<!-- panel: repos={bad} -->")).is_err(),
                "{bad}"
            );
        }
        assert!(meta("<!-- panel: repos=x adds=gui -->").is_err());
        assert!(meta("<!-- panel: repos=x adds=lang:rust,ops -->").is_ok());
    }

    #[test]
    fn the_review_section_and_a_requested_skip_are_found() {
        assert_eq!(
            section(PLAN),
            Section {
                present: true,
                unreviewed: false
            }
        );
        let skipped = PLAN.replace("👉 Decide: none", "⚠ unreviewed: the person asked to skip");
        assert!(section(&skipped).unreviewed);
        assert!(!section("# t\n```\n## Review panel\n```\n").present);
    }

    #[test]
    fn areas_come_from_the_tree_and_the_files_read_from_it() {
        let paths: Vec<String> = [
            "Cargo.toml",
            "src/main.rs",
            "web/package.json",
            "deploy/kustomization.yaml",
            "vendor/x/go.mod",
            "Cargo.lock",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut blobs = HashMap::new();
        blobs.insert(
            "web/package.json".to_string(),
            r#"{"scripts":{"dev":"vite"}}"#.to_string(),
        );
        let got = classify(&paths, &blobs, Some(10));
        assert_eq!(
            got,
            set(&["cli", "lang:rust", "lang:typescript", "ops", "ui"])
        );
        assert!(classify(&paths, &blobs, Some(500)).contains("large"));
        assert!(
            !classify(&paths, &blobs, None).contains("large"),
            "shallow: files only"
        );
    }

    #[test]
    fn adr_areas_declare_traits() {
        let t = "dir: docs\nareas:\n  # ui: x\n  \"apps/a/**\": [ui]\n  bootstrap/**: [cli, ui]\ndisclaims:\n  x/**: [ops]\n";
        assert_eq!(adr_traits(t), set(&["cli", "ui"]));
    }

    #[test]
    fn the_panel_follows_the_areas() {
        assert_eq!(roles(&set(&[])), set(&["backend"]));
        assert_eq!(
            roles(&set(&["lang:rust", "cli"])),
            set(&["backend", "lang:rust", "unix", "tui"])
        );
        assert_eq!(roles(&set(&["ui"])).len(), 5);
    }

    fn b(role: &str, sha: &str) -> Binding {
        Binding {
            role: role.into(),
            path: "/p.md".into(),
            sha: sha.into(),
        }
    }

    #[test]
    fn a_first_panel_needs_every_role_and_backend_on_the_final_body() {
        let classes = set(&["lang:rust", "cli"]);
        let old = "1".repeat(64);
        let new = "2".repeat(64);
        let bs = vec![
            b("lang:rust", &old),
            b("unix", &old),
            b("tui", &old),
            b("backend", &old),
        ];
        let f = Facts {
            sha: &new,
            legacy: "-",
            path: "/p.md",
            classes: &classes,
            bindings: &bs,
            baseline: None,
        };
        assert_eq!(
            judge(&f),
            Outcome::Short {
                missing: set(&[]),
                stale: set(&["backend"])
            }
        );
        let mut bs2 = bs.clone();
        bs2.push(b("backend", &new));
        let f = Facts {
            bindings: &bs2,
            ..f
        };
        assert!(matches!(judge(&f), Outcome::Pass(_)));
    }

    #[test]
    fn a_review_of_another_plan_does_not_bind() {
        let classes = set(&[]);
        let sha = "1".repeat(64);
        let mut other = b("backend", &sha);
        other.path = "/other.md".into();
        let bs = [other];
        let f = Facts {
            sha: &sha,
            legacy: "-",
            path: "/p.md",
            classes: &classes,
            bindings: &bs,
            baseline: None,
        };
        assert_eq!(
            judge(&f),
            Outcome::Short {
                missing: set(&["backend"]),
                stale: set(&[])
            }
        );
    }

    #[test]
    fn a_delta_needs_backend_and_the_new_areas_only() {
        let base = Baseline {
            sha: "1".repeat(64),
            roles: set(&["backend", "lang:rust"]),
            classes: set(&["lang:rust"]),
        };
        let new = "2".repeat(64);
        let classes = set(&["lang:rust", "ui"]);
        let bs = vec![b("backend", &new)];
        let f = Facts {
            sha: &new,
            legacy: "-",
            path: "/p.md",
            classes: &classes,
            bindings: &bs,
            baseline: Some(&base),
        };
        assert_eq!(
            judge(&f),
            Outcome::Short {
                missing: set(&["react", "ui-design", "ux-research", "game-ux"]),
                stale: set(&[])
            }
        );
        // Same body, no new area: passes with nothing new bound.
        let same = base.sha.clone();
        let old_classes = set(&["lang:rust"]);
        let f = Facts {
            sha: &same,
            classes: &old_classes,
            bindings: &[],
            ..f
        };
        assert!(matches!(judge(&f), Outcome::Pass(_)));
    }

    #[test]
    fn a_review_bound_to_the_legacy_sha_still_counts() {
        let classes = set(&["lang:rust"]);
        let (new, old) = ("2".repeat(64), "1".repeat(64));
        let bs = vec![b("lang:rust", &old), b("backend", &old)];
        let f = Facts {
            sha: &new,
            legacy: &old,
            path: "/p.md",
            classes: &classes,
            bindings: &bs,
            baseline: None,
        };
        assert!(
            matches!(judge(&f), Outcome::Pass(_)),
            "backend at the legacy sha is current"
        );
        let f = Facts { legacy: "-", ..f };
        assert!(
            matches!(judge(&f), Outcome::Short { ref stale, .. } if stale.contains("backend")),
            "without the legacy sha it is an older body"
        );
    }

    #[test]
    fn a_baseline_at_the_legacy_sha_is_current_for_judge_and_panel_alike() {
        let classes = set(&["lang:rust"]);
        let (new, old) = ("2".repeat(64), "1".repeat(64));
        let base = Baseline {
            sha: old.clone(),
            roles: set(&["backend", "lang:rust"]),
            classes: classes.clone(),
        };
        assert!(is_current(&base, &new, &old, &classes));
        assert!(!is_current(&base, &new, "-", &classes));
        assert!(!is_current(&base, &new, &old, &set(&["lang:rust", "ui"])));
        let f = Facts {
            sha: &new,
            legacy: &old,
            path: "/p.md",
            classes: &classes,
            bindings: &[],
            baseline: Some(&base),
        };
        assert!(matches!(judge(&f), Outcome::Pass(_)));
    }

    #[test]
    fn blocks_are_read_from_a_reviewer_prompt() {
        let sha = "a".repeat(64);
        let p = format!("Review this.\n<<<PLAN path=/x/my plan.md sha={sha} lang=rust>>>\nthanks");
        assert_eq!(
            parse_blocks("plan-review-language", &p),
            vec![Binding {
                role: "lang:rust".into(),
                path: "/x/my plan.md".into(),
                sha: sha.clone()
            }]
        );
        assert!(parse_blocks("plan-review-language", &p.replace(" lang=rust", "")).is_empty());
        assert_eq!(parse_blocks("plan-review-backend", &p)[0].role, "backend");
        assert!(parse_blocks("plan-review-backend", "<<<PLAN path=/x sha=short>>>").is_empty());
    }

    #[test]
    fn a_notification_names_what_completed() {
        let t = "<task-notification>\n<task-id>a</task-id>\n<tool-use-id>toolu_1</tool-use-id>\n<status>completed</status>\n</task-notification><task-notification><tool-use-id>toolu_2</tool-use-id><status>failed</status></task-notification>";
        assert_eq!(completed_ids(t), vec!["toolu_1"]);
    }
}
