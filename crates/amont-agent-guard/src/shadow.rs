//! The shadow switch of ADR-0042 (`harness.retire-on-evidence`).
//!
//! A part (a rule or an assertion) can be dropped to `observe` for a hashed
//! sample of sessions, so its journal rows show what it would have caught on
//! the current model. The rate is per part:
//!
//! ```text
//! git config --global amont.agent.<id>.shadow <rate>     # a number in [0,1]
//! ```
//!
//! Absent or `0` is off. An unparseable value is off too, and `doctor`
//! reports it. A part with `irreversible = true` ignores any rate: its failure
//! cannot be undone, so it is judged by replay, never switched off live.
//!
//! The draw is the design of relais `crates/relais/src/rng.rs`
//! `exploration_draw` (decisions ADR-0039, `harness.shared-code`): a SHA-256
//! of a machine seed and the keys, first eight bytes read big-endian as a
//! `u64`, divided by 2^64. The seed is created once per machine; the draw is
//! the same for a given (part, session) every time, so a session stays in one
//! arm for its whole life.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use crate::sha256::sha256;

const SEED_FILE: &str = "shadow-seed";
const SEED_LEN: usize = 32;

/// A part's configured shadow rate.
#[derive(Debug, Clone, PartialEq)]
pub enum Rate {
    Off,
    On(f64),
    /// Configured, but not a number in [0,1]. Behaves as off.
    Bad(String),
}

/// `raw` is whatever git printed for the key.
pub fn parse_rate(raw: &str) -> Rate {
    match raw.trim().parse::<f64>() {
        Ok(r) if r.is_finite() && (0.0..=1.0).contains(&r) => {
            if r == 0.0 {
                Rate::Off
            } else {
                Rate::On(r)
            }
        }
        _ => Rate::Bad(raw.trim().to_string()),
    }
}

pub fn key_for(id: &str) -> String {
    format!("amont.agent.{id}.shadow")
}

/// Every configured `amont.agent.<id>.shadow`, keyed by id. System first, then
/// global, so the user's own file wins. The repository's config is never read,
/// for the reason `gitconfig` gives. One process per scope, once per run.
pub fn configured() -> &'static BTreeMap<String, String> {
    static ALL: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut out = BTreeMap::new();
        for scope in ["--system", "--global"] {
            let Ok(o) = Command::new("git")
                .args([
                    "config",
                    scope,
                    "--get-regexp",
                    r"^amont\.agent\..*\.shadow$",
                ])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
            else {
                continue;
            };
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                let (key, value) = line.split_once(' ').unwrap_or((line, ""));
                if let Some(id) = key
                    .strip_prefix("amont.agent.")
                    .and_then(|k| k.strip_suffix(".shadow"))
                {
                    out.insert(id.to_string(), value.to_string());
                }
            }
        }
        out
    })
}

/// The configured rate for one part.
pub fn rate_for(id: &str) -> Rate {
    configured()
        .get(id)
        .map(|v| parse_rate(v))
        .unwrap_or(Rate::Off)
}

/// The unparseable settings, as (id, value), for `doctor`.
pub fn bad_settings() -> Vec<(String, String)> {
    configured()
        .iter()
        .filter(|(_, v)| matches!(parse_rate(v), Rate::Bad(_)))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// The machine seed at `<dir>/shadow-seed`. Created once, exclusively, at mode
/// 0600; if it exists, read back. Any other failure is `None`: no shadow.
pub fn seed_in(dir: &Path) -> Option<[u8; SEED_LEN]> {
    let path = dir.join(SEED_FILE);
    std::fs::create_dir_all(dir).ok()?;
    match create_new(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = std::fs::read(&path).ok()?;
            <[u8; SEED_LEN]>::try_from(bytes.as_slice()).ok()
        }
        Err(_) => None,
    }
}

fn create_new(path: &Path) -> std::io::Result<[u8; SEED_LEN]> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    // Entropy first, so a failure to get it leaves no empty seed file behind.
    let mut bytes = [0u8; SEED_LEN];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut f = opts.open(path)?;
    f.write_all(&bytes)?;
    Ok(bytes)
}

/// Where in [0,1) this (part, session) falls. Stable for the pair.
pub fn draw(seed: &[u8], id: &str, session: &str) -> f64 {
    let mut input = Vec::with_capacity(seed.len() + id.len() + session.len());
    input.extend_from_slice(seed);
    input.extend_from_slice(id.as_bytes());
    input.extend_from_slice(session.as_bytes());
    let h = sha256(&input);
    let mut first = [0u8; 8];
    first.copy_from_slice(&h[..8]);
    u64::from_be_bytes(first) as f64 / 18_446_744_073_709_551_616.0
}

/// The pure decision: in the shadow arm at `rate`? An irreversible part never
/// is, whatever the rate.
pub fn in_arm(rate: &Rate, irreversible: bool, seed: &[u8], id: &str, session: &str) -> bool {
    if irreversible {
        return false;
    }
    match rate {
        Rate::On(r) => draw(seed, id, session) < *r,
        Rate::Off | Rate::Bad(_) => false,
    }
}

static SESSION: OnceLock<String> = OnceLock::new();

/// The hook is one process per call; it names the session once, up front, so
/// every stance lookup below it can see which session it answers for.
pub fn enter_session(session: &str) {
    let _ = SESSION.set(session.to_string());
}

fn marks() -> &'static Mutex<BTreeSet<String>> {
    static M: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// Whether this part is in the shadow arm for the current session, and if so
/// remember it so the journal row says `arm=shadow`. False with no session.
pub fn drops(id: &str, irreversible: bool) -> bool {
    if irreversible {
        return false;
    }
    let Some(session) = SESSION.get().filter(|s| !s.is_empty()) else {
        return false;
    };
    let rate = rate_for(id);
    if rate == Rate::Off || matches!(rate, Rate::Bad(_)) {
        return false;
    }
    let Some(seed) = crate::journal::dir().and_then(|d| seed_in(&d)) else {
        return false;
    };
    let shadow = in_arm(&rate, irreversible, &seed, id, session);
    if shadow {
        if let Ok(mut m) = marks().lock() {
            m.insert(id.to_string());
        }
    }
    shadow
}

/// Whether a stance lookup in this process put `id` in the shadow arm.
pub fn is_marked(id: &str) -> bool {
    marks().lock().map(|m| m.contains(id)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_parse_and_bad_ones_are_off() {
        assert_eq!(parse_rate("0.1"), Rate::On(0.1));
        assert_eq!(parse_rate("0"), Rate::Off);
        assert_eq!(parse_rate("1"), Rate::On(1.0));
        assert!(matches!(parse_rate("1.5"), Rate::Bad(_)));
        assert!(matches!(parse_rate("-0.1"), Rate::Bad(_)));
        assert!(matches!(parse_rate("often"), Rate::Bad(_)));
        assert!(matches!(parse_rate("NaN"), Rate::Bad(_)));
    }

    #[test]
    fn the_draw_is_stable_for_a_session() {
        let seed = [7u8; 32];
        let a = draw(&seed, "pipe-to-tail", "sess-1");
        assert_eq!(a, draw(&seed, "pipe-to-tail", "sess-1"));
        assert_ne!(a, draw(&seed, "pipe-to-tail", "sess-2"));
        assert_ne!(a, draw(&seed, "other", "sess-1"));
        assert!((0.0..1.0).contains(&a));
    }

    #[test]
    fn the_shadow_share_is_near_the_rate() {
        let seed = [42u8; 32];
        let rate = Rate::On(0.1);
        let n = 10_000;
        let hits = (0..n)
            .filter(|i| in_arm(&rate, false, &seed, "pipe-to-tail", &format!("session-{i}")))
            .count();
        let share = hits as f64 / n as f64;
        assert!((0.09..=0.11).contains(&share), "share {share}");
    }

    #[test]
    fn an_irreversible_part_ignores_any_rate() {
        let seed = [1u8; 32];
        for i in 0..100 {
            assert!(!in_arm(&Rate::On(1.0), true, &seed, "p", &format!("s{i}")));
            assert!(in_arm(&Rate::On(1.0), false, &seed, "p", &format!("s{i}")));
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_seed_is_created_once_at_0600_and_read_back() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("amont-agent-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = seed_in(&dir).expect("created");
        let second = seed_in(&dir).expect("read back");
        assert_eq!(first, second);
        let mode = std::fs::metadata(dir.join(SEED_FILE))
            .expect("seed file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
