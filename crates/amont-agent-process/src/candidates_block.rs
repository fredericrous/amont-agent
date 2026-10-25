//! adr-lint's candidates in an implementation-review brief, and the
//! reviewer's rulings on them (decisions ADR-0044, `harness.tool-boundaries`).
//!
//! adr-lint owns the candidate document (`adr-lint.candidates/1`) and the
//! ruling record (`adr-lint.ruling/1`); this module reads the first and
//! writes the second. The brief prints a `<<<CANDIDATES …>>>` block for the
//! reviewer's prompt; the push check reads that block from the prompt and
//! the `Candidates:` rulings from the reviewer's result. Candidates are
//! advisory input: nothing here holds a push.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use amont_agent_guard::journal;

use crate::implementation_review::is_sha256;

/// The format token of the document adr-lint prints.
pub const DOCUMENT_FORMAT: &str = "adr-lint.candidates/1";
/// The format token of one ruling record.
pub const RULING_FORMAT: &str = "adr-lint.ruling/1";
/// Opens the block when adr-lint answered.
const BLOCK_OPEN: &str = "<<<CANDIDATES sha=";
const BLOCK_CLOSE: &str = ">>>";
/// Stands in the block when adr-lint could not answer.
pub const UNAVAILABLE: &str = "<<<CANDIDATES unavailable>>>";
/// Closes the block.
pub const BLOCK_END: &str = "<<<END CANDIDATES>>>";
/// A ruling may name its candidate by a prefix this long or longer.
const MIN_PREFIX_HEX: usize = 12;
const TOOL: &str = "amont-agent";
const RULINGS_DIR: &str = "rulings";
/// A review whose agent id is unknown.
const UNKNOWN_REVIEW: &str = "unknown";

/// One line adr-lint marked for the reviewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub rule: String,
    pub pattern_id: String,
    pub pattern: String,
    pub path: String,
    pub line: u64,
    pub column: u64,
    pub text: String,
}

/// What adr-lint printed: the candidates, the per-rule overflow, and the
/// hash of the exact bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub sha: String,
    pub candidates: Vec<Candidate>,
    pub overflow: BTreeMap<String, u64>,
}

fn text_field(object: &serde_json::Value, field: &str, what: &str) -> Result<String, String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{what} has no string `{field}`"))
}

fn number_field(object: &serde_json::Value, field: &str, what: &str) -> Result<u64, String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("{what} has no number `{field}`"))
}

/// Reads the document adr-lint printed. A format token other than
/// [`DOCUMENT_FORMAT`] is refused; fields this reader does not know are
/// ignored.
pub fn parse_document(stdout: &[u8]) -> Result<Document, String> {
    let document: serde_json::Value = serde_json::from_slice(stdout)
        .map_err(|e| format!("adr-lint candidates printed output that is not JSON: {e}"))?;
    match document.get("format").and_then(serde_json::Value::as_str) {
        Some(DOCUMENT_FORMAT) => {}
        Some(other) => {
            return Err(format!(
                "adr-lint candidates printed format `{other}`, not `{DOCUMENT_FORMAT}`"
            ))
        }
        None => return Err("adr-lint candidates printed no `format` token".to_string()),
    }
    let rows = document
        .get("candidates")
        .and_then(serde_json::Value::as_array)
        .ok_or("adr-lint candidates printed no `candidates` array")?;
    let mut candidates = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let what = format!("candidate {index}");
        let id = text_field(row, "id", &what)?;
        if !is_sha256(&id) {
            return Err(format!("{what} has an `id` that is not 64 hex"));
        }
        candidates.push(Candidate {
            id: id.to_ascii_lowercase(),
            rule: text_field(row, "rule", &what)?,
            pattern_id: text_field(row, "pattern_id", &what)?,
            pattern: text_field(row, "pattern", &what)?,
            path: text_field(row, "path", &what)?,
            line: number_field(row, "line", &what)?,
            column: number_field(row, "col", &what)?,
            text: text_field(row, "text", &what)?,
        });
    }
    let mut overflow = BTreeMap::new();
    if let Some(counts) = document
        .get("overflow")
        .and_then(serde_json::Value::as_object)
    {
        for (rule, count) in counts {
            let count = count
                .as_u64()
                .ok_or_else(|| format!("overflow of `{rule}` is not a number"))?;
            overflow.insert(rule.clone(), count);
        }
    }
    Ok(Document {
        sha: crate::review_brief::sha_of(stdout),
        candidates,
        overflow,
    })
}

/// One line of a prompt, kept on one line.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The block a reviewer's prompt carries, whole: pasted verbatim.
pub fn block(document: &Document) -> String {
    let mut out = format!(
        "{BLOCK_OPEN}{} n={}{BLOCK_CLOSE}\n",
        document.sha,
        document.candidates.len()
    );
    for candidate in &document.candidates {
        out.push_str(&format!(
            "{} {} {} {}:{}\n",
            candidate.id, candidate.rule, candidate.pattern_id, candidate.path, candidate.line
        ));
    }
    out.push_str(BLOCK_END);
    out
}

/// The `## Candidates` section of the brief body for an answer.
pub fn section(document: &Document) -> String {
    let mut out = String::from("## Candidates\n\n");
    if document.candidates.is_empty() {
        out.push_str(
            "adr-lint found no candidate on the lines this branch adds: no ruling is needed.\n",
        );
        return out;
    }
    out.push_str(
        "adr-lint marked these added lines. A candidate is a line to look at, not a finding. \
         Your result must contain a `Candidates:` block with one line per listed id: \
         `<id> violation: <finding>` or `<id> fine: <one clause quoted from the rule body>`.\n\n",
    );
    for candidate in &document.candidates {
        out.push_str(&format!(
            "- {} {}:{} rule {} \u{2014} {}\n  text: {}\n",
            candidate.id,
            candidate.path,
            candidate.line,
            candidate.rule,
            flat(&candidate.pattern),
            flat(&candidate.text)
        ));
    }
    if !document.overflow.is_empty() {
        out.push_str("\nCandidates beyond the limit, per rule:\n");
        for (rule, count) in &document.overflow {
            out.push_str(&format!("- {rule}: {count}\n"));
        }
    }
    out
}

/// The unavailable marker and the section naming the cause.
pub fn unavailable_section(cause: &str) -> String {
    format!(
        "## Candidates\n\nadr-lint's candidates are unavailable ({}): no ruling is needed. \
         They are advisory input; the review goes on without them.\n",
        flat(cause)
    )
}

/// One listed candidate as the prompt's block names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub id: String,
    pub rule: String,
    pub pattern_id: String,
    /// Where the candidate is: the repository-relative path (spaces kept) and
    /// its 1-based line.
    pub path: String,
    pub line: usize,
    /// The pattern message from the prompt's section, when it has one.
    pub pattern: Option<String>,
}

/// What a reviewer's prompt said about candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompted {
    /// No block.
    Unlisted,
    /// A pass from before candidates were stored: nothing is known.
    NotRecorded,
    /// The unavailable marker.
    Unavailable,
    /// A block and the candidates it lists.
    Block(Vec<Listed>),
}

/// `<id> <rule> <pattern_id> <path>:<line>`: the location is the rest of the
/// line after the third word, and its line number follows the last `:`, so a
/// path with spaces parses.
fn block_entry(line: &str) -> Option<Listed> {
    let (id, rest) = line.split_once(' ')?;
    let (rule, rest) = rest.split_once(' ')?;
    let (pattern_id, location) = rest.split_once(' ')?;
    let (path, number) = location.rsplit_once(':')?;
    let line_number: usize = number.parse().ok()?;
    let well_formed = !path.is_empty() && !rule.is_empty() && !pattern_id.is_empty();
    (is_sha256(id) && well_formed).then(|| Listed {
        id: id.to_ascii_lowercase(),
        rule: rule.to_string(),
        pattern_id: pattern_id.to_string(),
        path: path.to_string(),
        line: line_number,
        pattern: None,
    })
}

/// `- <id> <path>:<line> rule <rule> — <pattern>` of the brief's section:
/// the id and the pattern message.
fn section_pattern(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("- ")?;
    let (id, rest) = rest.split_once(' ')?;
    if !is_sha256(id) {
        return None;
    }
    let (_, pattern) = rest.split_once(" rule ")?.1.split_once(" \u{2014} ")?;
    Some((id.to_ascii_lowercase(), pattern.trim().to_string()))
}

/// The candidates a prompt lists. A header that is not well formed is not a
/// block, and a block with no end line is not one either.
pub fn parse_prompt(prompt: &str) -> Prompted {
    if prompt.lines().any(|line| line.trim_end() == UNAVAILABLE) {
        return Prompted::Unavailable;
    }
    let lines: Vec<&str> = prompt.lines().map(str::trim_end).collect();
    for (at, line) in lines.iter().enumerate() {
        let Some(header) = line
            .strip_prefix(BLOCK_OPEN)
            .and_then(|rest| rest.strip_suffix(BLOCK_CLOSE))
        else {
            continue;
        };
        let Some((sha, count)) = header.split_once(" n=") else {
            continue;
        };
        let Ok(count) = count.parse::<usize>() else {
            continue;
        };
        if !is_sha256(sha) {
            continue;
        }
        let Some(length) = lines[at + 1..].iter().position(|l| *l == BLOCK_END) else {
            continue;
        };
        let listed: Option<Vec<Listed>> = lines[at + 1..at + 1 + length]
            .iter()
            .map(|l| block_entry(l))
            .collect();
        let Some(mut listed) = listed.filter(|entries| entries.len() == count) else {
            continue;
        };
        let patterns: BTreeMap<String, String> =
            lines.iter().filter_map(|l| section_pattern(l)).collect();
        for entry in &mut listed {
            entry.pattern = patterns.get(&entry.id).cloned();
        }
        return Prompted::Block(listed);
    }
    Prompted::Unlisted
}

/// What a reviewer ruled on one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Violation,
    Fine,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Violation => "violation",
            Verdict::Fine => "fine",
        }
    }
}

/// One `<id> violation|fine: <text>` line of a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ruling {
    /// The id as written: 64 hex, or a prefix of 12 or more.
    pub id: String,
    pub verdict: Verdict,
    pub clause: String,
}

fn ruling_line(line: &str) -> Option<Ruling> {
    let line = line.trim().trim_start_matches(['-', '*', '`', ' ']);
    let (id, rest) = line.split_once(' ')?;
    let id = id.trim_end_matches('`');
    if id.len() < MIN_PREFIX_HEX || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let (word, clause) = rest.trim_start().split_once(':')?;
    let verdict = match word.trim().trim_matches('*') {
        "violation" => Verdict::Violation,
        "fine" => Verdict::Fine,
        _ => return None,
    };
    Some(Ruling {
        id: id.to_ascii_lowercase(),
        verdict,
        clause: clause.trim().trim_start_matches('*').trim().to_string(),
    })
}

/// The rulings of the `Candidates:` block of a result: the consecutive
/// ruling lines after the header, blank lines between them allowed.
pub fn parse_rulings(result: &str) -> Vec<Ruling> {
    let mut lines = result.lines().map(str::trim);
    let is_header = |line: &str| {
        line.trim_matches(['*', '#', ' '])
            .trim_end_matches(':')
            .trim_matches('*')
            .eq_ignore_ascii_case("Candidates")
    };
    if !lines.any(is_header) {
        return Vec::new();
    }
    let mut rulings = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        match ruling_line(line) {
            Some(ruling) => rulings.push(ruling),
            None => break,
        }
    }
    rulings
}

/// Everything the push check keeps of one review's candidates: the agent
/// that wrote them, what its prompt listed and what its result ruled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reviewed {
    pub review: String,
    pub prompted: Prompted,
    pub rulings: Vec<Ruling>,
}

/// A listed candidate with the reviewer's ruling on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disposed {
    pub listed: Listed,
    pub verdict: Verdict,
    pub clause: String,
}

impl Reviewed {
    pub fn read(agent_id: Option<&str>, prompt: &str, result: &str) -> Reviewed {
        Reviewed {
            review: agent_id.unwrap_or(UNKNOWN_REVIEW).to_string(),
            prompted: parse_prompt(prompt),
            rulings: parse_rulings(result),
        }
    }

    /// A pass from before candidates were stored, or one a person overruled.
    pub fn not_recorded() -> Reviewed {
        Reviewed {
            review: UNKNOWN_REVIEW.to_string(),
            prompted: Prompted::NotRecorded,
            rulings: Vec::new(),
        }
    }

    /// The ruling on each listed candidate that has one. A ruling names its
    /// candidate by the full id or by a prefix that only one listed id has.
    pub fn disposed(&self) -> Vec<Disposed> {
        let Prompted::Block(listed) = &self.prompted else {
            return Vec::new();
        };
        listed
            .iter()
            .filter_map(|entry| {
                let ruling = self.rulings.iter().find(|ruling| {
                    entry.id.starts_with(&ruling.id)
                        && listed
                            .iter()
                            .filter(|other| other.id.starts_with(&ruling.id))
                            .count()
                            == 1
                })?;
                Some(Disposed {
                    listed: entry.clone(),
                    verdict: ruling.verdict,
                    clause: ruling.clause.clone(),
                })
            })
            .collect()
    }

    /// The journal excerpt for how the reviewer disposed of the listed ids.
    pub fn excerpt(&self) -> String {
        match &self.prompted {
            Prompted::Unlisted => "review=candidates-unlisted".to_string(),
            Prompted::NotRecorded => "review=candidates-unknown".to_string(),
            Prompted::Unavailable => "review=candidates-unavailable".to_string(),
            Prompted::Block(listed) => {
                let undisposed = listed.len() - self.disposed().len();
                if undisposed == 0 {
                    format!("review=candidates-ruled n={}", listed.len())
                } else {
                    format!(
                        "review=candidates-undisposed n={undisposed}/{}",
                        listed.len()
                    )
                }
            }
        }
    }

    /// The stored form, kept in a pass file.
    pub fn to_json(&self) -> serde_json::Value {
        let (state, listed) = match &self.prompted {
            Prompted::Unlisted => ("unlisted", Vec::new()),
            Prompted::NotRecorded => ("not-recorded", Vec::new()),
            Prompted::Unavailable => ("unavailable", Vec::new()),
            Prompted::Block(listed) => (
                "block",
                listed
                    .iter()
                    .map(|entry| {
                        serde_json::json!({
                            "id": entry.id,
                            "rule": entry.rule,
                            "pattern_id": entry.pattern_id,
                            "path": entry.path,
                            "line": entry.line,
                            "pattern": entry.pattern,
                        })
                    })
                    .collect(),
            ),
        };
        serde_json::json!({
            "review": self.review,
            "state": state,
            "listed": listed,
            "rulings": self.rulings.iter().map(|ruling| serde_json::json!({
                "id": ruling.id,
                "ruling": ruling.verdict.as_str(),
                "clause": ruling.clause,
            })).collect::<Vec<_>>(),
        })
    }

    /// Reads the stored form; a pass from before it existed reads as
    /// [`Reviewed::not_recorded`].
    pub fn from_json(stored: Option<&serde_json::Value>) -> Reviewed {
        let Some(stored) = stored.filter(|value| !value.is_null()) else {
            return Reviewed::not_recorded();
        };
        let field = |value: &serde_json::Value, name: &str| {
            value
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        let prompted = match field(stored, "state").as_deref() {
            Some("unavailable") => Prompted::Unavailable,
            Some("not-recorded") => Prompted::NotRecorded,
            Some("block") => Prompted::Block(
                stored
                    .get("listed")
                    .and_then(serde_json::Value::as_array)
                    .map(|rows| {
                        rows.iter()
                            .filter_map(|row| {
                                Some(Listed {
                                    id: field(row, "id")?,
                                    rule: field(row, "rule")?,
                                    pattern_id: field(row, "pattern_id")?,
                                    path: field(row, "path")?,
                                    line: row
                                        .get("line")
                                        .and_then(serde_json::Value::as_u64)
                                        .and_then(|n| usize::try_from(n).ok())?,
                                    pattern: field(row, "pattern"),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            Some("unlisted") => Prompted::Unlisted,
            // A state this reader does not know says nothing it can trust.
            _ => Prompted::NotRecorded,
        };
        let rulings = stored
            .get("rulings")
            .and_then(serde_json::Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let verdict = match field(row, "ruling")?.as_str() {
                            "violation" => Verdict::Violation,
                            "fine" => Verdict::Fine,
                            _ => return None,
                        };
                        Some(Ruling {
                            id: field(row, "id")?,
                            verdict,
                            clause: field(row, "clause")?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Reviewed {
            review: field(stored, "review").unwrap_or_else(|| UNKNOWN_REVIEW.to_string()),
            prompted,
            rulings,
        }
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` for a count of seconds since the epoch.
pub fn rfc3339_utc(epoch_seconds: i64) -> String {
    let (year, month, day) =
        amont_agent_guard::civil::civil_from_days(epoch_seconds.div_euclid(86_400));
    let in_day = epoch_seconds.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        in_day / 3600,
        in_day % 3600 / 60,
        in_day % 60
    )
}

fn now_rfc3339() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    rfc3339_utc(seconds)
}

/// The (review, candidate) pairs the file already holds, read once per push.
/// A file that is missing holds nothing; a file that cannot be read, or a
/// line that cannot be parsed, is an error, so nothing is appended blind.
fn recorded_pairs(file: &Path) -> std::io::Result<BTreeSet<(String, String)>> {
    let mut pairs = BTreeSet::new();
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(pairs),
        Err(error) => {
            return Err(std::io::Error::new(
                error.kind(),
                format!("rulings file {} cannot be read: {error}", file.display()),
            ))
        }
    };
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let record = serde_json::from_str::<serde_json::Value>(line).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "rulings file {} has a line that cannot be parsed: {error}",
                    file.display()
                ),
            )
        })?;
        let text_of = |name: &str| {
            record
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        if let (Some(review), Some(candidate)) = (text_of("review"), text_of("candidate")) {
            pairs.insert((review, candidate));
        }
    }
    Ok(pairs)
}

/// Appends one `adr-lint.ruling/1` record per disposed candidate to
/// `<journal dir>/rulings/<repo>.jsonl`, skipping a (review, candidate) the
/// file already holds. Returns the number appended. The file is created
/// 0600 in a 0700 directory and only ever appended to.
pub fn append_rulings(repo: &str, tree: &str, reviewed: &Reviewed) -> std::io::Result<usize> {
    let disposed = reviewed.disposed();
    if disposed.is_empty() {
        return Ok(0);
    }
    if repo.is_empty() || repo.contains(['/', '\\']) || repo.contains("..") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("`{repo}` is not a name for a rulings file"),
        ));
    }
    let dir = journal::dir()
        .ok_or_else(|| std::io::Error::other("no amont-agent config directory"))?
        .join(RULINGS_DIR);
    std::fs::create_dir_all(&dir)?;
    journal::private(&dir, 0o700);
    let file = dir.join(format!("{repo}.jsonl"));
    let mut recorded = recorded_pairs(&file)?;
    let mut appended = 0;
    for item in disposed {
        if !recorded.insert((reviewed.review.clone(), item.listed.id.clone())) {
            continue;
        }
        let record = serde_json::json!({
            "format": RULING_FORMAT,
            "review": reviewed.review,
            "tool": TOOL,
            "repo": repo,
            "tree": tree,
            "candidate": item.listed.id,
            "rule": item.listed.rule,
            "pattern_id": item.listed.pattern_id,
            "pattern": item.listed.pattern.clone().unwrap_or_else(|| item.listed.rule.clone()),
            "ruling": item.verdict.as_str(),
            "clause": item.clause,
            "at": now_rfc3339(),
        });
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut open = options.open(&file)?;
        journal::private(&file, 0o600);
        // One write per record, so a concurrent append never tears a line.
        open.write_all(format!("{record}\n").as_bytes())?;
        appended += 1;
    }
    Ok(appended)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID_A: &str = "aaaaaaaaaaaa1111111111111111111111111111111111111111111111111111";
    const ID_B: &str = "aaaaaaaaaaaa2222222222222222222222222222222222222222222222222222";
    const ID_C: &str = "bbbbbbbbbbbb3333333333333333333333333333333333333333333333333333";

    fn prompt(ids: &[&str]) -> String {
        let mut out = format!(
            "{BLOCK_OPEN}{} n={}{BLOCK_CLOSE}\n",
            "0".repeat(64),
            ids.len()
        );
        for id in ids {
            out.push_str(&format!("{id} names.reveal-intent p1 src/a.rs:3\n"));
        }
        out.push_str(BLOCK_END);
        out.push('\n');
        out
    }

    #[test]
    fn a_prompt_block_lists_its_candidates_and_a_short_block_is_ignored() {
        let Prompted::Block(listed) = parse_prompt(&prompt(&[ID_A, ID_C])) else {
            panic!("a block");
        };
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[1].id, ID_C);
        assert_eq!(listed[1].pattern_id, "p1");
        let truncated = prompt(&[ID_A]).replace("n=1", "n=2");
        assert_eq!(parse_prompt(&truncated), Prompted::Unlisted);
        assert_eq!(parse_prompt("no block"), Prompted::Unlisted);
        assert_eq!(
            parse_prompt(&format!("x\n{UNAVAILABLE}\n")),
            Prompted::Unavailable
        );
    }

    #[test]
    fn a_ruling_matches_by_full_id_or_a_unique_prefix_only() {
        let reviewed = Reviewed::read(
            Some("a1"),
            &prompt(&[ID_A, ID_B, ID_C]),
            &format!(
                "Verdict: approve\n\nCandidates:\n{ID_A} fine: \"No placeholder\"\n- `bbbbbbbbbbbb` violation: bad name\naaaaaaaaaaaa fine: ambiguous\n"
            ),
        );
        let disposed = reviewed.disposed();
        let ids: Vec<&str> = disposed.iter().map(|d| d.listed.id.as_str()).collect();
        assert_eq!(ids, vec![ID_A, ID_C]);
        assert_eq!(disposed[1].verdict, Verdict::Violation);
        assert_eq!(disposed[1].clause, "bad name");
        assert_eq!(reviewed.excerpt(), "review=candidates-undisposed n=1/3");
    }

    #[test]
    fn every_listed_id_ruled_is_ruled_and_zero_listed_is_ruled_too() {
        let ruled = Reviewed::read(
            None,
            &prompt(&[ID_A]),
            &format!("Candidates:\n{ID_A} fine: x"),
        );
        assert_eq!(ruled.excerpt(), "review=candidates-ruled n=1");
        let none = Reviewed::read(None, &prompt(&[]), "Verdict: approve");
        assert_eq!(none.excerpt(), "review=candidates-ruled n=0");
    }

    #[test]
    fn the_stored_form_round_trips() {
        let reviewed = Reviewed::read(
            Some("a9"),
            &prompt(&[ID_A]),
            &format!("Candidates:\n{ID_A} violation: why"),
        );
        let back = Reviewed::from_json(Some(&reviewed.to_json()));
        assert_eq!(back, reviewed);
        assert_eq!(
            Reviewed::from_json(None).excerpt(),
            "review=candidates-unknown"
        );
        assert_eq!(
            Reviewed::from_json(Some(&serde_json::Value::Null)).prompted,
            Prompted::NotRecorded
        );
    }

    #[test]
    fn a_document_with_another_format_token_is_refused_and_unknown_fields_are_ignored() {
        let wrong = br#"{"format":"adr-lint.candidates/2","candidates":[]}"#;
        assert!(parse_document(wrong).unwrap_err().contains("candidates/2"));
        let row = format!(
            r#"{{"id":"{ID_A}","rule":"r","pattern_id":"p","pattern":"m","path":"a","line":1,"col":2,"text":"t","extra":1}}"#
        );
        let good = format!(
            r#"{{"format":"{DOCUMENT_FORMAT}","base":"b","head":"h","future":true,"candidates":[{row}],"overflow":{{"r":3}}}}"#
        );
        let document = parse_document(good.as_bytes()).unwrap();
        assert_eq!(document.candidates.len(), 1);
        assert_eq!(document.overflow["r"], 3);
        assert!(section(&document).contains("rule r"));
    }

    #[test]
    fn a_path_with_spaces_round_trips_through_the_block() {
        let row = format!(
            r#"{{"id":"{ID_A}","rule":"r.x","pattern_id":"p","pattern":"m","path":"src/my file.rs","line":7,"col":2,"text":"t"}}"#
        );
        let good = format!(r#"{{"format":"{DOCUMENT_FORMAT}","candidates":[{row}]}}"#);
        let document = parse_document(good.as_bytes()).unwrap();
        let brief = format!("{}\n\n{}", block(&document), section(&document));
        let Prompted::Block(listed) = parse_prompt(&brief) else {
            panic!("a block");
        };
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, ID_A);
        assert_eq!(listed[0].rule, "r.x");
        assert_eq!(listed[0].pattern_id, "p");
        assert_eq!(listed[0].path, "src/my file.rs");
        assert_eq!(listed[0].line, 7);
    }

    #[test]
    fn the_section_and_the_block_agree_on_the_pattern_message() {
        let row = format!(
            r#"{{"id":"{ID_A}","rule":"r.x","pattern_id":"p","pattern":"a message","path":"a.rs","line":1,"col":2,"text":"t"}}"#
        );
        let good = format!(r#"{{"format":"{DOCUMENT_FORMAT}","candidates":[{row}]}}"#);
        let document = parse_document(good.as_bytes()).unwrap();
        let brief = format!("{}\n\n{}", block(&document), section(&document));
        let Prompted::Block(listed) = parse_prompt(&brief) else {
            panic!("a block");
        };
        assert_eq!(listed[0].pattern.as_deref(), Some("a message"));
    }

    #[test]
    fn a_time_is_rfc3339_utc() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_000_000_000), "2001-09-09T01:46:40Z");
    }
}
