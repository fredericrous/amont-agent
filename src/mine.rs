//! Mining the transcripts for habits no rule names yet.
//!
//! `backtest` answers a question you already had: *how often would this rule
//! have fired?* It can only price the rules somebody already thought of. The
//! rules in this crate came out of a person sorting nineteen thousand Bash
//! calls by what failed, and that sort is the step this module automates —
//! not the rule-writing, which stays deliberately manual.
//!
//! ## The outcome comes from the transcript, never from a re-run
//!
//! Two signals, both readable after the fact:
//!
//! * **failed** — the tool result carried `is_error`. A non-zero exit, a
//!   refused permission, a run the harness killed.
//! * **corrected** — within the next few tool calls the model issued another
//!   Bash command of near-identical [shape](crate::shape). A model that
//!   re-issues the same shape is a model whose first attempt did not land,
//!   whether or not anything reported a failure. That is the whole point:
//!   the mistakes worth a rule are exactly the ones nothing reports.
//!
//! Neither signal is proof. A shape that is re-run for perfectly good reasons
//! — `git status`, `ls`, a poll — carries a high rate and names no mistake at
//! all, and a shape that failed once for an unrelated reason carries one too.
//! So the output is ranked suspicion with the samples attached, and the next
//! step is a person reading them. Every number printed here says as much.
//!
//! ## Why nothing learned here reaches the hook
//!
//! A mined shape never becomes a matcher. The path is
//! `mine → write the rule by hand → cases → corpus check → graduate`, and
//! what ships is the hand-written rule, with its reason, its remedy and its
//! reviewed corpus. A guard that refuses a command because a clustering run
//! found the command suspicious cannot explain itself to the person whose
//! work it just refused, and that is the one failure this crate cannot
//! afford.

use std::collections::{BTreeMap, HashMap};

use crate::civil::{iso, Day};
use crate::json;
use crate::rules::{self, Rule};
use crate::shape::{self, Shape};
use crate::shell;
use crate::transcript::{self, Outcome, Scan, ScanError, Stats};

pub struct Options {
    /// How far ahead a correction may arrive, counted in tool calls of any
    /// kind. Three is what the transcripts show: a model that is fixing the
    /// command it just ran does it immediately, sometimes after one look at a
    /// file. Beyond that it has moved on to something else and the "fix" is a
    /// coincidence.
    pub window: usize,
    pub min_support: u32,
    pub min_rate: f64,
    pub samples: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            window: 3,
            min_support: 5,
            min_rate: 0.3,
            samples: 3,
        }
    }
}

pub struct Group {
    pub shape: String,
    pub calls: u32,
    pub failed: u32,
    pub corrected: u32,
    /// Calls that failed OR were corrected. Not the sum: one call can do both,
    /// and counting it twice would put the rate above 1.
    pub troubled: u32,
    /// The rule that already fires on this shape, when one does.
    pub covered_by: Option<&'static str>,
    /// Redacted AND trimmed — one line, capped — because these are printed
    /// into a terminal beside a table.
    pub samples: Vec<String>,
    /// The same commands, redacted but whole. `--format cases` writes these:
    /// a case file is replayed through the rules by `corpus check`, and a
    /// command with its newlines collapsed and its tail cut off does not
    /// replay to anything. The corpus format escapes newlines itself.
    pub cases: Vec<String>,
    pub first: Day,
    pub last: Day,
}

impl Group {
    pub fn rate(&self) -> f64 {
        if self.calls == 0 {
            0.0
        } else {
            f64::from(self.troubled) / f64::from(self.calls)
        }
    }
    /// Corrections times rate. Rate alone floats every shape seen five times
    /// to the top; count alone floats whatever the model runs most. The
    /// product is "often wrong AND often run", which is the only kind of
    /// habit worth a rule.
    pub fn score(&self) -> f64 {
        f64::from(self.troubled) * self.rate()
    }
}

pub struct Report {
    pub proposed: Vec<Group>,
    pub covered: Vec<Group>,
    pub stats: Stats,
    pub shapes: usize,
    pub opaque: u64,
    pub options: Options,
}

#[derive(Default)]
struct Tally {
    calls: u32,
    failed: u32,
    corrected: u32,
    troubled: u32,
    rules: HashMap<&'static str, u32>,
    samples: Vec<String>,
    cases: Vec<String>,
    first: Day,
    last: Day,
}

pub fn run(scan: &Scan, options: Options) -> Result<Report, ScanError> {
    let mut groups: BTreeMap<String, Tally> = BTreeMap::new();
    let mut opaque = 0u64;

    let stats = transcript::for_each_transcript(scan, |records| {
        // Every Bash call's shape, once. The correction search looks ahead
        // from every call, so a shape computed per comparison would be
        // computed `window` times over.
        let shapes: Vec<Option<Shape>> = records
            .iter()
            .map(|r| {
                if r.tool != scan.tool || r.command.is_empty() {
                    return None;
                }
                let parsed = shell::lex(&r.command);
                if matches!(parsed, shell::Parsed::Opaque(_)) {
                    return None;
                }
                let s = Shape::of(&parsed);
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            })
            .collect();

        for (i, record) in records.iter().enumerate() {
            if record.tool != scan.tool || record.command.is_empty() {
                continue;
            }
            let Some(shape) = &shapes[i] else {
                opaque += 1;
                continue;
            };
            let corrected = starts_a_retry_chain(&shapes, i, options.window);
            let failed = record.outcome == Outcome::Failed;
            let entry = groups.entry(shape.text()).or_insert_with(|| Tally {
                first: record.day,
                last: record.day,
                ..Tally::default()
            });
            entry.calls += 1;
            entry.first = entry.first.min(record.day);
            entry.last = entry.last.max(record.day);
            if failed {
                entry.failed += 1;
            }
            if corrected {
                entry.corrected += 1;
            }
            if failed || corrected {
                entry.troubled += 1;
                // Samples are taken from the calls that went wrong. A sample
                // of a shape's HEALTHY runs is a sample of the wrong thing:
                // the reader is being asked what the mistake looks like.
                if entry.samples.len() < options.samples {
                    let sample = crate::journal::redact_and_trim(&record.command);
                    if !entry.samples.contains(&sample) {
                        entry.samples.push(sample);
                        entry.cases.push(crate::journal::redact(&record.command));
                    }
                }
            }
            let parsed = shell::lex(&record.command);
            for rule in rules::RULES {
                if fires(rule, &parsed) {
                    *entry.rules.entry(rule.id).or_default() += 1;
                }
            }
        }
    })?;

    let shapes = groups.len();
    let mut proposed = Vec::new();
    let mut covered = Vec::new();
    for (shape, t) in groups {
        let group = Group {
            shape,
            calls: t.calls,
            failed: t.failed,
            corrected: t.corrected,
            troubled: t.troubled,
            // The rule that fires on most of this shape's calls. A rule that
            // fires on one call in forty is not what covers the shape.
            covered_by: t
                .rules
                .iter()
                .max_by_key(|(id, n)| (**n, std::cmp::Reverse(**id)))
                .filter(|(_, n)| **n * 2 >= t.calls)
                .map(|(id, _)| *id),
            samples: t.samples,
            cases: t.cases,
            first: t.first,
            last: t.last,
        };
        if group.calls < options.min_support || group.rate() < options.min_rate {
            continue;
        }
        if group.covered_by.is_some() {
            covered.push(group);
        } else {
            proposed.push(group);
        }
    }
    for list in [&mut proposed, &mut covered] {
        // Score descending, then the shape text, so two runs over the same
        // transcripts print the same report in the same order.
        list.sort_by(|a, b| {
            b.score()
                .partial_cmp(&a.score())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.shape.cmp(&b.shape))
        });
    }

    Ok(Report {
        proposed,
        covered,
        stats,
        shapes,
        opaque,
        options,
    })
}

/// `examine` only, and never `confirm` — the same discipline the backtester
/// keeps. `confirm` touches the world, and the world has moved since these
/// commands ran.
fn fires(rule: &Rule, parsed: &shell::Parsed) -> bool {
    (rule.examine)(parsed).is_some()
}

/// Did a near-identical command follow this one — and is this the call that
/// STARTED the run of them?
///
/// The second half is what makes the number mean anything. A polling loop is
/// twenty near-identical calls in a row, and counting each one as a
/// correction of the last gives it a rate of 1.0 and puts `echo waiting` at
/// the top of the report — measured, on the real corpus, before this existed.
/// A run of near-identical calls is ONE decision repeated, so only its head
/// counts, and the rate becomes episodes per call rather than adjacencies per
/// call. A mistake followed by its fix is still 1 in 2; a twenty-poll wait is
/// 1 in 20 and drops below any threshold worth setting.
fn starts_a_retry_chain(shapes: &[Option<Shape>], i: usize, window: usize) -> bool {
    let Some(mine) = &shapes[i] else {
        return false;
    };
    let near = |other: &Shape| shape::distance(mine, other) <= shape::NEAR;
    let after = (i + 1 + window).min(shapes.len());
    if !shapes[i + 1..after].iter().flatten().any(near) {
        return false;
    }
    let before = i.saturating_sub(window);
    !shapes[before..i].iter().flatten().any(near)
}

impl Report {
    /// Every sample command in the report, for `--format cases`.
    pub fn sample_commands(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for g in &self.proposed {
            for s in &g.cases {
                out.push(s.as_str());
            }
        }
        out
    }

    pub fn render(&self, roots: &transcript::Roots) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "transcripts: {} ({} files, {:.0} MB, {} Bash calls in {} tool calls, {} duplicates dropped)\n",
            roots
                .dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            self.stats.files,
            self.stats.bytes as f64 / 1_048_576.0,
            self.stats.calls,
            self.stats.events,
            self.stats.duplicates,
        ));
        s.push_str(&format!(
            "{} distinct shapes; {} command(s) could not be read.\n",
            self.shapes, self.opaque
        ));
        s.push_str(&format!(
            "a shape is proposed at {} calls and a {:.2} rate, correcting within {} tool call(s).\n\n",
            self.options.min_support, self.options.min_rate, self.options.window
        ));
        s.push_str(
            "`bad` is calls that failed outright or drew a near-identical follow-up.\n\
             That is a SUSPICION, not a verdict: a shape re-run for good reasons carries\n\
             a high rate and names no mistake. Read the samples before writing anything.\n\n",
        );

        if self.proposed.is_empty() {
            s.push_str("no shape clears the thresholds.\n");
        } else {
            s.push_str(&Self::header());
            for g in &self.proposed {
                s.push_str(&Self::row(g));
                for sample in &g.samples {
                    s.push_str(&format!("      {sample}\n"));
                }
            }
        }

        if !self.covered.is_empty() {
            s.push_str(
                "\nalready covered by a rule — listed, not proposed. A shape here that is\n\
                 still going wrong is a rule that is observing rather than advising.\n\n",
            );
            s.push_str(&Self::header());
            for g in &self.covered {
                s.push_str(&Self::row(g));
                s.push_str(&format!(
                    "      covered by {}\n",
                    g.covered_by.unwrap_or("-")
                ));
            }
        }
        s
    }

    fn header() -> String {
        format!(
            "{:>7}{:>6}{:>7}{:>8}  {}\n",
            "calls", "bad", "rate", "score", "shape"
        )
    }

    fn row(g: &Group) -> String {
        format!(
            "{:>7}{:>6}{:>7.2}{:>8.1}  {}\n",
            g.calls,
            g.troubled,
            g.rate(),
            g.score(),
            g.shape
        )
    }

    pub fn to_json(&self, roots: &transcript::Roots) -> String {
        let one = |g: &Group| {
            json::object(&[
                json::string_field("shape", &g.shape),
                json::int_field("calls", i64::from(g.calls)),
                json::int_field("failed", i64::from(g.failed)),
                json::int_field("corrected", i64::from(g.corrected)),
                json::int_field("bad", i64::from(g.troubled)),
                json::string_field("rate", &format!("{:.2}", g.rate())),
                json::string_field("score", &format!("{:.1}", g.score())),
                json::string_field("first_seen", &iso(g.first)),
                json::string_field("last_seen", &iso(g.last)),
                json::string_field("covered_by", g.covered_by.unwrap_or("")),
                json::string_array_field("samples", &g.samples),
            ])
        };
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
            json::int_field("tool_calls", self.stats.events as i64),
            json::int_field("duplicates", self.stats.duplicates as i64),
            json::int_field("opaque", self.opaque as i64),
            json::int_field("shapes", self.shapes as i64),
            json::int_field("window", self.options.window as i64),
            json::int_field("min_support", i64::from(self.options.min_support)),
            json::string_field("min_rate", &format!("{:.2}", self.options.min_rate)),
            format!(
                "\"proposed\":{}",
                json::array(&self.proposed.iter().map(one).collect::<Vec<_>>())
            ),
            format!(
                "\"covered\":{}",
                json::array(&self.covered.iter().map(one).collect::<Vec<_>>())
            ),
            json::string_field(
                "note",
                "`bad` is failed-or-corrected, a suspicion rather than a verdict; \
                 local CLI sessions only, web sessions are not on this machine",
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Record;

    fn record(tool: &str, command: &str, outcome: Outcome) -> Record {
        Record {
            id: command.to_string(),
            tool: tool.to_string(),
            command: command.to_string(),
            day: 0,
            session: "s".into(),
            cwd: "/tmp".into(),
            model: "m".into(),
            sidechain: false,
            outcome,
        }
    }

    fn shapes_of(records: &[Record]) -> Vec<Option<Shape>> {
        records
            .iter()
            .map(|r| {
                if r.tool != "Bash" || r.command.is_empty() {
                    None
                } else {
                    Some(Shape::of_command(&r.command))
                }
            })
            .collect()
    }

    #[test]
    fn a_near_identical_retry_is_a_correction() {
        let records = [
            record("Bash", "git push origin main", Outcome::Ok),
            record("Bash", "git push origin other", Outcome::Ok),
        ];
        assert!(starts_a_retry_chain(&shapes_of(&records), 0, 3));
    }

    /// A polling loop is one decision repeated, not nineteen corrections.
    /// Before this, `echo waiting` was the top row of the real report.
    #[test]
    fn only_the_head_of_a_run_counts() {
        let records: Vec<Record> = ["echo waiting", "echo waiting", "echo waiting"]
            .iter()
            .map(|c| record("Bash", c, Outcome::Ok))
            .collect();
        let shapes = shapes_of(&records);
        assert!(starts_a_retry_chain(&shapes, 0, 3));
        assert!(!starts_a_retry_chain(&shapes, 1, 3));
        assert!(!starts_a_retry_chain(&shapes, 2, 3));
    }

    /// The window counts EVERY tool call, so two Bash calls with a Read
    /// between them are still neighbours — and two with a day's work between
    /// them are not.
    #[test]
    fn an_intervening_read_does_not_break_the_window_but_distance_does() {
        let near = [
            record("Bash", "git push origin main", Outcome::Ok),
            record("Read", "", Outcome::Ok),
            record("Bash", "git push origin other", Outcome::Ok),
        ];
        assert!(starts_a_retry_chain(&shapes_of(&near), 0, 3));

        let far = [
            record("Bash", "git push origin main", Outcome::Ok),
            record("Read", "", Outcome::Ok),
            record("Read", "", Outcome::Ok),
            record("Read", "", Outcome::Ok),
            record("Bash", "git push origin other", Outcome::Ok),
        ];
        assert!(!starts_a_retry_chain(&shapes_of(&far), 0, 3));
    }

    /// An unrelated command that happens to come next is not a correction.
    #[test]
    fn a_different_command_is_not_a_correction() {
        let records = [
            record("Bash", "git push origin main", Outcome::Ok),
            record("Bash", "cargo test --locked", Outcome::Ok),
        ];
        assert!(!starts_a_retry_chain(&shapes_of(&records), 0, 3));
    }

    /// One call that both failed and was corrected counts once, or the rate
    /// would exceed 1 and stop meaning "a share of the calls".
    #[test]
    fn the_rate_is_a_share_of_calls_not_a_sum_of_signals() {
        let g = Group {
            shape: "git push".into(),
            calls: 4,
            failed: 4,
            corrected: 4,
            troubled: 4,
            covered_by: None,
            samples: vec![],
            cases: vec![],
            first: 0,
            last: 0,
        };
        assert_eq!(g.rate(), 1.0);
        assert_eq!(g.score(), 4.0);
    }
}
