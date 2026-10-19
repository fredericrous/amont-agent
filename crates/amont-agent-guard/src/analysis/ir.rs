//! The crate-owned intermediate representation of a shell command.
//!
//! Syntax only: the frontend builds it, the interpreter gives it meaning.
//! Builtins (`break`, `local`, `set -e`, `[`) are ordinary [`SimpleCmd`]s here
//! and are recognised by the interpreter, because whether `break` is the
//! builtin depends on nothing the parser can see beyond the word itself, and
//! keeping one place that decides keeps the two layers from disagreeing.
//!
//! Every node carries the byte span it came from in the ORIGINAL source, so a
//! finding can point at the loop, the call and the transfer that produced it.
//!
//! Nothing outside `analysis::frontend` constructs these from text, and
//! nothing outside `analysis` sees them: that boundary is what keeps the
//! parser replaceable.

use std::ops::Range;

pub type Span = Range<usize>;

/// A complete command list: what the Bash tool's `command` string parses to.
#[derive(Debug, Clone)]
pub enum Cmd {
    Simple(SimpleCmd),
    /// `a; b`, `a & b` and newlines. `background` marks the elements
    /// followed by `&`, which run asynchronously and whose status is `0`.
    Seq {
        items: Vec<SeqItem>,
        span: Span,
    },
    /// `a && b || c`, left-associative as the shell evaluates it.
    AndOr {
        first: Box<Cmd>,
        rest: Vec<(AndOrOp, Cmd)>,
        span: Span,
    },
    /// `a | b | c`, optionally preceded by `!`.
    Pipeline {
        negated: bool,
        cmds: Vec<Cmd>,
        span: Span,
    },
    If {
        /// `if c1; then b1; elif c2; then b2` — conditions and bodies in order.
        arms: Vec<(Cmd, Cmd)>,
        otherwise: Option<Box<Cmd>>,
        span: Span,
    },
    Case {
        word: Word,
        arms: Vec<CaseArm>,
        span: Span,
    },
    /// `for var in items; do body; done`. `items: None` is `for var; do`,
    /// which iterates over the positional parameters.
    For {
        var: String,
        items: Option<Vec<Word>>,
        body: Box<Cmd>,
        span: Span,
    },
    /// `for ((init; cond; step)); do body; done`.
    ArithFor {
        init: Option<ArithExpr>,
        cond: Option<ArithExpr>,
        step: Option<ArithExpr>,
        body: Box<Cmd>,
        span: Span,
    },
    /// `while cond; do body; done` and `until cond; do body; done`.
    Loop {
        kind: LoopKind,
        cond: Box<Cmd>,
        body: Box<Cmd>,
        span: Span,
    },
    /// `{ …; }` — runs in the current shell.
    Group {
        body: Box<Cmd>,
        span: Span,
    },
    /// `( … )` — runs in a copy of the shell; nothing it writes survives.
    Subshell {
        body: Box<Cmd>,
        span: Span,
    },
    /// `name() { … }` or `function name { … }`. Defines; does not run.
    FuncDef {
        name: String,
        body: Box<Cmd>,
        span: Span,
    },
    /// Command-position `(( expr ))`: status 0 when the value is non-zero.
    Arith {
        expr: ArithExpr,
        span: Span,
    },
    /// `[[ expr ]]`.
    Cond {
        expr: TestExpr,
        span: Span,
    },
    /// A compound command followed by redirections: `while …; done < file`,
    /// `{ …; } 2>/dev/null`. A simple command keeps its own in
    /// [`SimpleCmd::redirects`]; this carries them for everything else, so a
    /// `done < <(curl …)` target, which runs a command, is not dropped.
    Redirected {
        body: Box<Cmd>,
        redirects: Vec<Redirect>,
        span: Span,
    },
    /// A construct the frontend recognised and deliberately does not model,
    /// or could not parse. The interpreter treats it as able to do anything
    /// to the shell's state.
    Unsupported {
        why: &'static str,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct SeqItem {
    pub cmd: Cmd,
    pub background: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndOrOp {
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopKind {
    While,
    Until,
}

#[derive(Debug, Clone)]
pub struct CaseArm {
    pub patterns: Vec<Word>,
    pub body: Cmd,
    pub term: CaseTerm,
    // Syntax kept whole for the frontend's golden tests; the interpreter
    // does not need it to count.
    #[allow(dead_code)]
    pub span: Span,
}

/// How a `case` arm ends, which decides what runs after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseTerm {
    /// `;;` — the `case` is done.
    Break,
    /// `;&` — fall into the next arm's body without testing it.
    FallThrough,
    /// `;;&` — go on testing the next arm's patterns.
    TestNext,
}

/// One command: assignments, words, redirections.
#[derive(Debug, Clone)]
pub struct SimpleCmd {
    pub assigns: Vec<Assign>,
    pub words: Vec<Word>,
    pub redirects: Vec<Redirect>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Assign {
    pub name: String,
    /// `v+=x` appends.
    pub append: bool,
    pub value: Word,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Redirect {
    /// The operator as written, with its fd prefix: `>`, `>>`, `<`, `2>`,
    /// `2>&` (whose target is `1`), `&>`, `<<`, `<<-`, `<<<`, …
    // Syntax kept whole for the frontend's golden tests; the interpreter
    // does not need it to count.
    #[allow(dead_code)]
    pub op: String,
    /// The target word; for a heredoc, the delimiter.
    pub target: Word,
    /// The heredoc's body, when `op` is `<<` or `<<-`. Data, never parsed.
    // Syntax kept whole for the frontend's golden tests; the interpreter
    // does not need it to count.
    #[allow(dead_code)]
    pub heredoc: Option<String>,
    pub span: Span,
}

/// A shell word, as the sequence of pieces that expansion will join.
#[derive(Debug, Clone)]
pub struct Word {
    pub parts: Vec<Part>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Part {
    /// Unquoted literal text, with no expansion-active characters in it.
    Lit(String),
    /// `'…'` and `$'…'` (the latter with its escapes already resolved).
    SingleQuoted(String),
    /// `"…"`: only `Lit`, `Escaped`, `Param`, `ParamOp`, `CmdSubst` and
    /// `Arith` occur inside, and nothing inside is split or globbed.
    DoubleQuoted(Vec<Part>),
    /// `\x` — the character, taken literally.
    Escaped(char),
    /// `$v`, `${v}`, `$1`, `${10}`, `$@`, `$*`, `$#`, `$?`, `$$`, `$!`.
    Param(ParamRef),
    /// `${…}` with an operator (`${v:-x}`, `${#v}`, `${v%x}`, `${v[@]}`…).
    /// Not evaluated: its value is unknown.
    ParamOp { name: String, raw: String },
    /// `$(…)` or backticks: a command list run in a subshell, each time the
    /// word is expanded.
    CmdSubst { body: Box<Cmd>, span: Span },
    /// `$((…))`.
    Arith(ArithExpr),
    /// Unquoted `{a..b[..step]}` or `{x,y,…}`.
    Brace(BraceExpr),
    /// Unquoted `*`, `?`, or the `[` that opens a bracket expression.
    Glob(char),
    /// A leading unquoted `~` (or `~user`, kept as written).
    // Syntax kept whole for the frontend's golden tests; the interpreter
    // does not need it to count.
    #[allow(dead_code)]
    Tilde(String),
    /// `<(…)` or `>(…)`.
    ProcSubst { body: Box<Cmd>, span: Span },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ParamRef {
    Named(String),
    Positional(u32),
    /// `$@` / `$*`.
    All,
    /// `$#`, `$?`, `$$`, `$!`, `$-`, `$0`.
    Special(char),
}

#[derive(Debug, Clone)]
pub enum BraceExpr {
    /// `{from..to[..step]}` over integers; `width` is the zero-padding width
    /// when either end was written with leading zeros.
    IntRange {
        from: i64,
        to: i64,
        step: i64,
        width: usize,
    },
    /// `{a..z}` over single characters.
    CharRange { from: char, to: char, step: i64 },
    /// `{x,y,…}` — each alternative is itself a word.
    List(Vec<Word>),
}

/// An arithmetic expression, as far as the analysis needs one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArithExpr {
    Num(i64),
    Var(String),
    Unary(ArithUnOp, Box<ArithExpr>),
    Binary(ArithBinOp, Box<ArithExpr>, Box<ArithExpr>),
    /// `v = e`, `v += e`, … (`op: None` is plain assignment).
    Assign {
        var: String,
        op: Option<ArithBinOp>,
        value: Box<ArithExpr>,
    },
    /// `++v`, `--v`, `v++`, `v--`.
    IncDec {
        var: String,
        delta: i64,
        prefix: bool,
    },
    /// `a, b` — evaluates both, value of the last.
    Comma(Box<ArithExpr>, Box<ArithExpr>),
    /// `c ? a : b`.
    Ternary(Box<ArithExpr>, Box<ArithExpr>, Box<ArithExpr>),
    /// Anything not modelled, kept as written.
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithUnOp {
    Neg,
    Not,
    BitNot,
    Plus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Pow,
}

/// A `[[ … ]]` / `[ … ]` / `test …` expression.
#[derive(Debug, Clone)]
pub enum TestExpr {
    /// `-n w`, `-z w`, `-f w`, … (`op` without the dash is kept as written).
    Unary {
        op: String,
        operand: Word,
    },
    /// `a -lt b`, `a = b`, `a != b`, `a < b`, `a =~ b`, …
    Binary {
        left: Word,
        op: String,
        right: Word,
    },
    /// A lone word: true when non-empty.
    Word(Word),
    Not(Box<TestExpr>),
    And(Box<TestExpr>, Box<TestExpr>),
    Or(Box<TestExpr>, Box<TestExpr>),
    /// Not modelled.
    Unknown,
}

impl Cmd {
    pub fn span(&self) -> Span {
        match self {
            Cmd::Simple(s) => s.span.clone(),
            Cmd::Seq { span, .. }
            | Cmd::AndOr { span, .. }
            | Cmd::Pipeline { span, .. }
            | Cmd::If { span, .. }
            | Cmd::Case { span, .. }
            | Cmd::For { span, .. }
            | Cmd::ArithFor { span, .. }
            | Cmd::Loop { span, .. }
            | Cmd::Group { span, .. }
            | Cmd::Subshell { span, .. }
            | Cmd::FuncDef { span, .. }
            | Cmd::Arith { span, .. }
            | Cmd::Cond { span, .. }
            | Cmd::Redirected { span, .. }
            | Cmd::Unsupported { span, .. } => span.clone(),
        }
    }
}

impl Word {
    /// The word's text when it is entirely literal — no parameter, no
    /// substitution, no glob, no brace — with quoting removed. `None` means
    /// its value depends on expansion.
    pub fn literal(&self) -> Option<String> {
        let mut out = String::new();
        for p in &self.parts {
            match p {
                Part::Lit(s) | Part::SingleQuoted(s) => out.push_str(s),
                Part::Escaped(c) => out.push(*c),
                Part::DoubleQuoted(inner) => {
                    for q in inner {
                        match q {
                            Part::Lit(s) => out.push_str(s),
                            Part::Escaped(c) => out.push(*c),
                            _ => return None,
                        }
                    }
                }
                _ => return None,
            }
        }
        Some(out)
    }
}
