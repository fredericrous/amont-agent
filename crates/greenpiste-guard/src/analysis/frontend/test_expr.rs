//! `[[ … ]]`, `[ … ]` and `test …` expressions to [`TestExpr`].
//!
//! The two syntaxes share one grammar with two differences: inside `[[`
//! the shell lexes `&&`, `||`, `(`, `)`, `<` and `>` as operators, while
//! `[` and `test` receive plain argv, where `-a`/`-o` are and/or and the
//! parentheses are words (written `\(` to get past the shell). A shape the
//! grammar does not recognise is [`TestExpr::Unknown`] as a whole, never a
//! partial reading.

use super::super::ir::{TestExpr, Word};
use super::super::limits::{Exhausted, MAX_DEPTH, MAX_NODES};

/// A `[[ … ]]` token as the command parser reads it.
#[derive(Debug, Clone)]
pub(super) enum CondTok {
    Word(Word),
    Op(&'static str),
}

const UNARY: &[&str] = &[
    "-a", "-b", "-c", "-d", "-e", "-f", "-g", "-h", "-k", "-p", "-r", "-s", "-t", "-u", "-w", "-x",
    "-G", "-L", "-N", "-O", "-S", "-z", "-n", "-o", "-v", "-R",
];

const BINARY: &[&str] = &[
    "=", "==", "!=", "<", ">", "-eq", "-ne", "-lt", "-le", "-gt", "-ge", "=~", "-nt", "-ot", "-ef",
];

#[derive(Clone, Copy)]
enum Tok<'a> {
    W(&'a Word),
    Op(&'static str),
}

/// Parse the tokens between `[[` and `]]`.
pub(super) fn parse_cond(
    toks: &[CondTok],
    depth: usize,
    nodes: &mut usize,
) -> Result<TestExpr, Exhausted> {
    let toks = toks
        .iter()
        .map(|t| match t {
            CondTok::Word(w) => Tok::W(w),
            CondTok::Op(op) => Tok::Op(op),
        })
        .collect();
    run(toks, false, depth, nodes)
}

fn run(
    toks: Vec<Tok<'_>>,
    bracket: bool,
    depth: usize,
    nodes: &mut usize,
) -> Result<TestExpr, Exhausted> {
    if toks.is_empty() {
        return Ok(TestExpr::Unknown);
    }
    let mut p = P {
        toks,
        i: 0,
        bracket,
        depth,
        nodes,
    };
    Ok(match p.or()? {
        Some(e) if p.i == p.toks.len() => e,
        _ => TestExpr::Unknown,
    })
}

struct P<'a, 'n> {
    toks: Vec<Tok<'a>>,
    i: usize,
    bracket: bool,
    depth: usize,
    nodes: &'n mut usize,
}

impl<'a> P<'a, '_> {
    fn lit(&self, k: usize) -> Option<String> {
        match self.toks.get(k) {
            Some(Tok::W(w)) => w.literal(),
            _ => None,
        }
    }

    fn is_op(&self, k: usize, op: &str) -> bool {
        matches!(self.toks.get(k), Some(Tok::Op(o)) if *o == op)
    }

    fn word(&self, k: usize) -> Option<&'a Word> {
        match self.toks.get(k) {
            Some(Tok::W(w)) => Some(w),
            _ => None,
        }
    }

    fn node(&mut self, e: TestExpr) -> Result<TestExpr, Exhausted> {
        *self.nodes += 1;
        if *self.nodes > MAX_NODES {
            return Err(Exhausted::Nodes);
        }
        Ok(e)
    }

    fn enter(&mut self) -> Result<(), Exhausted> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Exhausted::Depth);
        }
        Ok(())
    }

    fn is_and(&self) -> bool {
        self.is_op(self.i, "&&") || (self.bracket && self.lit(self.i).as_deref() == Some("-a"))
    }

    fn is_or(&self) -> bool {
        self.is_op(self.i, "||") || (self.bracket && self.lit(self.i).as_deref() == Some("-o"))
    }

    fn or(&mut self) -> Result<Option<TestExpr>, Exhausted> {
        let Some(mut lhs) = self.and()? else {
            return Ok(None);
        };
        while self.is_or() {
            self.i += 1;
            let Some(rhs) = self.and()? else {
                return Ok(None);
            };
            lhs = self.node(TestExpr::Or(Box::new(lhs), Box::new(rhs)))?;
        }
        Ok(Some(lhs))
    }

    fn and(&mut self) -> Result<Option<TestExpr>, Exhausted> {
        let Some(mut lhs) = self.not()? else {
            return Ok(None);
        };
        while self.is_and() {
            self.i += 1;
            let Some(rhs) = self.not()? else {
                return Ok(None);
            };
            lhs = self.node(TestExpr::And(Box::new(lhs), Box::new(rhs)))?;
        }
        Ok(Some(lhs))
    }

    fn not(&mut self) -> Result<Option<TestExpr>, Exhausted> {
        // `!` alone (nothing after it) is a non-empty word, not negation.
        if self.lit(self.i).as_deref() == Some("!") && self.i + 1 < self.toks.len() {
            self.enter()?;
            self.i += 1;
            let Some(inner) = self.not()? else {
                return Ok(None);
            };
            self.depth -= 1;
            return self.node(TestExpr::Not(Box::new(inner))).map(Some);
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Option<TestExpr>, Exhausted> {
        let i = self.i;
        if self.is_op(i, "(") {
            self.enter()?;
            self.i += 1;
            let Some(inner) = self.or()? else {
                return Ok(None);
            };
            if !self.is_op(self.i, ")") {
                return Ok(None);
            }
            self.i += 1;
            self.depth -= 1;
            return Ok(Some(inner));
        }
        let Some(left) = self.word(i) else {
            return Ok(None);
        };
        let binop = match self.toks.get(i + 1) {
            Some(Tok::Op(op @ ("<" | ">"))) => Some(op.to_string()),
            Some(Tok::W(_)) => self.lit(i + 1).filter(|op| BINARY.contains(&op.as_str())),
            _ => None,
        };
        if let (Some(op), Some(right)) = (binop, self.word(i + 2)) {
            self.i += 3;
            return self
                .node(TestExpr::Binary {
                    left: left.clone(),
                    op,
                    right: right.clone(),
                })
                .map(Some);
        }
        if let (Some(op), Some(operand)) = (
            self.lit(i).filter(|op| UNARY.contains(&op.as_str())),
            self.word(i + 1),
        ) {
            self.i += 2;
            return self
                .node(TestExpr::Unary {
                    op,
                    operand: operand.clone(),
                })
                .map(Some);
        }
        self.i += 1;
        self.node(TestExpr::Word(left.clone())).map(Some)
    }
}
