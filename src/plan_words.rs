//! What a plan says, read as CommonMark: the text a review binds to.
//!
//! A plan's sha used to be taken over its bytes, so a formatter that only
//! added blank lines before nested lists (website-builder's prettier hook,
//! 2026-10-06) made a reviewed plan look edited. [`words`] reads the plan
//! the way a Markdown renderer does and keeps what a reader would see:
//!
//! * `w:` a prose word, after escapes and emphasis are resolved and line
//!   breaks are read as spaces, so reflow and `*x*`/`_x_` do not count;
//! * `s:` a code span and `c:` a code block, exactly as parsed: blank lines,
//!   indentation inside the code and spaces inside a span all count;
//! * `u:` a link or image destination;
//! * `k:` the kind of a block: a list item, a quote, a heading, a table
//!   row or cell, an ordered list's start number, a code block's info
//!   string. A plain paragraph or list boundary is a bare `k:`, absorbed by
//!   any neighbouring `k:` item, so a tight list and the same list made
//!   loose by blank lines read the same, while `- deny` and `deny` do not.
//!
//! Structure decides through the parser, never by a token's place on a line:
//! `a * b` keeps its operator, and `a` followed by a line `* b` is a
//! paragraph and a list item. A line rule cannot tell those apart; that is
//! why this needs a parser (the person's review of the plan, 2026-10-06).

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// A block boundary that says nothing about its kind.
const BARE: &str = "k:";

/// The plan's items, one per line. Deterministic for a given parser version,
/// which is why `pulldown-cmark` is pinned exactly.
pub fn words(markdown: &str) -> String {
    // Smart punctuation stays off: it would rewrite quotes inside prose.
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut out = Items::default();
    let mut code: Option<String> = None;
    for event in Parser::new_ext(markdown, options) {
        // A code block's text arrives in as many events as the parser likes;
        // it is collected into one item so that the split never counts.
        if let Some(block) = code.as_mut() {
            if let Event::End(TagEnd::CodeBlock) = event {
                out.push(format!("c:{block}"));
                code = None;
                out.kind(BARE);
            } else if let Event::Text(t) = event {
                block.push_str(&t);
            }
            // CommonMark gives a code block only text; anything else inside
            // one would be a parser change, which the fixture sha catches.
            continue;
        }
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let info = match kind {
                    CodeBlockKind::Fenced(info) => info.trim().to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                out.kind(&format!("k:code {info}"));
                code = Some(String::new());
            }
            Event::Text(t) => out.prose.push_str(&t),
            Event::Code(c) | Event::InlineMath(c) | Event::DisplayMath(c) => {
                out.push(format!("s:{c}"));
            }
            Event::FootnoteReference(f) => out.push(format!("w:[^{f}]")),
            Event::SoftBreak | Event::HardBreak => out.prose.push(' '),
            Event::Html(h) | Event::InlineHtml(h) => out.prose.push_str(&h),
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                out.push(format!("u:{dest_url}"));
            }
            Event::Start(Tag::Emphasis | Tag::Strong)
            | Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Link | TagEnd::Image) => {}
            Event::Start(Tag::Strikethrough) | Event::End(TagEnd::Strikethrough) => {
                out.prose.push_str(" ~~ ");
            }
            Event::TaskListMarker(done) => out.prose.push_str(if done { " [x] " } else { " [ ] " }),
            Event::Start(Tag::List(Some(start))) => out.kind(&format!("k:list {start}")),
            Event::Start(Tag::Item) => out.kind("k:item"),
            Event::Start(Tag::BlockQuote(_)) => out.kind("k:quote"),
            Event::Start(Tag::Heading { .. }) => out.kind("k:heading"),
            Event::Start(Tag::TableRow | Tag::TableHead) => out.kind("k:row"),
            Event::Start(Tag::TableCell) => out.kind("k:cell"),
            // Every other block, and every end: a boundary of no kind. A tag
            // a parser bump adds lands here too, as a boundary, never as
            // nothing; the `Event` match itself is exhaustive.
            Event::Start(_) | Event::End(_) | Event::Rule => out.kind(BARE),
        }
    }
    out.flush();
    out.items.join("\n")
}

#[derive(Default)]
struct Items {
    items: Vec<String>,
    /// Prose not yet split into words: a word may span several text events
    /// (`a\_b` arrives as `a`, `_b`).
    prose: String,
}

impl Items {
    fn flush(&mut self) {
        for w in self.prose.split_whitespace() {
            self.items.push(format!("w:{w}"));
        }
        self.prose.clear();
    }

    fn push(&mut self, item: String) {
        self.flush();
        self.items.push(item);
    }

    /// A block boundary. A bare one next to any other boundary adds nothing,
    /// and a kind replaces a bare one before it.
    fn kind(&mut self, kind: &str) {
        self.flush();
        let last_is_kind = self.items.last().is_some_and(|s| s.starts_with(BARE));
        if kind == BARE {
            if !last_is_kind {
                self.items.push(BARE.to_string());
            }
            return;
        }
        if self.items.last().is_some_and(|s| s == BARE) {
            self.items.pop();
        }
        self.items.push(kind.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::words;

    /// Pairs a formatter produces, which must read the same.
    const SAME: &[(&str, &str, &str)] = &[
        (
            "blank lines",
            "# T\nOne.\nTwo.\n",
            "# T\n\nOne.\nTwo.\n\n\n",
        ),
        (
            "blank line before a nested list",
            "- a\n  - b\n",
            "- a\n\n  - b\n",
        ),
        (
            "loose nested list",
            "- a\n  - b\n- c\n",
            "- a\n\n  - b\n\n- c\n",
        ),
        ("reflow", "one two\nthree\n", "one\ntwo three\n"),
        (
            "table padding",
            "|a|b|\n|-|-|\n|1|2|\n",
            "| a   | b   |\n| --- | --- |\n| 1   | 2   |\n",
        ),
        ("emphasis style", "x *y* z\n", "x _y_ z\n"),
        ("quoted emphasis", "*\"x\"*\n", "_\"x\"_\n"),
        ("strong style", "**x**\n", "__x__\n"),
        ("escape", "a\\_b\n", "a_b\n"),
        ("list marker", "* a\n* b\n", "- a\n- b\n"),
        (
            "fence re-indented with its item",
            "-   x\n\n    ```py\n    if a:\n        go()\n    ```\n",
            "- x\n\n  ```py\n  if a:\n      go()\n  ```\n",
        ),
    ];

    /// Edits that change what the plan says, which must read differently.
    const DIFFERENT: &[(&str, &str, &str)] = &[
        ("a word", "Body.\n", "Body!\n"),
        (
            "operator before a break",
            "Calculate a * b\n",
            "Calculate a\n* b\n",
        ),
        (
            "list item vs words",
            "Calculate a b\n",
            "Calculate a\n* b\n",
        ),
        ("plus before a break", "a + b\n", "a\n+ b\n"),
        ("quote before a break", "x > y\n", "x\n> y\n"),
        ("operator", "a > b\n", "a >= b\n"),
        ("operator dropped", "a * b\n", "a b\n"),
        ("identifier", "MAX_BLOBS\n", "MAXBLOBS\n"),
        ("leading underscore", "_private x\n", "private x\n"),
        ("spaces in a span", "`\"a  b\"`\n", "`\"a b\"`\n"),
        ("tab in a span", "`a\tb`\n", "`a\t\tb`\n"),
        (
            "blank line in a string literal",
            "```py\ns = \"\"\"a\n\nb\"\"\"\n```\n",
            "```py\ns = \"\"\"a\nb\"\"\"\n```\n",
        ),
        (
            "indent inside a fence",
            "```py\nif a:\n    go()\n```\n",
            "```py\nif a:\n  go()\n```\n",
        ),
        ("fence language", "```py\nx\n```\n", "```sh\nx\n```\n"),
        ("number", "5 ms\n", "50 ms\n"),
        ("swapped words", "allow then deny\n", "deny then allow\n"),
        ("link destination", "[x](a.md)\n", "[x](b.md)\n"),
        ("paragraph vs item", "x\n\n- deny\n", "x\n\ndeny\n"),
        ("paragraph vs quote", "> deny\n", "deny\n"),
        ("ordered start", "5. a\n", "50. a\n"),
        ("span vs word", "`deny`\n", "deny\n"),
        (
            "table rows",
            "|a|b|\n|-|-|\n|1|2|\n",
            "|a|b|\n|-|-|\n|1|\n|2|\n",
        ),
    ];

    #[test]
    fn formatting_alone_reads_the_same() {
        for (why, a, b) in SAME {
            assert_eq!(words(a), words(b), "{why}");
        }
    }

    #[test]
    fn a_change_of_meaning_reads_differently() {
        for (why, a, b) in DIFFERENT {
            assert_ne!(words(a), words(b), "{why}");
        }
    }
}
