//! `amont-agent board [--repo <name>...] [--since <dur>] [--fetch] [--json]`:
//! the work in flight per repository and what landed recently, derived from
//! git and the session activity files on every call (decisions ADR-0051).
//! Read-only: `--fetch` updates remote-tracking refs and nothing else; the
//! activity files are only read, through `amont_agent_guard::activity`.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use amont_agent_guard::activity::{self, Seen};
use amont_agent_guard::git;

const DEFAULT_SINCE: Duration = Duration::from_secs(48 * 3600);
const FETCH_BUDGET: Duration = Duration::from_secs(60);

pub const USAGE: &str = "\
usage: amont-agent board [--repo <name>...] [--since <dur>] [--fetch] [--json]

The work in flight per repository, and what landed recently, derived from git
and the session activity files on every call. Read-only.

In flight: each worktree; each local or origin branch not merged into the
default branch whose tip commit is newer than --since, or that carries an
active plan (a plan file under docs/plans the branch adds), or that a
session's latest activity line names. Per entry: the path, the branch, the
active plan's title, `programme <slug> ticket <id>` for a prog/<slug>/<id>
branch, and the sessions whose latest activity line names that worktree, each
with its last-seen age. A squash-merged branch older than --since with no plan
and no session is left out.

Just landed: the first-parent commits of the default branch within --since.

  --repo <name>   a repository, resolved like plan-review repos (git config
                  amont.agent.plan-review.<name>.path, else <plan root>/<name>);
                  repeatable. Without it: the working directory's repository,
                  else every repository directly under the plan root.
  --since <dur>   the window: 45s, 90m, 3h, 2d (default 48h)
  --fetch         run `git fetch origin -q` on the selected repositories first
  --json          one JSON array on stdout: {repo, path, worktrees: [{path,
                  branch, plans: [title], programme_ticket, sessions: [{id,
                  last_seen_secs}]}], landed: [{sha, subject, age_secs}]}

A repository with nothing in flight and nothing landed prints nothing unless
it was named with --repo. Exit 0 unless the command itself fails (2 on a usage
error); an unreadable repository is reported on stderr and skipped.
";

struct Options {
    repos: Vec<String>,
    since: Duration,
    fetch: bool,
    json: bool,
}

enum Parsed {
    Run(Options),
    Help,
    Usage(String),
}

fn parse(args: &[String]) -> Parsed {
    let mut options = Options {
        repos: Vec::new(),
        since: DEFAULT_SINCE,
        fetch: false,
        json: false,
    };
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Parsed::Help;
    }
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        match flag {
            "--fetch" => options.fetch = true,
            "--json" => options.json = true,
            "--repo" | "--since" => {
                let Some(value) = inline.or_else(|| rest.next().cloned()) else {
                    return Parsed::Usage(format!("{flag} needs a value"));
                };
                if flag == "--repo" {
                    if !crate::plan_review::valid_name(&value) {
                        return Parsed::Usage(format!("`{value}` is not a repository name"));
                    }
                    options.repos.push(value);
                } else {
                    match amont_agent_guard::gitconfig::parse_duration(&value) {
                        Some(d) => options.since = d,
                        None => {
                            return Parsed::Usage(format!(
                                "`{value}` is not a duration (45s, 90m, 3h, 2d)"
                            ))
                        }
                    }
                }
            }
            _ => return Parsed::Usage(format!("unknown argument `{arg}`")),
        }
    }
    Parsed::Run(options)
}

struct Session {
    id: String,
    last_seen_secs: u64,
}

struct Entry {
    path: Option<PathBuf>,
    branch: String,
    plans: Vec<String>,
    sessions: Vec<Session>,
}

impl Entry {
    /// `x/t02` for a `prog/x/t02` branch.
    fn programme_ticket(&self) -> Option<String> {
        let rest = self.branch.strip_prefix("prog/")?;
        let (slug, id) = rest.split_once('/')?;
        (!slug.is_empty() && !id.is_empty()).then(|| format!("{slug}/{id}"))
    }
}

struct Landed {
    sha: String,
    subject: String,
    age_secs: u64,
}

struct Report {
    name: String,
    path: PathBuf,
    entries: Vec<Entry>,
    landed: Vec<Landed>,
}

pub fn run(args: &[OsString]) -> ExitCode {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let options = match parse(&args) {
        Parsed::Help => {
            let _ = write!(std::io::stdout(), "{USAGE}"); // a closed stdout has nobody left to read the help
            return ExitCode::SUCCESS;
        }
        Parsed::Usage(why) => {
            eprint!("amont-agent: board: {why}\n\n{USAGE}");
            return ExitCode::from(2);
        }
        Parsed::Run(options) => options,
    };
    let named = !options.repos.is_empty();
    let root = crate::plan_review::root();
    let Some(selected) = select(&options.repos, root.as_deref()) else {
        return ExitCode::from(1);
    };
    let sessions = activity::latest_all();
    let now = SystemTime::now();
    let mut reports = Vec::new();
    for dir in selected {
        if git::stdout_in(&dir, &["rev-parse", "--git-dir"]).is_none() {
            eprintln!(
                "amont-agent: board: {} is not a readable git repository",
                dir.display()
            );
            continue;
        }
        if options.fetch && !fetch_origin(&dir) {
            eprintln!(
                "amont-agent: board: `git fetch origin` failed in {}",
                dir.display()
            );
        }
        reports.push(report(&dir, &options, &sessions, now, root.as_deref()));
    }
    if !named {
        reports.retain(|r| !r.entries.is_empty() || !r.landed.is_empty());
    }
    let mut out = std::io::stdout();
    let written = if options.json {
        writeln!(out, "{}", to_json(&reports))
    } else {
        write!(out, "{}", to_text(&reports, options.since))
    };
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("amont-agent: board: cannot write output: {e}");
            ExitCode::from(1)
        }
    }
}

/// The repository directories to report on, or `None` (after saying why)
/// when none can be determined.
fn select(names: &[String], root: Option<&Path>) -> Option<Vec<PathBuf>> {
    if !names.is_empty() {
        let Some(root) = root else {
            eprintln!(
                "amont-agent: board: no plan root to resolve repositories under; {}",
                crate::plan_review::ROOT_REMEDY
            );
            return None;
        };
        return Some(
            names
                .iter()
                .map(|name| crate::plan_review::locate(root, name))
                .collect(),
        );
    }
    if let Some(cwd) = std::env::current_dir()
        .ok()
        .filter(|c| main_checkout(c).is_some())
    {
        return main_checkout(&cwd).map(|dir| vec![dir]);
    }
    let Some(root) = root else {
        eprintln!(
            "amont-agent: board: not inside a repository and no plan root; {}",
            crate::plan_review::ROOT_REMEDY
        );
        return None;
    };
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(".git").exists())
        .collect();
    dirs.sort();
    Some(dirs)
}

/// The main checkout of the repository `dir` is in (a linked worktree
/// resolves to it).
fn main_checkout(dir: &Path) -> Option<PathBuf> {
    let common = git::stdout_in(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common = PathBuf::from(common);
    if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        Some(common)
    }
}

fn fetch_origin(dir: &Path) -> bool {
    let child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["fetch", "origin", "-q"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if start.elapsed() < FETCH_BUDGET => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill(); // the budget expired; the child may already have exited
                let _ = child.wait(); // reaping only; the fetch has already failed
                return false;
            }
        }
    }
}

fn real(path: &Path) -> PathBuf {
    crate::plan_review::real(path)
}

/// The default branch as a ref to compare against: `origin/<name>` when it
/// exists, else the local branch.
fn default_ref(dir: &Path) -> Option<(String, String)> {
    let exists = |r: &str| {
        git::stdout_in(
            dir,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{r}^{{commit}}"),
            ],
        )
        .is_some()
    };
    let mut names: Vec<String> = Vec::new();
    if let Some(head) = git::stdout_in(
        dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        names.extend(head.strip_prefix("origin/").map(str::to_string));
    }
    names.extend(["main", "master"].map(str::to_string));
    names.into_iter().find_map(|name| {
        let remote = format!("origin/{name}");
        if exists(&remote) {
            Some((remote, name))
        } else if exists(&name) {
            Some((name.clone(), name))
        } else {
            None
        }
    })
}

struct Worktree {
    path: PathBuf,
    branch: Option<String>,
    bare: bool,
}

fn worktrees(dir: &Path) -> Vec<Worktree> {
    let text = git::stdout_in(dir, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    let mut found: Vec<Worktree> = Vec::new();
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            found.push(Worktree {
                path: PathBuf::from(path),
                branch: None,
                bare: false,
            });
        } else if let Some(current) = found.last_mut() {
            if let Some(reference) = line.strip_prefix("branch ") {
                current.branch = Some(
                    reference
                        .strip_prefix("refs/heads/")
                        .unwrap_or(reference)
                        .to_string(),
                );
            } else if line == "bare" {
                current.bare = true;
            }
        }
    }
    found
}

/// A branch the listing found, with its tip time and the full ref to read.
struct Unmerged {
    name: String,
    tip: u64,
    reference: String,
}

/// The ref to read for `branch`: the listed one, else the local branch.
fn reference_of(listed: &[Unmerged], branch: &str) -> String {
    listed
        .iter()
        .find(|b| b.name == branch)
        .map_or_else(|| format!("refs/heads/{branch}"), |b| b.reference.clone())
}

/// Branches not merged into `default`, with their tip commit time in epoch
/// seconds, freshest first. A local branch and its `origin/` twin are one.
fn unmerged_branches(dir: &Path, default: &str, default_name: &str) -> Vec<Unmerged> {
    let listing = git::stdout_in(
        dir,
        &[
            "for-each-ref",
            "--no-merged",
            default,
            "--format=%(refname)\t%(committerdate:unix)",
            "refs/heads",
            "refs/remotes/origin",
        ],
    )
    .unwrap_or_default();
    let mut found: Vec<Unmerged> = Vec::new();
    for line in listing.lines() {
        let Some((reference, tip)) = line.split_once('\t') else {
            continue;
        };
        let branch = reference
            .strip_prefix("refs/heads/")
            .or_else(|| reference.strip_prefix("refs/remotes/origin/"))
            .unwrap_or(reference);
        if branch == "HEAD" || branch == default_name {
            continue;
        }
        let tip: u64 = tip.parse().unwrap_or(0);
        match found.iter_mut().find(|b| b.name == branch) {
            Some(existing) => {
                existing.tip = existing.tip.max(tip);
                if reference.starts_with("refs/heads/") {
                    existing.reference = reference.to_string();
                }
            }
            None => found.push(Unmerged {
                name: branch.to_string(),
                tip,
                reference: reference.to_string(),
            }),
        }
    }
    found.sort_by(|a, b| b.tip.cmp(&a.tip).then_with(|| a.name.cmp(&b.name)));
    found
}

/// An active plan file a branch adds, and the repositories it names.
struct PlanRef {
    file: String,
    title: String,
    repos: Vec<String>,
}

/// The active plans the branch at `reference` adds under docs/plans,
/// relative to `default`.
fn active_plans(dir: &Path, default: &str, reference: &str) -> Vec<PlanRef> {
    let range = format!("{default}...{reference}");
    let added = git::stdout_in(
        dir,
        &[
            "diff",
            "--name-only",
            "--diff-filter=A",
            &range,
            "--",
            "docs/plans",
        ],
    )
    .unwrap_or_default();
    added
        .lines()
        .filter(|f| f.ends_with(".md") && !f.ends_with("README.md"))
        .filter_map(|file| {
            let text = git::stdout_in(dir, &["show", &format!("{reference}:{file}")])?;
            let title = plan_title(&text)?;
            let file = file.rsplit('/').next().unwrap_or(file).to_string();
            Some(PlanRef {
                file,
                title,
                repos: crate::shipped::plan_repos(&text).unwrap_or_default(),
            })
        })
        .collect()
}

/// The title of an `active` plan, `None` for any other status.
fn plan_title(text: &str) -> Option<String> {
    let (front, body) = crate::plans::split_front_matter(text);
    if crate::plans::field(front, "status").as_deref() != Some("active") {
        return None;
    }
    crate::plan_review::title(body)
}

fn age_secs(now: SystemTime, then: SystemTime) -> u64 {
    now.duration_since(then).map_or(0, |d| d.as_secs())
}

fn report(
    dir: &Path,
    options: &Options,
    sessions: &[Seen],
    now: SystemTime,
    plan_root: Option<&Path>,
) -> Report {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let default = default_ref(dir);
    let (default_ref_name, default_name) = default.clone().unwrap_or_default();
    let now_epoch = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let identity = git::stdout_in(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map(|c| real(Path::new(&c)));
    let here: Vec<&Seen> = sessions
        .iter()
        .filter(|s| {
            identity
                .as_ref()
                .is_some_and(|i| real(&s.repo_identity) == *i)
        })
        .collect();
    let sessions_of = |matches: &dyn Fn(&Seen) -> bool| -> Vec<Session> {
        let mut named: Vec<&&Seen> = here.iter().filter(|s| matches(s)).collect();
        named.sort_by(|a, b| {
            b.last_seen
                .cmp(&a.last_seen)
                .then_with(|| a.session.cmp(&b.session))
        });
        named
            .into_iter()
            .map(|s| Session {
                id: s.session.clone(),
                last_seen_secs: age_secs(now, s.last_seen),
            })
            .collect()
    };
    let listed = if default.is_some() {
        unmerged_branches(dir, &default_ref_name, &default_name)
    } else {
        Vec::new()
    };
    let plans_of = |branch: &str| {
        if default.is_some() {
            active_plans(dir, &default_ref_name, &reference_of(&listed, branch))
        } else {
            Vec::new()
        }
    };
    let mut own_plan_files: Vec<String> = Vec::new();

    let mut entries: Vec<Entry> = Vec::new();
    let mut checked_out: Vec<String> = Vec::new();
    for (index, tree) in worktrees(dir).into_iter().enumerate() {
        if tree.bare {
            continue;
        }
        let tree_path = real(&tree.path);
        let tree_sessions = sessions_of(&|s| real(&s.worktree) == tree_path);
        let on_default = tree.branch.as_deref() == Some(default_name.as_str());
        if index == 0 && on_default && tree_sessions.is_empty() {
            continue;
        }
        let branch = tree
            .branch
            .clone()
            .unwrap_or_else(|| "detached".to_string());
        let plan_refs = tree.branch.as_deref().map(plans_of).unwrap_or_default();
        own_plan_files.extend(plan_refs.iter().map(|p| p.file.clone()));
        let plans = plan_refs.into_iter().map(|p| p.title).collect();
        if let Some(b) = &tree.branch {
            checked_out.push(b.clone());
        }
        entries.push(Entry {
            path: Some(tree.path),
            branch,
            plans,
            sessions: tree_sessions,
        });
    }
    if default.is_some() {
        let window = options.since.as_secs();
        for Unmerged {
            name: branch, tip, ..
        } in &listed
        {
            let branch = branch.clone();
            if checked_out.contains(&branch) {
                continue;
            }
            let named = sessions_of(&|s| s.branch == branch);
            let recent = now_epoch.saturating_sub(*tip) <= window;
            if recent || !named.is_empty() {
                let plan_refs = plans_of(&branch);
                own_plan_files.extend(plan_refs.iter().map(|p| p.file.clone()));
                let plans: Vec<String> = plan_refs.into_iter().map(|p| p.title).collect();
                entries.push(Entry {
                    path: None,
                    branch,
                    plans,
                    sessions: named,
                });
            }
        }
    }
    entries.extend(sibling_plans(
        dir,
        &name,
        &own_plan_files,
        plan_root,
        options.since,
        now_epoch,
    ));
    let landed = if default.is_some() {
        landed(dir, &default_ref_name, options.since, now_epoch)
    } else {
        Vec::new()
    };
    Report {
        name,
        path: dir.to_path_buf(),
        entries,
        landed,
    }
}

/// Active plans that live in another repository under the plan root but name
/// `name` in their `repos:`, as branch-only entries labelled
/// `<repository>:<branch>`; a plan whose file `name` already carries is left
/// out.
///
/// Only a live branch is read: a tip within `since` or a checked-out
/// worktree. Sessions are not consulted here: a sibling's sessions show when
/// the board is asked about that repository. A sibling with no live branch
/// costs its default-branch probes and two listings.
// holds-until: ~100 repositories under the plan root — four to seven git calls per sibling per call (default-branch probes, `for-each-ref`, `worktree list`); cache per repository after
fn sibling_plans(
    dir: &Path,
    name: &str,
    own_plan_files: &[String],
    plan_root: Option<&Path>,
    since: Duration,
    now_epoch: u64,
) -> Vec<Entry> {
    let Some(root) = plan_root else {
        return Vec::new();
    };
    let mut siblings: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(".git").exists() && real(p) != real(dir))
        .collect();
    siblings.sort();
    let mut found = Vec::new();
    for sibling in siblings {
        let Some((default_ref_name, default_name)) = default_ref(&sibling) else {
            continue;
        };
        let sibling_name = sibling
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let listed = unmerged_branches(&sibling, &default_ref_name, &default_name);
        let mut branches: Vec<String> = worktrees(&sibling)
            .into_iter()
            .filter_map(|tree| tree.branch)
            .filter(|b| *b != default_name)
            .collect();
        for fresh in listed
            .iter()
            .filter(|b| now_epoch.saturating_sub(b.tip) <= since.as_secs())
        {
            if !branches.contains(&fresh.name) {
                branches.push(fresh.name.clone());
            }
        }
        for branch in branches {
            let reference = reference_of(&listed, &branch);
            let titles: Vec<String> = active_plans(&sibling, &default_ref_name, &reference)
                .into_iter()
                .filter(|p| p.repos.iter().any(|r| r == name) && !own_plan_files.contains(&p.file))
                .map(|p| p.title)
                .collect();
            if !titles.is_empty() {
                found.push(Entry {
                    path: None,
                    branch: format!("{sibling_name}:{branch}"),
                    plans: titles,
                    sessions: Vec::new(),
                });
            }
        }
    }
    found
}

fn landed(dir: &Path, default: &str, since: Duration, now_epoch: u64) -> Vec<Landed> {
    let window = format!("--since={} seconds ago", since.as_secs());
    let listing = git::stdout_in(
        dir,
        &[
            "log",
            "--first-parent",
            &window,
            "--format=%h\t%ct\t%s",
            default,
        ],
    )
    .unwrap_or_default();
    listing
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let sha = parts.next()?.to_string();
            let committed: u64 = parts.next()?.parse().ok()?;
            let subject = parts.next()?.to_string();
            Some(Landed {
                sha,
                subject,
                age_secs: now_epoch.saturating_sub(committed),
            })
        })
        .collect()
}

fn ago(secs: u64) -> String {
    let now = UNIX_EPOCH + Duration::from_secs(secs);
    activity::ago(now, UNIX_EPOCH)
}

fn to_text(reports: &[Report], since: Duration) -> String {
    let mut out = String::new();
    for report in reports {
        out.push_str(&format!("{} ({})\n", report.name, report.path.display()));
        if report.entries.is_empty() && report.landed.is_empty() {
            out.push_str("  nothing in flight, nothing landed\n");
            continue;
        }
        if !report.entries.is_empty() {
            out.push_str("  in flight:\n");
        }
        for entry in &report.entries {
            let place = entry
                .path
                .as_ref()
                .map_or("(no worktree)".to_string(), |p| p.display().to_string());
            out.push_str(&format!("    {} [{}]\n", place, entry.branch));
            if let Some(ticket) = entry.programme_ticket() {
                let (slug, id) = ticket.split_once('/').unwrap_or((&ticket, ""));
                out.push_str(&format!("      programme {slug} ticket {id}\n"));
            }
            for plan in &entry.plans {
                out.push_str(&format!("      plan: {plan}\n"));
            }
            for session in &entry.sessions {
                out.push_str(&format!(
                    "      session {} (seen {} ago)\n",
                    session.id,
                    ago(session.last_seen_secs)
                ));
            }
        }
        if !report.landed.is_empty() {
            out.push_str(&format!("  just landed (last {}):\n", ago(since.as_secs())));
            for landed in &report.landed {
                out.push_str(&format!(
                    "    {} {} ({} ago)\n",
                    landed.sha,
                    landed.subject,
                    ago(landed.age_secs)
                ));
            }
        }
    }
    out
}

fn to_json(reports: &[Report]) -> String {
    use serde_json::json;
    let rows: Vec<serde_json::Value> = reports
        .iter()
        .map(|report| {
            let trees: Vec<serde_json::Value> = report
                .entries
                .iter()
                .map(|entry| {
                    let sessions: Vec<serde_json::Value> = entry
                        .sessions
                        .iter()
                        .map(|s| json!({"id": s.id, "last_seen_secs": s.last_seen_secs}))
                        .collect();
                    json!({
                        "path": entry.path.as_ref().map(|p| p.display().to_string()),
                        "branch": entry.branch,
                        "plans": entry.plans,
                        "programme_ticket": entry.programme_ticket(),
                        "sessions": sessions,
                    })
                })
                .collect();
            let landed: Vec<serde_json::Value> = report
                .landed
                .iter()
                .map(|l| json!({"sha": l.sha, "subject": l.subject, "age_secs": l.age_secs}))
                .collect();
            json!({
                "repo": report.name,
                "path": report.path.display().to_string(),
                "worktrees": trees,
                "landed": landed,
            })
        })
        .collect();
    serde_json::Value::Array(rows).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_active_plan_has_a_title() {
        assert_eq!(
            plan_title("---\nstatus: active\n---\n# Board\n"),
            Some("Board".to_string())
        );
        assert_eq!(plan_title("---\nstatus: done\n---\n# Board\n"), None);
        assert_eq!(
            plan_title("---\nstatus: active\n---\n```\n# not the title\n```\n# Real\n"),
            Some("Real".to_string())
        );
    }

    #[test]
    fn programme_branches_are_labelled() {
        let entry = |branch: &str| Entry {
            path: None,
            branch: branch.to_string(),
            plans: Vec::new(),
            sessions: Vec::new(),
        };
        assert_eq!(
            entry("prog/x/t02").programme_ticket().as_deref(),
            Some("x/t02")
        );
        assert_eq!(entry("feat/x").programme_ticket(), None);
    }

    #[test]
    fn a_bad_duration_is_a_usage_error() {
        let args = ["--since".to_string(), "soon".to_string()];
        assert!(matches!(parse(&args), Parsed::Usage(_)));
    }
}
