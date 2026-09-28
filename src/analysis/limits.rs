//! Deterministic bounds on the analysis itself.
//!
//! The hook runs on every Bash call. A tiny command can still describe an
//! enormous amount of work — `{1..999999999999}`, a thousand nested
//! parentheses, functions calling functions — so every recursive or repeated
//! step draws on a budget, and exhausting it ends the analysis with an
//! explicit `Incomplete`, never a hang, never a guess. Cardinalities (brace
//! ranges, `seq`, curl URL ranges) are computed arithmetically and never
//! materialized, so they cost nothing here.

/// Maximum syntactic nesting the frontend descends into.
pub const MAX_DEPTH: usize = 64;
/// Maximum IR nodes the frontend builds.
pub const MAX_NODES: usize = 20_000;
/// Maximum interpreter steps (one per command node visited).
pub const MAX_STEPS: usize = 200_000;
/// Maximum nested function calls being analysed at once.
pub const MAX_CALL_DEPTH: usize = 16;
/// Maximum rounds of a loop's state fixpoint before values are widened.
pub const WIDEN_AFTER: usize = 3;
/// Maximum rounds before a fixpoint gives up (after widening).
pub const MAX_ROUNDS: usize = 8;
/// Maximum distinct variables tracked; past this, new ones stay unknown.
pub const MAX_VARS: usize = 512;

/// Why an analysis stopped short.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exhausted {
    Depth,
    Nodes,
    Steps,
    CallDepth,
    Recursion,
}

impl Exhausted {
    pub fn describe(self) -> &'static str {
        match self {
            Exhausted::Depth => "nesting deeper than the analysis follows",
            Exhausted::Nodes => "a command larger than the analysis reads",
            Exhausted::Steps => "more work than the analysis budget allows",
            Exhausted::CallDepth => "function calls nested deeper than the analysis follows",
            Exhausted::Recursion => "a recursive function",
        }
    }
}

/// A countdown shared by one analysis.
#[derive(Debug, Clone)]
pub struct Budget {
    steps: usize,
}

impl Budget {
    pub fn new() -> Budget {
        Budget { steps: MAX_STEPS }
    }

    /// Take one step, or report that the budget is spent.
    pub fn step(&mut self) -> Result<(), Exhausted> {
        if self.steps == 0 {
            return Err(Exhausted::Steps);
        }
        self.steps -= 1;
        Ok(())
    }
}

impl Default for Budget {
    fn default() -> Budget {
        Budget::new()
    }
}
