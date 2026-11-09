//! Markdown source → a small block/inline document model.
//!
//! Parsing happens in two passes:
//!
//! 1. A **math pre-pass** finds `$…$`, `\(…\)`, `$$…$$` and `\[…\]` outside code
//!    and replaces each equation with one private-use placeholder character.
//!    That stops markdown emphasis from eating `_` and `*` inside TeX, and lets
//!    a surrounding `<span>` style the equation like any other text.
//! 2. `pulldown-cmark` (CommonMark + tables, strikethrough and task lists)
//!    produces events, which are folded into [`Block`]s and [`Inline`]s. The
//!    only HTML interpreted is `<span style="…">`; every other tag stays text.
//!
//! The model is deliberately small: it is exactly what the note layout draws.
//! Nothing here allocates without bound beyond a copy of the input.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::style::{SpanStyle, is_span_close, parse_declarations, span_open};

/// Largest equation the math path accepts, in bytes of TeX.
pub const MAX_EQUATION_BYTES: usize = 4096;

/// Most equations one note may contain; later ones stay literal text.
pub const MAX_EQUATIONS: usize = 128;

/// First private-use code point used for math placeholders.
const PLACEHOLDER_BASE: u32 = 0xF_0000;

/// One equation found by the pre-pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathAtom {
    /// The TeX source, without delimiters.
    pub tex: String,
    /// Whether the equation was written with display delimiters.
    pub display: bool,
}

/// Inline formatting in force for a run of text.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InlineStyle {
    /// Bold weight.
    pub bold: bool,
    /// Italic style.
    pub italic: bool,
    /// Strikethrough decoration.
    pub strike: bool,
    /// Inside a link: underlined, link colour.
    pub link: bool,
    /// Inline code: monospace on a tinted background, literal text.
    pub code: bool,
    /// `<span style>` colour and font size.
    pub span: SpanStyle,
}

/// One inline element.
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    /// Styled text. `\n` is a hard line break.
    Text(String, InlineStyle),
    /// An equation, by index into [`Document::math`].
    Math(usize, InlineStyle),
}

/// One list entry.
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// `Some(checked)` for a task-list item.
    pub task: Option<bool>,
    /// The item's content.
    pub blocks: Vec<Block>,
}

/// One block element.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// A paragraph.
    Paragraph(Vec<Inline>),
    /// An ATX or setext heading, level 1–6.
    Heading(u8, Vec<Inline>),
    /// A fenced or indented code block.
    Code {
        /// The fence's info string, when it had one.
        lang: Option<String>,
        /// The literal code, tabs expanded.
        text: String,
    },
    /// An ordered (`start` set) or unordered list.
    List {
        /// The first number of an ordered list.
        start: Option<u64>,
        /// The entries.
        items: Vec<ListItem>,
    },
    /// A blockquote.
    Quote(Vec<Block>),
    /// A thematic break.
    Rule,
    /// A GFM table.
    Table {
        /// Header cells.
        header: Vec<Vec<Inline>>,
        /// Body rows of cells.
        rows: Vec<Vec<Vec<Inline>>>,
    },
    /// A display equation, by index into [`Document::math`], with the span
    /// style in force where it was written.
    Math(usize, InlineStyle),
}

/// A parsed note.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Document {
    /// Top-level blocks in source order.
    pub blocks: Vec<Block>,
    /// Every equation the pre-pass extracted.
    pub math: Vec<MathAtom>,
}

/// Parses a note's markdown source.
pub fn parse(source: &str) -> Document {
    let (text, math) = extract_math(source);
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut builder = Builder::new(&math);
    for event in Parser::new_ext(&text, options) {
        builder.event(event);
    }
    Document {
        blocks: builder.finish(),
        math,
    }
}

// ---------------------------------------------------------------------------
// Math pre-pass
// ---------------------------------------------------------------------------

/// Replaces every equation outside code with a placeholder character.
pub fn extract_math(source: &str) -> (String, Vec<MathAtom>) {
    let mut out = String::with_capacity(source.len());
    let mut math = Vec::new();
    let mut prose = String::new();
    let mut fence: Option<(char, usize)> = None;
    let mut previous_blank = true;
    let mut in_indented_code = false;

    for line in source.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if let Some((fence_char, fence_len)) = fence {
            out.push_str(line);
            if is_fence(content).is_some_and(|(c, n, rest)| {
                c == fence_char && n >= fence_len && rest.trim().is_empty()
            }) {
                fence = None;
            }
            previous_blank = false;
            continue;
        }
        if let Some((c, n, _)) = is_fence(content) {
            flush_prose(&mut prose, &mut out, &mut math);
            fence = Some((c, n));
            out.push_str(line);
            previous_blank = false;
            continue;
        }
        let blank = content.trim().is_empty();
        let indented = content.starts_with("    ") || content.starts_with('\t');
        if indented && (previous_blank || in_indented_code) && !blank {
            flush_prose(&mut prose, &mut out, &mut math);
            in_indented_code = true;
            out.push_str(line);
            previous_blank = false;
            continue;
        }
        if !blank {
            in_indented_code = false;
        }
        prose.push_str(line);
        previous_blank = blank;
    }
    flush_prose(&mut prose, &mut out, &mut math);
    (out, math)
}

/// A code fence opener: its character, length and the rest of the line.
fn is_fence(line: &str) -> Option<(char, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let c = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = rest.len() - rest.trim_start_matches(c).len();
    (n >= 3).then(|| (c, n, &rest[n..]))
}

fn flush_prose(prose: &mut String, out: &mut String, math: &mut Vec<MathAtom>) {
    if !prose.is_empty() {
        scan_prose(prose, out, math);
        prose.clear();
    }
}

/// Scans prose (no code blocks) for code spans and equations.
fn scan_prose(text: &str, out: &mut String, math: &mut Vec<MathAtom>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'`' => {
                let run = count_run(bytes, i, b'`');
                match find_backtick_run(bytes, i + run, run) {
                    Some(end) => {
                        out.push_str(&text[i..end + run]);
                        i = end + run;
                    }
                    None => {
                        out.push_str(&text[i..i + run]);
                        i += run;
                    }
                }
            }
            b'\\' if i + 1 < bytes.len() => {
                let next = bytes[i + 1];
                let (close, display) = match next {
                    b'(' => ("\\)", false),
                    b'[' => ("\\]", true),
                    _ => {
                        // `\$` and every other escape go to the markdown parser
                        // untouched; it turns `\$` into a literal dollar.
                        let width = text[i + 1..].chars().next().map_or(1, char::len_utf8);
                        out.push_str(&text[i..i + 1 + width]);
                        i += 1 + width;
                        continue;
                    }
                };
                match text[i + 2..].find(close) {
                    Some(offset) if push_atom(&text[i + 2..i + 2 + offset], display, out, math) => {
                        i += 2 + offset + 2;
                    }
                    _ => {
                        out.push_str(&text[i..i + 2]);
                        i += 2;
                    }
                }
            }
            b'$' if bytes.get(i + 1) == Some(&b'$') => match text[i + 2..].find("$$") {
                Some(offset) if push_atom(text[i + 2..i + 2 + offset].trim(), true, out, math) => {
                    i += 2 + offset + 2;
                }
                _ => {
                    out.push_str("$$");
                    i += 2;
                }
            },
            b'$' => match inline_dollar_end(text, i) {
                Some(end) if push_atom(&text[i + 1..end], false, out, math) => i = end + 1,
                _ => {
                    out.push('$');
                    i += 1;
                }
            },
            _ => {
                let width = text[i..].chars().next().map_or(1, char::len_utf8);
                out.push_str(&text[i..i + width]);
                i += width;
            }
        }
    }
}

/// The byte index of the `$` closing an inline equation opened at `open`.
///
/// The opener must not be followed by whitespace, the closer must not be
/// preceded by whitespace or a backslash nor followed by a digit (so "$5 and
/// $10" stays prose), and an equation never crosses a blank line.
fn inline_dollar_end(text: &str, open: usize) -> Option<usize> {
    let after = text[open + 1..].chars().next()?;
    if after.is_whitespace() || after == '$' {
        return None;
    }
    let bytes = text.as_bytes();
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'\n'
                if text[i + 1..]
                    .split('\n')
                    .next()
                    .is_some_and(|line| line.trim().is_empty()) =>
            {
                return None;
            }
            // `$$` never closes an inline equation.
            b'$' if bytes.get(i + 1) == Some(&b'$') => i += 2,
            b'$' => {
                let before = text[..i].chars().next_back()?;
                let digit_after = bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
                if !before.is_whitespace() && !digit_after {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn count_run(bytes: &[u8], start: usize, byte: u8) -> usize {
    bytes[start..].iter().take_while(|b| **b == byte).count()
}

/// Start of the next backtick run of exactly `len` at or after `from`.
fn find_backtick_run(bytes: &[u8], mut from: usize, len: usize) -> Option<usize> {
    while from < bytes.len() {
        if bytes[from] == b'`' {
            let run = count_run(bytes, from, b'`');
            if run == len {
                return Some(from);
            }
            from += run;
        } else {
            from += 1;
        }
    }
    None
}

/// Records an equation and writes its placeholder; `false` (and nothing
/// written) when it is empty or over a limit, so the caller keeps it literal.
fn push_atom(tex: &str, display: bool, out: &mut String, math: &mut Vec<MathAtom>) -> bool {
    if tex.trim().is_empty() || tex.len() > MAX_EQUATION_BYTES || math.len() >= MAX_EQUATIONS {
        if !tex.trim().is_empty() {
            tracing::warn!(
                len = tex.len(),
                "equation over the note limits; kept as text"
            );
        }
        return false;
    }
    let Some(placeholder) = char::from_u32(PLACEHOLDER_BASE + math.len() as u32) else {
        return false;
    };
    math.push(MathAtom {
        tex: tex.to_owned(),
        display,
    });
    out.push(placeholder);
    true
}

/// The equation index a placeholder character stands for.
fn placeholder_index(c: char, count: usize) -> Option<usize> {
    let index = (c as u32).checked_sub(PLACEHOLDER_BASE)? as usize;
    (index < count).then_some(index)
}

// ---------------------------------------------------------------------------
// Event folding
// ---------------------------------------------------------------------------

enum Frame {
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Item {
        task: Option<bool>,
        blocks: Vec<Block>,
    },
    Paragraph {
        inlines: Vec<Inline>,
        implicit: bool,
    },
    Heading(u8, Vec<Inline>),
    Code {
        lang: Option<String>,
        text: String,
    },
    Html(String),
    Table {
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
        row: Vec<Vec<Inline>>,
        in_head: bool,
    },
    Cell(Vec<Inline>),
}

struct Builder<'m> {
    math: &'m [MathAtom],
    root: Vec<Block>,
    stack: Vec<Frame>,
    bold: u32,
    italic: u32,
    strike: u32,
    link: u32,
    spans: Vec<SpanStyle>,
    /// Alt text being collected for an image; `Some` while inside one.
    image: Option<String>,
}

impl<'m> Builder<'m> {
    fn new(math: &'m [MathAtom]) -> Self {
        Self {
            math,
            root: Vec::new(),
            stack: Vec::new(),
            bold: 0,
            italic: 0,
            strike: 0,
            link: 0,
            spans: Vec::new(),
            image: None,
        }
    }

    fn style(&self) -> InlineStyle {
        InlineStyle {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            link: self.link > 0,
            code: false,
            span: self
                .spans
                .iter()
                .fold(SpanStyle::default(), |outer, inner| outer.layered(*inner)),
        }
    }

    fn event(&mut self, event: Event<'_>) {
        if let Some(alt) = self.image.as_mut() {
            match event {
                Event::End(TagEnd::Image) => {
                    let alt = std::mem::take(alt);
                    self.image = None;
                    let style = self.style();
                    self.push_text(&format!("[{alt}]"), style);
                }
                Event::Text(text) | Event::Code(text) => alt.push_str(&text),
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if let Some(Frame::Code { text: code, .. }) = self.stack.last_mut() {
                    code.push_str(&text.replace('\t', "    "));
                } else if let Some(Frame::Html(html)) = self.stack.last_mut() {
                    html.push_str(&text);
                } else {
                    let style = self.style();
                    self.push_text(&text, style);
                }
            }
            Event::Code(text) => {
                let mut style = self.style();
                style.code = true;
                self.push_inline(Inline::Text(text.into_string(), style));
            }
            Event::InlineHtml(html) => self.inline_html(&html),
            Event::Html(html) => {
                if let Some(Frame::Html(text)) = self.stack.last_mut() {
                    text.push_str(&html);
                } else {
                    let style = self.style();
                    self.push_text(&html, style);
                }
            }
            Event::SoftBreak => {
                let style = self.style();
                self.push_text(" ", style);
            }
            Event::HardBreak => {
                let style = self.style();
                self.push_text("\n", style);
            }
            Event::Rule => {
                self.close_implicit();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(checked) => {
                // The marker lands in the item, or in the implicit paragraph
                // just opened inside it. (No let-chains here: the fuzz target
                // compiles this file on the 2021 edition.)
                let depth = match self.stack.last() {
                    Some(Frame::Item { .. }) => 0,
                    Some(Frame::Paragraph { .. }) => 1,
                    _ => return,
                };
                if let Some(Frame::Item { task, .. }) = self.stack.iter_mut().rev().nth(depth) {
                    *task = Some(checked);
                }
            }
            Event::FootnoteReference(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                let style = self.style();
                self.push_text(&text, style);
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.close_implicit();
                self.stack.push(Frame::Paragraph {
                    inlines: Vec::new(),
                    implicit: false,
                });
            }
            Tag::Heading { level, .. } => {
                self.close_implicit();
                self.stack
                    .push(Frame::Heading(heading_level(level), Vec::new()));
            }
            Tag::BlockQuote(_) => {
                self.close_implicit();
                self.stack.push(Frame::Quote(Vec::new()));
            }
            Tag::CodeBlock(kind) => {
                self.close_implicit();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .filter(|lang| !lang.is_empty())
                        .map(str::to_owned),
                    CodeBlockKind::Indented => None,
                };
                self.stack.push(Frame::Code {
                    lang,
                    text: String::new(),
                });
            }
            Tag::HtmlBlock => {
                self.close_implicit();
                self.stack.push(Frame::Html(String::new()));
            }
            Tag::List(start) => {
                self.close_implicit();
                self.stack.push(Frame::List {
                    start,
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                self.close_implicit();
                self.stack.push(Frame::Item {
                    task: None,
                    blocks: Vec::new(),
                });
            }
            Tag::Table(_) => {
                self.close_implicit();
                self.stack.push(Frame::Table {
                    header: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(Frame::Table { in_head, .. }) = self.stack.last_mut() {
                    *in_head = true;
                }
            }
            Tag::TableRow => {}
            Tag::TableCell => self.stack.push(Frame::Cell(Vec::new())),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { .. } => self.link += 1,
            Tag::Image { .. } => self.image = Some(String::new()),
            // Footnotes, definition lists, super/subscript and metadata are
            // not enabled; their content (if any) flows as ordinary text.
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => self.link = self.link.saturating_sub(1),
            TagEnd::TableHead => {
                if let Some(Frame::Table {
                    header,
                    row,
                    in_head,
                    ..
                }) = self.stack.last_mut()
                {
                    *header = std::mem::take(row);
                    *in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(Frame::Table { rows, row, .. }) = self.stack.last_mut() {
                    rows.push(std::mem::take(row));
                }
            }
            TagEnd::TableCell => {
                self.spans.clear();
                if let Some(Frame::Cell(inlines)) = self.stack.pop() {
                    let inlines = self.inline_math_only(inlines);
                    if let Some(Frame::Table { row, .. }) = self.stack.last_mut() {
                        row.push(inlines);
                    }
                }
            }
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::Table => {
                if matches!(tag, TagEnd::Item | TagEnd::BlockQuote(_)) {
                    self.close_implicit();
                }
                self.pop_block();
            }
            _ => {}
        }
    }

    /// Pops the top frame and attaches the block it built to its parent.
    fn pop_block(&mut self) {
        let Some(frame) = self.stack.pop() else {
            return;
        };
        // A span never reaches past the end of its block.
        self.spans.clear();
        match frame {
            Frame::Paragraph { inlines, .. } => {
                for block in self.split_display_math(inlines) {
                    self.push_block(block);
                }
            }
            Frame::Heading(level, inlines) => {
                let inlines = self.inline_math_only(inlines);
                self.push_block(Block::Heading(level, inlines));
            }
            Frame::Quote(blocks) => self.push_block(Block::Quote(blocks)),
            Frame::Code { lang, text } => {
                let text = text.strip_suffix('\n').unwrap_or(&text).to_owned();
                self.push_block(Block::Code { lang, text });
            }
            Frame::Html(text) => {
                let text = text.trim_end_matches('\n').to_owned();
                if !text.is_empty() {
                    self.push_block(Block::Paragraph(vec![Inline::Text(
                        text,
                        InlineStyle::default(),
                    )]));
                }
            }
            Frame::List { start, items } => self.push_block(Block::List { start, items }),
            Frame::Item { task, blocks } => {
                if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                    items.push(ListItem { task, blocks });
                } else {
                    for block in blocks {
                        self.push_block(block);
                    }
                }
            }
            Frame::Table { header, rows, .. } => self.push_block(Block::Table { header, rows }),
            Frame::Cell(inlines) => self.push_block(Block::Paragraph(inlines)),
        }
    }

    fn push_block(&mut self, block: Block) {
        match self.stack.last_mut() {
            Some(Frame::Quote(blocks) | Frame::Item { blocks, .. }) => blocks.push(block),
            Some(Frame::Paragraph { .. } | Frame::Heading(..) | Frame::Cell(_)) => {
                // A block cannot nest inside a leaf; close the leaf first.
                self.pop_block();
                self.push_block(block);
            }
            Some(_) | None => {
                if self.stack.is_empty() {
                    self.root.push(block);
                } else {
                    // Lists only hold items and tables only hold cells; a
                    // stray block (malformed input) lands at the root.
                    self.root.push(block);
                }
            }
        }
    }

    /// Ends a paragraph that was opened implicitly for tight-list text.
    fn close_implicit(&mut self) {
        if matches!(
            self.stack.last(),
            Some(Frame::Paragraph { implicit: true, .. })
        ) {
            self.pop_block();
        }
    }

    fn push_inline(&mut self, inline: Inline) {
        match self.stack.last_mut() {
            Some(
                Frame::Paragraph { inlines, .. }
                | Frame::Heading(_, inlines)
                | Frame::Cell(inlines),
            ) => push_merged(inlines, inline),
            Some(Frame::Code { text, .. } | Frame::Html(text)) => {
                if let Inline::Text(t, _) = inline {
                    text.push_str(&t);
                }
            }
            _ => {
                self.stack.push(Frame::Paragraph {
                    inlines: vec![inline],
                    implicit: true,
                });
            }
        }
    }

    /// Pushes text, turning placeholder characters back into equations.
    fn push_text(&mut self, text: &str, style: InlineStyle) {
        let mut start = 0;
        for (index, c) in text.char_indices() {
            if let Some(atom) = placeholder_index(c, self.math.len()) {
                if start < index {
                    self.push_inline(Inline::Text(text[start..index].to_owned(), style));
                }
                self.push_inline(Inline::Math(atom, style));
                start = index + c.len_utf8();
            }
        }
        if start < text.len() {
            self.push_inline(Inline::Text(text[start..].to_owned(), style));
        }
    }

    fn inline_html(&mut self, html: &str) {
        if let Some(style) = span_open(html.trim()) {
            self.spans.push(parse_declarations(style));
        } else if is_span_close(html.trim()) {
            // An unmatched `</span>` is ignored.
            self.spans.pop();
        } else {
            let style = self.style();
            self.push_text(html, style);
        }
    }

    /// Splits a paragraph around display equations.
    fn split_display_math(&self, inlines: Vec<Inline>) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut current = Vec::new();
        for inline in inlines {
            match inline {
                Inline::Math(atom, style) if self.math[atom].display => {
                    flush_paragraph(&mut current, &mut blocks);
                    blocks.push(Block::Math(atom, style));
                }
                other => current.push(other),
            }
        }
        flush_paragraph(&mut current, &mut blocks);
        blocks
    }

    /// Headings and cells cannot hold a display block: their equations stay
    /// inline. The model keeps the atom; layout sizes it as inline math.
    fn inline_math_only(&self, inlines: Vec<Inline>) -> Vec<Inline> {
        inlines
    }

    fn finish(mut self) -> Vec<Block> {
        while !self.stack.is_empty() {
            self.close_implicit();
            if self.stack.is_empty() {
                break;
            }
            self.pop_block();
        }
        self.root
    }
}

fn flush_paragraph(current: &mut Vec<Inline>, blocks: &mut Vec<Block>) {
    // Drop whitespace-only fragments left by splitting around an equation.
    let blank = current.iter().all(|inline| match inline {
        Inline::Text(text, _) => text.trim().is_empty(),
        Inline::Math(..) => false,
    });
    if !blank {
        trim_edges(current);
        blocks.push(Block::Paragraph(std::mem::take(current)));
    }
    current.clear();
}

fn trim_edges(inlines: &mut [Inline]) {
    if let Some(Inline::Text(text, _)) = inlines.first_mut() {
        *text = text.trim_start().to_owned();
    }
    if let Some(Inline::Text(text, _)) = inlines.last_mut() {
        *text = text.trim_end().to_owned();
    }
}

/// Appends `inline`, merging adjacent text of identical style.
#[allow(clippy::collapsible_if)] // no let-chains: see the fuzz target
fn push_merged(inlines: &mut Vec<Inline>, inline: Inline) {
    if let (Some(Inline::Text(last, last_style)), Inline::Text(text, style)) =
        (inlines.last_mut(), &inline)
    {
        if last_style == style {
            last.push_str(text);
            return;
        }
    }
    inlines.push(inline);
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Every inline in a block tree flattened to text, for tests.
#[cfg(test)]
pub fn plain_text(blocks: &[Block]) -> String {
    let mut out = String::new();
    fn inlines(out: &mut String, list: &[Inline]) {
        for inline in list {
            match inline {
                Inline::Text(text, _) => out.push_str(text),
                Inline::Math(index, _) => out.push_str(&format!("<math {index}>")),
            }
        }
    }
    fn walk(out: &mut String, blocks: &[Block]) {
        for block in blocks {
            match block {
                Block::Paragraph(list) | Block::Heading(_, list) => inlines(out, list),
                Block::Code { text, .. } => out.push_str(text),
                Block::List { items, .. } => {
                    for item in items {
                        walk(out, &item.blocks);
                    }
                }
                Block::Quote(blocks) => walk(out, blocks),
                Block::Rule => out.push_str("---"),
                Block::Table { header, rows } => {
                    for cell in header.iter().chain(rows.iter().flatten()) {
                        inlines(out, cell);
                        out.push('|');
                    }
                }
                Block::Math(index, _) => out.push_str(&format!("<display {index}>")),
            }
            out.push('\n');
        }
    }
    walk(&mut out, blocks);
    out
}

#[cfg(test)]
mod tests {
    use super::{Block, Inline, InlineStyle, extract_math, parse, plain_text};

    fn text(inline: &Inline) -> (&str, InlineStyle) {
        match inline {
            Inline::Text(text, style) => (text, *style),
            Inline::Math(..) => panic!("expected text, got {inline:?}"),
        }
    }

    fn paragraph(block: &Block) -> &[Inline] {
        match block {
            Block::Paragraph(inlines) => inlines,
            other => panic!("expected a paragraph, got {other:?}"),
        }
    }

    #[test]
    fn the_two_requested_span_examples_colour_only_their_range() {
        let doc = parse(r#"This is <span style="color: red;">red text</span> in a sentence"#);
        let inlines = paragraph(&doc.blocks[0]);
        assert_eq!(inlines.len(), 3);
        assert_eq!(text(&inlines[0]).0, "This is ");
        assert_eq!(text(&inlines[0]).1.span.color, None);
        assert_eq!(text(&inlines[1]).0, "red text");
        assert_eq!(text(&inlines[1]).1.span.color, Some([255, 0, 0, 255]));
        assert_eq!(text(&inlines[2]).0, " in a sentence");

        let doc = parse(r#"This is <span style="color: #ff5733;">Custom color</span>"#);
        let inlines = paragraph(&doc.blocks[0]);
        assert_eq!(text(&inlines[1]).0, "Custom color");
        assert_eq!(
            text(&inlines[1]).1.span.color,
            Some([0xff, 0x57, 0x33, 255])
        );
    }

    #[test]
    fn spans_nest_and_close_at_the_end_of_their_block() {
        let doc = parse(
            "<span style=\"color: red; font-size: 24px\">a <span style=\"color: blue\">b</span> c\n\nnext",
        );
        let first = paragraph(&doc.blocks[0]);
        assert_eq!(text(&first[0]).1.span.color, Some([255, 0, 0, 255]));
        assert_eq!(text(&first[0]).1.span.font_size, Some(24.0));
        assert_eq!(text(&first[1]).1.span.color, Some([0, 0, 255, 255]));
        assert_eq!(text(&first[1]).1.span.font_size, Some(24.0), "inherited");
        assert_eq!(text(&first[2]).1.span.color, Some([255, 0, 0, 255]));
        let second = paragraph(&doc.blocks[1]);
        assert_eq!(text(&second[0]).0, "next");
        assert_eq!(
            text(&second[0]).1.span.color,
            None,
            "unclosed span ends with its block"
        );
    }

    #[test]
    fn other_html_and_unmatched_closers_stay_literal_or_vanish() {
        let doc = parse("a <b>bold?</b> </span> z");
        let all: String = paragraph(&doc.blocks[0])
            .iter()
            .map(|inline| text(inline).0.to_owned())
            .collect();
        assert_eq!(all, "a <b>bold?</b>  z");
        let doc = parse("<div>\nblock html\n</div>");
        assert!(plain_text(&doc.blocks).contains("<div>"));
    }

    #[test]
    fn emphasis_strike_code_and_links_set_their_flags() {
        let doc = parse("**b** *i* ~~s~~ `c <span>` [l](http://x)");
        let inlines = paragraph(&doc.blocks[0]);
        let find = |needle: &str| {
            inlines
                .iter()
                .map(text)
                .find(|(t, _)| *t == needle)
                .unwrap_or_else(|| panic!("{needle} in {inlines:?}"))
                .1
        };
        assert!(find("b").bold);
        assert!(find("i").italic);
        assert!(find("s").strike);
        assert!(find("c <span>").code, "spans inside code are literal");
        assert!(find("l").link);
    }

    #[test]
    fn block_structure_covers_the_markdown_subset() {
        let source = "# Title\n\n## Sub\n\npara\n\n- one\n- [x] done\n- [ ] todo\n\n3. three\n4. four\n\n> quote\n> > nested\n\n---\n\n```rust\nfn x() {}\n```\n\n    indented\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n![alt text](img.png)";
        let doc = parse(source);
        let kinds: Vec<&str> = doc
            .blocks
            .iter()
            .map(|block| match block {
                Block::Heading(..) => "h",
                Block::Paragraph(_) => "p",
                Block::List { .. } => "list",
                Block::Quote(_) => "quote",
                Block::Rule => "rule",
                Block::Code { .. } => "code",
                Block::Table { .. } => "table",
                Block::Math(..) => "math",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "h", "h", "p", "list", "list", "quote", "rule", "code", "code", "table", "p"
            ]
        );
        let Block::List { start, items } = &doc.blocks[3] else {
            panic!()
        };
        assert_eq!(*start, None);
        assert_eq!(items.len(), 3);
        assert_eq!(items[1].task, Some(true));
        assert_eq!(items[2].task, Some(false));
        assert!(matches!(doc.blocks[4], Block::List { start: Some(3), .. }));
        let Block::Quote(inner) = &doc.blocks[5] else {
            panic!()
        };
        assert!(inner.iter().any(|block| matches!(block, Block::Quote(_))));
        assert!(matches!(
            &doc.blocks[7],
            Block::Code { lang: Some(lang), text } if lang == "rust" && text == "fn x() {}"
        ));
        let Block::Table { header, rows } = &doc.blocks[9] else {
            panic!()
        };
        assert_eq!(header.len(), 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(text(&paragraph(&doc.blocks[10])[0]).0, "[alt text]");
    }

    #[test]
    fn math_is_extracted_outside_code_only() {
        let (text, math) =
            extract_math("a $x_1*y*$ b `$no$` c\n```\n$also no$\n```\n$$\\int_0^1 x$$");
        assert_eq!(math.len(), 2);
        assert_eq!(math[0].tex, "x_1*y*");
        assert!(!math[0].display);
        assert_eq!(math[1].tex, "\\int_0^1 x");
        assert!(math[1].display);
        assert!(text.contains("`$no$`"));
        assert!(text.contains("$also no$"));
    }

    #[test]
    fn dollar_rules_keep_prices_and_escapes_literal() {
        assert!(extract_math("costs $5 and $10 total").1.is_empty());
        assert!(
            extract_math("a $ b $ c").1.is_empty(),
            "whitespace after the opener"
        );
        assert!(extract_math("\\$x$ escaped").1.is_empty());
        assert!(
            extract_math("open $x\n\ny$ across a blank line")
                .1
                .is_empty()
        );
        assert!(extract_math("unclosed $x and $$y").1.is_empty());
        let (_, math) = extract_math("\\(a+b\\) and \\[c\\]");
        assert_eq!(math.len(), 2);
        assert!(!math[0].display);
        assert!(math[1].display);
        let doc = parse("\\$5");
        assert_eq!(plain_text(&doc.blocks).trim(), "$5");
    }

    #[test]
    fn display_math_splits_its_paragraph() {
        let doc = parse("before $$E=mc^2$$ after and $y$ inline");
        assert_eq!(doc.blocks.len(), 3);
        assert!(matches!(doc.blocks[1], Block::Math(0, _)));
        let after = paragraph(&doc.blocks[2]);
        assert!(
            after
                .iter()
                .any(|inline| matches!(inline, Inline::Math(1, _)))
        );
        assert_eq!(text(&paragraph(&doc.blocks[0])[0]).0, "before");
    }

    #[test]
    fn a_span_around_math_styles_the_equation() {
        let doc = parse("<span style=\"color: #00ff00; font-size: 30px\">$a^2$</span>");
        let Inline::Math(0, style) = paragraph(&doc.blocks[0])[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(style.span.color, Some([0, 255, 0, 255]));
        assert_eq!(style.span.font_size, Some(30.0));
    }

    #[test]
    fn equation_limits_keep_the_rest_literal() {
        let many: String = (0..200).map(|i| format!("$x{i}$ ")).collect();
        let (_, math) = extract_math(&many);
        assert_eq!(math.len(), super::MAX_EQUATIONS);
        let huge = format!("${}$", "x".repeat(super::MAX_EQUATION_BYTES + 1));
        assert!(extract_math(&huge).1.is_empty());
    }

    #[test]
    fn malformed_input_never_panics() {
        let corpus = [
            "",
            "$",
            "$$",
            "\\",
            "\\(",
            "`",
            "``x`",
            "<span",
            "<span style=\"",
            "</span></span><span>",
            "| a |\n|",
            "- [",
            "> - > - > -",
            "```\n$x$",
            "\u{F0000}\u{F0001} stray placeholders",
            "*$*$*",
            "[$x$](<$y$>)",
            "![![a](b)](c)",
            "1. a\n   ```\n   $x$\n   ```",
            "a\\\nb",
            "é$ü$€",
        ];
        for source in corpus {
            let _ = parse(source);
        }
        // A cheap generated corpus over the interesting bytes.
        let alphabet = [
            '$', '\\', '`', '<', '>', '/', '*', '_', '|', '-', '\n', ' ', 'a', '[', ']', '(', ')',
            '#', '~', '"',
        ];
        let mut state = 0x1234_5678_u32;
        for _ in 0..2000 {
            let mut source = String::new();
            for _ in 0..24 {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                source.push(alphabet[(state >> 24) as usize % alphabet.len()]);
            }
            let _ = parse(&source);
        }
    }
}
