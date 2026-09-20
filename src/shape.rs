//! A command's SHAPE: what it *is*, with what it was *about* taken out.
//!
//! Mining asks a question backtesting cannot: not "how often would this rule
//! have fired" but "what is the model getting wrong that no rule names yet".
//! That needs commands grouped, and two commands only group together if the
//! things that differ between them — a branch name, a path, a line number, a
//! commit message — stop being part of their identity.
//!
//! So a shape is the [`crate::shell`] parse with the literals masked:
//!
//! ```text
//!   git push origin feat/mine 2>&1 | tail -5   ─┐
//!   git push origin fix/lint  2>&1 | tail -20  ─┴→  git push origin <word> 2>&1 | tail -5
//! ```
//!
//! Three decisions, each of which the alternative got wrong when tried:
//!
//! 1. **It is built from the parse, never from the text.** The same reason
//!    rules are: a `--force` inside a commit message is not a flag, and a
//!    `| tail` in a different clause is not a pipe into tail. A shape derived
//!    from a regex over the string groups those together and every number
//!    downstream is then about a fiction.
//!
//! 2. **Flags are sorted, operands are not.** `git commit -a -m x` and
//!    `git commit -m x -a` are one habit written two ways, so the flag SET is
//!    the identity. Operand ORDER is not interchangeable in the same way —
//!    `cp notes.md backup` is not `cp backup notes.md` — so it is kept.
//!
//! 3. **A verb-taking program keeps its first two verbs.** Masking every
//!    literal collapses `git stash pop` into `git stash list`, and those are
//!    not one habit. So for the programs in [`VERB_PROGRAMS`] the first two
//!    bare-word operands survive — `git worktree remove`, `kubectl get pods`,
//!    `gh pr merge` — while the branch, the path and the number that follow
//!    are masked like anything else.
//!
//!    Everywhere else every operand is masked, because there the leading
//!    operand is DATA: keeping it would put `grep needle f.rs` and
//!    `grep other f.rs` in different groups, and a mining report fragmented
//!    one pattern per group has no support left to threshold on. The cost is
//!    the honest one — a verb-taking program missing from the table has its
//!    verbs folded together, which shows up as a group whose samples do two
//!    different things. Add it to the table when it does.
//!
//! What is left over — the difference between `git checkout main` and
//! `git checkout develop` — is what [`distance`] is for. It is a token-level
//! edit distance, normalised, so "same program, one word apart" is a small
//! number and "a different program entirely" is 1.0.
//!
//! Nothing here is on the hook path. A learned matcher never decides whether
//! a command runs: mining proposes, a human writes the rule by hand, and the
//! rule is the deterministic thing that ships.

use crate::shell::{Connector, Parsed, Simple, Word};

/// Two shapes this close or closer are the same habit written twice.
///
/// One token in three may differ, which is exactly "same program, same flags,
/// one literal changed" for a three-token command and proportionally looser
/// for a longer one. Above it, `git checkout main` and `git commit -m x` stop
/// being confusable, which is the property the threshold exists to hold.
pub const NEAR: f64 = 0.34;

/// Past this many tokens a command is a script, and the quadratic edit
/// distance below stops being free. Comparing the first 48 tokens of two
/// scripts answers the question just as well as comparing all 300.
const MAX_TOKENS: usize = 48;

/// Programs whose leading operands are VERBS rather than data.
///
/// The test for membership is not "has subcommands" but "does the shape lose
/// its meaning without them": `git stash pop` and `git stash list` are two
/// habits, `grep alpha` and `grep beta` are one. Every entry here was in the
/// measured corpus; a program nobody runs does not need to be in a table.
const VERB_PROGRAMS: &[&str] = &[
    "amont",
    "amont-agent",
    "argocd",
    "aws",
    "az",
    "brew",
    "bun",
    "cargo",
    "deno",
    "docker",
    "flux",
    "gcloud",
    "gh",
    "git",
    "glab",
    "go",
    "helm",
    "just",
    "kubectl",
    "launchctl",
    "make",
    "npm",
    "pip",
    "pip3",
    "pnpm",
    "podman",
    "rustup",
    "systemctl",
    "terraform",
    "tofu",
    "uv",
    "yarn",
];

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Shape {
    tokens: Vec<String>,
}

impl Shape {
    /// The shape of an already-parsed command.
    ///
    /// Clauses that ran inside a `$(…)` are left out. They are read as clauses
    /// of their own by the lexer and appended after the line's, so including
    /// them would put a substitution's insides after the line that contains
    /// it — a shape no reader could match back to a command they wrote.
    pub fn of(parsed: &Parsed) -> Shape {
        let clauses: Vec<&Simple> = parsed
            .clauses()
            .iter()
            .filter(|c| c.nested.is_none())
            .collect();
        let mut tokens = Vec::new();
        for (i, c) in clauses.iter().enumerate() {
            if i > 0 {
                if let Some(conn) = clauses[i - 1].next {
                    tokens.push(connector(conn).to_string());
                }
            }
            if c.opaque.is_some() {
                // Named rather than dropped: a shape that silently omits the
                // pipeline nobody could read would group a command with the
                // one that has that pipeline missing.
                tokens.push("<unread>".to_string());
                continue;
            }
            stage(c, &mut tokens);
        }
        Shape { tokens }
    }

    /// The shape of a command string.
    pub fn of_command(src: &str) -> Shape {
        Shape::of(&crate::shell::lex(src))
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// The canonical, human-readable spelling. This is what a report prints
    /// and what a group is keyed on, so it has to round-trip through a
    /// terminal unchanged — hence plain words and no colour.
    pub fn text(&self) -> String {
        self.tokens.join(" ")
    }
}

fn connector(c: Connector) -> &'static str {
    match c {
        Connector::Pipe => "|",
        Connector::AndAnd => "&&",
        Connector::OrOr => "||",
        Connector::Semi => ";",
        Connector::Amp => "&",
    }
}

/// One pipeline stage: program, operands in order, flags sorted, redirects.
fn stage(c: &Simple, out: &mut Vec<String>) {
    let Some(prog) = c.program() else {
        // `FOO=1` on its own, or an empty clause. It ran; it has no verb.
        out.push("<assign>".to_string());
        return;
    };
    let name = basename(prog);
    out.push(name.to_string());

    // Two bare words survive where they are verbs; elsewhere the leading
    // operand is data and is masked like the rest. See the module note.
    let verbs = if VERB_PROGRAMS.contains(&name) { 2 } else { 0 };
    let mut kept = 0usize;
    for w in c.operands() {
        if kept < verbs && is_bare_word(w) {
            out.push(w.text.clone());
            kept += 1;
        } else {
            out.push(mask(w).to_string());
        }
    }

    let mut flags: Vec<String> = Vec::new();
    for w in c.args() {
        if !w.quoted && w.text == "--" {
            break;
        }
        // A quoted word is never a flag — the same decision `has_flag` makes,
        // and the one that keeps `gh pr create --body "…--auto…"` honest.
        if w.quoted || w.expanded {
            continue;
        }
        let t = w.text.as_str();
        if t.len() < 2 || !t.starts_with('-') {
            continue;
        }
        // `--flag=value` keeps the flag and drops the value: the value is a
        // literal like any other, and `--include=*.ts` and `--include=*.rs`
        // are one habit.
        let name = match t.find('=') {
            Some(k) => format!("{}=", &t[..k]),
            None => t.to_string(),
        };
        if !flags.contains(&name) {
            flags.push(name);
        }
    }
    flags.sort();
    out.extend(flags);

    for (op, target) in &c.redirects {
        out.push(op.clone());
        // `2>&1` carries its target inside the operator and leaves the word
        // empty; `> out.log` does not.
        if !target.raw.is_empty() {
            out.push(mask(target).to_string());
        }
    }
    if c.heredoc {
        out.push("<<".to_string());
    }
}

/// The command name without the path it was found at, so `/usr/bin/git` and
/// `git` are one program rather than two.
fn basename(prog: &str) -> &str {
    prog.rsplit('/').next().unwrap_or(prog)
}

fn is_bare_word(w: &Word) -> bool {
    !w.quoted
        && !w.expanded
        && !w.text.starts_with('-')
        && w.text.len() <= 32
        && w.text.chars().any(|c| c.is_ascii_alphabetic())
        && w.text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What a literal becomes. The order of the tests is load-bearing: a glob is
/// usually also a path (`src/*.rs`), and a version number is usually also a
/// word.
fn mask(w: &Word) -> &'static str {
    if w.expanded {
        return "<sub>";
    }
    if w.quoted {
        return "<str>";
    }
    let t = w.text.as_str();
    if t.is_empty() {
        return "<str>";
    }
    if t.contains("://") {
        return "<url>";
    }
    if t.chars().any(|c| matches!(c, '*' | '?' | '[')) {
        return "<glob>";
    }
    if t.chars().any(|c| c.is_ascii_digit()) && !t.chars().any(|c| c.is_ascii_alphabetic()) {
        return "<n>";
    }
    if t.contains('/') || t.starts_with('.') || t.starts_with('~') {
        return "<path>";
    }
    if t.rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && (1..=5).contains(&ext.len()))
    {
        return "<path>";
    }
    "<word>"
}

/// How far apart two shapes are: 0.0 identical, 1.0 nothing in common.
///
/// Token-level Levenshtein over the canonical token lists, divided by the
/// longer of the two. Token-level rather than character-level on purpose: a
/// character distance says `git push` and `git pull` are nearly the same
/// command, which is the one pair a correction detector must never confuse.
pub fn distance(a: &Shape, b: &Shape) -> f64 {
    let x = &a.tokens[..a.tokens.len().min(MAX_TOKENS)];
    let y = &b.tokens[..b.tokens.len().min(MAX_TOKENS)];
    let longest = x.len().max(y.len());
    if longest == 0 {
        return 0.0;
    }
    levenshtein(x, y) as f64 / longest as f64
}

fn levenshtein(a: &[String], b: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ai) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, bj) in b.iter().enumerate() {
            let cost = usize::from(ai != bj);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Pick `n` candidates as unlike each other, and unlike what is already
/// reviewed, as possible.
///
/// Greedy max-min ("farthest point"): each round takes the candidate whose
/// nearest already-chosen neighbour is furthest away. That is the selection
/// that moves a precision estimate most per case a human labels — twenty
/// spellings of the same command teach nothing after the first, while twenty
/// unlike ones are twenty chances for the rule to be wrong in a new way.
///
/// `seeds` are the shapes already labelled. They count as chosen without
/// being returned, so a second review pass does not hand back the first
/// pass's cases.
///
/// Ties go to the earliest candidate, so the answer does not depend on how
/// the transcripts happened to sort.
pub fn pick_novel(candidates: &[Shape], seeds: &[Shape], n: usize) -> Vec<usize> {
    let mut nearest: Vec<f64> = candidates
        .iter()
        .map(|c| {
            seeds
                .iter()
                .map(|s| distance(c, s))
                .fold(f64::INFINITY, f64::min)
        })
        .collect();
    let mut chosen = Vec::new();
    while chosen.len() < n.min(candidates.len()) {
        let mut best = usize::MAX;
        let mut best_d = f64::NEG_INFINITY;
        for (i, d) in nearest.iter().enumerate() {
            if chosen.contains(&i) {
                continue;
            }
            if *d > best_d {
                best_d = *d;
                best = i;
            }
        }
        if best == usize::MAX {
            break;
        }
        chosen.push(best);
        for (i, d) in nearest.iter_mut().enumerate() {
            *d = d.min(distance(&candidates[i], &candidates[best]));
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(src: &str) -> String {
        Shape::of_command(src).text()
    }

    #[test]
    fn literals_are_masked_and_the_verb_survives() {
        assert_eq!(
            shape("git push origin feat/mine 2>&1 | tail -5"),
            "git push origin <path> 2>&1 | tail -5"
        );
        assert_eq!(shape("git stash pop"), "git stash pop");
        assert_eq!(shape("git stash list"), "git stash list");
    }

    /// The whole point of grouping: two runs of one habit are one shape.
    #[test]
    fn two_spellings_of_one_habit_are_one_shape() {
        assert_eq!(
            shape("grep -rn needle src/foo.rs"),
            shape("grep -rn other src/bar.rs")
        );
        assert_eq!(shape("sed -n '1,80p' a.rs"), shape("sed -n '4,9p' b.rs"));
        assert_eq!(
            shape("git worktree add ../a -b feat/x"),
            shape("git worktree add ../b -b fix/y")
        );
    }

    /// The table is the difference between a verb and a needle. Without it,
    /// every distinct search pattern was a group of its own and the support
    /// threshold had nothing left to stand on.
    #[test]
    fn a_verb_survives_and_a_search_pattern_does_not() {
        assert_ne!(shape("git stash pop"), shape("git stash list"));
        assert_eq!(shape("grep alpha"), shape("grep beta"));
    }

    /// Flag order is not identity; operand order is.
    #[test]
    fn flags_sort_and_operands_do_not() {
        assert_eq!(shape("git commit -a -m 'x'"), shape("git commit -m 'x' -a"));
        assert_ne!(shape("cp notes.md backup"), shape("cp backup notes.md"));
    }

    /// A flag's value is a literal like any other.
    #[test]
    fn a_flag_value_is_dropped_and_the_flag_is_kept() {
        assert_eq!(
            shape("rg --include=*.ts pat"),
            shape("rg --include=*.rs pat")
        );
        assert!(shape("rg --include=*.ts pat").contains("--include="));
    }

    /// The parse, not the text. A `--force` in a message is not a flag, and a
    /// `| tail` in a quoted string is not a pipe.
    #[test]
    fn a_flag_inside_a_message_is_not_part_of_the_shape() {
        assert_eq!(
            shape("git commit -m 'use --force here'"),
            shape("git commit -m 'nothing special'")
        );
        assert!(!shape("git commit -m 'use --force here'").contains("--force"));
    }

    #[test]
    fn a_pipeline_keeps_its_stages_and_its_connectors() {
        assert_eq!(
            shape("git status -s && git push | tail -2"),
            "git status -s && git push | tail -2"
        );
    }

    #[test]
    fn the_path_a_program_was_found_at_is_not_its_identity() {
        assert_eq!(shape("/usr/bin/git status"), shape("git status"));
    }

    #[test]
    fn an_unreadable_pipeline_is_named_rather_than_dropped() {
        let s = shape("git status -s | xargs git add && git push");
        assert!(s.contains("<unread>"), "{s}");
        assert_ne!(s, shape("git status -s && git push"));
    }

    #[test]
    fn distance_is_zero_for_one_shape_and_one_for_two_programs() {
        let a = Shape::of_command("git push origin main");
        let b = Shape::of_command("git push origin other");
        assert_eq!(distance(&a, &a), 0.0);
        assert!(distance(&a, &b) <= NEAR, "{}", distance(&a, &b));
        let c = Shape::of_command("kubectl apply -f x.yaml");
        assert!(distance(&a, &c) > NEAR);
    }

    /// The pair a token-level distance exists to keep apart. Character-level
    /// would call these a one-letter edit.
    #[test]
    fn push_and_pull_are_not_near_each_other() {
        let push = Shape::of_command("git push");
        let pull = Shape::of_command("git pull");
        assert!(distance(&push, &pull) > NEAR, "{}", distance(&push, &pull));
    }

    #[test]
    fn novelty_prefers_the_unlike_and_then_the_next_unlike() {
        let candidates: Vec<Shape> = [
            "git push origin main | tail -5",
            "git push origin other | tail -5",
            "kubectl apply -f a.yaml | tail -3",
            "npm publish | tail -1",
        ]
        .iter()
        .map(|c| Shape::of_command(c))
        .collect();
        let picked = pick_novel(&candidates, &[], 2);
        assert_eq!(picked.len(), 2);
        // The first two are near-duplicates: whichever is taken first, the
        // other must not be second.
        assert!(!(picked.contains(&0) && picked.contains(&1)), "{picked:?}");
    }

    /// A case already reviewed is not worth reviewing again.
    #[test]
    fn a_seed_pushes_its_own_neighbourhood_down_the_list() {
        let candidates: Vec<Shape> = ["git push origin main | tail -5", "npm publish | tail -1"]
            .iter()
            .map(|c| Shape::of_command(c))
            .collect();
        let seeds = vec![Shape::of_command("git push origin feat/x | tail -9")];
        assert_eq!(pick_novel(&candidates, &seeds, 1), vec![1]);
    }

    /// Determinism is the contract: the same input picks the same cases.
    #[test]
    fn the_pick_is_reproducible() {
        let candidates: Vec<Shape> = ["a b", "c d", "e f", "g h"]
            .iter()
            .map(|c| Shape::of_command(c))
            .collect();
        let once = pick_novel(&candidates, &[], 3);
        assert_eq!(once, pick_novel(&candidates, &[], 3));
    }
}
