//! What a push or a land did, recorded before it runs and read back after:
//! the guard's assertions (`push-published`, `land-retry`) and the preview
//! rule both read these records, so they belong to neither. Which records to
//! write, and what `ui` and `approved` say, is the preview rule's policy
//! (`preview::record_before`); this module owns the format, the store and
//! its upkeep. The store keeps its historical place, `previews/pushes` and
//! `previews/lands` under the journal's directory.

use std::path::{Path, PathBuf};

use crate::journal;

/// The directory both records live under (and the preview rule's own
/// stores, beside them).
pub fn dir() -> Option<PathBuf> {
    Some(journal::dir()?.join("previews"))
}

/// Create `dir`, private to this user.
pub fn ensure(dir: &Path) -> Option<()> {
    std::fs::create_dir_all(dir).ok()?;
    journal::private(dir, 0o700);
    Some(())
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// Record, for this call, where each branch destination stood before the
/// push. Nothing is written for an empty list or an id that names no single
/// file.
pub fn write_before(tool_use_id: &str, rows: &[Before]) {
    if rows.is_empty() || tool_use_id.is_empty() || tool_use_id.contains('/') {
        return;
    }
    let body: String = rows
        .iter()
        .map(|r| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                clean(&r.repo.to_string_lossy()),
                clean(&r.remote),
                clean(&r.dst),
                r.before.as_deref().unwrap_or("-"),
                r.src,
                r.ui,
                r.approved
            )
        })
        .collect();
    if let Some(d) = dir().map(|d| d.join("pushes")) {
        if ensure(&d).is_some() {
            let _ = crate::atomic::write_atomic(&d.join(tool_use_id), &body); // best-effort: no record means push-published stays silent
        }
    }
}

/// What PreToolUse judged for one land: the sha, and where its receipt will
/// be written. Nanoseconds since the epoch, compared with the receipt's own
/// `started`, which comes later.
pub struct LandJudged {
    pub judged: String,
    pub receipt_dir: PathBuf,
    pub branch: String,
    pub since: i128,
}

/// How long a land's judgement record is kept: the check reads it moments
/// later, and a week covers a session left open.
const LAND_RECORD_KEEP: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

pub fn record_land(tool_use_id: &str, judged: &str, l: &crate::land::Landing) {
    // The same rule `land_judged` reads with: an id that names no single file
    // in `lands/` would be written where nothing reads it.
    if tool_use_id.is_empty() || tool_use_id.contains('/') {
        return;
    }
    // No clock, no record: a `since` of 0 would match every older receipt of
    // the branch, and the check would then judge the wrong land.
    let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        eprintln!("amont-agent: land-retry: the clock is before 1970, so this land's judgement is not recorded");
        return;
    };
    let since = now.as_nanos() as i128;
    let body = format!(
        "{}\t{}\t{}\t{since}\n",
        clean(judged),
        clean(&l.receipt_dir.to_string_lossy()),
        clean(&l.branch)
    );
    let Some(d) = dir().map(|d| d.join("lands")) else {
        eprintln!(
            "amont-agent: land-retry: no state directory, so this land's judgement is not recorded"
        );
        return;
    };
    if ensure(&d).is_none() {
        eprintln!("amont-agent: land-retry: could not create {}", d.display());
        return;
    }
    prune_land_records(&d);
    let path = d.join(tool_use_id);
    if let Err(e) = crate::atomic::write_atomic(&path, &body) {
        eprintln!(
            "amont-agent: land-retry: could not write {}: {e}",
            path.display()
        );
    }
}

/// Deletes judgement records older than [`LAND_RECORD_KEEP`].
fn prune_land_records(dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "amont-agent: land-retry: could not list {}: {e}",
                dir.display()
            );
            return;
        }
    };
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > LAND_RECORD_KEEP);
        if old {
            if let Err(err) = std::fs::remove_file(e.path()) {
                eprintln!(
                    "amont-agent: land-retry: could not delete the old record {}: {err}",
                    e.path().display()
                );
            }
        }
    }
}

/// The land PreToolUse judged for this call, if it recorded one.
pub fn land_judged(tool_use_id: &str) -> Option<LandJudged> {
    if tool_use_id.is_empty() || tool_use_id.contains('/') {
        return None;
    }
    let text = std::fs::read_to_string(dir()?.join("lands").join(tool_use_id)).ok()?;
    let mut f = text.trim_end().split('\t');
    Some(LandJudged {
        judged: f.next()?.to_string(),
        receipt_dir: PathBuf::from(f.next()?),
        branch: f.next()?.to_string(),
        since: f.next()?.parse().ok()?,
    })
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

/// Take (read and remove) what [`write_before`] wrote for this call.
pub fn take_before(tool_use_id: &str) -> Vec<Before> {
    if tool_use_id.is_empty() || tool_use_id.contains('/') {
        return Vec::new();
    }
    let Some(path) = dir().map(|d| d.join("pushes").join(tool_use_id)) else {
        return Vec::new();
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path); // best-effort: a record left behind is swept with the scratch
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
