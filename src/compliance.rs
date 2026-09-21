//! Does the advice work?
//!
//! `advise` buys its place in the model's context with tokens, every session,
//! forever. The backtest says what that costs — firings per thousand calls.
//! It says nothing about what it buys, and a rule that speaks into a model
//! that then does the same thing anyway is pure cost.
//!
//! So: for every firing, look at what the model did NEXT. If the next
//! equivalent command — same [shape](crate::shape), within a few tool calls —
//! no longer matches the rule, the habit changed. If it matches again, the
//! advice was read and ignored, or never reached the model at all.
//!
//! ## `observe` is the control, and it is not optional
//!
//! An `advise` rule with a 70% compliance share proves nothing on its own,
//! because models correct themselves constantly without being told. The
//! `observe` rules are the counterfactual: they say nothing, so their share
//! IS the rate a habit corrects on its own. Advice is worth its tokens only
//! where the advised share beats the silent one — which is the same argument
//! the stance ladder rests on, measured instead of assumed.
//!
//! ## What this cannot see
//!
//! A transcript does not record whether the hook actually spoke. It records
//! what the model ran. So a firing here means "the rule WOULD fire on this
//! command as the rule stands today", replayed — not "the model was advised
//! on that day". Where a rule was widened, or was observing at the time and
//! advises now, the two differ and the number is a reconstruction. It is
//! still the only evidence there is, and it is honest about which it is.
//!
//! `deny` rules are left out entirely: a refused command never ran, so there
//! is no next command to compare it against.

use std::collections::BTreeMap;

use crate::civil::{iso, week_start, Day};
use crate::json;
use crate::rules::{Rule, Stance};
use crate::shape::{self, Shape};
use crate::shell;
use crate::transcript::{self, Scan, ScanError, Stats};

/// A model that answers to no id. Sessions from older Claude Code versions,
/// and a few synthetic turns, carry none; they are kept under one name rather
/// than dropped, so the firings still add up to the backtest's total.
const UNKNOWN_MODEL: &str = "(unnamed)";

pub struct Options {
    /// How far ahead the next equivalent command may be, in tool calls of
    /// any kind.
    ///
    /// Much wider than mining's three, and deliberately: the two are asking
    /// different questions. A CORRECTION is immediate — a model fixing the
    /// command it just ran does it at once, and anything later is a
    /// coincidence. COMPLIANCE is "the next time it wanted to do this",
    /// which is however long the work in between took. Measured over the
    /// real corpus, widening 3 → 10 → 20 → 40 roughly quadruples how many
    /// firings get an answer and moves every share by a point or two, which
    /// is what a stable measurement with a loose parameter looks like.
    pub window: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { window: 20 }
    }
}

#[derive(Default, Clone)]
pub struct Cell {
    pub firings: u32,
    /// An equivalent command followed, and it no longer matches.
    pub complied: u32,
    /// An equivalent command followed, and it matches again.
    pub ignored: u32,
    /// Nothing equivalent followed inside the window. Not a verdict either
    /// way: the model may have moved on, or the session may have ended.
    pub unanswered: u32,
}

impl Cell {
    pub fn answered(&self) -> u32 {
        self.complied + self.ignored
    }
    /// The share of ANSWERED firings that changed. `None` when nothing was
    /// answered — an unmeasured share, which is not the same as zero.
    pub fn share(&self) -> Option<f64> {
        if self.answered() == 0 {
            None
        } else {
            Some(f64::from(self.complied) / f64::from(self.answered()))
        }
    }
}

pub struct Subject {
    pub id: &'static str,
    pub stance: Stance,
    /// One cell per (model, week starting), both in ascending order.
    pub cells: BTreeMap<(String, Day), Cell>,
}

impl Subject {
    pub fn total(&self) -> Cell {
        let mut t = Cell::default();
        for c in self.cells.values() {
            t.firings += c.firings;
            t.complied += c.complied;
            t.ignored += c.ignored;
            t.unanswered += c.unanswered;
        }
        t
    }
}

pub struct Report {
    pub subjects: Vec<Subject>,
    pub stats: Stats,
    pub options: Options,
}

pub fn run(scan: &Scan, rules: &[&'static Rule], options: Options) -> Result<Report, ScanError> {
    // `deny` never gets a "next command" to compare against: the command it
    // names did not run. Dropped here rather than reported empty, because a
    // row of zeroes reads as "the advice did nothing".
    let watched: Vec<(&'static Rule, Stance)> = rules
        .iter()
        .map(|r| (*r, crate::stance::resolve(r)))
        .filter(|(_, s)| *s != Stance::Deny)
        .collect();
    let mut cells: Vec<BTreeMap<(String, Day), Cell>> =
        watched.iter().map(|_| BTreeMap::new()).collect();

    let stats = transcript::for_each_transcript(scan, |records| {
        let parsed: Vec<Option<(shell::Parsed, Shape)>> = records
            .iter()
            .map(|r| {
                if r.tool != scan.tool || r.command.is_empty() {
                    return None;
                }
                let p = shell::lex(&r.command);
                if matches!(p, shell::Parsed::Opaque(_)) {
                    return None;
                }
                let s = Shape::of(&p);
                if s.is_empty() {
                    None
                } else {
                    Some((p, s))
                }
            })
            .collect();

        for (i, record) in records.iter().enumerate() {
            let Some((here, shape)) = &parsed[i] else {
                continue;
            };
            // The next equivalent command, whatever any rule thinks of it.
            // Found once and reused: every rule is asking the same question
            // about the same successor.
            let end = (i + 1 + options.window).min(records.len());
            let next = parsed[i + 1..end]
                .iter()
                .flatten()
                .find(|(_, other)| shape::distance(shape, other) <= shape::NEAR);

            for (k, (rule, _)) in watched.iter().enumerate() {
                if (rule.examine)(here).is_none() {
                    continue;
                }
                let model = if record.model.is_empty() {
                    UNKNOWN_MODEL.to_string()
                } else {
                    record.model.clone()
                };
                let cell = cells[k].entry((model, week_start(record.day))).or_default();
                cell.firings += 1;
                match next {
                    None => cell.unanswered += 1,
                    Some((then, _)) if (rule.examine)(then).is_some() => cell.ignored += 1,
                    Some(_) => cell.complied += 1,
                }
            }
        }
    })?;

    let mut subjects: Vec<Subject> = watched
        .iter()
        .zip(cells)
        .map(|((rule, stance), cells)| Subject {
            id: rule.id,
            stance: *stance,
            cells,
        })
        .collect();
    // Advice first — it is the one that costs tokens — then the silent
    // control, each by id so a rerun prints the same order.
    subjects.sort_by_key(|s| (std::cmp::Reverse(s.stance), s.id));
    subjects.retain(|s| s.total().firings > 0);
    Ok(Report {
        subjects,
        stats,
        options,
    })
}

fn pct(share: Option<f64>) -> String {
    match share {
        Some(p) => format!("{:.0}%", p * 100.0),
        None => "-".to_string(),
    }
}

impl Report {
    pub fn render(&self, roots: &transcript::Roots) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "transcripts: {} ({} files, {} Bash calls, {} duplicates dropped)\n",
            roots
                .dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            self.stats.files,
            self.stats.calls,
            self.stats.duplicates,
        ));
        s.push_str(&format!(
            "UTC weeks. A firing is answered when an equivalent command follows within {} tool\n\
             call(s): COMPLIED when it no longer matches, IGNORED when it matches again.\n\
             `observe` rules said nothing — their share is the rate the habit corrects on its\n\
             own, and advice is worth its tokens only where it beats that. `deny` is left out:\n\
             a refused command never ran. Replayed against the rules AS THEY STAND TODAY, which\n\
             is not the same as what the model was told on the day.\n\n",
            self.options.window
        ));
        if self.subjects.is_empty() {
            s.push_str("no rule fired over these transcripts.\n");
            return s;
        }
        for subject in &self.subjects {
            let t = subject.total();
            s.push_str(&format!(
                "{} [{}] — {} firings, {} answered, {} complied\n",
                subject.id,
                subject.stance.as_str(),
                t.firings,
                t.answered(),
                pct(t.share()),
            ));
            s.push_str(&format!(
                "  {:<24}{:<14}{:>8}{:>10}{:>10}{:>11}\n",
                "model", "week starting", "firings", "answered", "ignored", "complied"
            ));
            for ((model, week), c) in &subject.cells {
                s.push_str(&format!(
                    "  {:<24}{:<14}{:>8}{:>10}{:>10}{:>11}\n",
                    model,
                    iso(*week),
                    c.firings,
                    c.answered(),
                    c.ignored,
                    pct(c.share()),
                ));
            }
            s.push('\n');
        }
        s
    }

    pub fn to_json(&self, roots: &transcript::Roots) -> String {
        let subjects: Vec<String> = self
            .subjects
            .iter()
            .map(|subject| {
                let weeks: Vec<String> = subject
                    .cells
                    .iter()
                    .map(|((model, week), c)| {
                        json::object(&[
                            json::string_field("model", model),
                            json::string_field("week", &iso(*week)),
                            json::int_field("firings", i64::from(c.firings)),
                            json::int_field("complied", i64::from(c.complied)),
                            json::int_field("ignored", i64::from(c.ignored)),
                            json::int_field("unanswered", i64::from(c.unanswered)),
                            json::string_field("complied_share", &pct(c.share())),
                        ])
                    })
                    .collect();
                let t = subject.total();
                json::object(&[
                    json::string_field("id", subject.id),
                    json::string_field("stance", subject.stance.as_str()),
                    json::int_field("firings", i64::from(t.firings)),
                    json::int_field("answered", i64::from(t.answered())),
                    json::string_field("complied_share", &pct(t.share())),
                    format!("\"weeks\":{}", json::array(&weeks)),
                ])
            })
            .collect();
        json::object(&[
            json::string_array_field(
                "transcripts",
                &roots
                    .dirs
                    .iter()
                    .map(|d| d.display().to_string())
                    .collect::<Vec<_>>(),
            ),
            json::int_field("files", self.stats.files as i64),
            json::int_field("calls", self.stats.calls as i64),
            json::int_field("window", self.options.window as i64),
            format!("\"rules\":{}", json::array(&subjects)),
            json::string_field(
                "note",
                "replayed against the rules as they stand today, which is not what the model \
                 was told on the day; `observe` rows are the silent control",
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unanswered_firing_is_neither_complied_nor_ignored() {
        let c = Cell {
            firings: 3,
            complied: 0,
            ignored: 0,
            unanswered: 3,
        };
        assert_eq!(c.answered(), 0);
        assert_eq!(c.share(), None, "unmeasured, not zero");
    }

    #[test]
    fn the_share_is_over_answered_firings_only() {
        let c = Cell {
            firings: 10,
            complied: 3,
            ignored: 1,
            unanswered: 6,
        };
        assert_eq!(c.share(), Some(0.75));
        assert_eq!(pct(c.share()), "75%");
    }
}
