//! The harness review (ADR-0042, `harness.revisit-on-a-new-model` and
//! `harness.retire-on-evidence`): one row per rule and assertion, so a part is
//! kept or retired on what it caught on the current model.
//!
//! Three sources, none of them new. The journal says how often a part fired
//! and when (live rows only; a shadow row is the experiment, not the part).
//! The compliance replay says what the model did NEXT, per model. And for a
//! part dropped to observe in a hashed sample of sessions, the journal's `arm`
//! field is joined to the transcript's session id through the journal's
//! eight-character session prefix, because a transcript does not record
//! whether the hook spoke.
//!
//! A part guarding an irreversible action is never switched off live, so it
//! has no shadow arm to compare. It is judged by replaying: how many of the
//! newest model's Bash calls its `examine` matches.
//!
//! `per_1000` is live journal rows over the transcripts' Bash calls, times
//! 1000. The journal keeps no call counter, so the transcripts are the
//! denominator.
//! holds-until: the journal and the transcripts cover different spans, which
//! skews the rate; ends when the journal records the calls it saw.
//!
//! "Newest model" is the one first seen on the latest day in the transcripts'
//! Bash calls, ties broken by last seen and then by name; empty ids and
//! synthetic turns (`<synthetic>`) are not models.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::backtest::Subject as Part;
use crate::civil::{iso, Day};
use crate::compliance::{self, Cell};
use crate::journal::{self, Arm};
use crate::json;
use crate::rules::{Rule, Stance};
use crate::transcript::{self, Scan, ScanError};

/// Compliance of one part on one model.
pub struct ModelShare {
    pub model: String,
    pub firings: u32,
    pub answered: u32,
    pub complied: u32,
}

/// The `ignored` share per arm, for a part that has shadow rows.
pub struct ArmShare {
    pub live: Cell,
    pub shadow: Cell,
}

pub struct Row {
    pub id: &'static str,
    pub kind: &'static str,
    pub prevents: &'static str,
    pub stance: &'static str,
    pub irreversible: bool,
    pub live_rows: u32,
    pub per_1000: f64,
    pub last_fired: Option<String>,
    pub compliance: Vec<ModelShare>,
    pub arms: Option<ArmShare>,
    /// Newest model's Bash calls the part's `examine` matches. Irreversible
    /// parts only.
    pub replay: Option<u32>,
}

pub struct Review {
    pub date: String,
    pub newest_model: Option<String>,
    pub calls: u64,
    pub rows: Vec<Row>,
}

struct ModelSeen {
    first: Day,
    last: Day,
    /// Matches per replayed part index.
    matches: Vec<u32>,
}

struct Survey {
    calls: u64,
    models: BTreeMap<String, ModelSeen>,
}

impl Survey {
    fn newest(&self) -> Option<&str> {
        self.models
            .iter()
            .max_by_key(|(name, m)| (m.first, m.last, std::cmp::Reverse((*name).clone())))
            .map(|(name, _)| name.as_str())
    }
}

/// One pass over the transcripts: which models, and for each of `replayed` how
/// many Bash calls of each model it matches. Lexing is skipped when nothing is
/// replayed.
fn survey(scan: &Scan, replayed: &[Part]) -> Result<Survey, ScanError> {
    let mut models: BTreeMap<String, ModelSeen> = BTreeMap::new();
    let stats = transcript::for_each_transcript(scan, |records| {
        for r in records {
            if r.tool != scan.tool || r.model.is_empty() || r.model.starts_with('<') {
                continue;
            }
            let seen = models.entry(r.model.clone()).or_insert_with(|| ModelSeen {
                first: r.day,
                last: r.day,
                matches: vec![0; replayed.len()],
            });
            seen.first = seen.first.min(r.day);
            seen.last = seen.last.max(r.day);
            if r.command.is_empty() {
                continue;
            }
            for (i, part) in replayed.iter().enumerate() {
                if part.fires_on(&r.command) {
                    seen.matches[i] += 1;
                }
            }
        }
    })?;
    Ok(Survey {
        calls: stats.calls,
        models,
    })
}

fn parts(rules: &[&'static Rule]) -> Vec<Part> {
    rules
        .iter()
        .map(|r| Part::Rule(r))
        .chain(crate::assertions::ASSERTIONS.iter().map(Part::Assertion))
        .collect()
}

fn describe(part: &Part, stance: Stance) -> (&'static str, &'static str, &'static str, bool) {
    match part {
        Part::Rule(r) => ("rule", r.prevents, stance.as_str(), r.irreversible),
        Part::Assertion(a) => ("assertion", a.prevents, stance.as_str(), a.irreversible),
    }
}

fn date_of(ts: u64) -> String {
    iso((ts / 86_400) as Day)
}

/// Build the review. `journal_paths` are the journal generations, oldest
/// first; `today` is the UTC day the review is dated. `stance_of` is the
/// stance a rule or assertion runs at (`stance::resolve` and
/// `stance::resolve_assertion` in the binary), asked once per part. A `deny`
/// rule has no live compliance, its refused command never having run; when it
/// has shadow rows it is replayed as `observe` and gets a shadow-arm share only.
pub fn review(
    scan: &Scan,
    rules: &[&'static Rule],
    stance_of: fn(&Part) -> Stance,
    journal_paths: &[PathBuf],
    today: Day,
) -> Result<Review, ScanError> {
    let all: Vec<(Part, Stance)> = parts(rules)
        .into_iter()
        .map(|p| {
            let stance = stance_of(&p);
            (p, stance)
        })
        .collect();
    let irreversible: Vec<Part> = all
        .iter()
        .filter(|(p, s)| describe(p, *s).3)
        .map(|(p, _)| *p)
        .collect();
    let surveyed = survey(scan, &irreversible)?;
    let newest = surveyed.newest().map(str::to_string);

    let by_rule: BTreeMap<&str, Stance> = all
        .iter()
        .filter(|(p, _)| matches!(p, Part::Rule(_)))
        .map(|(p, s)| (p.id(), *s))
        .collect();
    let rows_seen = journal::rows_in(journal_paths);
    // A deny rule with shadow rows is replayed as observe, so its shadow arm
    // has an ignored share; its live arm stays empty (see below).
    let shadowed: std::collections::BTreeSet<&str> = rows_seen
        .iter()
        .filter(|r| r.arm == Arm::Shadow)
        .map(|r| r.rule.as_str())
        .collect();
    let compliant =
        compliance::run_with(
            scan,
            rules,
            compliance::Options::default(),
            |r| match by_rule.get(r.id).copied().unwrap_or(r.default_stance) {
                Stance::Deny if shadowed.contains(r.id) => Stance::Observe,
                other => other,
            },
        )?;
    // The transcripts' calls are counted from `--since`, so the journal rows
    // over them are too.
    let floor = scan
        .since
        .map_or(0, |d| u64::try_from(d).unwrap_or(0) * 86_400);

    let mut rows = Vec::new();
    for (part, part_stance) in &all {
        let id = part.id();
        let (kind, prevents, stance, is_irreversible) = describe(part, *part_stance);
        let mine: Vec<&journal::Row> = rows_seen.iter().filter(|r| r.rule == id).collect();
        let live: Vec<&&journal::Row> = mine
            .iter()
            .filter(|r| r.arm == Arm::Live && r.ts >= floor)
            .collect();
        let live_rows = live.len() as u32;
        let per_1000 = if surveyed.calls == 0 {
            0.0
        } else {
            1000.0 * f64::from(live_rows) / surveyed.calls as f64
        };
        let last_fired = live.iter().map(|r| r.ts).max().map(date_of);

        let subject = compliant.subjects.iter().find(|s| s.id == id);
        let mut per_model: BTreeMap<String, Cell> = BTreeMap::new();
        if let Some(s) = subject {
            for ((model, _), c) in &s.cells {
                per_model.entry(model.clone()).or_default().add(c);
            }
        }
        // A deny rule refuses on the live arm: there is no live compliance.
        let denied = *part_stance == Stance::Deny;
        let compliance = per_model
            .into_iter()
            .filter(|_| !denied)
            .map(|(model, c)| ModelShare {
                model,
                firings: c.firings,
                answered: c.answered(),
                complied: c.complied,
            })
            .collect();

        let arms = if mine.iter().any(|r| r.arm == Arm::Shadow) {
            let mut arm_of: BTreeMap<&str, Arm> = BTreeMap::new();
            for r in &mine {
                arm_of.entry(r.session.as_str()).or_insert(r.arm);
            }
            let mut share = ArmShare {
                live: Cell::default(),
                shadow: Cell::default(),
            };
            if let Some(s) = subject {
                for (session, cell) in &s.by_session {
                    let prefix: String = session.chars().take(8).collect();
                    match arm_of.get(prefix.as_str()) {
                        Some(Arm::Live) if !denied => share.live.add(cell),
                        Some(Arm::Live) => {}
                        Some(Arm::Shadow) => share.shadow.add(cell),
                        None => {}
                    }
                }
            }
            Some(share)
        } else {
            None
        };

        let replay = if is_irreversible {
            let at = irreversible.iter().position(|p| p.id() == id);
            Some(match (newest.as_ref(), at) {
                (Some(m), Some(i)) => surveyed.models.get(m).map_or(0, |s| s.matches[i]),
                _ => 0,
            })
        } else {
            None
        };

        rows.push(Row {
            id,
            kind,
            prevents,
            stance,
            irreversible: is_irreversible,
            live_rows,
            per_1000,
            last_fired,
            compliance,
            arms,
            replay,
        });
    }
    Ok(Review {
        date: iso(today),
        newest_model: newest,
        calls: surveyed.calls,
        rows,
    })
}

fn share(num: u32, den: u32) -> String {
    if den == 0 {
        "null".to_string()
    } else {
        format!("{:.3}", f64::from(num) / f64::from(den))
    }
}

fn opt_string(key: &str, v: Option<&str>) -> String {
    match v {
        Some(v) => json::string_field(key, v),
        None => format!("\"{}\":null", json::escape(key)),
    }
}

fn cell_json(c: &Cell) -> String {
    json::object(&[
        json::int_field("firings", i64::from(c.firings)),
        json::int_field("answered", i64::from(c.answered())),
        json::int_field("ignored", i64::from(c.ignored)),
        format!("\"ignored_share\":{}", share(c.ignored, c.answered())),
    ])
}

impl Row {
    fn to_json(&self) -> String {
        let compliance: Vec<String> = self
            .compliance
            .iter()
            .map(|m| {
                json::object(&[
                    json::string_field("model", &m.model),
                    json::int_field("firings", i64::from(m.firings)),
                    json::int_field("answered", i64::from(m.answered)),
                    format!("\"complied_share\":{}", share(m.complied, m.answered)),
                ])
            })
            .collect();
        let arms = match &self.arms {
            Some(a) => json::object(&[
                format!("\"live\":{}", cell_json(&a.live)),
                format!("\"shadow\":{}", cell_json(&a.shadow)),
            ]),
            None => "null".to_string(),
        };
        let replay = match self.replay {
            Some(n) => n.to_string(),
            None => "null".to_string(),
        };
        json::object(&[
            json::string_field("id", self.id),
            json::string_field("kind", self.kind),
            json::string_field("prevents", self.prevents),
            json::string_field("stance", self.stance),
            json::bool_field("irreversible", self.irreversible),
            json::int_field("live_rows", i64::from(self.live_rows)),
            format!("\"per_1000\":{:.2}", self.per_1000),
            opt_string("last_fired", self.last_fired.as_deref()),
            format!("\"compliance\":{}", json::array(&compliance)),
            format!("\"arms\":{arms}"),
            format!("\"replay\":{replay}"),
        ])
    }
}

impl Review {
    pub fn to_json(&self) -> String {
        let rows: Vec<String> = self.rows.iter().map(Row::to_json).collect();
        json::object(&[
            json::string_field("date", &self.date),
            opt_string("newest_model", self.newest_model.as_deref()),
            json::int_field("calls", self.calls as i64),
            format!("\"rows\":{}", json::array(&rows)),
        ])
    }

    pub fn render(&self) -> String {
        let pct = |c: &Cell| match c.ignored_share() {
            Some(p) => format!("{:.0}%", p * 100.0),
            None => "-".to_string(),
        };
        let mut s = format!(
            "harness review {} - newest model: {} ({} Bash calls)\n\n",
            self.date,
            self.newest_model.as_deref().unwrap_or("(none seen)"),
            self.calls
        );
        for r in &self.rows {
            s.push_str(&format!(
                "{} [{}{}] per_1000 {:.2}, {} live rows, last fired {}\n  prevents: {}\n",
                r.id,
                r.stance,
                if r.irreversible { ", irreversible" } else { "" },
                r.per_1000,
                r.live_rows,
                r.last_fired.as_deref().unwrap_or("never"),
                r.prevents
            ));
            for m in &r.compliance {
                s.push_str(&format!(
                    "  {:<28}{:>6} firings {:>6} answered {:>5} complied\n",
                    m.model,
                    m.firings,
                    m.answered,
                    if m.answered == 0 {
                        "-".to_string()
                    } else {
                        format!(
                            "{:.0}%",
                            100.0 * f64::from(m.complied) / f64::from(m.answered)
                        )
                    }
                ));
            }
            if let Some(a) = &r.arms {
                s.push_str(&format!(
                    "  ignored share: live {} ({} answered), shadow {} ({} answered)\n",
                    pct(&a.live),
                    a.live.answered(),
                    pct(&a.shadow),
                    a.shadow.answered()
                ));
            }
            if let Some(n) = r.replay {
                s.push_str(&format!("  replay: {n} newest-model call(s) matched\n"));
            }
        }
        s
    }
}

/// Where reviews live: next to the journal.
pub fn reviews_dir(journal_dir: &Path) -> PathBuf {
    journal_dir.join("harness-review")
}

/// Write `<dir>/<date>.json`. The binary passes [`reviews_dir`] when `--out` is
/// absent.
pub fn write(dir: &Path, review: &Review) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", review.date));
    crate::atomic::write_atomic(&path, &format!("{}\n", review.to_json()))?;
    Ok(path)
}

/// The model the newest review file (latest date in its name) was made for:
/// `Ok(None)` when there is no review, `Ok(Some(None))` when it saw no model,
/// `Err` naming the file when it cannot be read or parsed.
fn reviewed_model(dir: &Path) -> Result<Option<Option<String>>, String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(None);
    };
    let Some(newest) = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .max()
    else {
        return Ok(None);
    };
    let shown = newest.display();
    let text = std::fs::read_to_string(&newest)
        .map_err(|e| format!("cannot read the review file {shown}: {e}"))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("cannot parse the review file {shown}: {e}"))?;
    Ok(Some(
        value
            .get("newest_model")
            .and_then(|m| m.as_str())
            .map(str::to_string),
    ))
}

/// `harness-review --check`: the one line to print when the newest model in
/// the transcripts is not the one the newest review was made for, or when the
/// newest review file cannot be read.
pub fn check(dir: &Path, scan: &Scan) -> Result<Option<String>, ScanError> {
    let surveyed = survey(scan, &[])?;
    let Some(newest) = surveyed.newest() else {
        return Ok(None);
    };
    Ok(match reviewed_model(dir) {
        Ok(Some(Some(m))) if m == newest => None,
        Ok(Some(_)) => Some(format!(
            "harness-review: new model {newest} since the last review; run `amont-agent harness-review`"
        )),
        Ok(None) => Some(format!(
            "harness-review: new model {newest} and no review on record; run `amont-agent harness-review`"
        )),
        Err(why) => Some(format!(
            "harness-review: {why}; run `amont-agent harness-review`"
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::civil;
    use crate::transcript::{Discovery, Roots};

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("amont-agent-hr-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn call(session: &str, model: &str, day: &str, n: u32, command: &str) -> String {
        format!(
            "{{\"timestamp\":\"{day}T10:00:00Z\",\"sessionId\":\"{session}\",\"cwd\":\"/w\",\
             \"message\":{{\"model\":\"{model}\",\"content\":[{{\"type\":\"tool_use\",\
             \"id\":\"t-{session}-{n}\",\"name\":\"Bash\",\"input\":{{\"command\":\"{command}\"}}}}]}}}}\n"
        )
    }

    /// Two rules: `sed-in-place` (reversible, shadowed) and `no-verify`
    /// (irreversible). Session `aaaaaaaa` is in the live arm and ignores the
    /// advice; `bbbbbbbb` is in the shadow arm and changes anyway.
    struct Fixture {
        root: PathBuf,
        roots: Roots,
        journal: PathBuf,
    }

    fn fixture(name: &str) -> Fixture {
        let root = scratch(name);
        let proj = root.join("projects").join("p");
        std::fs::create_dir_all(&proj).unwrap();
        let a = [
            call(
                "aaaaaaaa-1",
                "claude-new",
                "2026-10-01",
                1,
                "sed -i '' 's/a/b/' f",
            ),
            call(
                "aaaaaaaa-1",
                "claude-new",
                "2026-10-01",
                2,
                "sed -i '' 's/a/c/' g",
            ),
            call(
                "aaaaaaaa-1",
                "claude-new",
                "2026-10-01",
                3,
                "git commit -n -m x",
            ),
        ]
        .concat();
        let b = [
            call(
                "bbbbbbbb-2",
                "claude-new",
                "2026-10-02",
                1,
                "sed -i '' 's/a/b/' f",
            ),
            call(
                "bbbbbbbb-2",
                "claude-new",
                "2026-10-02",
                2,
                "sed 's/a/b/' f -i",
            ),
        ]
        .concat();
        let old = [
            call(
                "cccccccc-3",
                "claude-old",
                "2026-09-01",
                1,
                "git commit --no-verify -m y",
            ),
            call("cccccccc-3", "claude-old", "2026-09-01", 2, "ls"),
        ]
        .concat();
        std::fs::write(proj.join("a.jsonl"), a).unwrap();
        std::fs::write(proj.join("b.jsonl"), b).unwrap();
        std::fs::write(proj.join("c.jsonl"), old).unwrap();
        let journal = root.join("journal.log");
        // 2026-10-01 10:00:00 UTC is 1790848800.
        std::fs::write(
            &journal,
            "F 1790848800 sed-in-place advised advised aaaaaaaa repo default arm=live x\n\
             F 1790935200 sed-in-place watched watched bbbbbbbb repo default arm=shadow x\n\
             F 1790848800 no-verify watched watched aaaaaaaa repo default arm=live x\n",
        )
        .unwrap();
        let roots = Roots {
            dirs: vec![root.join("projects")],
            how: Discovery::Explicit,
        };
        Fixture {
            root,
            roots,
            journal,
        }
    }

    fn rules() -> Vec<&'static Rule> {
        vec![
            &crate::rules::sed_in_place::RULE,
            &crate::rules::no_verify::RULE,
        ]
    }

    fn run(f: &Fixture) -> Review {
        let scan = Scan {
            roots: &f.roots,
            tool: "Bash",
            since: None,
        };
        review(
            &scan,
            &rules(),
            |p| match p {
                Part::Rule(r) => r.default_stance,
                Part::Assertion(a) => a.default_stance,
            },
            std::slice::from_ref(&f.journal),
            civil::days_from_civil(2026, 10, 21),
        )
        .unwrap()
    }

    fn row<'a>(r: &'a Review, id: &str) -> &'a Row {
        r.rows.iter().find(|x| x.id == id).unwrap()
    }

    #[test]
    fn rows_for_two_rules_carry_the_journal_and_the_newest_model() {
        let f = fixture("rows");
        let r = run(&f);
        assert_eq!(r.newest_model.as_deref(), Some("claude-new"));
        assert_eq!(
            r.rows.len(),
            crate::assertions::ASSERTIONS.len() + 2,
            "one row per rule and assertion"
        );
        let sed = row(&r, "sed-in-place");
        assert_eq!(sed.live_rows, 1, "the shadow row is not a live row");
        assert_eq!(sed.last_fired.as_deref(), Some("2026-10-01"));
        assert!(sed.per_1000 > 0.0);
        assert!(!sed.irreversible);
        assert!(sed.replay.is_none());
        let nv = row(&r, "no-verify");
        assert_eq!(nv.live_rows, 1);
        assert!(nv.irreversible);
        assert!(nv.arms.is_none(), "no shadow rows, no arm comparison");
        assert!(r.to_json().contains("\"id\":\"no-verify\""));
        let _ = std::fs::remove_dir_all(&f.root);
    }

    /// A deny rule refuses on the live arm, so it has no live compliance; its
    /// shadow rows are replayed as observe and give the shadow arm a share.
    #[test]
    fn a_shadowed_deny_rule_has_a_shadow_share_and_no_live_one() {
        let f = fixture("deny-shadow");
        // aaaaaaaa ignored the advice; here it is the session in the shadow arm.
        std::fs::write(
            &f.journal,
            "F 1790848800 sed-in-place watched watched aaaaaaaa repo default arm=shadow x\n",
        )
        .unwrap();
        let scan = Scan {
            roots: &f.roots,
            tool: "Bash",
            since: None,
        };
        let r = review(
            &scan,
            &rules(),
            |p| match p {
                Part::Rule(r) if r.id == "sed-in-place" => Stance::Deny,
                Part::Rule(r) => r.default_stance,
                Part::Assertion(a) => a.default_stance,
            },
            std::slice::from_ref(&f.journal),
            civil::days_from_civil(2026, 10, 21),
        )
        .unwrap();
        let arms = row(&r, "sed-in-place")
            .arms
            .as_ref()
            .expect("a deny rule with shadow rows is replayed");
        let shadow = arms.shadow.ignored_share().expect("a shadow share");
        assert!(
            shadow > 0.0,
            "aaaaaaaa ignored it in the shadow arm: {shadow}"
        );
        assert_eq!(
            arms.live.ignored_share(),
            None,
            "deny has no live compliance"
        );
        let _ = std::fs::remove_dir_all(&f.root);
    }

    #[test]
    fn the_ignored_share_is_split_by_the_arm_the_journal_names() {
        let f = fixture("arms");
        let r = run(&f);
        let arms = row(&r, "sed-in-place").arms.as_ref().expect("shadow rows");
        assert_eq!(arms.live.ignored_share(), Some(1.0), "aaaaaaaa ignored it");
        assert_eq!(arms.shadow.ignored_share(), Some(0.0), "bbbbbbbb changed");
        let _ = std::fs::remove_dir_all(&f.root);
    }

    #[test]
    fn an_irreversible_part_counts_only_the_newest_models_matches() {
        let f = fixture("replay");
        let r = run(&f);
        assert_eq!(
            row(&r, "no-verify").replay,
            Some(1),
            "the old model's --no-verify is not counted"
        );
        let _ = std::fs::remove_dir_all(&f.root);
    }

    #[test]
    fn check_flags_an_unseen_model_and_passes_after_its_review() {
        let f = fixture("check");
        let scan = Scan {
            roots: &f.roots,
            tool: "Bash",
            since: None,
        };
        let line = check(&reviews_dir(&f.root), &scan)
            .unwrap()
            .expect("no review yet");
        assert!(line.contains("claude-new"), "{line}");
        let r = run(&f);
        write(&reviews_dir(&f.root), &r).unwrap();
        assert_eq!(check(&reviews_dir(&f.root), &scan).unwrap(), None);
        // A newer model appears.
        let proj = f.root.join("projects").join("p");
        std::fs::write(
            proj.join("d.jsonl"),
            call("dddddddd-4", "claude-newer", "2026-10-10", 1, "ls"),
        )
        .unwrap();
        let line = check(&reviews_dir(&f.root), &scan)
            .unwrap()
            .expect("unseen model");
        assert!(line.contains("claude-newer"), "{line}");
        let _ = std::fs::remove_dir_all(&f.root);
    }

    #[test]
    fn per_1000_counts_only_journal_rows_on_or_after_since() {
        let f = fixture("since");
        let scan = Scan {
            roots: &f.roots,
            tool: "Bash",
            since: Some(civil::days_from_civil(2026, 10, 2)),
        };
        let r = review(
            &scan,
            &rules(),
            |p| match p {
                Part::Rule(r) => r.default_stance,
                Part::Assertion(a) => a.default_stance,
            },
            std::slice::from_ref(&f.journal),
            civil::days_from_civil(2026, 10, 21),
        )
        .unwrap();
        let sed = row(&r, "sed-in-place");
        assert_eq!(sed.live_rows, 0, "the live row is on 2026-10-01");
        assert_eq!(sed.per_1000, 0.0);
        assert_eq!(row(&r, "no-verify").live_rows, 0);

        // The boundary: a row on the --since day itself counts.
        let on_the_day = Scan {
            roots: &f.roots,
            tool: "Bash",
            since: Some(civil::days_from_civil(2026, 10, 1)),
        };
        let r = review(
            &on_the_day,
            &rules(),
            |p| match p {
                Part::Rule(r) => r.default_stance,
                Part::Assertion(a) => a.default_stance,
            },
            std::slice::from_ref(&f.journal),
            civil::days_from_civil(2026, 10, 21),
        )
        .unwrap();
        assert_eq!(
            row(&r, "sed-in-place").live_rows,
            1,
            "the row is on 2026-10-01"
        );
        let _ = std::fs::remove_dir_all(&f.root);
    }

    #[test]
    fn check_names_an_unparsable_review_file() {
        let f = fixture("corrupt");
        let scan = Scan {
            roots: &f.roots,
            tool: "Bash",
            since: None,
        };
        let dir = reviews_dir(&f.root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2026-10-20.json"), "{not json").unwrap();
        let line = check(&reviews_dir(&f.root), &scan)
            .unwrap()
            .expect("a finding");
        assert!(
            line.contains("2026-10-20.json") && line.contains("parse"),
            "{line}"
        );
        let _ = std::fs::remove_dir_all(&f.root);
    }
}
