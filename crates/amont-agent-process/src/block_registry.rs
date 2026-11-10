//! Storage for the reviewed-block registry (decisions ADR-0067): entries,
//! copies, the fingerprint-level `revoked` file and pin records under
//! `<base>/implementation-review/by-block/<fp>/`.
//!
//! Immutable files are written once: staged as `.tmp-<pid>-<name>`, then
//! hard-linked to their name, which fails atomically on an existing name, so
//! concurrent hooks lose no write. Readers skip every name starting with `.`
//! and read an unparseable file as absent, except `revoked`, whose presence
//! revokes whatever it holds. Storage only: no hook or CLI wiring.

use amont_agent_guard::atomic::write_atomic;
use amont_agent_guard::sha256::{hex, sha256};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const FINGERPRINT_LEN: usize = 64;
const PREFIX_LEN: usize = 12;
const REPO_KEY_LEN: usize = 16;
const REVOKED: &str = "revoked";

/// The registry root under a state directory (the journal directory in a
/// real run, a temp directory in tests).
pub fn store(base: &Path) -> PathBuf {
    base.join("implementation-review").join("by-block")
}

/// A rules sha and a digest of the profile agent files, both full hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub rules_sha: String,
    pub agent_digest: String,
}

impl Generation {
    fn dir_name(&self) -> io::Result<String> {
        let rules = hex_prefix(&self.rules_sha)?;
        let agents = hex_prefix(&self.agent_digest)?;
        Ok(format!("{rules}-{agents}"))
    }
}

/// What a registered block records besides its generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryData {
    pub roles: Vec<String>,
    pub source_branch: String,
    pub content_id: String,
}

/// A registered generation as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub generation: Generation,
    pub fingerprint: String,
    pub data: EntryData,
}

/// One recorded copy of a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copy {
    pub repo: String,
    pub commit: String,
    pub branch: String,
}

/// The latest pin check for a pushed tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub passed: bool,
    pub detail: String,
    pub gh_override_set: bool,
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what.to_string())
}

fn is_hex(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_fingerprint(name: &str) -> bool {
    name.len() == FINGERPRINT_LEN && is_hex(name)
}

fn hex_prefix(full: &str) -> io::Result<&str> {
    match full.get(..PREFIX_LEN) {
        Some(prefix) if is_hex(prefix) => Ok(prefix),
        _ => Err(invalid("expected at least 12 lowercase hex characters")),
    }
}

fn is_generation_dir(name: &str) -> bool {
    match name.split_once('-') {
        Some((rules, agents)) => {
            rules.len() == PREFIX_LEN
                && agents.len() == PREFIX_LEN
                && is_hex(rules)
                && is_hex(agents)
        }
        None => false,
    }
}

fn block_dir(base: &Path, fingerprint: &str) -> io::Result<PathBuf> {
    if !is_fingerprint(fingerprint) {
        return Err(invalid(
            "a fingerprint is exactly 64 lowercase hex characters",
        ));
    }
    Ok(store(base).join(fingerprint))
}

/// Write `body` once as `dir/name`. An existing name is success: the first
/// writer won and the file never changes.
fn write_once(dir: &Path, name: &str, body: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let staged = dir.join(format!(".tmp-{}-{name}", std::process::id()));
    let _ = fs::remove_file(&staged); // a killed run's leftover; absence is fine
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staged)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()
    })();
    let linked = written.and_then(|()| fs::hard_link(&staged, dir.join(name)));
    let _ = fs::remove_file(&staged); // litter whatever the link did; readers skip dot names
    match linked {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        other => other,
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn visible_names(dir: &Path) -> Vec<String> {
    let Ok(listing) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = listing
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// Register one generation of a block. A generation that already exists is
/// success.
pub fn register_entry(
    base: &Path,
    fingerprint: &str,
    generation: &Generation,
    data: &EntryData,
) -> io::Result<()> {
    let dir = block_dir(base, fingerprint)?.join(generation.dir_name()?);
    let body = json!({
        "fingerprint": fingerprint,
        "roles": data.roles,
        "rules_sha": generation.rules_sha,
        "agent_digest": generation.agent_digest,
        "source_branch": data.source_branch,
        "content_id": data.content_id,
    });
    write_once(&dir, "entry.json", &body.to_string())
}

fn parse_entry(value: &Value) -> Option<Entry> {
    let roles = value
        .get("roles")?
        .as_array()?
        .iter()
        .map(|role| role.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    Some(Entry {
        generation: Generation {
            rules_sha: text(value, "rules_sha")?,
            agent_digest: text(value, "agent_digest")?,
        },
        fingerprint: text(value, "fingerprint")?,
        data: EntryData {
            roles,
            source_branch: text(value, "source_branch")?,
            content_id: text(value, "content_id")?,
        },
    })
}

/// Every readable generation of a block, in directory-name order. A
/// generation whose stored fingerprint differs from the directory it sits in,
/// or whose name does not match its stored sha prefixes, is ignored.
pub fn entries(base: &Path, fingerprint: &str) -> Vec<Entry> {
    let Ok(dir) = block_dir(base, fingerprint) else {
        return Vec::new();
    };
    visible_names(&dir)
        .into_iter()
        .filter(|name| is_generation_dir(name))
        .filter_map(|name| {
            let entry = parse_entry(&read_json(&dir.join(&name).join("entry.json"))?)?;
            let named = entry.generation.dir_name().ok()?;
            (entry.fingerprint == fingerprint && named == name).then_some(entry)
        })
        .collect()
}

/// Every fingerprint directory in the registry (exactly 64 hex characters).
pub fn fingerprints(base: &Path) -> Vec<String> {
    visible_names(&store(base))
        .into_iter()
        .filter(|name| is_fingerprint(name))
        .collect()
}

/// Record one copy of a block, once per repository and commit.
pub fn record_copy(
    base: &Path,
    fingerprint: &str,
    repo: &str,
    commit: &str,
    branch: &str,
) -> io::Result<()> {
    if !is_hex(commit) {
        return Err(invalid("a commit is lowercase hex"));
    }
    let dir = block_dir(base, fingerprint)?.join("copies");
    let repo_key = &hex(&sha256(repo.as_bytes()))[..REPO_KEY_LEN];
    let body = json!({ "repo": repo, "commit": commit, "branch": branch });
    write_once(
        &dir,
        &format!("{repo_key}-{commit}.json"),
        &body.to_string(),
    )
}

/// Every readable copy of a block.
pub fn copies(base: &Path, fingerprint: &str) -> Vec<Copy> {
    let Ok(dir) = block_dir(base, fingerprint) else {
        return Vec::new();
    };
    let dir = dir.join("copies");
    visible_names(&dir)
        .into_iter()
        .filter_map(|name| {
            let value = read_json(&dir.join(name))?;
            Some(Copy {
                repo: text(&value, "repo")?,
                commit: text(&value, "commit")?,
                branch: text(&value, "branch")?,
            })
        })
        .collect()
}

/// Revoke a fingerprint, whatever generation exists or is created later. The
/// first reason stands; revoking again is success.
pub fn revoke(base: &Path, fingerprint: &str, because: &str) -> io::Result<()> {
    let dir = block_dir(base, fingerprint)?;
    write_once(&dir, REVOKED, &json!({ "because": because }).to_string())
}

/// Lift a revocation. Removing an absent `revoked` file is success.
pub fn reinstate(base: &Path, fingerprint: &str) -> io::Result<()> {
    match fs::remove_file(block_dir(base, fingerprint)?.join(REVOKED)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// `Some(reason)` when the fingerprint is revoked. The file's presence
/// revokes even when it cannot be parsed; the reason is then empty.
pub fn revoked(base: &Path, fingerprint: &str) -> Option<String> {
    let path = block_dir(base, fingerprint).ok()?.join(REVOKED);
    if !path.exists() {
        return None;
    }
    Some(
        read_json(&path)
            .and_then(|value| text(&value, "because"))
            .unwrap_or_default(),
    )
}

fn pin_path(base: &Path, fingerprint: &str, content_id: &str) -> io::Result<PathBuf> {
    if !is_hex(content_id) {
        return Err(invalid("a content id is lowercase hex"));
    }
    Ok(block_dir(base, fingerprint)?
        .join("pins")
        .join(format!("{content_id}.json")))
}

/// Replace the latest pin check for a tree atomically: the last write wins.
/// `gh_override_set` notes whether `AMONT_AGENT_GH` was set for the check.
pub fn write_pin(base: &Path, fingerprint: &str, content_id: &str, pin: &Pin) -> io::Result<()> {
    let path = pin_path(base, fingerprint, content_id)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let body = json!({
        "passed": pin.passed,
        "detail": pin.detail,
        "gh_override_set": pin.gh_override_set,
    });
    write_atomic(&path, &body.to_string())
}

/// The latest pin check for a tree; an unreadable record reads as absent.
pub fn read_pin(base: &Path, fingerprint: &str, content_id: &str) -> Option<Pin> {
    let value = read_json(&pin_path(base, fingerprint, content_id).ok()?)?;
    Some(Pin {
        passed: value.get("passed")?.as_bool()?,
        detail: text(&value, "detail")?,
        gh_override_set: value.get("gh_override_set")?.as_bool()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "amont-block-registry-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir); // a leftover from an earlier run; absence is fine
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn fp() -> String {
        "ab".repeat(32)
    }

    fn generation(n: u32) -> Generation {
        Generation {
            rules_sha: format!("{n:012x}{}", "1".repeat(52)),
            agent_digest: format!("{n:012x}{}", "2".repeat(52)),
        }
    }

    fn generation_dir(base: &Path, fingerprint: &str, n: u32) -> PathBuf {
        block_dir(base, fingerprint)
            .expect("block dir")
            .join(generation(n).dir_name().expect("generation name"))
    }

    fn data() -> EntryData {
        EntryData {
            roles: vec!["backend".into(), "lang:rust".into()],
            source_branch: "pilot".into(),
            content_id: "c0ffee".into(),
        }
    }

    #[test]
    fn register_twice_is_success() {
        let base = scratch("twice");
        register_entry(&base, &fp(), &generation(1), &data()).expect("first");
        register_entry(&base, &fp(), &generation(1), &data()).expect("second");
        let found = entries(&base, &fp());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].data, data());
        assert_eq!(found[0].generation, generation(1));
    }

    #[test]
    fn sixteen_parallel_writers_lose_nothing() {
        let base = Arc::new(scratch("parallel"));
        let handles: Vec<_> = (0..16u32)
            .map(|n| {
                let base = Arc::clone(&base);
                thread::spawn(move || {
                    register_entry(&base, &fp(), &generation(n), &data()).expect("register");
                    record_copy(&base, &fp(), &format!("repo{n}"), "abc123", "b").expect("copy");
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread");
        }
        assert_eq!(entries(&base, &fp()).len(), 16);
        assert_eq!(copies(&base, &fp()).len(), 16);
    }

    #[test]
    fn revoke_twice_succeeds_and_keeps_first_reason() {
        let base = scratch("revoke-twice");
        revoke(&base, &fp(), "first").expect("revoke");
        revoke(&base, &fp(), "second").expect("revoke again");
        assert_eq!(revoked(&base, &fp()).as_deref(), Some("first"));
        reinstate(&base, &fp()).expect("reinstate");
        assert_eq!(revoked(&base, &fp()), None);
        reinstate(&base, &fp()).expect("reinstate again");
    }

    #[test]
    fn revoke_in_the_middle_of_registration_stays_revoked() {
        let base = Arc::new(scratch("revoke-race"));
        let writers: Vec<_> = (0..16u32)
            .map(|n| {
                let base = Arc::clone(&base);
                thread::spawn(move || {
                    register_entry(&base, &fp(), &generation(n), &data()).expect("register");
                })
            })
            .collect();
        revoke(&base, &fp(), "defect").expect("revoke");
        for handle in writers {
            handle.join().expect("thread");
        }
        assert_eq!(revoked(&base, &fp()).as_deref(), Some("defect"));
    }

    #[test]
    fn unparseable_revoked_still_revokes() {
        let base = scratch("garbled");
        let dir = block_dir(&base, &fp()).expect("dir");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join(REVOKED), "{not json").expect("write");
        assert_eq!(revoked(&base, &fp()), Some(String::new()));
    }

    #[test]
    fn mismatched_entry_is_ignored() {
        let base = scratch("mismatch");
        let other = "cd".repeat(32);
        register_entry(&base, &other, &generation(1), &data()).expect("register");
        let into = generation_dir(&base, &fp(), 1);
        fs::create_dir_all(&into).expect("mkdir");
        fs::copy(
            generation_dir(&base, &other, 1).join("entry.json"),
            into.join("entry.json"),
        )
        .expect("copy");
        assert!(entries(&base, &fp()).is_empty());
        assert_eq!(entries(&base, &other).len(), 1);
    }

    #[test]
    fn unparseable_entry_reads_as_absent() {
        let base = scratch("garbled-entry");
        let dir = generation_dir(&base, &fp(), 1);
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join("entry.json"), "nope").expect("write");
        assert!(entries(&base, &fp()).is_empty());
    }

    #[test]
    fn dot_prefixed_names_are_skipped() {
        let base = scratch("dots");
        register_entry(&base, &fp(), &generation(1), &data()).expect("register");
        record_copy(&base, &fp(), "repo", "abc", "b").expect("copy");
        let hidden_generation = block_dir(&base, &fp()).expect("dir").join(format!(
            ".{}",
            generation(2).dir_name().expect("generation name")
        ));
        fs::create_dir_all(&hidden_generation).expect("mkdir");
        fs::copy(
            generation_dir(&base, &fp(), 1).join("entry.json"),
            hidden_generation.join("entry.json"),
        )
        .expect("copy");
        fs::write(
            block_dir(&base, &fp())
                .expect("dir")
                .join("copies")
                .join(".tmp-1-x.json"),
            json!({"repo": "r", "commit": "c", "branch": "b"}).to_string(),
        )
        .expect("write");
        fs::create_dir_all(store(&base).join(format!(".{}", fp()))).expect("mkdir");
        assert_eq!(entries(&base, &fp()).len(), 1);
        assert_eq!(copies(&base, &fp()).len(), 1);
        assert_eq!(fingerprints(&base), vec![fp()]);
    }

    #[test]
    fn staged_names_are_removed_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let base = scratch("mode");
        register_entry(&base, &fp(), &generation(1), &data()).expect("register");
        let dir = generation_dir(&base, &fp(), 1);
        let names: Vec<_> = fs::read_dir(&dir)
            .expect("dir")
            .map(|listed| listed.expect("listed").file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("entry.json")]);
        let mode = fs::metadata(dir.join("entry.json"))
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn bad_fingerprint_is_refused() {
        let base = scratch("bad-fp");
        assert!(register_entry(&base, "../x", &generation(1), &data()).is_err());
        assert!(entries(&base, "short").is_empty());
    }

    #[test]
    fn copy_file_is_named_by_repo_hash_and_commit() {
        let base = scratch("copy-name");
        record_copy(&base, &fp(), "repo", "abc123", "b").expect("copy");
        // sha256("repo") from `printf repo | shasum -a 256`
        let expected = "071ca22277547058-abc123.json";
        let names = visible_names(&block_dir(&base, &fp()).expect("dir").join("copies"));
        assert_eq!(names, vec![expected.to_string()]);
    }

    #[test]
    fn pin_replace_keeps_the_last_write() {
        let base = scratch("pin");
        let failed = Pin {
            passed: false,
            detail: "asset missing".into(),
            gh_override_set: true,
        };
        let passed = Pin {
            passed: true,
            detail: "ok".into(),
            gh_override_set: false,
        };
        assert_eq!(read_pin(&base, &fp(), "beef"), None);
        write_pin(&base, &fp(), "beef", &passed).expect("first");
        write_pin(&base, &fp(), "beef", &failed).expect("second");
        assert_eq!(read_pin(&base, &fp(), "beef"), Some(failed));
    }
}
