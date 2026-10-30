//! `amont-agent shipped <plan.md> [--json]`: what shipped under a plan,
//! derived from git (decisions ADR-0036). Read-only: the one thing it writes
//! is the two local note refs it fetches, and temporary files for the
//! signature check, which it removes.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use amont_agent_guard::git;

const PROVENANCE_REF: &str = "refs/notes/amont-provenance";
const ATTEST_REF: &str = "refs/notes/amont-attest";
const NAMESPACE: &str = "amont-provenance";
const MAGIC: &str = "amont-provenance-v1";
const SIG_END: &str = "-----END SSH SIGNATURE-----";
const FETCH_BUDGET: Duration = Duration::from_secs(60);
const STALE_DAYS: u64 = 30;

pub const USAGE: &str = "\
usage: amont-agent shipped <plan.md> [--json]

What shipped under a plan, derived from git (decisions ADR-0036). For each
repository in the plan's `repos:` list, every merge commit on origin/main
that touched docs/plans/<the plan's file name>: its PR number, its head
commit, its first release tag, and the signed amont-provenance record and
attestation gates found for it. Read-only: the two note refs are fetched
from origin into local refs of the same name (a failed fetch is said once),
and nothing else is written.

  --json   one JSON array on stdout: {repo, pr, merge, head, release,
           provenance: {status, review, preview, plan_body_sha, flags} | null,
           gates}

Exit 0, 1 when no repository could be read, 2 on a usage error.
";

struct Row {
    repo: String,
    pr: Option<u64>,
    merge: String,
    head: Option<String>,
    release: Option<String>,
    prov: Option<Prov>,
    gates: Vec<String>,
}

struct Prov {
    status: &'static str,
    review: String,
    preview: String,
    plan_sha: String,
    flags: Vec<&'static str>,
}

/// One payload with its armored signature.
struct Block {
    payload: String,
    sig: String,
}

impl Block {
    fn field(&self, prefix: &str) -> Option<&str> {
        self.payload
            .lines()
            .find_map(|l| l.strip_prefix(prefix)?.strip_prefix(' '))
    }
    fn is_record(&self) -> bool {
        self.payload.lines().next() == Some(MAGIC)
    }
}

fn blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    loop {
        while lines.next_if(|l| l.trim().is_empty()).is_some() {}
        if lines.peek().is_none() {
            return out;
        }
        let mut payload = String::new();
        while let Some(l) = lines.next_if(|l| !l.trim().is_empty()) {
            payload.push_str(l);
            payload.push('\n');
        }
        while lines.next_if(|l| l.trim().is_empty()).is_some() {}
        let mut sig = String::new();
        if lines.peek().is_some_and(|l| l.starts_with("-----BEGIN")) {
            for l in lines.by_ref() {
                sig.push_str(l);
                sig.push('\n');
                if l.trim() == SIG_END {
                    break;
                }
            }
        }
        out.push(Block { payload, sig });
    }
}

/// The repository names of the plan: its front matter `repos:` list, else
/// the machine comment's `repos=`.
fn plan_repos(text: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut lines = text.lines();
    if lines.next().map(str::trim) == Some("---") {
        let mut in_list = false;
        for l in lines.take_while(|l| l.trim() != "---") {
            if let Some(rest) = l.strip_prefix("repos:") {
                let rest = rest.trim();
                in_list = rest.is_empty();
                names.extend(
                    rest.trim_start_matches('[')
                        .trim_end_matches(']')
                        .split(',')
                        .map(|s| s.trim().trim_matches(['"', '\'']).to_string())
                        .filter(|s| !s.is_empty()),
                );
            } else if in_list {
                match l.trim().strip_prefix("- ") {
                    Some(n) => names.push(n.trim().trim_matches(['"', '\'']).to_string()),
                    None => in_list = false,
                }
            }
        }
    }
    if names.is_empty() {
        names = crate::plan_review::meta(text)
            .map_err(|_| "the plan names no repository (front matter `repos:`)".to_string())?
            .repos;
    }
    if let Some(bad) = names.iter().find(|n| !crate::plan_review::valid_name(n)) {
        return Err(format!("`{bad}` is not a repository name"));
    }
    Ok(names)
}

fn pr_number(subject: &str) -> Option<u64> {
    subject.rmatch_indices("(#").find_map(|(at, _)| {
        let rest = &subject[at + 2..];
        rest[..rest.find(')')?].parse().ok()
    })
}

/// `owner/repo` of a github.com origin URL.
fn github_slug(url: &str) -> Option<String> {
    let rest = [
        "https://github.com/",
        "git@github.com:",
        "ssh://git@github.com/",
    ]
    .iter()
    .find_map(|p| url.strip_prefix(p))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    (rest.split('/').count() == 2).then(|| rest.to_string())
}

/// The head of a GitHub squash: the one `gh` call per PR. `AMONT_AGENT_GH`
/// names another program (tests); an absent or failing gh gives `None`.
fn gh_head(slug: &str, pr: u64) -> Option<String> {
    let prog = std::env::var_os("AMONT_AGENT_GH").unwrap_or_else(|| OsString::from("gh"));
    let out = Command::new(prog)
        .args(["pr", "view", &pr.to_string(), "--repo", slug])
        .args(["--json", "headRefOid"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    v.get("headRefOid")?.as_str().map(str::to_string)
}

fn fetch_notes(dir: &Path) -> bool {
    let mut ok = true;
    for r in [PROVENANCE_REF, ATTEST_REF] {
        let spec = format!("+{r}:{r}");
        let child = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["fetch", "--quiet", "origin", &spec])
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            ok = false;
            continue;
        };
        let start = Instant::now();
        let done = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s.success(),
                Ok(None) if start.elapsed() < FETCH_BUDGET => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break false;
                }
            }
        };
        ok &= done;
    }
    ok
}

fn notes_show(dir: &Path, note_ref: &str, object: &str) -> Option<String> {
    let ref_arg = format!("--ref={note_ref}");
    git::stdout_in(dir, &["notes", &ref_arg, "show", object])
}

/// Is the record's commit on origin, or the PR head of this merge (whose
/// branch may be gone from origin)?
fn reachable(dir: &Path, commit: &str, head: Option<&str>) -> bool {
    head == Some(commit) && !commit.is_empty() || reachable_from_origin(dir, commit)
}

fn reachable_from_origin(dir: &Path, commit: &str) -> bool {
    if commit.is_empty() || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    git::stdout_in(
        dir,
        &[
            "for-each-ref",
            "--count=1",
            "--format=%(refname)",
            "--contains",
            commit,
            "refs/remotes/origin",
        ],
    )
    .is_some_and(|o| !o.is_empty())
}

fn patch_id(dir: &Path, parent: &str, merge: &str) -> Option<String> {
    crate::implementation_review::branch_patch(dir, parent, merge).ok()
}

struct Found {
    block: Block,
    object: String,
}

fn lookup(dir: &Path, merge: &str, head: Option<&str>, parent: Option<&str>) -> Option<Found> {
    let tree = git::stdout_in(dir, &["rev-parse", &format!("{merge}^{{tree}}")]);
    for object in head.into_iter().map(str::to_string).chain(tree) {
        let Some(text) = notes_show(dir, PROVENANCE_REF, &object) else {
            continue;
        };
        for block in blocks(&text) {
            if block.is_record() && reachable(dir, block.field("commit").unwrap_or(""), head) {
                return Some(Found { block, object });
            }
        }
    }
    let want = patch_id(dir, parent?, merge)?;
    let ref_arg = format!("--ref={PROVENANCE_REF}");
    let listing = git::stdout_in(dir, &["notes", &ref_arg, "list"])?;
    let pairs: Vec<(String, String)> = listing
        .lines()
        .filter_map(|l| {
            let (blob, obj) = l.split_once(' ')?;
            Some((blob.to_string(), obj.to_string()))
        })
        .collect();
    let names: Vec<String> = pairs.iter().map(|(b, _)| b.clone()).collect();
    let found = git::cat_file_batch(dir, &names).ok()??;
    for (blob, bytes) in found {
        let object = pairs.iter().find(|(b, _)| *b == blob).map(|(_, o)| o);
        for block in blocks(&String::from_utf8_lossy(&bytes)) {
            if block.is_record()
                && block.field("patch") == Some(want.as_str())
                && reachable(dir, block.field("commit").unwrap_or(""), head)
            {
                return Some(Found {
                    block,
                    object: object?.clone(),
                });
            }
        }
    }
    None
}

fn scratch_dir() -> Option<PathBuf> {
    let d = std::env::temp_dir().join(format!("amont-shipped-{}", std::process::id()));
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

/// What a record's signature status is when it could not be checked at all:
/// no `ssh-keygen`, or no scratch file for it. Never a verdict on the record.
const NOT_CHECKED: &str = "signature not checked";

/// Run `ssh-keygen` with `args`, feeding the payload on stdin: `Some(ok)` for
/// its answer, `None` when it could not be run.
fn keygen_accepts(args: &[&std::ffi::OsStr], payload: &str) -> Option<bool> {
    let mut child = Command::new("ssh-keygen")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(payload.as_bytes());
    }
    child.wait().ok().map(|s| s.success())
}

/// The status of a block's signature: `verified` when a principal of the
/// committed allowed_signers verifies it under the `amont-provenance`
/// namespace; `unsigned` without a signature; `bad signature` when the
/// signature is not valid for the payload at all; `signed, no
/// allowed_signers` without the file; `signature not checked` when
/// `ssh-keygen` or a scratch file is unavailable; else `signed, signer not
/// allowed`.
fn signature_status(dir: &Path, block: &Block) -> &'static str {
    if block.sig.trim().is_empty() {
        return "unsigned";
    }
    let Some(tmp) = scratch_dir() else {
        return NOT_CHECKED;
    };
    let (sig_file, signers_file) = (tmp.join("sig"), tmp.join("allowed_signers"));
    let status = signature_status_in(dir, block, &sig_file, &signers_file);
    let _ = std::fs::remove_dir_all(&tmp);
    status
}

fn signature_status_in(
    dir: &Path,
    block: &Block,
    sig_file: &Path,
    signers_file: &Path,
) -> &'static str {
    if std::fs::write(sig_file, &block.sig).is_err() {
        return NOT_CHECKED;
    }
    let namespace = std::ffi::OsStr::new(NAMESPACE);
    let check = [
        "-Y".as_ref(),
        "check-novalidate".as_ref(),
        "-n".as_ref(),
        namespace,
        "-s".as_ref(),
        sig_file.as_os_str(),
    ];
    match keygen_accepts(&check, &block.payload) {
        None => return NOT_CHECKED,
        Some(false) => return "bad signature",
        Some(true) => {}
    }
    let signers = [".github/allowed_signers", ".forgejo/allowed_signers"]
        .iter()
        .find_map(|p| {
            git::bytes_in(dir, &["show", &format!("origin/main:{p}")], None)
                .ok()
                .flatten()
        });
    let Some(signers) = signers else {
        return "signed, no allowed_signers";
    };
    if std::fs::write(signers_file, &signers).is_err() {
        return NOT_CHECKED;
    }
    let text = String::from_utf8_lossy(&signers).into_owned();
    let principals = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_whitespace().next())
        .flat_map(|f| f.split(','))
        .map(|p| p.trim_matches('"').to_string());
    for who in principals {
        let args = [
            "-Y".as_ref(),
            "verify".as_ref(),
            "-n".as_ref(),
            namespace,
            "-I".as_ref(),
            std::ffi::OsStr::new(&who),
            "-s".as_ref(),
            sig_file.as_os_str(),
            "-f".as_ref(),
            signers_file.as_os_str(),
        ];
        match keygen_accepts(&args, &block.payload) {
            None => return NOT_CHECKED,
            Some(true) => return "verified",
            Some(false) => {}
        }
    }
    "signed, signer not allowed"
}

fn plan_sha_at(dir: &Path, rev: &str, path: &str) -> Option<String> {
    let bytes = git::bytes_in(dir, &["show", &format!("{rev}:{path}")], None).ok()??;
    crate::plan_text::shas(&String::from_utf8_lossy(&bytes))
        .ok()
        .map(|s| s.sha)
}

fn describe(dir: &Path, found: &Found, plan_file: &str) -> Prov {
    let b = &found.block;
    let status = signature_status(dir, b);
    let mut plan = b.field("plan").unwrap_or("").split_whitespace();
    let path = plan.next().unwrap_or("").to_string();
    let plan_sha = plan.next().unwrap_or("").to_string();
    let mut flags = Vec::new();
    let tree = b.field("tree").unwrap_or("");
    if plan_sha.is_empty() || plan_sha_at(dir, tree, &path).as_deref() != Some(plan_sha.as_str()) {
        flags.push("plan line does not match tree");
    }
    let main_sha = plan_sha_at(dir, "origin/main", &format!("docs/plans/{plan_file}"));
    if main_sha.as_deref() != Some(plan_sha.as_str()) {
        flags.push("reviewed an earlier body");
    }
    Prov {
        status,
        review: b
            .field("review")
            .and_then(|r| r.split_whitespace().next())
            .unwrap_or("?")
            .to_string(),
        preview: b.field("preview").unwrap_or("?").to_string(),
        plan_sha,
        flags,
    }
}

fn gates_of(dir: &Path, object: &str) -> Vec<String> {
    let Some(text) = notes_show(dir, ATTEST_REF, object) else {
        return Vec::new();
    };
    blocks(&text)
        .first()
        .and_then(|b| b.field("gates").map(str::to_string))
        .map(|g| g.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

fn rows_of(name: &str, dir: &Path, plan_file: &str) -> Vec<Row> {
    let path = format!("docs/plans/{plan_file}");
    let log = git::stdout_in(
        dir,
        &[
            "log",
            "--first-parent",
            "--format=%H%x1f%P%x1f%s",
            "origin/main",
            "--",
            &path,
        ],
    )
    .unwrap_or_default();
    // The URL git fetches from (an `insteadOf` alias resolved), else the
    // configured one: either may be the one that names GitHub.
    let slug = git::stdout_in(dir, &["remote", "get-url", "origin"])
        .as_deref()
        .and_then(github_slug)
        .or_else(|| {
            git::stdout_in(dir, &["config", "--get", "remote.origin.url"])
                .as_deref()
                .and_then(github_slug)
        });
    let mut rows = Vec::new();
    for line in log.lines() {
        let mut f = line.splitn(3, '\u{1f}');
        let (Some(merge), Some(parents), Some(subject)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let parents: Vec<&str> = parents.split_whitespace().collect();
        let pr = pr_number(subject);
        let head = match (parents.get(1), pr, &slug) {
            (Some(p), _, _) => Some(p.to_string()),
            (None, Some(n), Some(s)) => gh_head(s, n),
            _ => None,
        };
        let release = git::stdout_in(
            dir,
            &[
                "tag",
                "--list",
                "v*",
                "--contains",
                merge,
                "--sort=v:refname",
            ],
        )
        .and_then(|t| t.lines().next().map(str::to_string));
        let found = lookup(dir, merge, head.as_deref(), parents.first().copied());
        let gates = found
            .as_ref()
            .map(|f| gates_of(dir, &f.object))
            .unwrap_or_default();
        let prov = found.as_ref().map(|f| describe(dir, f, plan_file));
        rows.push(Row {
            repo: name.to_string(),
            pr,
            merge: merge.to_string(),
            head,
            release,
            prov,
            gates,
        });
    }
    rows
}

fn human(r: &Row) -> String {
    let mut s = format!(
        "{}  {}  {}  {}  ",
        r.repo,
        r.pr.map_or("-".to_string(), |n| format!("#{n}")),
        &r.merge[..r.merge.len().min(7)],
        r.release.as_deref().unwrap_or("not released"),
    );
    match &r.prov {
        None => s.push_str("no provenance"),
        Some(p) => {
            s.push_str(&format!(
                "review {} · preview {} · plan {}",
                p.review,
                p.preview,
                &p.plan_sha[..p.plan_sha.len().min(12)]
            ));
            if !r.gates.is_empty() {
                s.push_str(&format!(" · gates {}", r.gates.join(" ")));
            }
            s.push_str(&format!(" · {}", p.status));
            for f in &p.flags {
                s.push_str(&format!(" · {f}"));
            }
        }
    }
    s
}

fn to_json(r: &Row) -> serde_json::Value {
    serde_json::json!({
        "repo": r.repo,
        "pr": r.pr,
        "merge": r.merge,
        "head": r.head,
        "release": r.release,
        "provenance": r.prov.as_ref().map(|p| serde_json::json!({
            "status": p.status,
            "review": p.review,
            "preview": p.preview,
            "plan_body_sha": p.plan_sha,
            "flags": p.flags,
        })),
        "gates": r.gates,
    })
}

/// Payload files older than 30 days under the hook's provenance directory.
fn leftovers(repo: &str) -> usize {
    let Some(dir) = amont_agent_guard::journal::dir().map(|d| d.join("provenance").join(repo))
    else {
        return 0;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let limit = Duration::from_secs(STALE_DAYS * 86_400);
    entries
        .flatten()
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > limit)
        })
        .count()
}

pub fn run(args: &[OsString]) -> ExitCode {
    let args: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        let _ = write!(std::io::stdout(), "{USAGE}");
        return ExitCode::SUCCESS;
    }
    let mut json = false;
    let mut file = None;
    for a in &args {
        if a == "--json" {
            json = true;
        } else if a.starts_with('-') {
            eprintln!("amont-agent: unknown flag `{a}`; try `amont-agent shipped --help`");
            return ExitCode::from(2);
        } else if file.replace(a).is_some() {
            eprint!("amont-agent: shipped takes one plan file\n\n{USAGE}");
            return ExitCode::from(2);
        }
    }
    let Some(file) = file else {
        eprint!("amont-agent: shipped needs a plan file\n\n{USAGE}");
        return ExitCode::from(2);
    };
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("amont-agent: cannot read {file}: {e}");
            return ExitCode::from(2);
        }
    };
    let repos = match plan_repos(&text) {
        Ok(r) => r,
        Err(why) => {
            eprintln!("amont-agent: {why}");
            return ExitCode::from(2);
        }
    };
    let Some(plan_file) = Path::new(file)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
    else {
        eprintln!("amont-agent: {file} has no file name");
        return ExitCode::from(2);
    };
    let Some(root) = crate::plan_review::root() else {
        eprintln!(
            "amont-agent: no plan root to resolve repositories under; {}",
            crate::plan_review::ROOT_REMEDY
        );
        return ExitCode::from(1);
    };
    let (mut rows, mut read, mut lines, mut fetch_failed) = (Vec::new(), 0, Vec::new(), false);
    for name in &repos {
        let dir = crate::plan_review::locate(&root, name);
        if git::stdout_in(&dir, &["rev-parse", "--verify", "origin/main^{commit}"]).is_none() {
            eprintln!(
                "amont-agent: {name}: cannot read origin/main in {}",
                dir.display()
            );
            continue;
        }
        read += 1;
        fetch_failed |= !fetch_notes(&dir);
        let n = leftovers(name);
        if n > 0 {
            eprintln!(
                "amont-agent: {n} payload file(s) older than {STALE_DAYS} days under provenance/{name}/ (kept)"
            );
        }
        let has_plan = git::succeeds_in(
            &dir,
            &[
                "cat-file",
                "-e",
                &format!("origin/main:docs/plans/{plan_file}"),
            ],
        );
        if !has_plan {
            lines.push(format!("{name}  no plan file"));
            continue;
        }
        let found = rows_of(name, &dir, &plan_file);
        lines.extend(found.iter().map(human));
        rows.extend(found);
    }
    if fetch_failed {
        eprintln!("amont-agent: the provenance notes could not be fetched from origin; using what is local");
    }
    if read == 0 {
        eprintln!("amont-agent: no repository could be read");
        return ExitCode::from(1);
    }
    let mut out = std::io::stdout().lock();
    let written = if json {
        let all: Vec<serde_json::Value> = rows.iter().map(to_json).collect();
        writeln!(out, "{}", serde_json::Value::Array(all))
    } else {
        lines.iter().try_for_each(|l| writeln!(out, "{l}"))
    };
    match written {
        Ok(()) => ExitCode::SUCCESS,
        // A reader that stopped reading (`| head`) asked for no more.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        // Cut-off output must not pass for a complete answer.
        Err(e) => {
            eprintln!("amont-agent: could not write the answer: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pr_number;

    #[test]
    fn pr_number_is_the_last_marker_anywhere() {
        assert_eq!(pr_number("feat: a (#7)"), Some(7));
        assert_eq!(
            pr_number("Merge pull request 'feat: x' (#12) from feat/x into main"),
            Some(12)
        );
        assert_eq!(pr_number("fix (#3) again (#4)"), Some(4));
        assert_eq!(pr_number("no number (#x)"), None);
    }
}
