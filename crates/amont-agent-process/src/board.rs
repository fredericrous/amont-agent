//! `amont-agent board [--repo <name>...] [--since <dur>] [--fetch] [--json]`:
//! the work in flight per repository and what landed recently, derived from
//! git and the session activity files on every call (decisions ADR-0051).
//! Read-only: `--fetch` (`git fetch --prune origin -q`) updates and prunes
//! remote-tracking refs and nothing else; the
//! activity files are only read, through `amont_agent_guard::activity`.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use amont_agent_guard::activity::{self, Seen};
use amont_agent_guard::git;

const DEFAULT_SINCE: Duration = Duration::from_secs(48 * 3600);
const FETCH_BUDGET: Duration = Duration::from_secs(60);
/// Repositories fetched at once.
const FETCH_PARALLEL: usize = 4;
/// Each bounded git lookup of the report.
const LOOKUP_BUDGET: Duration = Duration::from_secs(30);
/// Keeps a diff or a log from the user's own git configuration out of the
/// patch-id input.
const PLAIN_DIFF: [&str; 3] = ["--no-ext-diff", "--no-textconv", "--no-color"];

pub const USAGE: &str = "\
usage: amont-agent board [--repo <name>...] [--since <dur>] [--fetch] [--json]

The work in flight per repository, and what landed recently, derived from git
and the session activity files on every call. Read-only.

In flight: each worktree; each local or origin branch not merged into the
default branch that is live: its tip commit is newer than --since, it is
checked out in a worktree, or a session's latest activity line names it. A
branch whose whole change is already on the default branch as one commit
(same stable patch-id: a squash merge) is merged: left out, or, when a worktree
holds it, listed with `(merged: git worktree remove <path>)`. Per entry: the
path, the branch, the active plan's title, `programme <slug> ticket <id>` for a prog/<slug>/<id>
branch, and the sessions whose latest activity line names that worktree, each
with its last-seen age. A branch older than --since that is not
checked out and that no session names is left out. Listing the whole plan root
(no --repo) skips the scan of sibling repositories for plans that name one.

Just landed: the first-parent commits of the default branch within --since.

  --repo <name>   a repository, resolved like plan-review repos (git config
                  amont.agent.plan-review.<name>.path, else <plan root>/<name>);
                  repeatable. Without it: the working directory's repository,
                  else every repository directly under the plan root.
  --since <dur>   the window: 45s, 90m, 3h, 2d (default 48h)
  --fetch         run `git fetch --prune origin -q` on the selected
                  repositories first (up to 4 at once)
  --json          one JSON array on stdout: {repo, path, worktrees: [{path,
                  branch, merged, plans: [title], programme_ticket, sessions:
                  [{id, last_seen_secs}]}], landed: [{sha, subject, age_secs}]}

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
    /// The branch's whole change is already on the default branch.
    merged: bool,
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
    // Listing the whole root already shows every repository, so the
    // per-repository scan of the others for plans that name it is skipped.
    let whole_root = !named && selected.len() > 1;
    let sibling_root = if whole_root { None } else { root.as_deref() };
    let sessions = activity::latest_all();
    let now = SystemTime::now();
    let mut readable: Vec<PathBuf> = Vec::new();
    for dir in selected {
        if git::stdout_in(&dir, &["rev-parse", "--git-dir"]).is_none() {
            eprintln!(
                "amont-agent: board: {} is not a readable git repository",
                dir.display()
            );
            continue;
        }
        readable.push(dir);
    }
    if options.fetch {
        fetch_all(&readable);
    }
    let reports: Vec<Report> = readable
        .iter()
        .map(|dir| report(dir, &options, &sessions, now, sibling_root))
        .collect();
    let mut reports = reports;
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
        .filter(|p| p.join(".git").is_dir())
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

fn repo_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `git fetch --prune origin -q` in every repository, at most
/// [`FETCH_PARALLEL`] at once, each within [`FETCH_BUDGET`]. Every fetch
/// finishes before this returns; failures are reported in the order of `dirs`,
/// one stderr line each starting `<repo>:`.
fn fetch_all(dirs: &[PathBuf]) {
    use std::io::IsTerminal as _;
    use std::sync::atomic::{AtomicUsize, Ordering};
    if dirs.is_empty() {
        return;
    }
    if std::io::stderr().is_terminal() {
        eprintln!("fetching {} repositories...", dirs.len());
    }
    let next = AtomicUsize::new(0);
    let outcomes: Vec<std::sync::OnceLock<bool>> =
        dirs.iter().map(|_| std::sync::OnceLock::new()).collect();
    std::thread::scope(|scope| {
        for _ in 0..FETCH_PARALLEL.min(dirs.len()) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::SeqCst);
                let Some(dir) = dirs.get(index) else {
                    return;
                };
                let deadline = Instant::now() + FETCH_BUDGET;
                let fetched =
                    git::bytes_bounded(dir, &["fetch", "--prune", "origin", "-q"], None, deadline)
                        .is_some();
                // Each index is claimed by exactly one worker.
                let _ = outcomes[index].set(fetched);
            });
        }
    });
    for (dir, outcome) in dirs.iter().zip(&outcomes) {
        if outcome.get() != Some(&true) {
            eprintln!(
                "{}: `git fetch --prune origin -q` failed in {}",
                repo_name(dir),
                dir.display()
            );
        }
    }
}

fn real(path: &Path) -> PathBuf {
    crate::plan_review::real(path)
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
    let mut index: HashMap<String, usize> = HashMap::new();
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
        match index.get(branch) {
            Some(&at) => {
                let existing = &mut found[at];
                existing.tip = existing.tip.max(tip);
                if reference.starts_with("refs/heads/") {
                    existing.reference = reference.to_string();
                }
            }
            None => {
                index.insert(branch.to_string(), found.len());
                found.push(Unmerged {
                    name: branch.to_string(),
                    tip,
                    reference: reference.to_string(),
                });
            }
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
    let deadline = Instant::now() + LOOKUP_BUDGET;
    git::active_added_plans(dir, default, reference, deadline)
        .into_iter()
        .map(|plan| {
            let title = plan.title;
            let file = plan
                .file
                .rsplit('/')
                .next()
                .unwrap_or(&plan.file)
                .to_string();
            PlanRef {
                file,
                title,
                repos: crate::shipped::plan_repos(&plan.text).unwrap_or_default(),
            }
        })
        .collect()
}

/// The branches among `candidates` (name, ref) whose whole change from the
/// merge-base is already on `default` as one non-merge commit: the same
/// `git patch-id --stable`. A squash merge reads as merged this way. Nothing is
/// written to the repository.
///
/// One `git log -p` of the default branch since the oldest merge-base is built
/// per call; each candidate then costs a merge-base and one diff.
fn merged_branches(dir: &Path, default: &str, candidates: &[(String, String)]) -> HashSet<String> {
    let deadline = Instant::now() + LOOKUP_BUDGET;
    let mut changes: Vec<(&str, String, String)> = Vec::new();
    for (branch, reference) in candidates {
        let Some(base) = git::stdout_bounded(dir, &["merge-base", default, reference], deadline)
        else {
            continue;
        };
        let mut diff = vec!["diff"];
        diff.extend(PLAIN_DIFF);
        diff.extend([base.as_str(), reference.as_str()]);
        let Some(patch) = git::bytes_bounded(dir, &diff, None, deadline) else {
            continue;
        };
        if patch.is_empty() {
            continue;
        }
        if let Some(id) = first_patch_id(&patch, deadline, dir) {
            changes.push((branch.as_str(), base, id));
        }
    }
    if changes.is_empty() {
        return HashSet::new();
    }
    let mut bases: Vec<&str> = changes.iter().map(|(_, base, _)| base.as_str()).collect();
    bases.sort_unstable();
    bases.dedup();
    let oldest = if let [only] = bases.as_slice() {
        (*only).to_string()
    } else {
        let mut octopus = vec!["merge-base", "--octopus"];
        octopus.extend(&bases);
        match git::stdout_bounded(dir, &octopus, deadline) {
            Some(oldest) => oldest,
            None => return HashSet::new(),
        }
    };
    let range = format!("{oldest}..{default}");
    let mut log = vec!["log", "-p", "--no-merges"];
    log.extend(PLAIN_DIFF);
    log.push(&range);
    let Some(history) = git::bytes_bounded(dir, &log, None, deadline) else {
        return HashSet::new();
    };
    let Some(ids) = git::bytes_bounded(dir, &["patch-id", "--stable"], Some(&history), deadline)
    else {
        return HashSet::new();
    };
    let on_default: HashSet<String> = String::from_utf8_lossy(&ids)
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .collect();
    changes
        .into_iter()
        .filter(|(_, _, id)| on_default.contains(id))
        .map(|(branch, _, _)| branch.to_string())
        .collect()
}

/// The stable patch-id of a single diff.
fn first_patch_id(patch: &[u8], deadline: Instant, dir: &Path) -> Option<String> {
    let out = git::bytes_bounded(dir, &["patch-id", "--stable"], Some(patch), deadline)?;
    String::from_utf8_lossy(&out)
        .split_whitespace()
        .next()
        .map(str::to_string)
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
    let default = git::default_ref(dir, Instant::now() + LOOKUP_BUDGET);
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

    let trees: Vec<Worktree> = worktrees(dir).into_iter().filter(|t| !t.bare).collect();
    let checked_out: HashSet<&str> = trees.iter().filter_map(|t| t.branch.as_deref()).collect();
    let window = options.since.as_secs();
    let live: Vec<(&Unmerged, Vec<Session>)> = listed
        .iter()
        .filter(|b| !checked_out.contains(b.name.as_str()))
        .filter_map(|b| {
            let named = sessions_of(&|s| s.branch == b.name);
            let recent = now_epoch.saturating_sub(b.tip) <= window;
            (recent || !named.is_empty()).then_some((b, named))
        })
        .collect();
    let candidates: Vec<(String, String)> = listed
        .iter()
        .filter(|b| checked_out.contains(b.name.as_str()))
        .chain(live.iter().map(|(b, _)| *b))
        .map(|b| (b.name.clone(), b.reference.clone()))
        .collect();
    let merged = if default.is_some() {
        merged_branches(dir, &default_ref_name, &candidates)
    } else {
        HashSet::new()
    };

    let mut entries: Vec<Entry> = Vec::new();
    for (index, tree) in trees.into_iter().enumerate() {
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
        entries.push(Entry {
            path: Some(tree.path),
            merged: tree.branch.as_ref().is_some_and(|b| merged.contains(b)),
            branch,
            plans,
            sessions: tree_sessions,
        });
    }
    for (Unmerged { name: branch, .. }, named) in live {
        if merged.contains(branch.as_str()) {
            continue;
        }
        let plan_refs = plans_of(branch);
        own_plan_files.extend(plan_refs.iter().map(|p| p.file.clone()));
        let plans: Vec<String> = plan_refs.into_iter().map(|p| p.title).collect();
        entries.push(Entry {
            path: None,
            branch: branch.clone(),
            plans,
            sessions: named,
            merged: false,
        });
    }
    entries.extend(sibling_plans(
        identity.clone().unwrap_or_else(|| real(&dir.join(".git"))),
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

/// What one `for-each-ref` call reads of a sibling repository.
struct SiblingRefs {
    default_ref_name: String,
    /// Non-default branches: name, freshest tip, checked out in a worktree.
    branches: Vec<(String, u64, bool)>,
}

/// The default branch and the branches of a primary checkout from a single
/// `git for-each-ref` over `refs/heads` and `refs/remotes/origin`: the default
/// is origin/HEAD's target, else main, else master (the remote ref when it
/// exists, else the local one). Branches are every non-default name with its
/// freshest tip and whether a worktree holds it.
fn sibling_refs(dir: &Path) -> Option<SiblingRefs> {
    let listing = git::stdout_in(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname)\t%(symref)\t%(committerdate:unix)\t%(worktreepath)",
            "refs/heads",
            "refs/remotes/origin",
        ],
    )?;
    let mut refs: Vec<(String, u64, bool)> = Vec::new();
    let mut origin_head: Option<String> = None;
    for line in listing.lines() {
        let mut fields = line.split('\t');
        let (Some(reference), Some(symref), Some(tip)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let worktree = fields.next().unwrap_or("");
        if reference == "refs/remotes/origin/HEAD" {
            origin_head = symref
                .strip_prefix("refs/remotes/origin/")
                .map(str::to_string);
            continue;
        }
        refs.push((
            reference.to_string(),
            tip.parse().unwrap_or(0),
            !worktree.is_empty(),
        ));
    }
    let has = |reference: &str| refs.iter().any(|(r, _, _)| r == reference);
    let mut names: Vec<String> = origin_head.into_iter().collect();
    names.extend(["main", "master"].map(str::to_string));
    let (default_ref_name, default_name) = names.into_iter().find_map(|name| {
        if has(&format!("refs/remotes/origin/{name}")) {
            Some((format!("origin/{name}"), name))
        } else if has(&format!("refs/heads/{name}")) {
            Some((name.clone(), name))
        } else {
            None
        }
    })?;
    let mut branches: Vec<(String, u64, bool)> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (reference, tip, checked_out) in &refs {
        let branch = reference
            .strip_prefix("refs/heads/")
            .or_else(|| reference.strip_prefix("refs/remotes/origin/"))
            .unwrap_or(reference);
        if branch == default_name {
            continue;
        }
        match index.get(branch) {
            Some(&at) => {
                let existing = &mut branches[at];
                existing.1 = existing.1.max(*tip);
                existing.2 |= *checked_out;
            }
            None => {
                index.insert(branch, branches.len());
                branches.push((branch.to_string(), *tip, *checked_out));
            }
        }
    }
    Some(SiblingRefs {
        default_ref_name,
        branches,
    })
}

/// Active plans that live in another repository under the plan root but name
/// `name` in their `repos:`, as branch-only entries labelled
/// `<repository>:<branch>`; a plan whose file `name` already carries is left
/// out.
///
/// Only primary checkouts are scanned (a `.git` directory, deduplicated by it;
/// a linked worktree's `.git` is a file) and only a live branch is read: a tip
/// within `since` or a checked-out worktree. Sessions are not consulted here:
/// a sibling's sessions show when the board is asked about that repository.
/// A sibling with no live branch costs one git call.
// holds-until: ~100 repositories under the plan root — one git call per sibling with no live branch per call; cache per repository after
fn sibling_plans(
    own_common_dir: PathBuf,
    name: &str,
    own_plan_files: &[String],
    plan_root: Option<&Path>,
    since: Duration,
    now_epoch: u64,
) -> Vec<Entry> {
    let Some(root) = plan_root else {
        return Vec::new();
    };
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(".git").is_dir())
        .collect();
    candidates.sort();
    let mut seen: Vec<PathBuf> = vec![own_common_dir];
    let siblings: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|p| {
            let common = real(&p.join(".git"));
            let fresh = !seen.contains(&common);
            seen.push(common);
            fresh
        })
        .collect();
    let mut found = Vec::new();
    for sibling in siblings {
        let Some(refs) = sibling_refs(&sibling) else {
            continue;
        };
        let live: Vec<&(String, u64, bool)> = refs
            .branches
            .iter()
            .filter(|(_, tip, checked_out)| {
                *checked_out || now_epoch.saturating_sub(*tip) <= since.as_secs()
            })
            .collect();
        if live.is_empty() {
            continue;
        }
        let sibling_name = sibling
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut command: Vec<String> = vec![
            "for-each-ref".into(),
            "--no-merged".into(),
            refs.default_ref_name.clone(),
            "--format=%(refname)".into(),
        ];
        for (branch, _, _) in &live {
            command.push(format!("refs/heads/{branch}"));
            command.push(format!("refs/remotes/origin/{branch}"));
        }
        let command: Vec<&str> = command.iter().map(String::as_str).collect();
        let unmerged = git::stdout_in(&sibling, &command).unwrap_or_default();
        for (branch, _, _) in live {
            let local = format!("refs/heads/{branch}");
            let remote = format!("refs/remotes/origin/{branch}");
            let reference = if unmerged.lines().any(|l| l == local) {
                local
            } else if unmerged.lines().any(|l| l == remote) {
                remote
            } else {
                continue;
            };
            let titles: Vec<String> = active_plans(&sibling, &refs.default_ref_name, &reference)
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
                    merged: false,
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
            let label = match (&entry.path, entry.merged) {
                (Some(path), true) => {
                    format!(" (merged: git worktree remove {})", path.display())
                }
                _ => String::new(),
            };
            out.push_str(&format!("    {} [{}]{label}\n", place, entry.branch));
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
                        "merged": entry.merged,
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
    fn programme_branches_are_labelled() {
        let entry = |branch: &str| Entry {
            path: None,
            branch: branch.to_string(),
            plans: Vec::new(),
            sessions: Vec::new(),
            merged: false,
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
