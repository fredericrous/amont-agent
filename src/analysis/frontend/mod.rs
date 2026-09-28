//! Shell source to [`ir`](super::ir): a recursive-descent parser for the
//! supported subset (see `docs/analysis.md`).
//!
//! Two promises, both load-bearing for everything downstream:
//!
//! - **Never guess.** A construct outside the subset becomes
//!   [`Cmd::Unsupported`](super::ir::Cmd::Unsupported) with its span; text
//!   the parser cannot make sense of at all is an [`Err`].
//! - **Delimiters belong to their execution context.** The `do` of a loop
//!   inside `$(…)` closes nothing outside it; `do for …` is a body that
//!   begins with a `for` command.

use super::ir::{Cmd, Span};
use super::limits::Exhausted;

/// Why the source could not be turned into IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Malformed or unterminated input.
    Syntax { why: &'static str, span: Span },
    /// A resource limit was reached while parsing.
    Limit(Exhausted),
}

/// Parse a whole command string.
pub fn parse(src: &str) -> Result<Cmd, ParseError> {
    let _ = src;
    Err(ParseError::Syntax {
        why: "the parser is not written yet",
        span: 0..src.len(),
    })
}
