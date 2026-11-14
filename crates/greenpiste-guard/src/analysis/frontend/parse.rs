//! Commands: lists, and-or chains, pipelines, compound and simple commands.
//!
//! Word-level scanning (quotes, expansions, braces) is in `word.rs`; both
//! are methods on the one [`Parser`], because a word can contain a command
//! list (`$(…)`) and a command list is made of words.

use super::super::ir::{
    AndOrOp, Assign, CaseArm, CaseTerm, Cmd, LoopKind, Redirect, SeqItem, SimpleCmd, Span, Word,
};
use super::super::limits::{Exhausted, MAX_DEPTH, MAX_NODES};
use super::test_expr::{self, CondTok};
use super::word::Mode;
use super::{arith, ParseError};

/// Words that are keywords when they begin a command.
const RESERVED: &[&str] = &[
    "if", "then", "elif", "else", "fi", "case", "esac", "for", "select", "while", "until", "do",
    "done", "in", "function", "coproc", "time", "{", "}", "!", "[[", "]]",
];

/// Keywords that close the list before them. Seeing one where a command
/// should start ends the list; the construct that owns it consumes it.
const CLOSERS: &[&str] = &["then", "elif", "else", "fi", "do", "done", "esac", "}"];

/// Characters that end an unquoted word.
pub(super) fn is_meta(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>'
    )
}

pub(super) fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

pub(super) fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

pub(super) fn syntax(why: &'static str, span: Span) -> ParseError {
    ParseError::Syntax { why, span }
}

/// Total bytes the parser may spend looking ahead for a closing `]` or `}`
/// that turns out not to be there. Those scans restart at every candidate,
/// so a pathological word could make them quadratic; the budget keeps the
/// worst case linear and is far beyond anything a real command uses.
const LOOKAHEAD_BUDGET: usize = 1 << 20;

/// State saved around a nested parse that may fail and be recovered from.
pub(super) struct Snapshot {
    pos: usize,
    pub(super) end: usize,
    depth: usize,
    heredoc_line: Option<(usize, usize)>,
}

pub(super) struct Parser<'a> {
    pub(super) src: &'a str,
    pub(super) b: &'a [u8],
    pub(super) pos: usize,
    /// Where the current context's input stops: the source length, or the
    /// closing backquote / brace comma while parsing inside one.
    pub(super) end: usize,
    pub(super) depth: usize,
    pub(super) nodes: usize,
    pub(super) lookahead: usize,
    /// A heredoc was read on the current line: `(newline, resume)` — when
    /// the parser consumes the newline at `newline`, it continues at
    /// `resume`, past the heredoc bodies that follow it.
    pub(super) heredoc_line: Option<(usize, usize)>,
}

impl<'a> Parser<'a> {
    pub(super) fn new(src: &'a str) -> Parser<'a> {
        Parser {
            src,
            b: src.as_bytes(),
            pos: 0,
            end: src.len(),
            depth: 0,
            nodes: 0,
            lookahead: LOOKAHEAD_BUDGET,
            heredoc_line: None,
        }
    }

    pub(super) fn enter(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(ParseError::Limit(Exhausted::Depth));
        }
        Ok(())
    }

    pub(super) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub(super) fn node(&mut self) -> Result<(), ParseError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(ParseError::Limit(Exhausted::Nodes));
        }
        Ok(())
    }

    pub(super) fn spend_lookahead(&mut self, n: usize) -> Result<(), ParseError> {
        if self.lookahead < n {
            return Err(ParseError::Limit(Exhausted::Nodes));
        }
        self.lookahead -= n;
        Ok(())
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        Snapshot {
            pos: self.pos,
            end: self.end,
            depth: self.depth,
            heredoc_line: self.heredoc_line,
        }
    }

    pub(super) fn restore(&mut self, s: Snapshot) {
        self.pos = s.pos;
        self.end = s.end;
        self.depth = s.depth;
        self.heredoc_line = s.heredoc_line;
    }

    pub(super) fn at(&self, i: usize) -> Option<u8> {
        if i < self.end {
            Some(self.b[i])
        } else {
            None
        }
    }

    pub(super) fn peek(&self) -> Option<u8> {
        self.at(self.pos)
    }

    pub(super) fn peek_at(&self, k: usize) -> Option<u8> {
        self.at(self.pos + k)
    }

    pub(super) fn starts(&self, s: &str) -> bool {
        self.pos <= self.end && self.b[self.pos..self.end].starts_with(s.as_bytes())
    }

    /// Take one character (not byte) of literal text.
    pub(super) fn bump_char(&mut self) -> char {
        match self.src[self.pos..self.end].chars().next() {
            Some(c) => {
                self.pos += c.len_utf8();
                c
            }
            None => {
                self.pos += 1;
                '\0'
            }
        }
    }

    /// Skip spaces, tabs, line continuations and a comment (which only
    /// starts where a word could).
    pub(super) fn skip_blanks(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' => self.pos += 1,
                b'\\' if self.peek_at(1) == Some(b'\n') => self.pos += 2,
                b'#' => {
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
    }

    /// Consume the newline under the cursor, jumping over any heredoc
    /// bodies that were announced on the line it ends.
    pub(super) fn consume_newline(&mut self) {
        if let Some((nl, resume)) = self.heredoc_line {
            if nl == self.pos {
                self.pos = resume.min(self.end);
                self.heredoc_line = None;
                return;
            }
        }
        self.pos += 1;
    }

    pub(super) fn skip_linebreaks(&mut self) {
        loop {
            self.skip_blanks();
            if self.peek() == Some(b'\n') {
                self.consume_newline();
            } else {
                break;
            }
        }
    }

    /// The unquoted run of non-metacharacters at the cursor.
    fn raw_run(&self) -> &'a str {
        let mut i = self.pos;
        while i < self.end && !is_meta(self.b[i]) {
            i += 1;
        }
        &self.src[self.pos..i]
    }

    /// The keyword at the cursor, if the next word is exactly one.
    pub(super) fn peek_reserved(&self) -> Option<&'static str> {
        let run = self.raw_run();
        RESERVED.iter().copied().find(|r| *r == run)
    }

    fn expect_word(
        &mut self,
        w: &'static str,
        why: &'static str,
        start: usize,
    ) -> Result<(), ParseError> {
        self.skip_linebreaks();
        if self.peek_reserved() == Some(w) {
            self.pos += w.len();
            Ok(())
        } else {
            Err(syntax(why, start..self.pos))
        }
    }

    pub(super) fn parse_program(mut self) -> Result<Cmd, ParseError> {
        let cmd = self.parse_list()?;
        self.skip_linebreaks();
        if self.pos < self.end {
            let why = match self.peek_reserved() {
                Some("do") | Some("done") => "unbalanced `do`/`done`",
                Some(_) => "unexpected keyword",
                None if self.peek() == Some(b')') => "unexpected `)`",
                None if self.peek() == Some(b';') => "unexpected `;;`",
                None => "unexpected token",
            };
            return Err(syntax(why, self.pos..self.pos + 1));
        }
        Ok(cmd)
    }

    fn at_list_end(&self) -> bool {
        match self.peek() {
            None | Some(b')') => true,
            Some(b';') => matches!(self.peek_at(1), Some(b';') | Some(b'&')),
            _ => self.peek_reserved().is_some_and(|w| CLOSERS.contains(&w)),
        }
    }

    /// A command list, ending before a closer (keyword, `)`, `;;`, the
    /// context's end). Possibly empty: callers that need a command say so.
    pub(super) fn parse_list(&mut self) -> Result<Cmd, ParseError> {
        self.enter()?;
        let start = self.pos;
        let mut items: Vec<SeqItem> = Vec::new();
        let mut last = start;
        loop {
            self.skip_linebreaks();
            if self.at_list_end() {
                break;
            }
            let cmd = self.parse_andor()?;
            last = cmd.span().end;
            self.skip_blanks();
            let mut background = false;
            let mut more = false;
            match self.peek() {
                Some(b'&') => {
                    self.pos += 1;
                    last = self.pos;
                    background = true;
                    more = true;
                }
                Some(b';') if !matches!(self.peek_at(1), Some(b';') | Some(b'&')) => {
                    self.pos += 1;
                    more = true;
                }
                Some(b'\n') => more = true,
                _ => {}
            }
            items.push(SeqItem { cmd, background });
            if !more {
                break;
            }
        }
        self.leave();
        if items.len() == 1 && !items[0].background {
            if let Some(item) = items.pop() {
                return Ok(item.cmd);
            }
        }
        self.node()?;
        let span = match items.first() {
            Some(first) => first.cmd.span().start..last,
            None => self.pos..self.pos,
        };
        Ok(Cmd::Seq { items, span })
    }

    fn parse_body(&mut self, why: &'static str) -> Result<Cmd, ParseError> {
        let at = self.pos;
        let cmd = self.parse_list()?;
        if matches!(&cmd, Cmd::Seq { items, .. } if items.is_empty()) {
            return Err(syntax(why, at..self.pos));
        }
        Ok(cmd)
    }

    fn parse_andor(&mut self) -> Result<Cmd, ParseError> {
        let first = self.parse_pipeline()?;
        let mut rest = Vec::new();
        loop {
            self.skip_blanks();
            let op = if self.starts("&&") {
                AndOrOp::And
            } else if self.starts("||") {
                AndOrOp::Or
            } else {
                break;
            };
            self.pos += 2;
            self.skip_linebreaks();
            rest.push((op, self.parse_pipeline()?));
        }
        if rest.is_empty() {
            return Ok(first);
        }
        self.node()?;
        let span = first.span().start..rest.last().map_or(first.span().end, |(_, c)| c.span().end);
        Ok(Cmd::AndOr {
            first: Box::new(first),
            rest,
            span,
        })
    }

    fn parse_pipeline(&mut self) -> Result<Cmd, ParseError> {
        self.skip_blanks();
        let start = self.pos;
        let mut negated = false;
        let mut prefixed = false;
        loop {
            match self.peek_reserved() {
                Some("!") => {
                    self.pos += 1;
                    negated = !negated;
                    prefixed = true;
                }
                // `time` reports how long the pipeline took and changes
                // nothing about what it does, so the pipeline stands alone.
                Some("time") => {
                    self.pos += 4;
                    self.skip_blanks();
                    if self.raw_run() == "-p" {
                        self.pos += 2;
                    }
                    prefixed = true;
                }
                _ => break,
            }
            self.skip_blanks();
        }
        let mut cmds = vec![self.parse_command()?];
        loop {
            self.skip_blanks();
            if self.peek() == Some(b'|') && self.peek_at(1) != Some(b'|') {
                // `|&` also pipes stderr, which changes no command's effects.
                self.pos += if self.peek_at(1) == Some(b'&') { 2 } else { 1 };
                self.skip_linebreaks();
                cmds.push(self.parse_command()?);
            } else {
                break;
            }
        }
        if !negated && cmds.len() == 1 {
            if let Some(only) = cmds.pop() {
                return Ok(only);
            }
        }
        self.node()?;
        let first = if prefixed {
            start
        } else {
            cmds[0].span().start
        };
        let span = first..cmds.last().map_or(start, |c| c.span().end);
        Ok(Cmd::Pipeline {
            negated,
            cmds,
            span,
        })
    }

    fn parse_command(&mut self) -> Result<Cmd, ParseError> {
        self.skip_blanks();
        let start = self.pos;
        let cmd = match self.peek_reserved() {
            Some("if") => self.parse_if()?,
            Some("for") => self.parse_for(false)?,
            Some("select") => self.parse_for(true)?,
            Some("while") => self.parse_loop(LoopKind::While)?,
            Some("until") => self.parse_loop(LoopKind::Until)?,
            Some("case") => self.parse_case()?,
            Some("{") => self.parse_group()?,
            Some("[[") => self.parse_cond()?,
            Some("function") => return self.parse_function_keyword(),
            Some("coproc") => return self.parse_coproc(),
            Some(w) if CLOSERS.contains(&w) => {
                let why = match w {
                    "do" | "done" => "unbalanced `do`/`done`",
                    _ => "unexpected keyword",
                };
                return Err(syntax(why, start..start + w.len()));
            }
            _ => {
                if self.starts("((") {
                    match self.parse_arith_command()? {
                        Some(c) => c,
                        None => self.parse_subshell()?,
                    }
                } else if self.peek() == Some(b'(') {
                    self.parse_subshell()?
                } else if let Some((name_end, body_at)) = self.funcdef_ahead() {
                    return self.parse_funcdef(start, name_end, body_at);
                } else {
                    return self.parse_simple();
                }
            }
        };
        self.with_redirects(cmd, start)
    }

    /// Redirections after a compound command apply to all of it.
    fn with_redirects(&mut self, cmd: Cmd, start: usize) -> Result<Cmd, ParseError> {
        let mut redirects = Vec::new();
        loop {
            self.skip_blanks();
            match self.redirect_op() {
                Some(len) => redirects.push(self.parse_redirect(len)?),
                None => break,
            }
        }
        if redirects.is_empty() {
            return Ok(cmd);
        }
        self.node()?;
        let end = redirects.last().map_or(cmd.span().end, |r| r.span.end);
        Ok(Cmd::Redirected {
            body: Box::new(cmd),
            redirects,
            span: start..end,
        })
    }

    fn parse_if(&mut self) -> Result<Cmd, ParseError> {
        let start = self.pos;
        self.pos += 2;
        let mut arms = Vec::new();
        let mut otherwise = None;
        loop {
            let cond = self.parse_body("`if` with an empty condition")?;
            self.expect_word("then", "`if` without `then`", start)?;
            let body = self.parse_body("`then` with an empty body")?;
            arms.push((cond, body));
            self.skip_linebreaks();
            match self.peek_reserved() {
                Some("elif") => self.pos += 4,
                Some("else") => {
                    self.pos += 4;
                    otherwise = Some(Box::new(self.parse_body("`else` with an empty body")?));
                    self.expect_word("fi", "`if` without `fi`", start)?;
                    break;
                }
                Some("fi") => {
                    self.pos += 2;
                    break;
                }
                _ => return Err(syntax("`if` without `fi`", start..self.pos)),
            }
        }
        self.node()?;
        Ok(Cmd::If {
            arms,
            otherwise,
            span: start..self.pos,
        })
    }

    fn parse_for(&mut self, select: bool) -> Result<Cmd, ParseError> {
        let start = self.pos;
        self.pos += if select { 6 } else { 3 };
        self.skip_blanks();
        if !select && self.starts("((") {
            return self.parse_arith_for(start);
        }
        let name_start = self.pos;
        if !self.peek().is_some_and(is_name_start) {
            return Err(syntax("`for` without a variable name", start..self.pos));
        }
        while self.peek().is_some_and(is_name_char) {
            self.pos += 1;
        }
        let var = self.src[name_start..self.pos].to_string();
        if self.peek().is_some_and(|c| !is_meta(c)) {
            return Err(syntax("`for` without a variable name", start..self.pos));
        }
        self.skip_blanks();
        let items = if self.peek() == Some(b';') {
            self.pos += 1;
            None
        } else {
            self.skip_linebreaks();
            if self.peek_reserved() == Some("in") {
                self.pos += 2;
                Some(self.parse_for_items(start)?)
            } else {
                None
            }
        };
        self.expect_word("do", "`for` without `do`", start)?;
        let body = self.parse_body("`do` with an empty body")?;
        self.expect_word("done", "unbalanced `do`/`done`", start)?;
        self.node()?;
        if select {
            return Ok(Cmd::Unsupported {
                why: "select",
                span: start..self.pos,
            });
        }
        Ok(Cmd::For {
            var,
            items,
            body: Box::new(body),
            span: start..self.pos,
        })
    }

    fn parse_for_items(&mut self, start: usize) -> Result<Vec<Word>, ParseError> {
        let mut items = Vec::new();
        loop {
            self.skip_blanks();
            match self.peek() {
                None => break,
                Some(b';') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\n') => {
                    self.consume_newline();
                    break;
                }
                Some(c) if is_meta(c) => {
                    return Err(syntax(
                        "unexpected token in a `for` list",
                        start..self.pos + 1,
                    ))
                }
                Some(_) => {
                    let before = self.pos;
                    let w = self.parse_word(Mode::NORMAL)?;
                    if self.pos == before {
                        return Err(syntax(
                            "unexpected token in a `for` list",
                            before..before + 1,
                        ));
                    }
                    items.push(w);
                }
            }
        }
        Ok(items)
    }

    fn parse_arith_for(&mut self, start: usize) -> Result<Cmd, ParseError> {
        let open = self.pos;
        let Some(close) = self.scan_arith_end(open + 2) else {
            return Err(syntax("unterminated `for ((`", start..self.end));
        };
        let mut clauses = Vec::new();
        let mut from = open + 2;
        let mut depth = 0usize;
        for i in open + 2..close {
            match self.b[i] {
                b'(' => depth += 1,
                b')' => depth = depth.saturating_sub(1),
                b';' if depth == 0 => {
                    clauses.push(from..i);
                    from = i + 1;
                }
                _ => {}
            }
        }
        clauses.push(from..close);
        if clauses.len() != 3 {
            return Err(syntax("`for ((…))` needs three clauses", start..close + 2));
        }
        let mut exprs = Vec::new();
        for r in clauses {
            if self.src[r.clone()].trim().is_empty() {
                exprs.push(None);
            } else {
                exprs.push(Some(self.arith(r)?));
            }
        }
        self.pos = close + 2;
        self.skip_blanks();
        if self.peek() == Some(b';') {
            self.pos += 1;
        }
        self.expect_word("do", "`for` without `do`", start)?;
        let body = self.parse_body("`do` with an empty body")?;
        self.expect_word("done", "unbalanced `do`/`done`", start)?;
        self.node()?;
        let mut exprs = exprs.into_iter();
        Ok(Cmd::ArithFor {
            init: exprs.next().flatten(),
            cond: exprs.next().flatten(),
            step: exprs.next().flatten(),
            body: Box::new(body),
            span: start..self.pos,
        })
    }

    fn parse_loop(&mut self, kind: LoopKind) -> Result<Cmd, ParseError> {
        let start = self.pos;
        self.pos += 5;
        let cond = self.parse_body("a loop with an empty condition")?;
        self.expect_word("do", "a loop without `do`", start)?;
        let body = self.parse_body("`do` with an empty body")?;
        self.expect_word("done", "unbalanced `do`/`done`", start)?;
        self.node()?;
        Ok(Cmd::Loop {
            kind,
            cond: Box::new(cond),
            body: Box::new(body),
            span: start..self.pos,
        })
    }

    fn parse_case(&mut self) -> Result<Cmd, ParseError> {
        let start = self.pos;
        self.pos += 4;
        self.skip_blanks();
        let word = self.parse_word(Mode::CASE_WORD)?;
        if word.parts.is_empty() {
            return Err(syntax("`case` without a word", start..self.pos));
        }
        self.expect_word("in", "`case` without `in`", start)?;
        let mut arms = Vec::new();
        loop {
            self.skip_linebreaks();
            if self.peek_reserved() == Some("esac") {
                self.pos += 4;
                break;
            }
            if self.peek().is_none() {
                return Err(syntax("`case` without `esac`", start..self.pos));
            }
            let arm_start = self.pos;
            if self.peek() == Some(b'(') {
                self.pos += 1;
            }
            let mut patterns = Vec::new();
            loop {
                self.skip_blanks();
                let w = self.parse_word(Mode::PATTERN)?;
                if w.parts.is_empty() {
                    return Err(syntax("expected a `case` pattern", arm_start..self.pos + 1));
                }
                patterns.push(w);
                self.skip_blanks();
                match self.peek() {
                    Some(b'|') => self.pos += 1,
                    Some(b')') => {
                        self.pos += 1;
                        break;
                    }
                    _ => {
                        return Err(syntax(
                            "expected `)` after a `case` pattern",
                            arm_start..self.pos,
                        ))
                    }
                }
            }
            let body = self.parse_list()?;
            self.skip_linebreaks();
            let term = if self.starts(";;&") {
                self.pos += 3;
                CaseTerm::TestNext
            } else if self.starts(";;") {
                self.pos += 2;
                CaseTerm::Break
            } else if self.starts(";&") {
                self.pos += 2;
                CaseTerm::FallThrough
            } else if self.peek_reserved() == Some("esac") {
                CaseTerm::Break
            } else {
                return Err(syntax("expected `;;` or `esac`", arm_start..self.pos));
            };
            self.node()?;
            arms.push(CaseArm {
                patterns,
                body,
                term,
                span: arm_start..self.pos,
            });
        }
        self.node()?;
        Ok(Cmd::Case {
            word,
            arms,
            span: start..self.pos,
        })
    }

    fn parse_group(&mut self) -> Result<Cmd, ParseError> {
        let start = self.pos;
        self.pos += 1;
        let body = self.parse_body("`{` with an empty body")?;
        self.expect_word("}", "`{` without `}`", start)?;
        self.node()?;
        Ok(Cmd::Group {
            body: Box::new(body),
            span: start..self.pos,
        })
    }

    fn parse_subshell(&mut self) -> Result<Cmd, ParseError> {
        let start = self.pos;
        self.pos += 1;
        let body = self.parse_body("`(` with an empty body")?;
        self.skip_linebreaks();
        if self.peek() != Some(b')') {
            return Err(syntax("`(` without `)`", start..self.pos));
        }
        self.pos += 1;
        self.node()?;
        Ok(Cmd::Subshell {
            body: Box::new(body),
            span: start..self.pos,
        })
    }

    /// `(( expr ))` in command position, or `None` when the parentheses do
    /// not close as `))` — then it is a subshell starting with a subshell.
    fn parse_arith_command(&mut self) -> Result<Option<Cmd>, ParseError> {
        let start = self.pos;
        let Some(close) = self.scan_arith_end(start + 2) else {
            return Ok(None);
        };
        let expr = self.arith(start + 2..close)?;
        self.pos = close + 2;
        self.node()?;
        Ok(Some(Cmd::Arith {
            expr,
            span: start..self.pos,
        }))
    }

    /// `[[ … ]]`: inside, `&&`, `||`, `(`, `)`, `<` and `>` are operators
    /// of the test, not of the shell, and the right side of `=~` is a regex
    /// in which `(` and `|` are ordinary.
    fn parse_cond(&mut self) -> Result<Cmd, ParseError> {
        let start = self.pos;
        self.pos += 2;
        let mut toks = Vec::new();
        loop {
            self.skip_linebreaks();
            let c = match self.peek() {
                None => return Err(syntax("`[[` without `]]`", start..self.pos)),
                Some(c) => c,
            };
            let op = match (c, self.peek_at(1)) {
                (b'&', Some(b'&')) => Some("&&"),
                (b'|', Some(b'|')) => Some("||"),
                (b'(', _) => Some("("),
                (b')', _) => Some(")"),
                (b'<', n) if n != Some(b'(') => Some("<"),
                (b'>', n) if n != Some(b'(') => Some(">"),
                (b';' | b'&' | b'|', _) => {
                    return Err(syntax("unexpected token in `[[`", self.pos..self.pos + 1))
                }
                _ => None,
            };
            if let Some(op) = op {
                self.pos += op.len();
                toks.push(CondTok::Op(op));
                continue;
            }
            if self.peek_reserved() == Some("]]") {
                self.pos += 2;
                break;
            }
            let before = self.pos;
            let w = self.parse_word(Mode::COND)?;
            if self.pos == before {
                return Err(syntax("unexpected token in `[[`", before..before + 1));
            }
            let regex = w.literal().as_deref() == Some("=~");
            toks.push(CondTok::Word(w));
            if regex {
                self.skip_blanks();
                let stop = self.scan_regex_end();
                if stop > self.pos {
                    let saved = self.end;
                    self.end = stop;
                    let w = self.parse_word(Mode::REGEX);
                    self.end = saved;
                    toks.push(CondTok::Word(w?));
                }
            }
        }
        let expr =
            test_expr::parse_cond(&toks, self.depth, &mut self.nodes).map_err(ParseError::Limit)?;
        self.node()?;
        Ok(Cmd::Cond {
            expr,
            span: start..self.pos,
        })
    }

    /// Where the regex operand of `=~` ends: the first unquoted blank
    /// outside parentheses, or an unmatched `)`.
    fn scan_regex_end(&self) -> usize {
        let mut i = self.pos;
        let mut depth = 0usize;
        while i < self.end {
            match self.b[i] {
                b' ' | b'\t' | b'\n' if depth == 0 => break,
                b'\\' => i += 1,
                q @ (b'\'' | b'"') => {
                    let mut j = i + 1;
                    while j < self.end && self.b[j] != q {
                        if q == b'"' && self.b[j] == b'\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                    i = j;
                }
                b'(' => depth += 1,
                b')' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            i += 1;
        }
        i.min(self.end)
    }

    /// `name ( )` ahead: the end of the name and where the body starts.
    fn funcdef_ahead(&self) -> Option<(usize, usize)> {
        let mut i = self.pos;
        while i < self.end {
            let c = self.b[i];
            if is_meta(c)
                || matches!(
                    c,
                    b'\'' | b'"' | b'\\' | b'$' | b'`' | b'=' | b'{' | b'}' | b'*' | b'?' | b'['
                )
            {
                break;
            }
            i += 1;
        }
        let name_end = i;
        if name_end == self.pos {
            return None;
        }
        let skip = |mut j: usize| {
            while j < self.end && matches!(self.b[j], b' ' | b'\t') {
                j += 1;
            }
            j
        };
        let j = skip(name_end);
        if self.at(j) != Some(b'(') {
            return None;
        }
        let j = skip(j + 1);
        if self.at(j) != Some(b')') {
            return None;
        }
        Some((name_end, j + 1))
    }

    fn parse_funcdef(
        &mut self,
        start: usize,
        name_end: usize,
        body_at: usize,
    ) -> Result<Cmd, ParseError> {
        let name = self.src[start..name_end].to_string();
        self.pos = body_at;
        self.finish_funcdef(start, name)
    }

    fn parse_function_keyword(&mut self) -> Result<Cmd, ParseError> {
        let start = self.pos;
        self.pos += "function".len();
        self.skip_blanks();
        let name = self.raw_run();
        if name.is_empty()
            || name
                .bytes()
                .any(|c| matches!(c, b'\'' | b'"' | b'\\' | b'$' | b'`'))
        {
            return Err(syntax("`function` without a name", start..self.pos));
        }
        self.pos += name.len();
        self.skip_blanks();
        if self.peek() == Some(b'(') {
            self.pos += 1;
            self.skip_blanks();
            if self.peek() != Some(b')') {
                return Err(syntax("`function name(` without `)`", start..self.pos));
            }
            self.pos += 1;
        }
        self.finish_funcdef(start, name.to_string())
    }

    fn finish_funcdef(&mut self, start: usize, name: String) -> Result<Cmd, ParseError> {
        self.skip_linebreaks();
        let body = self.parse_command()?;
        if !is_compound(&body) {
            return Err(syntax(
                "a function body must be a compound command",
                start..self.pos,
            ));
        }
        self.node()?;
        Ok(Cmd::FuncDef {
            name,
            body: Box::new(body),
            span: start..self.pos,
        })
    }

    /// `coproc [NAME] command`: an asynchronous process with pipes the
    /// analysis does not model. Parsed through so what follows still reads.
    fn parse_coproc(&mut self) -> Result<Cmd, ParseError> {
        let start = self.pos;
        self.pos += "coproc".len();
        self.enter()?;
        self.skip_blanks();
        let run = self.raw_run();
        if !run.is_empty() && run.bytes().all(is_name_char) && run.as_bytes()[0] != b'{' {
            let save = self.pos;
            self.pos += run.len();
            self.skip_blanks();
            let compound = self.peek() == Some(b'(')
                || matches!(
                    self.peek_reserved(),
                    Some("{" | "if" | "for" | "while" | "until" | "case" | "[[")
                );
            if !compound {
                self.pos = save;
            }
        }
        let body = self.parse_command()?;
        self.leave();
        self.node()?;
        Ok(Cmd::Unsupported {
            why: "coproc",
            span: start..body.span().end.max(start),
        })
    }

    fn parse_simple(&mut self) -> Result<Cmd, ParseError> {
        self.skip_blanks();
        let start = self.pos;
        let mut assigns: Vec<Assign> = Vec::new();
        let mut words: Vec<Word> = Vec::new();
        let mut redirects: Vec<Redirect> = Vec::new();
        let mut unsupported: Option<&'static str> = None;
        let mut last = start;
        loop {
            self.skip_blanks();
            let Some(c) = self.peek() else { break };
            let proc_subst = matches!(c, b'<' | b'>') && self.peek_at(1) == Some(b'(');
            if !proc_subst {
                if let Some(len) = self.redirect_op() {
                    let r = self.parse_redirect(len)?;
                    last = r.span.end;
                    redirects.push(r);
                    continue;
                }
                if matches!(c, b';' | b'&' | b'|' | b'\n' | b')') {
                    break;
                }
                if c == b'(' {
                    return Err(syntax("unexpected `(`", self.pos..self.pos + 1));
                }
                if words.is_empty() {
                    if let Some(a) = self.parse_assignment(&mut unsupported)? {
                        last = a.span.end;
                        assigns.push(a);
                        continue;
                    }
                }
            }
            let before = self.pos;
            let w = self.parse_word(Mode::NORMAL)?;
            if self.pos == before {
                return Err(syntax("unexpected character", before..before + 1));
            }
            last = w.span.end;
            words.push(w);
        }
        if assigns.is_empty() && words.is_empty() && redirects.is_empty() {
            let at = self.pos;
            return Err(syntax("expected a command", at..(at + 1).min(self.b.len())));
        }
        self.node()?;
        let span = start..last;
        if let Some(why) = unsupported {
            return Ok(Cmd::Unsupported { why, span });
        }
        Ok(Cmd::Simple(SimpleCmd {
            assigns,
            words,
            redirects,
            span,
        }))
    }

    /// `NAME=value` / `NAME+=value` at the cursor. `NAME=(…)` and
    /// `NAME[i]=…` are arrays, which the subset leaves out: they are read
    /// through and flag the whole command.
    fn parse_assignment(
        &mut self,
        unsupported: &mut Option<&'static str>,
    ) -> Result<Option<Assign>, ParseError> {
        let start = self.pos;
        if !self.peek().is_some_and(is_name_start) {
            return Ok(None);
        }
        let mut i = start;
        while self.at(i).is_some_and(is_name_char) {
            i += 1;
        }
        let name_end = i;
        let mut element = false;
        if self.at(i) == Some(b'[') {
            let mut j = i + 1;
            while self.at(j).is_some_and(|c| c != b']' && !is_meta(c)) {
                j += 1;
            }
            if self.at(j) != Some(b']') {
                return Ok(None);
            }
            i = j + 1;
            element = true;
        }
        let append = self.at(i) == Some(b'+');
        if append {
            i += 1;
        }
        if self.at(i) != Some(b'=') {
            return Ok(None);
        }
        i += 1;
        let name = self.src[start..name_end].to_string();
        self.pos = i;
        if self.peek() == Some(b'(') {
            let Some(close) = self.scan_balanced(i) else {
                return Err(syntax("unterminated array assignment", start..self.end));
            };
            self.pos = close + 1;
            *unsupported = Some("array assignment");
            self.node()?;
            return Ok(Some(Assign {
                name,
                append,
                value: Word {
                    parts: Vec::new(),
                    span: i..close + 1,
                },
                span: start..self.pos,
            }));
        }
        let value = self.parse_word(Mode::ASSIGN)?;
        if element {
            *unsupported = Some("array assignment");
        }
        self.node()?;
        Ok(Some(Assign {
            name,
            append,
            value,
            span: start..self.pos,
        }))
    }

    /// Length of the redirection operator at the cursor, fd prefix
    /// included, or `None`.
    pub(super) fn redirect_op(&self) -> Option<usize> {
        let mut i = self.pos;
        while self.at(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
        let fd = i - self.pos;
        let rest = &self.b[i.min(self.end)..self.end];
        const OPS: &[&str] = &["<<<", "<<-", "<<", "<&", "<>", ">>", ">&", ">|", "<", ">"];
        if fd == 0 {
            for op in ["&>>", "&>"] {
                if rest.starts_with(op.as_bytes()) {
                    return Some(op.len());
                }
            }
        }
        OPS.iter()
            .find(|op| rest.starts_with(op.as_bytes()))
            .map(|op| fd + op.len())
    }

    fn parse_redirect(&mut self, len: usize) -> Result<Redirect, ParseError> {
        let start = self.pos;
        let op = self.src[start..start + len].to_string();
        self.pos += len;
        self.skip_blanks();
        let heredoc = (op.ends_with("<<") && !op.ends_with("<<<")) || op.ends_with("<<-");
        let target = self.parse_word(if heredoc { Mode::HEREDOC } else { Mode::NORMAL })?;
        if target.parts.is_empty() {
            return Err(syntax("a redirection without a target", start..self.pos));
        }
        let body = if heredoc {
            let tag = target
                .literal()
                .unwrap_or_else(|| self.src[target.span.clone()].to_string());
            Some(self.read_heredoc(&tag, op.ends_with("<<-"), start)?)
        } else {
            None
        };
        self.node()?;
        Ok(Redirect {
            op,
            span: start..target.span.end,
            target,
            heredoc: body,
        })
    }

    /// Read a heredoc body. It starts on the line after the one being
    /// parsed (after any earlier heredoc's body on the same line), and the
    /// newline that ends this line is marked to jump over it.
    fn read_heredoc(
        &mut self,
        tag: &str,
        strip_tabs: bool,
        start: usize,
    ) -> Result<String, ParseError> {
        let body_start = match self.heredoc_line {
            Some((nl, resume)) if nl >= self.pos => resume,
            _ => {
                let nl = self
                    .scan_line_end()
                    .ok_or_else(|| syntax("unterminated heredoc", start..self.end))?;
                self.heredoc_line = Some((nl, nl + 1));
                nl + 1
            }
        };
        let mut body = String::new();
        let mut i = body_start;
        loop {
            if i >= self.end {
                return Err(syntax("unterminated heredoc", start..self.end));
            }
            let line_end = self.b[i..self.end]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(self.end, |k| i + k);
            let line = &self.src[i..line_end];
            let line = if strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if line == tag {
                let resume = (line_end + 1).min(self.end);
                if let Some((nl, _)) = self.heredoc_line {
                    self.heredoc_line = Some((nl, resume));
                }
                return Ok(body);
            }
            body.push_str(line);
            body.push('\n');
            i = line_end + 1;
        }
    }

    /// The newline that ends the current line, skipping quoted text.
    fn scan_line_end(&self) -> Option<usize> {
        let mut i = self.pos;
        while i < self.end {
            match self.b[i] {
                b'\n' => return Some(i),
                b'\\' => i += 1,
                q @ (b'\'' | b'"') => {
                    let mut j = i + 1;
                    while j < self.end && self.b[j] != q {
                        if q == b'"' && self.b[j] == b'\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                    i = j;
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Where `((` / `$((` content ends: the index of the first `)` of a
    /// `))` at nesting zero, or `None` when the first unmatched `)` is not
    /// followed by another (then it was never arithmetic).
    pub(super) fn scan_arith_end(&self, from: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut i = from;
        while i < self.end {
            match self.b[i] {
                b'\\' => i += 1,
                b'(' => depth += 1,
                b')' => {
                    if depth == 0 {
                        return (self.at(i + 1) == Some(b')')).then_some(i);
                    }
                    depth -= 1;
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Index of the `)` matching the `(` at `open`, skipping quotes,
    /// escapes and backquotes. Used to step over text that is not parsed.
    pub(super) fn scan_balanced(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut i = open;
        while i < self.end {
            match self.b[i] {
                b'\\' => i += 1,
                q @ (b'\'' | b'"' | b'`') => {
                    let mut j = i + 1;
                    while j < self.end && self.b[j] != q {
                        if q != b'\'' && self.b[j] == b'\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                    if j >= self.end {
                        return None;
                    }
                    i = j;
                }
                b'(' => depth += 1,
                b')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    pub(super) fn arith(&mut self, range: Span) -> Result<super::super::ir::ArithExpr, ParseError> {
        arith::parse(&self.src[range], self.depth, &mut self.nodes).map_err(ParseError::Limit)
    }
}

fn is_compound(cmd: &Cmd) -> bool {
    match cmd {
        Cmd::Group { .. }
        | Cmd::Subshell { .. }
        | Cmd::If { .. }
        | Cmd::Case { .. }
        | Cmd::For { .. }
        | Cmd::ArithFor { .. }
        | Cmd::Loop { .. }
        | Cmd::Arith { .. }
        | Cmd::Cond { .. }
        | Cmd::Unsupported { .. } => true,
        Cmd::Redirected { body, .. } => is_compound(body),
        _ => false,
    }
}
