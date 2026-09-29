//! The preview guide (fleet rule `work.preview-is-guided`): what the person
//! reads before they are asked to approve a preview.
//!
//! The person works on several projects at once and does not remember where
//! this one stood, so "Ship repo@sha?" alone gives them nothing to decide
//! with. `preview register` therefore takes a markdown guide, refuses one
//! that lacks a required section ([`check`]), and renders it to a small,
//! self-contained HTML page beside it ([`render`]).
//!
//! The converter is hand-rolled on purpose, for the constrained subset a
//! guide needs: headings, paragraphs, bullet and numbered lists (one level of
//! nesting), fenced code, block quotes, and inline bold, code, links, images
//! and bare URLs. Everything else is text, and every text byte is escaped. A
//! markdown crate would be a new dependency on a trust path (ADR-0020,
//! `change.dependency-bar`) for a page nobody but the person reads.

use std::path::Path;

/// The sections a guide must carry, as H2 headings, in the order the page
/// shows them.
pub const SECTIONS: [&str; 5] = [
    "Where we are",
    "What you should see",
    "Try it",
    "Reference",
    "Already checked",
];

const TRY_IT: usize = 2;
const REFERENCE: usize = 3;

/// A guide larger than this is not a guide.
pub const MAX_BYTES: u64 = 1024 * 1024;

/// One `## ` section: the heading as written and its body lines.
#[derive(Debug, Clone)]
struct Section<'a> {
    heading: &'a str,
    body: Vec<&'a str>,
}

/// A parsed guide: what precedes the first H2, and each section.
#[derive(Debug)]
pub struct Guide<'a> {
    preamble: Vec<&'a str>,
    sections: Vec<Section<'a>>,
}

/// The heading's words, without emoji, punctuation or case: `📍 Where we
/// are:` and `where we are` are the same section.
fn normalise(heading: &str) -> String {
    let words: String = heading
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect();
    words.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a heading as written names the section `name`: the same words,
/// emoji, punctuation and case aside — or the section's words followed by a
/// note, introduced by `(`, a dash or `:` (`Try it (about 2 minutes)`,
/// `Reference — screenshots`). Other trailing words are a different heading:
/// `Try it now` is not `Try it`.
fn names_section(heading: &str, name: &str) -> bool {
    if normalise(heading) == normalise(name) {
        return true;
    }
    let h = heading.trim_start_matches(|c: char| !c.is_alphanumeric());
    let Some(head) = h.get(..name.len()) else {
        return false;
    };
    if !head.eq_ignore_ascii_case(name) {
        return false;
    }
    let rest = &h[name.len()..];
    if rest.chars().next().is_some_and(char::is_alphanumeric) {
        return false;
    }
    let rest = rest.trim_start();
    rest.is_empty() || rest.starts_with(['(', '-', '–', '—', ':'])
}

fn h2(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("## ")?;
    Some(rest.trim().trim_end_matches('#').trim())
}

/// Lines of a fenced code block are never headings or steps.
fn outside_fences(lines: &[&str]) -> Vec<bool> {
    let mut inside = false;
    lines
        .iter()
        .map(|l| {
            if l.trim_start().starts_with("```") {
                inside = !inside;
                return false;
            }
            !inside
        })
        .collect()
}

pub fn parse(text: &str) -> Guide<'_> {
    let lines: Vec<&str> = text.lines().collect();
    let open = outside_fences(&lines);
    let mut g = Guide {
        preamble: Vec::new(),
        sections: Vec::new(),
    };
    for (line, open) in lines.iter().zip(open) {
        match h2(line).filter(|_| open) {
            Some(heading) => g.sections.push(Section {
                heading,
                body: Vec::new(),
            }),
            None => match g.sections.last_mut() {
                Some(s) => s.body.push(line),
                None => g.preamble.push(line),
            },
        }
    }
    g
}

impl<'a> Guide<'a> {
    /// The first section whose heading is `name`, emoji and case aside.
    fn section(&self, name: &str) -> Option<&Section<'a>> {
        self.sections
            .iter()
            .find(|s| names_section(s.heading, name))
    }
}

/// A numbered list item: `1. `, `12) ` — digits, then `.` or `)`, then a
/// space.
fn numbered(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let rest = &t[digits..];
    let rest = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?;
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    Some((t[..digits].parse().ok()?, rest.trim_start()))
}

fn bullet(line: &str) -> Option<&str> {
    let t = line.trim_start();
    ["- ", "* ", "+ "]
        .iter()
        .find_map(|m| t.strip_prefix(m))
        .or_else(|| (t == "-" || t == "*").then_some(""))
}

fn has_url(line: &str) -> bool {
    line.contains("http://") || line.contains("https://")
}

/// What a guide lacks, one line each; empty when it is complete.
pub fn check(text: &str) -> Vec<String> {
    let g = parse(text);
    let mut missing = Vec::new();
    for name in SECTIONS {
        match g.section(name) {
            None => missing.push(format!("the section `## {name}`")),
            Some(s) if s.body.iter().all(|l| l.trim().is_empty()) => {
                missing.push(format!("any text under `## {name}`"))
            }
            Some(_) => {}
        }
    }
    if let Some(s) = g.section(SECTIONS[TRY_IT]) {
        let open = outside_fences(&s.body);
        let steps = s
            .body
            .iter()
            .zip(&open)
            .any(|(l, o)| *o && numbered(l).is_some());
        if !steps {
            missing.push(
                "a numbered step (`1. …`) under `## Try it`: each click, named by its label, and what should appear".to_string(),
            );
        }
        if !s.body.iter().any(|l| has_url(l)) {
            missing
                .push("an http(s) URL under `## Try it`: where the first step starts".to_string());
        }
    }
    missing
}

// --- rendering ------------------------------------------------------------

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A URL the page may carry in `href`/`src`: http(s), mailto, file, or a
/// scheme-less relative path. `javascript:` and friends are `None`, and the
/// markdown around them renders as text.
fn safe_url(url: &str) -> Option<&str> {
    let u = url.trim();
    if u.is_empty() || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let scheme_end = u.find(':');
    let path_start = u.find(['/', '?', '#']).unwrap_or(u.len());
    match scheme_end {
        Some(i) if i < path_start => {
            let scheme = u[..i].to_ascii_lowercase();
            // A Windows drive letter (`C:\…`) is a path, not a scheme.
            let drive = i == 1 && u[..1].chars().all(|c| c.is_ascii_alphabetic());
            (drive || matches!(scheme.as_str(), "http" | "https" | "mailto" | "file")).then_some(u)
        }
        _ => Some(u),
    }
}

/// The file name of an image source, lowercased.
fn file_name(src: &str) -> String {
    src.rsplit(['/', '\\'])
        .next()
        .unwrap_or(src)
        .to_ascii_lowercase()
}

/// Every image source the guide references, in order.
pub fn images(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("![") {
        rest = &rest[i + 2..];
        let Some((_, src, after)) = link_parts(rest) else {
            continue;
        };
        found.push(src.to_string());
        rest = after;
    }
    found
}

/// After `[` or `![`: `alt](src)` → (alt, src, what follows).
fn link_parts(s: &str) -> Option<(&str, &str, &str)> {
    let close = s.find("](")?;
    let text = &s[..close];
    if text.contains('\n') {
        return None;
    }
    let after = &s[close + 2..];
    let end = after.find(')')?;
    let src = after[..end].trim();
    // `(src "title")`: the title is dropped.
    let src = src.split_once(" \"").map(|(s, _)| s).unwrap_or(src);
    Some((text, src, &after[end + 1..]))
}

/// The before/after pair, when the guide references an image whose file
/// name says `before` and another whose name says `after`.
fn pair(text: &str) -> Option<(String, String)> {
    let all = images(text);
    let before = all.iter().find(|s| file_name(s).contains("before"))?;
    let after = all
        .iter()
        .find(|s| *s != before && file_name(s).contains("after"))?;
    Some((before.clone(), after.clone()))
}

struct Ctx<'p> {
    /// Sources rendered in the before/after block, so skipped inline.
    paired: Option<&'p (String, String)>,
}

impl Ctx<'_> {
    fn is_paired(&self, src: &str) -> bool {
        self.paired.is_some_and(|(b, a)| b == src || a == src)
    }
}

fn img(alt: &str, src: &str) -> String {
    format!(
        "<img src=\"{}\" alt=\"{}\" loading=\"lazy\">",
        escape(src),
        escape(alt)
    )
}

/// Inline markdown to HTML: `code`, **bold**, ![alt](src), [text](url), a
/// bare http(s) URL, `\` escapes. Everything else is escaped text.
fn inline(s: &str, ctx: &Ctx) -> String {
    let mut out = String::new();
    let mut i = 0;
    let b = s.as_bytes();
    while i < s.len() {
        let rest = &s[i..];
        if rest.starts_with('\\') && rest.len() > 1 {
            let c = rest[1..].chars().next().unwrap_or(' ');
            if c.is_ascii_punctuation() {
                out.push_str(&escape(&c.to_string()));
                i += 1 + c.len_utf8();
                continue;
            }
        }
        if let Some(r) = rest.strip_prefix('`') {
            if let Some(end) = r.find('`') {
                out.push_str(&format!("<code>{}</code>", escape(&r[..end])));
                i += end + 2;
                continue;
            }
        }
        if let Some(r) = rest.strip_prefix("**") {
            if let Some(end) = r.find("**").filter(|&e| e > 0) {
                out.push_str(&format!("<strong>{}</strong>", inline(&r[..end], ctx)));
                i += end + 4;
                continue;
            }
        }
        if let Some(r) = rest.strip_prefix("![") {
            if let Some((alt, src, after)) = link_parts(r) {
                if let Some(src) = safe_url(src) {
                    if !ctx.is_paired(src) {
                        out.push_str(&img(alt, src));
                    }
                    i = s.len() - after.len();
                    continue;
                }
            }
        }
        if let Some(r) = rest.strip_prefix('[') {
            if let Some((text, url, after)) = link_parts(r) {
                if let Some(url) = safe_url(url) {
                    out.push_str(&format!(
                        "<a href=\"{}\">{}</a>",
                        escape(url),
                        inline(text, ctx)
                    ));
                    i = s.len() - after.len();
                    continue;
                }
            }
        }
        let boundary = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'/');
        if boundary && (rest.starts_with("http://") || rest.starts_with("https://")) {
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"')
                .unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
            out.push_str(&format!("<a href=\"{0}\">{0}</a>", escape(url)));
            i += url.len();
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        out.push_str(&escape(&c.to_string()));
        i += c.len_utf8();
    }
    out
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// A list marker at the start of `line`: (ordered, start number, text,
/// width of indent plus marker).
fn marker(line: &str) -> Option<(bool, usize, &str, usize)> {
    let ind = indent(line);
    if let Some(text) = bullet(line) {
        return Some((false, 1, text, ind + 2));
    }
    let (n, text) = numbered(line)?;
    let t = line.trim_start();
    let width = t.len() - text.len();
    Some((true, n, text, ind + width.max(1)))
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    let level = t.chars().take_while(|&c| c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &t[level..];
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    Some((level, rest.trim().trim_end_matches('#').trim()))
}

fn starts_block(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with('>') || heading(line).is_some() || marker(line).is_some()
}

/// Block-level markdown to HTML. `tight` renders a lone paragraph without
/// `<p>`, as a list item's text reads.
fn blocks(lines: &[&str], ctx: &Ctx, tight: bool, out: &mut String) {
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if t.is_empty() {
            i += 1;
            continue;
        }
        if t.starts_with("```") {
            let mut j = i + 1;
            let mut code = Vec::new();
            while j < lines.len() && !lines[j].trim_start().starts_with("```") {
                code.push(lines[j]);
                j += 1;
            }
            out.push_str(&format!(
                "<pre><code>{}</code></pre>\n",
                escape(&code.join("\n"))
            ));
            i = j + 1;
            continue;
        }
        if let Some((level, text)) = heading(line) {
            // H1 and H2 belong to the page itself.
            let level = level.max(3);
            out.push_str(&format!("<h{level}>{}</h{level}>\n", inline(text, ctx)));
            i += 1;
            continue;
        }
        if t.starts_with('>') {
            let mut quoted = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let q = lines[i].trim_start()[1..].strip_prefix(' ');
                quoted.push(q.unwrap_or(&lines[i].trim_start()[1..]));
                i += 1;
            }
            out.push_str("<blockquote>\n");
            blocks(&quoted, ctx, false, out);
            out.push_str("</blockquote>\n");
            continue;
        }
        if let Some((ordered, start, _, _)) = marker(line) {
            i = list(lines, i, ordered, start, ctx, out);
            continue;
        }
        // A paragraph: until a blank line or another block.
        let mut para = vec![t];
        i += 1;
        while i < lines.len() && !lines[i].trim().is_empty() && !starts_block(lines[i]) {
            para.push(lines[i].trim());
            i += 1;
        }
        let text = para.join(" ");
        let only_image = text.starts_with("![") && images(&text).len() == 1 && text.ends_with(')');
        if only_image {
            let html = inline(&text, ctx);
            if !html.is_empty() {
                let alt = &text[2..text.find("](").unwrap_or(2)];
                out.push_str(&format!(
                    "<figure>{html}<figcaption>{}</figcaption></figure>\n",
                    escape(alt)
                ));
            }
        } else {
            let html = inline(&text, ctx);
            let html = html.trim();
            if html.is_empty() {
                // Only images shown elsewhere (the before/after pair).
            } else if tight {
                out.push_str(html);
            } else {
                out.push_str(&format!("<p>{html}</p>\n"));
            }
        }
    }
}

/// One list, starting at `lines[i]`; returns the index after it.
fn list(
    lines: &[&str],
    mut i: usize,
    ordered: bool,
    start: usize,
    ctx: &Ctx,
    out: &mut String,
) -> usize {
    let base = indent(lines[i]);
    let (open, close) = if ordered {
        (
            if start == 1 {
                "<ol>".to_string()
            } else {
                format!("<ol start=\"{start}\">")
            },
            "</ol>",
        )
    } else {
        ("<ul>".to_string(), "</ul>")
    };
    out.push_str(&open);
    out.push('\n');
    while i < lines.len() {
        let Some((o, _, text, width)) = marker(lines[i]) else {
            break;
        };
        if o != ordered || indent(lines[i]) != base {
            break;
        }
        // The item: its first line, then every line that is blank or
        // indented past the list's own indent, up to the next item.
        let mut body = vec![text];
        let mut j = i + 1;
        while j < lines.len() {
            let l = lines[j];
            if l.trim().is_empty() {
                // A blank line ends the item unless indented content follows.
                let next = lines[j + 1..].iter().find(|l| !l.trim().is_empty());
                if next.is_some_and(|n| indent(n) > base) {
                    body.push("");
                    j += 1;
                    continue;
                }
                break;
            }
            if indent(l) <= base && (marker(l).is_some() || starts_block(l)) {
                break;
            }
            let strip = indent(l).min(width);
            body.push(&l[strip..]);
            j += 1;
        }
        let tight = !body.contains(&"");
        out.push_str("<li>");
        blocks(&body, ctx, tight, out);
        out.push_str("</li>\n");
        i = j;
        // Blank lines between items keep the list going.
        while i < lines.len() && lines[i].trim().is_empty() {
            let next = lines[i + 1..].iter().find(|l| !l.trim().is_empty());
            if next.is_some_and(|n| indent(n) == base && marker(n).is_some_and(|m| m.0 == ordered))
            {
                i += 1;
            } else {
                break;
            }
        }
    }
    out.push_str(close);
    out.push('\n');
    i
}

/// Plain text of a line of inline markdown, for `<title>`: tags stripped
/// from our own rendering (the text in it is already escaped).
fn plain(s: &str) -> String {
    let html = inline(s, &Ctx { paired: None });
    let mut out = String::new();
    let mut tag = false;
    for c in html.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn slug(name: &str) -> String {
    normalise(name).replace(' ', "-")
}

/// What the page is made from.
pub struct Page<'a> {
    /// `repo@shortsha`.
    pub label: &'a str,
    pub commit: &'a str,
    pub url: &'a str,
}

/// The first line of "Where we are", list marker and markup aside.
fn headline(g: &Guide) -> String {
    let Some(s) = g.section(SECTIONS[0]) else {
        return String::new();
    };
    let first = s
        .body
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let first = bullet(first)
        .or_else(|| numbered(first).map(|(_, t)| t))
        .unwrap_or(first);
    plain(first)
}

/// The guide as one self-contained HTML page: inline CSS, no script, no
/// external asset; images are the guide's own, relative to the page.
pub fn render(text: &str, page: &Page) -> String {
    let g = parse(text);
    let paired = pair(text);
    let ctx = Ctx {
        paired: paired.as_ref(),
    };
    let headline = headline(&g);
    let title = if headline.is_empty() {
        escape(page.label)
    } else {
        format!("{} — {headline}", escape(page.label))
    };
    let mut body = String::new();
    // The preamble, less any H1: the page has its own.
    let pre: Vec<&str> = g
        .preamble
        .iter()
        .copied()
        .filter(|l| !l.trim_start().starts_with("# "))
        .collect();
    if pre.iter().any(|l| !l.trim().is_empty()) {
        body.push_str("<section class=\"intro\">\n");
        blocks(&pre, &ctx, false, &mut body);
        body.push_str("</section>\n");
    }
    // The required sections in their order, then any other in the guide's.
    let mut order: Vec<&Section> = SECTIONS.iter().filter_map(|n| g.section(n)).collect();
    for s in &g.sections {
        if !order.iter().any(|o| std::ptr::eq(*o, s)) {
            order.push(s);
        }
    }
    let reference = normalise(SECTIONS[REFERENCE]);
    for s in order {
        let id = slug(s.heading);
        body.push_str(&format!(
            "<section id=\"{}\">\n<h2>{}</h2>\n",
            escape(&id),
            inline(s.heading, &ctx)
        ));
        if normalise(s.heading) == reference {
            if let Some((before, after)) = &paired {
                body.push_str(&format!(
                    "<div class=\"pair\">\n<figure>{}<figcaption>Before</figcaption></figure>\n<figure>{}<figcaption>After</figcaption></figure>\n</div>\n",
                    img("Before", before),
                    img("After", after)
                ));
            }
        }
        blocks(&s.body, &ctx, false, &mut body);
        body.push_str("</section>\n");
    }
    let url = safe_url(page.url).unwrap_or("#");
    format!(
        "<!doctype html>
<html lang=\"en\">
<head>
<meta charset=\"utf-8\">
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
<meta name=\"color-scheme\" content=\"light dark\">
<title>{title}</title>
<style>{STYLE}</style>
</head>
<body>
<main>
<header>
<p class=\"label\">{label}</p>
<h1>{h1}</h1>
<p><a class=\"open\" href=\"{url}\" target=\"_blank\" rel=\"noopener\">Open the app</a></p>
<p class=\"url\"><code>{url_text}</code></p>
</header>
{body}<footer>Guide for commit <code>{commit}</code> · rendered by <code>amont-agent preview register</code></footer>
</main>
</body>
</html>
",
        label = escape(page.label),
        h1 = if headline.is_empty() {
            escape(page.label)
        } else {
            headline.clone()
        },
        url = escape(url),
        url_text = escape(page.url),
        commit = escape(page.commit),
    )
}

const STYLE: &str = "
:root{--bg:#fbfbfa;--fg:#1d1d1f;--muted:#5f6368;--line:#dcdcdc;--card:#fff;--accent:#0b57d0;--accent-fg:#fff;--code:#f1f1ef}
@media (prefers-color-scheme: dark){:root{--bg:#161618;--fg:#ececec;--muted:#a0a0a8;--line:#35353a;--card:#1f1f22;--accent:#8ab4f8;--accent-fg:#0b0b0c;--code:#2a2a2e}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.55 system-ui,-apple-system,'Segoe UI',sans-serif}
main{max-width:980px;margin:0 auto;padding:24px 16px 48px}
header{border-bottom:1px solid var(--line);padding-bottom:16px;margin-bottom:8px}
.label{margin:0;color:var(--muted);font:600 14px/1.4 ui-monospace,SFMono-Regular,Menlo,monospace}
h1{font-size:26px;line-height:1.25;margin:6px 0 16px}
h2{font-size:20px;margin:0 0 8px}
h3{font-size:17px;margin:16px 0 6px}
section{padding:16px 0;border-bottom:1px solid var(--line)}
a{color:var(--accent)}
a.open{display:inline-block;background:var(--accent);color:var(--accent-fg);text-decoration:none;font-weight:700;font-size:20px;padding:12px 24px;border-radius:10px}
a.open:hover{filter:brightness(1.08)}
.url{margin:8px 0 0;color:var(--muted);overflow-wrap:anywhere}
code{font:0.9em ui-monospace,SFMono-Regular,Menlo,monospace;background:var(--code);padding:1px 5px;border-radius:4px}
pre{background:var(--code);padding:12px;border-radius:8px;overflow-x:auto}
pre code{padding:0;background:none}
ol,ul{padding-left:24px}
li{margin:6px 0}
#try-it ol>li{margin:10px 0}
blockquote{margin:8px 0;padding:4px 12px;border-left:3px solid var(--line);color:var(--muted)}
img{max-width:100%;height:auto;border:1px solid var(--line);border-radius:8px;background:var(--card)}
figure{margin:12px 0}
figcaption{color:var(--muted);font-size:14px;margin-top:4px}
.pair{display:grid;grid-template-columns:1fr 1fr;gap:16px}
.pair figure{margin:0}
@media (max-width:640px){.pair{grid-template-columns:1fr}}
footer{margin-top:24px;color:var(--muted);font-size:14px}
";

/// A `file://` URL for an absolute path, percent-encoded. A Windows
/// verbatim prefix (`\\?\`) is dropped and backslashes become slashes.
pub fn file_url(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let raw = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    let mut p = raw.replace('\\', "/");
    if !p.starts_with('/') {
        p.insert(0, '/');
    }
    let mut out = String::from("file://");
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Relative image sources that do not exist beside the guide.
pub fn missing_images(text: &str, dir: &Path) -> Vec<String> {
    images(text)
        .into_iter()
        .filter(|s| safe_url(s).is_some() && !s.contains(':') && !s.starts_with('/'))
        .filter(|s| !dir.join(s).is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "\
# ignored title

## 📍 Where we are
duro-app, branch `feat/x`, plan **Settings**.

## What you should see
The **Save** button moved.

## 👉 Try it
1. Open http://localhost:5173/settings
   - You should see the form.
2. Click **Save**, top right.

## Reference
![before](shots/before.png) ![after](shots/after.png)

## Already checked
- console clean
";

    #[test]
    fn a_complete_guide_passes() {
        assert!(check(GOOD).is_empty(), "{:?}", check(GOOD));
    }

    #[test]
    fn headings_match_without_emoji_or_case() {
        assert_eq!(normalise("📍 Where we are:"), "where we are");
        assert_eq!(normalise("TRY IT"), "try it");
    }

    #[test]
    fn a_heading_with_a_trailing_note_matches() {
        for (heading, name) in [
            ("👉 Try it (about 2 minutes)", "Try it"),
            ("Try it — 2 min", "Try it"),
            ("Reference - screenshots", "Reference"),
            ("📍 Where we are: the masthead", "Where we are"),
        ] {
            assert!(names_section(heading, name), "{heading:?} names {name:?}");
        }
        let text = GOOD.replace("## 👉 Try it\n", "## 👉 Try it (about 2 minutes)\n");
        assert_ne!(text, GOOD, "the fixture has a `## 👉 Try it` heading");
        let m = check(&text);
        assert!(!m.iter().any(|m| m.contains("Try it")), "{m:?}");
    }

    #[test]
    fn extra_words_are_not_a_note() {
        for (heading, name) in [
            ("Try it now", "Try it"),
            ("Reference material", "Reference"),
            ("Try items", "Try it"),
            ("Already", "Already checked"),
        ] {
            assert!(!names_section(heading, name), "{heading:?} is not {name:?}");
        }
    }

    #[test]
    fn a_missing_section_is_named() {
        let text = GOOD.replace("## Already checked", "## Checked");
        let m = check(&text);
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].contains("Already checked"));
    }

    #[test]
    fn a_heading_inside_a_fence_is_not_a_section() {
        let text = GOOD.replace(
            "## Already checked\n- console clean\n",
            "```\n## Already checked\n```\n",
        );
        assert!(check(&text).iter().any(|m| m.contains("Already checked")));
    }

    #[test]
    fn inline_markup_is_escaped_and_unsafe_links_are_text() {
        let ctx = Ctx { paired: None };
        assert_eq!(
            inline("<script>x</script>", &ctx),
            "&lt;script&gt;x&lt;/script&gt;"
        );
        assert!(!inline("[x](javascript:alert(1))", &ctx).contains("<a"));
        assert_eq!(
            inline("see http://localhost:1/a.", &ctx),
            "see <a href=\"http://localhost:1/a\">http://localhost:1/a</a>."
        );
        assert_eq!(
            inline("**b** `c<`", &ctx),
            "<strong>b</strong> <code>c&lt;</code>"
        );
    }

    #[test]
    fn steps_nest_their_expectations() {
        let html = render(
            GOOD,
            &Page {
                label: "app@abc1234",
                commit: "abc",
                url: "http://localhost:5173/",
            },
        );
        assert!(html.contains("<ol>\n<li>Open"), "{html}");
        assert!(
            html.contains("<ul>\n<li>You should see the form."),
            "{html}"
        );
        assert!(html.contains("<div class=\"pair\">"), "{html}");
        assert!(
            html.contains("<title>app@abc1234 — duro-app, branch feat/x, plan Settings.</title>"),
            "{html}"
        );
    }

    #[test]
    fn file_urls_are_encoded() {
        assert_eq!(file_url(Path::new("/a b/c.html")), "file:///a%20b/c.html");
        assert_eq!(
            file_url(Path::new(r"\\?\C:\x\i.html")),
            "file:///C:/x/i.html"
        );
    }
}
