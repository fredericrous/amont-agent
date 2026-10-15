//! `amont-agent shipped <plan.md> [--json]`: what shipped under a plan,
//! derived from git (decisions ADR-0036). Read-only: the one thing it writes
//! is the two local note refs it fetches, and temporary files for the
//! signature check, which it removes.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use crate::git;

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
    let inner = subject.trim_end().strip_suffix(')')?;
    let at = inner.rfind("(#")?;
    inner[at + 2..].parse().ok()
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

fn reachable(dir: &Path, commit: &str) -> bool {
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
    let diff = git::bytes_in(
        dir,
        &["diff", "--binary", parent, merge, "--", ".", ":!docs/plans"],
        None,
    )
    .ok()??;
    let out = git::bytes_in(dir, &["patch-id", "--verbatim"], Some(&diff)).ok()??;
    String::from_utf8_lossy(&out)
        .split_whitespace()
        .next()
        .map(str::to_string)
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
            if block.is_record() && reachable(dir, block.field("commit").unwrap_or("")) {
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
                && reachable(dir, block.field("commit").unwrap_or(""))
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

/// Does the signature verify for any principal of the committed
/// allowed_signers, under the `amont-provenance` namespace?
fn signature_verifies(dir: &Path, block: &Block) -> bool {
    let signers = [".github/allowed_signers", ".forgejo/allowed_signers"]
        .iter()
        .find_map(|p| {
            git::bytes_in(dir, &["show", &format!("origin/main:{p}")], None)
                .ok()
                .flatten()
        });
    let (Some(signers), Some(tmp)) = (signers, scratch_dir()) else {
        return false;
    };
    let text = String::from_utf8_lossy(&signers).into_owned();
    let (sig_file, signers_file) = (tmp.join("sig"), tmp.join("allowed_signers"));
    let written = std::fs::write(&sig_file, &block.sig).is_ok()
        && std::fs::write(&signers_file, &signers).is_ok();
    let mut verified = false;
    if written {
        let principals = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .filter_map(|l| l.split_whitespace().next())
            .flat_map(|f| f.split(','))
            .map(|p| p.trim_matches('"').to_string());
        for who in principals {
            let child = Command::new("ssh-keygen")
                .args(["-Y", "verify", "-n", NAMESPACE, "-I", &who])
                .arg("-s")
                .arg(&sig_file)
                .arg("-f")
                .arg(&signers_file)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            let Ok(mut child) = child else { break };
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(block.payload.as_bytes());
            }
            if child.wait().is_ok_and(|s| s.success()) {
                verified = true;
                break;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    verified
}

fn plan_sha_at(dir: &Path, rev: &str, path: &str) -> Option<String> {
    let bytes = git::bytes_in(dir, &["show", &format!("{rev}:{path}")], None).ok()??;
    crate::plan_review::shas(&String::from_utf8_lossy(&bytes))
        .ok()
        .map(|s| s.sha)
}

fn describe(dir: &Path, found: &Found, plan_file: &str) -> Prov {
    let b = &found.block;
    let status = if signature_verifies(dir, b) {
        "verified"
    } else {
        "signed, signer not allowed"
    };
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
    let slug = git::stdout_in(dir, &["remote", "get-url", "origin"])
        .as_deref()
        .and_then(github_slug);
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
    let Some(dir) = crate::journal::dir().map(|d| d.join("provenance").join(repo)) else {
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
        print!("{USAGE}");
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
        eprintln!("amont-agent: no home directory to resolve repositories under");
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
    if json {
        let all: Vec<serde_json::Value> = rows.iter().map(to_json).collect();
        println!("{}", serde_json::Value::Array(all));
    } else {
        for l in lines {
            println!("{l}");
        }
    }
    ExitCode::SUCCESS
}
