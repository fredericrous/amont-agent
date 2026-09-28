//! What known network clients do with their arguments.
//!
//! Each model turns one invocation — the program and its arguments after
//! shell expansion, as abstract values — into the explicit command-line
//! transfers it makes: one [`Transfer`] per recognised destination. Anything
//! a model cannot establish stays explicit: an option it does not know makes
//! every later operand [`Unresolved::UnknownOption`], never a guessed URL.

use super::domain::Count;
use super::effects::{Auth, Factor, TargetSet, Unresolved};
use super::ir::Span;
use super::state::AbsVal;

/// One argument after expansion.
#[derive(Debug, Clone)]
pub struct Arg {
    /// Its value. When `fields` is more than one (an unquoted brace range,
    /// a split parameter), this is the join of the fields' values.
    pub val: AbsVal,
    /// How many shell words this argument expanded to.
    pub fields: Count,
    /// Where it was written.
    pub span: Span,
}

/// One destination one invocation reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub program: &'static str,
    /// Transfers to this destination per invocation.
    pub per_call: Count,
    pub targets: TargetSet,
    pub auth: Auth,
    /// Requests the client may make that are not counted.
    pub uncounted: Vec<&'static str>,
    /// Multipliers inside the invocation (`--retry`, a URL range), in order.
    pub builtin: Vec<Factor>,
    /// The operand (or the invocation) this transfer came from.
    pub span: Span,
}

/// The programs a model exists for — what the hook's prefilter looks for.
pub const PROGRAMS: &[&str] = &[];

/// The transfers `program args…` makes, or `None` when `program` is not a
/// network client the analysis models. `span` is the whole invocation.
pub fn model(program: &str, args: &[Arg], span: Span) -> Option<Vec<Transfer>> {
    let _ = (program, args, span, Unresolved::Dynamic);
    None
}
