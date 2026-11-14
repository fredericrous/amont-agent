//! `$(( … ))`, `(( … ))` and `for (( … ))` clauses to [`ArithExpr`].
//!
//! Precedence follows bash's table. What the analysis cannot use — array
//! subscripts, `${v:-0}`, a `$(…)` inside — becomes an
//! [`ArithExpr::Unknown`] leaf when it is a whole operand, and the whole
//! expression becomes `Unknown` when the text does not parse at all, so an
//! unrecognised operator never turns into a wrong but confident value.

use super::super::ir::{ArithBinOp, ArithExpr, ArithUnOp};
use super::super::limits::{Exhausted, MAX_DEPTH, MAX_NODES};

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(i64),
    Var(String),
    /// An operand the analysis does not model, as written.
    Opaque(String),
    Op(&'static str),
}

/// Longest first, so `<<=` is not read as `<<` then `=`.
const OPS: &[&str] = &[
    "<<=", ">>=", "**", "++", "--", "<<", ">>", "<=", ">=", "==", "!=", "&&", "||", "+=", "-=",
    "*=", "/=", "%=", "&=", "^=", "|=", "+", "-", "*", "/", "%", "<", ">", "=", "!", "~", "&", "^",
    "|", "?", ":", ",", "(", ")",
];

/// Parse arithmetic text. `depth` is the nesting already used by the
/// caller; `nodes` is the caller's node count, shared so the limit covers
/// the whole command.
pub(super) fn parse(src: &str, depth: usize, nodes: &mut usize) -> Result<ArithExpr, Exhausted> {
    let unknown = || ArithExpr::Unknown(src.trim().to_string());
    let Some(toks) = lex(src) else {
        return Ok(unknown());
    };
    if toks.is_empty() {
        // `$(( ))` is 0.
        return Ok(ArithExpr::Num(0));
    }
    let mut p = P {
        toks,
        i: 0,
        depth,
        nodes,
    };
    match p.expr(0)? {
        Some(e) if p.i == p.toks.len() => Ok(e),
        _ => Ok(unknown()),
    }
}

fn lex(src: &str) -> Option<Vec<Tok>> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < b.len()
                && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'#' | b'@' | b'_'))
            {
                i += 1;
            }
            let text = &src[start..i];
            out.push(match number(text) {
                Some(n) => Tok::Num(n),
                None => Tok::Opaque(text.to_string()),
            });
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            if b.get(i) == Some(&b'[') {
                i = close_of(b, i, b'[', b']')? + 1;
                out.push(Tok::Opaque(src[start..i].to_string()));
            } else {
                out.push(Tok::Var(src[start..i].to_string()));
            }
        } else if c == b'$' {
            let start = i;
            match b.get(i + 1) {
                Some(&n) if n.is_ascii_alphabetic() || n == b'_' => {
                    i += 1;
                    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                        i += 1;
                    }
                    out.push(Tok::Var(src[start + 1..i].to_string()));
                }
                Some(b'{') => {
                    i = close_of(b, i + 1, b'{', b'}')? + 1;
                    let inner = &src[start + 2..i - 1];
                    let ib = inner.as_bytes();
                    if !ib.is_empty()
                        && (ib[0].is_ascii_alphabetic() || ib[0] == b'_')
                        && ib.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
                    {
                        out.push(Tok::Var(inner.to_string()));
                    } else {
                        out.push(Tok::Opaque(src[start..i].to_string()));
                    }
                }
                Some(b'(') => {
                    i = close_of(b, i + 1, b'(', b')')? + 1;
                    out.push(Tok::Opaque(src[start..i].to_string()));
                }
                Some(_) => {
                    // `$1`, `$#`, `$?`: values the shell has, the analysis
                    // does not track as arithmetic variables.
                    i += 2;
                    out.push(Tok::Opaque(src[start..i].to_string()));
                }
                None => return None,
            }
        } else {
            let rest = &b[i..];
            let op = OPS.iter().find(|op| rest.starts_with(op.as_bytes()))?;
            out.push(Tok::Op(op));
            i += op.len();
        }
    }
    Some(out)
}

/// Index of the byte closing the bracket at `open`, by nesting count.
fn close_of(b: &[u8], open: usize, l: u8, r: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (k, &c) in b.iter().enumerate().skip(open) {
        if c == l {
            depth += 1;
        } else if c == r {
            depth -= 1;
            if depth == 0 {
                return Some(k);
            }
        }
    }
    None
}

/// Decimal, `0x` hex, leading-zero octal, and `base#digits` (base 2–64).
fn number(text: &str) -> Option<i64> {
    if let Some((base, digits)) = text.split_once('#') {
        let base: u32 = base.parse().ok()?;
        if !(2..=64).contains(&base) || digits.is_empty() {
            return None;
        }
        let mut n: i64 = 0;
        for c in digits.bytes() {
            let d = match c {
                b'0'..=b'9' => u32::from(c - b'0'),
                b'a'..=b'z' => u32::from(c - b'a') + 10,
                b'A'..=b'Z' if base <= 36 => u32::from(c - b'A') + 10,
                b'A'..=b'Z' => u32::from(c - b'A') + 36,
                b'@' => 62,
                b'_' => 63,
                _ => return None,
            };
            if d >= base {
                return None;
            }
            n = n.checked_mul(i64::from(base))?.checked_add(i64::from(d))?;
        }
        return Some(n);
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16).ok();
    }
    if text.len() > 1 && text.starts_with('0') {
        return i64::from_str_radix(&text[1..], 8).ok();
    }
    text.parse().ok()
}

struct P<'n> {
    toks: Vec<Tok>,
    i: usize,
    depth: usize,
    nodes: &'n mut usize,
}

/// Binding power of a binary operator (higher binds tighter) and whether
/// it groups to the right.
fn binary(op: &str) -> Option<(u8, bool, Option<ArithBinOp>)> {
    use ArithBinOp::*;
    Some(match op {
        "," => (1, false, None),
        "||" => (4, false, Some(Or)),
        "&&" => (5, false, Some(And)),
        "|" => (6, false, Some(BitOr)),
        "^" => (7, false, Some(BitXor)),
        "&" => (8, false, Some(BitAnd)),
        "==" => (9, false, Some(Eq)),
        "!=" => (9, false, Some(Ne)),
        "<" => (10, false, Some(Lt)),
        "<=" => (10, false, Some(Le)),
        ">" => (10, false, Some(Gt)),
        ">=" => (10, false, Some(Ge)),
        "<<" => (11, false, Some(Shl)),
        ">>" => (11, false, Some(Shr)),
        "+" => (12, false, Some(Add)),
        "-" => (12, false, Some(Sub)),
        "*" => (13, false, Some(Mul)),
        "/" => (13, false, Some(Div)),
        "%" => (13, false, Some(Rem)),
        "**" => (14, true, Some(Pow)),
        _ => return None,
    })
}

/// Assignment operators: plain `=` is `None`.
fn assignment(op: &str) -> Option<Option<ArithBinOp>> {
    use ArithBinOp::*;
    Some(match op {
        "=" => None,
        "+=" => Some(Add),
        "-=" => Some(Sub),
        "*=" => Some(Mul),
        "/=" => Some(Div),
        "%=" => Some(Rem),
        "<<=" => Some(Shl),
        ">>=" => Some(Shr),
        "&=" => Some(BitAnd),
        "^=" => Some(BitXor),
        "|=" => Some(BitOr),
        _ => return None,
    })
}

const ASSIGN_BP: u8 = 2;
const TERNARY_BP: u8 = 3;
const UNARY_BP: u8 = 15;

impl P<'_> {
    fn peek_op(&self) -> Option<&'static str> {
        match self.toks.get(self.i) {
            Some(Tok::Op(op)) => Some(op),
            _ => None,
        }
    }

    fn node(&mut self, e: ArithExpr) -> Result<ArithExpr, Exhausted> {
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

    /// Precedence climbing. `Ok(None)` is text that is not an expression.
    fn expr(&mut self, min_bp: u8) -> Result<Option<ArithExpr>, Exhausted> {
        self.enter()?;
        let Some(mut lhs) = self.prefix()? else {
            return Ok(None);
        };
        while let Some(op) = self.peek_op() {
            if let Some(kind) = assignment(op) {
                if ASSIGN_BP < min_bp {
                    break;
                }
                let ArithExpr::Var(var) = lhs else {
                    return Ok(None);
                };
                self.i += 1;
                let Some(value) = self.expr(ASSIGN_BP)? else {
                    return Ok(None);
                };
                lhs = self.node(ArithExpr::Assign {
                    var,
                    op: kind,
                    value: Box::new(value),
                })?;
                continue;
            }
            if op == "?" {
                if TERNARY_BP < min_bp {
                    break;
                }
                self.i += 1;
                let Some(then) = self.expr(ASSIGN_BP)? else {
                    return Ok(None);
                };
                if self.peek_op() != Some(":") {
                    return Ok(None);
                }
                self.i += 1;
                let Some(other) = self.expr(TERNARY_BP)? else {
                    return Ok(None);
                };
                lhs = self.node(ArithExpr::Ternary(
                    Box::new(lhs),
                    Box::new(then),
                    Box::new(other),
                ))?;
                continue;
            }
            let Some((bp, right, kind)) = binary(op) else {
                break;
            };
            if bp < min_bp {
                break;
            }
            self.i += 1;
            let Some(rhs) = self.expr(if right { bp } else { bp + 1 })? else {
                return Ok(None);
            };
            lhs = self.node(match kind {
                Some(k) => ArithExpr::Binary(k, Box::new(lhs), Box::new(rhs)),
                None => ArithExpr::Comma(Box::new(lhs), Box::new(rhs)),
            })?;
        }
        self.depth -= 1;
        Ok(Some(lhs))
    }

    fn prefix(&mut self) -> Result<Option<ArithExpr>, Exhausted> {
        let Some(tok) = self.toks.get(self.i).cloned() else {
            return Ok(None);
        };
        self.i += 1;
        let e = match tok {
            Tok::Num(n) => ArithExpr::Num(n),
            Tok::Var(v) => {
                return match self.peek_op() {
                    Some(op @ ("++" | "--")) => {
                        self.i += 1;
                        self.node(ArithExpr::IncDec {
                            var: v,
                            delta: if op == "++" { 1 } else { -1 },
                            prefix: false,
                        })
                        .map(Some)
                    }
                    _ => self.node(ArithExpr::Var(v)).map(Some),
                };
            }
            Tok::Opaque(s) => ArithExpr::Unknown(s),
            Tok::Op("(") => {
                let Some(inner) = self.expr(0)? else {
                    return Ok(None);
                };
                if self.peek_op() != Some(")") {
                    return Ok(None);
                }
                self.i += 1;
                return Ok(Some(inner));
            }
            Tok::Op(op @ ("++" | "--")) => {
                let Some(Tok::Var(v)) = self.toks.get(self.i).cloned() else {
                    return Ok(None);
                };
                self.i += 1;
                ArithExpr::IncDec {
                    var: v,
                    delta: if op == "++" { 1 } else { -1 },
                    prefix: true,
                }
            }
            Tok::Op(op @ ("-" | "+" | "!" | "~")) => {
                let Some(operand) = self.expr(UNARY_BP)? else {
                    return Ok(None);
                };
                let u = match op {
                    "-" => ArithUnOp::Neg,
                    "+" => ArithUnOp::Plus,
                    "!" => ArithUnOp::Not,
                    _ => ArithUnOp::BitNot,
                };
                ArithExpr::Unary(u, Box::new(operand))
            }
            Tok::Op(_) => return Ok(None),
        };
        self.node(e).map(Some)
    }
}
