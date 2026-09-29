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

// --- mockup mode ------------------------------------------------------------

/// The section a guide adds when its commit carries a picked mockup.
pub const DIFFERENCES: &str = "Differences from the mockup";

/// What an image shows, read from its file name: the picked design, the
/// built screen, or the screen before the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Design,
    Built,
    Before,
}

/// The role word in a file name and what is left of its stem around it:
/// `artboard-empty.png` → (Design, `-empty`). The first matching word wins.
fn role(src: &str) -> Option<(Role, String)> {
    let name = file_name(src);
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&name);
    [
        ("artboard", Role::Design),
        ("mockup", Role::Design),
        ("after", Role::Built),
        ("live", Role::Built),
        ("before", Role::Before),
    ]
    .iter()
    .find_map(|(word, r)| {
        let i = stem.find(word)?;
        Some((*r, format!("{}{}", &stem[..i], &stem[i + word.len()..])))
    })
}

/// A design image and the built screen it is compared with, for one state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    /// What distinguishes this state's file names (`-empty`); empty for the
    /// default pair.
    pub state: String,
    pub design: String,
    pub design_alt: String,
    pub built: String,
    pub built_alt: String,
}

/// Every image in the guide with its alt text.
fn images_with_alt(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("![") {
        rest = &rest[i + 2..];
        let Some((alt, src, after)) = link_parts(rest) else {
            continue;
        };
        found.push((alt.to_string(), src.to_string()));
        rest = after;
    }
    found
}

/// The design/built pairs, matched by what their names share besides the
/// role word: `artboard.png` with `after.png`, `artboard-empty.png` with
/// `live-empty.png`.
pub fn proofs(text: &str) -> Vec<Proof> {
    let all = images_with_alt(text);
    let mut out: Vec<Proof> = Vec::new();
    for (alt, src) in &all {
        let Some((Role::Design, key)) = role(src) else {
            continue;
        };
        if out.iter().any(|p| p.state == key) {
            continue;
        }
        let built = all
            .iter()
            .find(|(_, s)| role(s) == Some((Role::Built, key.clone())));
        if let Some((balt, bsrc)) = built {
            out.push(Proof {
                state: key,
                design: src.clone(),
                design_alt: alt.clone(),
                built: bsrc.clone(),
                built_alt: balt.clone(),
            });
        }
    }
    out
}

/// The image the guide shows as the screen before the change, if any.
fn before_image(text: &str) -> Option<(String, String)> {
    images_with_alt(text)
        .into_iter()
        .find(|(_, s)| matches!(role(s), Some((Role::Before, _))))
}

/// A local image source as a file: relative to the guide's folder, or an
/// absolute path, or a `file://` URL. A remote URL is `None`.
fn local_file(src: &str, folder: &Path) -> Option<std::path::PathBuf> {
    let s = src.trim();
    if let Some(p) = s.strip_prefix("file://") {
        return Some(std::path::PathBuf::from(p));
    }
    if s.starts_with('/') {
        return Some(std::path::PathBuf::from(s));
    }
    if s.contains(':') {
        return None;
    }
    Some(folder.join(s))
}

/// The `Viewport: <w>px · <theme>` line, if a body carries one.
fn viewport(lines: &[&str]) -> Option<String> {
    lines.iter().find_map(|l| {
        let low = l.to_ascii_lowercase();
        let i = low.find("viewport")?;
        let rest = low[i + "viewport".len()..].trim_start_matches(['*', ':', ' ']);
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        (digits > 0 && rest[digits..].trim_start().starts_with("px"))
            .then(|| plain(l.trim().trim_start_matches(['-', '*', ' '])))
    })
}

/// How the differences section reads: `Ok` for `None` or bullets that each
/// say `fixed:` or `deliberate: <reason>`, else what is wrong.
fn differences(lines: &[&str]) -> Result<(), String> {
    let mut bullets = 0;
    let mut unclassified = 0;
    let mut none = false;
    for l in lines {
        let low = l.trim().to_ascii_lowercase();
        if low.trim_end_matches('.') == "none" {
            none = true;
            continue;
        }
        let Some(b) = bullet(l) else { continue };
        bullets += 1;
        let b = b.trim().to_ascii_lowercase();
        let b = b.trim_start_matches("**");
        let classified = ["fixed", "deliberate"].iter().any(|w| {
            b.strip_prefix(w)
                .map(|r| r.trim_start_matches("**"))
                .and_then(|r| r.strip_prefix(':'))
                .is_some_and(|r| !r.trim().is_empty())
        });
        if !classified {
            unclassified += 1;
        }
    }
    if unclassified > 0 {
        return Err(format!(
            "{unclassified} bullet(s) under `## {DIFFERENCES}` say neither `fixed: …` nor `deliberate: <reason>`"
        ));
    }
    if bullets == 0 && !none {
        return Err(format!(
            "`## {DIFFERENCES}` lists nothing: write `None`, or one bullet per difference (`- fixed: …` / `- deliberate: <reason>`)"
        ));
    }
    Ok(())
}

/// What a guide lacks in mockup mode — its commit carries the picked
/// artboards of `screens` — on top of [`check`]. Images resolve against
/// `folder`, the guide's own.
pub fn check_mockup(text: &str, screens: &[String], folder: &Path) -> Vec<String> {
    let g = parse(text);
    let mut missing = Vec::new();
    let reference: Vec<&str> = g
        .section(SECTIONS[REFERENCE])
        .map(|s| s.body.clone())
        .unwrap_or_default();
    for screen in screens {
        if !reference.iter().any(|l| l.contains(screen.as_str())) {
            missing.push(format!("the screen's path `{screen}` under `## Reference`"));
        }
    }
    if viewport(&reference).is_none() {
        missing.push(
            "a viewport line under `## Reference`: `Viewport: 1120px · light`, the width and theme both images were taken at".to_string(),
        );
    }
    let found = proofs(text);
    if !found.iter().any(|p| p.state.is_empty()) {
        missing.push(format!(
            "the picked artboard beside the built screen under `## Reference`: `artboard.png` and `after.png` in {}",
            folder.display()
        ));
    }
    for p in &found {
        for src in [&p.design, &p.built] {
            match local_file(src, folder) {
                Some(f) if f.is_file() => {}
                Some(f) => missing.push(format!(
                    "the image {} (the guide shows `{src}`)",
                    f.display()
                )),
                None => missing.push(format!(
                    "`{src}` as a file beside the guide: a remote image cannot be checked"
                )),
            }
        }
    }
    match g.section(DIFFERENCES) {
        None => missing.push(format!("the section `## {DIFFERENCES}`")),
        Some(s) => {
            if let Err(why) = differences(&s.body) {
                missing.push(why);
            }
        }
    }
    missing
}

/// Width of a PNG, from its IHDR chunk; `None` for anything else.
pub fn png_width(path: &Path) -> Option<u32> {
    use std::io::Read;
    let mut head = [0u8; 24];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if head[..8] != [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] || &head[12..16] != b"IHDR" {
        return None;
    }
    Some(u32::from_be_bytes([head[16], head[17], head[18], head[19]]))
}

/// Pairs whose two PNGs differ in width by more than a tenth: a divergence
/// would read as a scaling difference.
pub fn width_warnings(text: &str, folder: &Path) -> Vec<String> {
    proofs(text)
        .iter()
        .filter_map(|p| {
            let d = png_width(&local_file(&p.design, folder)?)?;
            let b = png_width(&local_file(&p.built, folder)?)?;
            let (lo, hi) = (d.min(b), d.max(b));
            (hi - lo > hi / 10).then(|| {
                format!(
                    "{} is {d}px wide and {} is {b}px: take the built screen at the artboard's width",
                    p.design, p.built
                )
            })
        })
        .collect()
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
    /// Sources rendered in the Reference block, so skipped inline.
    paired: &'p [String],
}

impl Ctx<'_> {
    fn is_paired(&self, src: &str) -> bool {
        self.paired.iter().any(|s| s == src)
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
    let html = inline(s, &Ctx { paired: &[] });
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
    let found = proofs(text);
    let paired = if found.is_empty() { pair(text) } else { None };
    let before = if found.is_empty() {
        None
    } else {
        before_image(text)
    };
    let mut skipped: Vec<String> = Vec::new();
    if let Some((b, a)) = &paired {
        skipped.extend([b.clone(), a.clone()]);
    }
    for p in &found {
        skipped.extend([p.design.clone(), p.built.clone()]);
    }
    if let Some((_, b)) = &before {
        skipped.push(b.clone());
    }
    let ctx = Ctx { paired: &skipped };
    let direction = direction(text);
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
            let view = viewport(&s.body).map(|v| escape(&v)).unwrap_or_default();
            for (i, p) in found.iter().enumerate() {
                let state = p.state.trim_matches(['-', '_', ' ']);
                let design_caption = match &direction {
                    Some(d) => format!("Mockup, direction {d}"),
                    None => "Mockup".to_string(),
                };
                body.push_str(&format!(
                    "<div class=\"proof\">\n{state_line}<input type=\"checkbox\" id=\"overlay-{i}\" class=\"overlay\"><label for=\"overlay-{i}\">Overlay the built screen on the mockup</label>\n<div class=\"stack\">\n<figure class=\"design\"><a href=\"{dsrc}\">{dimg}</a><figcaption>{dcap}{view_line}</figcaption></figure>\n<figure class=\"built\"><a href=\"{bsrc}\">{bimg}</a><figcaption>Built, <code>{sha}</code>{view_line}</figcaption></figure>\n</div>\n</div>\n",
                    state_line = if state.is_empty() {
                        String::new()
                    } else {
                        format!("<p class=\"state\">State: {}</p>\n", escape(state))
                    },
                    dsrc = escape(&p.design),
                    dimg = img(alt_or(&p.design_alt, &design_caption), &p.design),
                    dcap = escape(&design_caption),
                    bsrc = escape(&p.built),
                    bimg = img(alt_or(&p.built_alt, "Built screen"), &p.built),
                    sha = escape(&page.commit[..page.commit.len().min(7)]),
                    view_line = if view.is_empty() { String::new() } else { format!(" · {view}") },
                ));
            }
            if let Some((alt, src)) = &before {
                body.push_str(&format!(
                    "<figure class=\"before\"><a href=\"{}\">{}</a><figcaption>Before</figcaption></figure>\n",
                    escape(src),
                    img(alt_or(alt, "Before"), src)
                ));
            }
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
.proof{margin:12px 0 20px}
.proof .state{margin:0 0 6px;font-weight:600}
.proof label{display:inline-block;margin:0 0 8px;color:var(--muted);font-size:14px;cursor:pointer}
.proof .overlay{margin-right:6px}
.stack{display:grid;grid-template-columns:1fr;gap:16px}
.stack figure{margin:0}
.stack img{width:100%}
.overlay:checked+label+.stack{gap:0}
.overlay:checked+label+.stack figure{grid-area:1/1}
.overlay:checked+label+.stack .built img{opacity:.5}
.overlay:checked+label+.stack .built figcaption{display:none}
.pair{display:grid;grid-template-columns:1fr 1fr;gap:16px}
.pair figure{margin:0}
@media (max-width:640px){.pair{grid-template-columns:1fr}}
footer{margin-top:24px;color:var(--muted);font-size:14px}
";

/// The alt text as written, or the role when the guide left it empty.
fn alt_or<'a>(alt: &'a str, role: &'a str) -> &'a str {
    if alt.trim().is_empty() {
        role
    } else {
        alt
    }
}

/// The picked direction a guide names (`direction B`), if any.
fn direction(text: &str) -> Option<String> {
    let low = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = low[from..].find("direction ") {
        let at = from + i + "direction ".len();
        let word: String = text[at..]
            .chars()
            .take_while(|c| c.is_alphanumeric())
            .collect();
        if word.chars().count() == 1 && word.chars().all(|c| c.is_ascii_alphabetic()) {
            return Some(word.to_ascii_uppercase());
        }
        from = at;
    }
    None
}

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

    const MOCKUP_GUIDE: &str = "\
## Where we are
app, branch `feat/x`, direction B.

## What you should see
A pill.

## Try it
1. Open http://localhost:5173/

## Reference
Artboards: docs/mockups/pill
Viewport: 1120px · light

![Mockup B](artboard.png) ![Built](after.png)

## Already checked
- console clean

## Differences from the mockup
- fixed: the separate History segment is gone
- deliberate: a hidden live region copy, for screen readers
";

    fn folder_with(files: &[&str]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "amont-agent-guide-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for f in files {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        d
    }

    #[test]
    fn a_complete_mockup_guide_lacks_nothing() {
        let d = folder_with(&["artboard.png", "after.png"]);
        let m = check_mockup(MOCKUP_GUIDE, &["docs/mockups/pill".to_string()], &d);
        assert!(m.is_empty(), "{m:?}");
        assert!(check(MOCKUP_GUIDE).is_empty());
    }

    #[test]
    fn mockup_mode_names_every_missing_piece() {
        let d = folder_with(&[]);
        let m = check_mockup(GOOD, &["docs/mockups/pill".to_string()], &d);
        let all = m.join("\n");
        for piece in [
            "docs/mockups/pill",
            "viewport line",
            "artboard.png",
            "## Differences from the mockup",
        ] {
            assert!(all.contains(piece), "{piece} in {all}");
        }
    }

    #[test]
    fn a_missing_image_is_refused_even_by_absolute_path() {
        let d = folder_with(&["after.png"]);
        let text = MOCKUP_GUIDE.replace("(artboard.png)", "(/nonexistent/artboard.png)");
        let m = check_mockup(&text, &["docs/mockups/pill".to_string()], &d);
        assert!(
            m.iter().any(|m| m.contains("/nonexistent/artboard.png")),
            "{m:?}"
        );
    }

    #[test]
    fn differences_must_be_classified_or_none() {
        assert!(differences(&["None"]).is_ok());
        assert!(differences(&["- fixed: spacing", "- **deliberate:** a11y copy"]).is_ok());
        assert!(differences(&["- spacing is off"])
            .unwrap_err()
            .contains("1 bullet"));
        assert!(
            differences(&["- deliberate:"]).is_err(),
            "a reason is required"
        );
        assert!(differences(&[]).is_err());
    }

    #[test]
    fn proofs_pair_by_state_and_accept_the_aliases() {
        let text = "![a](shots/mockup.png) ![b](live.png) ![c](artboard-empty.png) ![d](after-empty.png) ![e](before.png)";
        let p = proofs(text);
        assert_eq!(p.len(), 2, "{p:?}");
        assert_eq!(
            (p[0].design.as_str(), p[0].built.as_str()),
            ("shots/mockup.png", "live.png")
        );
        assert_eq!(p[1].state, "-empty");
        assert_eq!(
            before_image(text).map(|(_, s)| s),
            Some("before.png".to_string())
        );
    }

    #[test]
    fn the_page_shows_the_pair_full_width_with_an_overlay() {
        let html = render(
            MOCKUP_GUIDE,
            &Page {
                label: "app@abcdef0",
                commit: "abcdef0123",
                url: "http://localhost:5173/",
            },
        );
        assert!(
            html.contains("Mockup, direction B"),
            "caption names the direction"
        );
        assert!(html.contains("Built, <code>abcdef0</code>"));
        assert!(html.contains("Viewport: 1120px · light"));
        assert!(html.contains("class=\"overlay\""));
        assert!(
            html.contains("<a href=\"artboard.png\">"),
            "links to the full size"
        );
        assert_eq!(
            html.matches("src=\"artboard.png\"").count(),
            1,
            "not repeated inline"
        );
    }

    #[test]
    fn a_png_width_is_read_from_its_header() {
        let d = folder_with(&[]);
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
        png.extend(b"IHDR");
        png.extend(1120u32.to_be_bytes());
        png.extend(800u32.to_be_bytes());
        std::fs::write(d.join("a.png"), &png).unwrap();
        assert_eq!(png_width(&d.join("a.png")), Some(1120));
        std::fs::write(d.join("b.png"), b"not a png at all, just text").unwrap();
        assert_eq!(png_width(&d.join("b.png")), None);
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
        let ctx = Ctx { paired: &[] };
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
