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
//!
//! The parser is scannerless: shell tokenisation depends on where the parser
//! is (`do` is a keyword in command position and a word after `echo`, `)`
//! ends a subshell and a `case` pattern), so one cursor over the original
//! bytes does both jobs. That is also what makes every span — including
//! those inside substitutions, which are parsed in place rather than copied
//! out — an offset into the string the user wrote.
//!
//! A list of one command is that command, not a one-element
//! [`Cmd::Seq`](super::ir::Cmd::Seq); an empty source is an empty `Seq`.

mod arith;
mod parse;
pub mod test_expr;
mod word;

#[cfg(test)]
mod tests;

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
    parse::Parser::new(src).parse_program()
}
