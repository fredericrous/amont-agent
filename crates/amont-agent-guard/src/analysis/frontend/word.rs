//! Words: quoting, parameter and command substitution, arithmetic, globs,
//! tildes and brace expansion.

use super::super::ir::{BraceExpr, Cmd, ParamRef, Part, Span, Word};
use super::parse::{is_meta, is_name_char, is_name_start, syntax, Parser};
use super::ParseError;

/// Which expansions are live where a word is read. Brace expansion and
/// globbing do not happen in an assignment value or a `case` word, and a
/// heredoc delimiter is only ever text.
#[derive(Debug, Clone, Copy)]
pub(super) struct Mode {
    brace: bool,
    glob: bool,
    tilde: bool,
    /// The right side of `[[ … =~ … ]]`, where `(`, `)`, `|`, `<`, `>`
    /// and `&` are regex characters; the caller bounds the word instead.
    regex: bool,
}

impl Mode {
    pub(super) const NORMAL: Mode = Mode {
        brace: true,
        glob: true,
        tilde: true,
        regex: false,
    };
    pub(super) const ASSIGN: Mode = Mode {
        brace: false,
        glob: false,
        tilde: true,
        regex: false,
    };
    pub(super) const CASE_WORD: Mode = Mode::ASSIGN;
    pub(super) const PATTERN: Mode = Mode {
        brace: false,
        glob: true,
        tilde: true,
        regex: false,
    };
    pub(super) const COND: Mode = Mode::PATTERN;
    pub(super) const HEREDOC: Mode = Mode {
        brace: false,
        glob: false,
        tilde: false,
        regex: false,
    };
    pub(super) const REGEX: Mode = Mode {
        brace: false,
        glob: false,
        tilde: false,
        regex: true,
    };
}

/// How far a `[` looks for the `]` that would make it a bracket
/// expression. Real ones are a few characters long.
const BRACKET_REACH: usize = 256;

/// Where a `{` found its match: the `}` and the top-level commas.
struct BraceScan {
    close: usize,
    commas: Vec<usize>,
}

impl Parser<'_> {
    fn push_part(&mut self, parts: &mut Vec<Part>, part: Part) -> Result<(), ParseError> {
        self.node()?;
        parts.push(part);
        Ok(())
    }

    fn flush(&mut self, parts: &mut Vec<Part>, lit: &mut String) -> Result<(), ParseError> {
        if !lit.is_empty() {
            let s = std::mem::take(lit);
            self.push_part(parts, Part::Lit(s))?;
        }
        Ok(())
    }

    /// One word, up to the first unquoted metacharacter. Empty when the
    /// cursor is already on one.
    pub(super) fn parse_word(&mut self, mode: Mode) -> Result<Word, ParseError> {
        let start = self.pos;
        let mut parts = Vec::new();
        let mut lit = String::new();
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\n' => break,
                b';' | b'&' | b'|' | b'(' | b')' if !mode.regex => break,
                b'<' | b'>' if !mode.regex => {
                    if self.peek_at(1) != Some(b'(') {
                        break;
                    }
                    self.flush(&mut parts, &mut lit)?;
                    let at = self.pos;
                    let (body, span) = self.parse_subst_body(at, 2)?;
                    self.push_part(&mut parts, Part::ProcSubst { body, span })?;
                }
                b'\\' => match self.peek_at(1) {
                    Some(b'\n') => self.pos += 2,
                    None => {
                        self.pos += 1;
                        lit.push('\\');
                    }
                    Some(_) => {
                        self.pos += 1;
                        let ch = self.bump_char();
                        self.flush(&mut parts, &mut lit)?;
                        self.push_part(&mut parts, Part::Escaped(ch))?;
                    }
                },
                b'\'' => {
                    self.flush(&mut parts, &mut lit)?;
                    let p = self.parse_single_quoted()?;
                    self.push_part(&mut parts, p)?;
                }
                b'"' => {
                    self.flush(&mut parts, &mut lit)?;
                    let p = self.parse_double_quoted()?;
                    self.push_part(&mut parts, p)?;
                }
                b'$' => match self.parse_dollar(true)? {
                    Some(p) => {
                        self.flush(&mut parts, &mut lit)?;
                        self.push_part(&mut parts, p)?;
                    }
                    None => lit.push('$'),
                },
                b'`' => {
                    self.flush(&mut parts, &mut lit)?;
                    let p = self.parse_backquote(false)?;
                    self.push_part(&mut parts, p)?;
                }
                b'~' if mode.tilde && self.pos == start => match self.tilde_end() {
                    Some(e) => {
                        let t = self.src[self.pos..e].to_string();
                        self.pos = e;
                        self.push_part(&mut parts, Part::Tilde(t))?;
                    }
                    None => {
                        self.pos += 1;
                        lit.push('~');
                    }
                },
                b'*' | b'?' if mode.glob => {
                    self.flush(&mut parts, &mut lit)?;
                    self.pos += 1;
                    self.push_part(&mut parts, Part::Glob(c as char))?;
                }
                b'[' if mode.glob && self.bracket_closes()? => {
                    self.flush(&mut parts, &mut lit)?;
                    self.pos += 1;
                    self.push_part(&mut parts, Part::Glob('['))?;
                }
                b'{' if mode.brace => match self.parse_brace(mode)? {
                    Some(p) => {
                        self.flush(&mut parts, &mut lit)?;
                        self.push_part(&mut parts, p)?;
                    }
                    None => {
                        self.pos += 1;
                        lit.push('{');
                    }
                },
                _ => lit.push(self.bump_char()),
            }
        }
        self.flush(&mut parts, &mut lit)?;
        self.node()?;
        Ok(Word {
            parts,
            span: start..self.pos,
        })
    }

    fn parse_single_quoted(&mut self) -> Result<Part, ParseError> {
        let start = self.pos;
        let from = start + 1;
        let Some(k) = self.b[from.min(self.end)..self.end]
            .iter()
            .position(|&c| c == b'\'')
        else {
            return Err(syntax("unterminated single quote", start..self.end));
        };
        let s = self.src[from..from + k].to_string();
        self.pos = from + k + 1;
        Ok(Part::SingleQuoted(s))
    }

    /// `"…"`: backslash escapes only `$`, `` ` ``, `"`, `\` and newline.
    fn parse_double_quoted(&mut self) -> Result<Part, ParseError> {
        let start = self.pos;
        self.pos += 1;
        let mut parts = Vec::new();
        let mut lit = String::new();
        loop {
            match self.peek() {
                None => return Err(syntax("unterminated double quote", start..self.end)),
                Some(b'"') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => match self.peek_at(1) {
                    Some(c @ (b'$' | b'`' | b'"' | b'\\')) => {
                        self.pos += 2;
                        self.flush(&mut parts, &mut lit)?;
                        self.push_part(&mut parts, Part::Escaped(c as char))?;
                    }
                    Some(b'\n') => self.pos += 2,
                    _ => {
                        self.pos += 1;
                        lit.push('\\');
                    }
                },
                Some(b'$') => match self.parse_dollar(false)? {
                    Some(p) => {
                        self.flush(&mut parts, &mut lit)?;
                        self.push_part(&mut parts, p)?;
                    }
                    None => lit.push('$'),
                },
                Some(b'`') => {
                    self.flush(&mut parts, &mut lit)?;
                    let p = self.parse_backquote(true)?;
                    self.push_part(&mut parts, p)?;
                }
                Some(_) => lit.push(self.bump_char()),
            }
        }
        self.flush(&mut parts, &mut lit)?;
        Ok(Part::DoubleQuoted(parts))
    }

    /// Everything a `$` can start. `None` is a literal `$` (the cursor has
    /// moved past it). `$'…'` and `$"…"` are only quoting when unquoted.
    fn parse_dollar(&mut self, unquoted: bool) -> Result<Option<Part>, ParseError> {
        let start = self.pos;
        match self.peek_at(1) {
            Some(b'(') => {
                if self.peek_at(2) == Some(b'(') {
                    if let Some(close) = self.scan_arith_end(start + 3) {
                        let expr = self.arith(start + 3..close)?;
                        self.pos = close + 2;
                        return Ok(Some(Part::Arith(expr)));
                    }
                }
                let (body, span) = self.parse_subst_body(start, 2)?;
                Ok(Some(Part::CmdSubst { body, span }))
            }
            Some(b'{') => self.parse_braced_param().map(Some),
            Some(b'\'') if unquoted => self.parse_ansi_c().map(Some),
            Some(b'"') if unquoted => {
                self.pos += 1;
                self.parse_double_quoted().map(Some)
            }
            Some(c) if is_name_start(c) => {
                let from = start + 1;
                let mut i = from;
                while self.at(i).is_some_and(is_name_char) {
                    i += 1;
                }
                self.pos = i;
                Ok(Some(Part::Param(ParamRef::Named(
                    self.src[from..i].to_string(),
                ))))
            }
            Some(c) if c.is_ascii_digit() => {
                self.pos += 2;
                Ok(Some(Part::Param(if c == b'0' {
                    ParamRef::Special('0')
                } else {
                    ParamRef::Positional(u32::from(c - b'0'))
                })))
            }
            Some(b'@' | b'*') => {
                self.pos += 2;
                Ok(Some(Part::Param(ParamRef::All)))
            }
            Some(c @ (b'#' | b'?' | b'$' | b'!' | b'-')) => {
                self.pos += 2;
                Ok(Some(Part::Param(ParamRef::Special(c as char))))
            }
            _ => {
                self.pos += 1;
                Ok(None)
            }
        }
    }

    /// `${…}`: a plain reference when the braces hold only a name, a
    /// number or a special parameter; anything else is an operator form
    /// kept as written.
    fn parse_braced_param(&mut self) -> Result<Part, ParseError> {
        let start = self.pos;
        let Some(close) = self.scan_param_end(start + 2) else {
            return Err(syntax("unterminated `${`", start..self.end));
        };
        let inner = &self.src[start + 2..close];
        self.pos = close + 1;
        let ib = inner.as_bytes();
        let plain = if !ib.is_empty() && is_name_start(ib[0]) && ib.iter().all(|&c| is_name_char(c))
        {
            Some(ParamRef::Named(inner.to_string()))
        } else if !ib.is_empty() && ib.iter().all(u8::is_ascii_digit) {
            match inner.parse::<u32>() {
                Ok(0) if inner == "0" => Some(ParamRef::Special('0')),
                Ok(n) if n > 0 => Some(ParamRef::Positional(n)),
                _ => None,
            }
        } else {
            match inner {
                "@" | "*" => Some(ParamRef::All),
                "#" | "?" | "$" | "!" | "-" => inner.chars().next().map(ParamRef::Special),
                _ => None,
            }
        };
        Ok(match plain {
            Some(r) => Part::Param(r),
            None => Part::ParamOp {
                name: param_op_name(inner),
                raw: self.src[start..self.pos].to_string(),
            },
        })
    }

    /// Index of the `}` closing a `${`, skipping quotes and nested braces.
    fn scan_param_end(&self, from: usize) -> Option<usize> {
        let mut depth = 1usize;
        let mut i = from;
        while i < self.end {
            match self.b[i] {
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
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
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

    /// `$'…'` with its ANSI-C escapes resolved.
    fn parse_ansi_c(&mut self) -> Result<Part, ParseError> {
        let start = self.pos;
        self.pos += 2;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(syntax("unterminated `$'`", start..self.end)),
                Some(b'\'') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => {
                    self.pos += 1;
                    self.ansi_escape(&mut out);
                }
                Some(_) => out.push(self.bump_char()),
            }
        }
        Ok(Part::SingleQuoted(out))
    }

    /// One escape after the backslash (already consumed) of a `$'…'`.
    fn ansi_escape(&mut self, out: &mut String) {
        let Some(c) = self.peek() else {
            out.push('\\');
            return;
        };
        let simple = match c {
            b'a' => Some('\x07'),
            b'b' => Some('\x08'),
            b'e' | b'E' => Some('\x1b'),
            b'f' => Some('\x0c'),
            b'n' => Some('\n'),
            b'r' => Some('\r'),
            b't' => Some('\t'),
            b'v' => Some('\x0b'),
            b'\\' => Some('\\'),
            b'\'' => Some('\''),
            b'"' => Some('"'),
            b'?' => Some('?'),
            _ => None,
        };
        if let Some(ch) = simple {
            self.pos += 1;
            out.push(ch);
            return;
        }
        let digits = |p: &Parser<'_>, from: usize, max: usize, radix: u32| {
            let mut i = from;
            while i < from + max && p.at(i).is_some_and(|d| (d as char).is_digit(radix)) {
                i += 1;
            }
            i
        };
        let (from, stop, radix) = match c {
            b'0'..=b'7' => (self.pos, digits(self, self.pos, 3, 8), 8),
            b'x' => (self.pos + 1, digits(self, self.pos + 1, 2, 16), 16),
            b'u' => (self.pos + 1, digits(self, self.pos + 1, 4, 16), 16),
            b'U' => (self.pos + 1, digits(self, self.pos + 1, 8, 16), 16),
            b'c' => {
                if let Some(d) = self.peek_at(1) {
                    self.pos += 2;
                    out.push(char::from(d & 0x1f));
                } else {
                    self.pos += 1;
                    out.push_str("\\c");
                }
                return;
            }
            _ => {
                out.push('\\');
                out.push(self.bump_char());
                return;
            }
        };
        if stop == from {
            out.push('\\');
            out.push(self.bump_char());
            return;
        }
        let value = u32::from_str_radix(&self.src[from..stop], radix).unwrap_or(0);
        out.push(char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER));
        self.pos = stop;
    }

    /// The body of `$(…)` / `<(…)` / `>(…)`: parsed in place as its own
    /// command list, so its keywords pair only with each other. A body that
    /// does not parse is kept as [`Cmd::Unsupported`] when its closing
    /// parenthesis can still be found; resource limits are never swallowed.
    pub(super) fn parse_subst_body(
        &mut self,
        start: usize,
        open: usize,
    ) -> Result<(Box<Cmd>, Span), ParseError> {
        let saved = self.snapshot();
        self.pos = start + open;
        let parsed = self.parse_list().and_then(|body| {
            self.skip_linebreaks();
            if self.peek() == Some(b')') {
                self.pos += 1;
                Ok(body)
            } else {
                Err(syntax("unterminated substitution", start..self.pos))
            }
        });
        match parsed {
            Ok(body) => Ok((Box::new(body), start..self.pos)),
            Err(ParseError::Limit(e)) => Err(ParseError::Limit(e)),
            Err(err @ ParseError::Syntax { .. }) => {
                self.restore(saved);
                let Some(close) = self.scan_balanced(start + open - 1) else {
                    return Err(err);
                };
                self.pos = close + 1;
                self.node()?;
                let span = start..self.pos;
                Ok((
                    Box::new(Cmd::Unsupported {
                        why: "unparseable substitution",
                        span: span.clone(),
                    }),
                    span,
                ))
            }
        }
    }

    /// `` `…` ``: parsed in place when no backslash in it means something
    /// different inside backquotes than outside; otherwise the body would
    /// need unescaping, which would lose its spans, so it is unsupported.
    fn parse_backquote(&mut self, in_dq: bool) -> Result<Part, ParseError> {
        let start = self.pos;
        let mut i = start + 1;
        let mut escapes = false;
        loop {
            if i >= self.end {
                return Err(syntax("unterminated backquote", start..self.end));
            }
            match self.b[i] {
                b'\\' => {
                    if matches!(self.at(i + 1), Some(b'`' | b'$' | b'\\'))
                        || (in_dq && self.at(i + 1) == Some(b'"'))
                    {
                        escapes = true;
                    }
                    i += 2;
                }
                b'`' => break,
                _ => i += 1,
            }
        }
        let close = i;
        let span = start..close + 1;
        let body = if escapes {
            self.node()?;
            Cmd::Unsupported {
                why: "escaped characters inside backquotes",
                span: span.clone(),
            }
        } else {
            let saved = self.snapshot();
            self.pos = start + 1;
            self.end = close;
            let parsed = self.parse_list().and_then(|body| {
                self.skip_linebreaks();
                if self.pos == close {
                    Ok(body)
                } else {
                    Err(syntax("unexpected token in backquotes", self.pos..close))
                }
            });
            self.end = saved.end;
            match parsed {
                Ok(body) => body,
                Err(ParseError::Limit(e)) => return Err(ParseError::Limit(e)),
                Err(ParseError::Syntax { .. }) => {
                    self.restore(saved);
                    self.node()?;
                    Cmd::Unsupported {
                        why: "unparseable substitution",
                        span: span.clone(),
                    }
                }
            }
        };
        self.pos = close + 1;
        Ok(Part::CmdSubst {
            body: Box::new(body),
            span,
        })
    }

    /// End of a leading `~`, `~user`, `~+` or `~-` prefix, when it is
    /// followed by `/` or the end of the word; `None` means a literal `~`.
    fn tilde_end(&self) -> Option<usize> {
        let mut i = self.pos + 1;
        while self
            .at(i)
            .is_some_and(|c| is_name_char(c) || matches!(c, b'.' | b'-' | b'+'))
        {
            i += 1;
        }
        match self.at(i) {
            None | Some(b'/') => Some(i),
            Some(c) if is_meta(c) => Some(i),
            _ => None,
        }
    }

    /// Whether the `[` under the cursor could open a bracket expression:
    /// a `]` follows within the same word.
    fn bracket_closes(&mut self) -> Result<bool, ParseError> {
        let mut i = self.pos + 1;
        if matches!(self.at(i), Some(b'!' | b'^')) {
            i += 1;
        }
        if self.at(i) == Some(b']') {
            i += 1;
        }
        let limit = (self.pos + BRACKET_REACH).min(self.end);
        let from = i;
        let mut found = false;
        while i < limit {
            match self.b[i] {
                b']' => {
                    found = true;
                    break;
                }
                b'\\' => i += 2,
                c if is_meta(c) => break,
                _ => i += 1,
            }
        }
        self.spend_lookahead(i.saturating_sub(from) + 1)?;
        Ok(found)
    }

    /// Brace expansion at the `{` under the cursor, by bash's rules: a
    /// list needs a top-level unquoted comma, a range is `x..y[..step]`
    /// over integers or single letters, and anything else — `{}`, `{x}`,
    /// or braces broken by an unquoted blank — is literal (`None`).
    fn parse_brace(&mut self, mode: Mode) -> Result<Option<Part>, ParseError> {
        let open = self.pos;
        let Some(scan) = self.scan_brace(open)? else {
            return Ok(None);
        };
        if scan.commas.is_empty() {
            let Some(range) = parse_range(&self.src[open + 1..scan.close]) else {
                return Ok(None);
            };
            self.pos = scan.close + 1;
            return Ok(Some(Part::Brace(range)));
        }
        self.enter()?;
        let saved_end = self.end;
        let mut alts = Vec::new();
        let mut from = open + 1;
        for &stop in scan.commas.iter().chain(std::iter::once(&scan.close)) {
            self.pos = from;
            self.end = stop;
            let w = self.parse_word(mode);
            self.end = saved_end;
            let w = w?;
            if self.pos != stop {
                return Err(syntax("unexpected token in a brace expansion", from..stop));
            }
            alts.push(w);
            from = stop + 1;
        }
        self.leave();
        self.pos = scan.close + 1;
        Ok(Some(Part::Brace(BraceExpr::List(alts))))
    }

    /// Find the `}` matching the `{` at `open` within the current word,
    /// noting its top-level commas.
    fn scan_brace(&mut self, open: usize) -> Result<Option<BraceScan>, ParseError> {
        let mut depth = 1usize;
        let mut commas = Vec::new();
        let mut i = open + 1;
        let result = loop {
            if i >= self.end {
                break None;
            }
            match self.b[i] {
                b'\\' => i += 2,
                q @ (b'\'' | b'"' | b'`') => {
                    let mut j = i + 1;
                    while j < self.end && self.b[j] != q {
                        if q != b'\'' && self.b[j] == b'\\' {
                            j += 1;
                        }
                        j += 1;
                    }
                    if j >= self.end {
                        break None;
                    }
                    i = j + 1;
                }
                b'$' if self.at(i + 1) == Some(b'(') => match self.scan_balanced(i + 1) {
                    Some(j) => i = j + 1,
                    None => break None,
                },
                b'$' if self.at(i + 1) == Some(b'{') => match self.scan_param_end(i + 2) {
                    Some(j) => i = j + 1,
                    None => break None,
                },
                b'{' => {
                    depth += 1;
                    i += 1;
                }
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        break Some(BraceScan { close: i, commas });
                    }
                    i += 1;
                }
                b',' if depth == 1 => {
                    commas.push(i);
                    i += 1;
                }
                c if is_meta(c) => break None,
                _ => i += 1,
            }
        };
        self.spend_lookahead(i.saturating_sub(open) + 1)?;
        Ok(result)
    }
}

/// The name a `${…}` operator form refers to: after a leading `#` or `!`,
/// the identifier, number or special character that starts it.
fn param_op_name(inner: &str) -> String {
    let rest = match inner.as_bytes() {
        [b'#' | b'!', _, ..] => &inner[1..],
        _ => inner,
    };
    let b = rest.as_bytes();
    let n = if b.first().copied().is_some_and(is_name_start) {
        b.iter().take_while(|&&c| is_name_char(c)).count()
    } else if b.first().is_some_and(u8::is_ascii_digit) {
        b.iter().take_while(|c| c.is_ascii_digit()).count()
    } else {
        usize::from(b.first().is_some_and(|c| b"@*#?$!-".contains(c)))
    };
    rest[..n].to_string()
}

/// `x..y[..step]`: integer or single-letter ranges. Never expanded — the
/// interpreter counts them arithmetically. bash ignores the step's sign
/// (direction comes from the ends) and treats a zero step as one, so the
/// step is stored normalised to a positive magnitude.
fn parse_range(inner: &str) -> Option<BraceExpr> {
    let fields: Vec<&str> = inner.split("..").collect();
    if !(2..=3).contains(&fields.len()) {
        return None;
    }
    let step = match fields.get(2) {
        Some(s) => int(s)?.checked_abs()?.max(1),
        None => 1,
    };
    if let (Some(from), Some(to)) = (int(fields[0]), int(fields[1])) {
        let padded = |s: &str| {
            let d = s.trim_start_matches(['-', '+']);
            d.len() > 1 && d.starts_with('0')
        };
        let width = if padded(fields[0]) || padded(fields[1]) {
            fields[0].len().max(fields[1].len())
        } else {
            0
        };
        return Some(BraceExpr::IntRange {
            from,
            to,
            step,
            width,
        });
    }
    let letter = |s: &str| {
        let mut cs = s.chars();
        match (cs.next(), cs.next()) {
            (Some(c), None) if c.is_ascii_alphabetic() => Some(c),
            _ => None,
        }
    };
    Some(BraceExpr::CharRange {
        from: letter(fields[0])?,
        to: letter(fields[1])?,
        step,
    })
}

fn int(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}
