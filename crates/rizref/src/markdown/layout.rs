//! Block layout for markdown notes, in world units.
//!
//! The layout depends on the parsed document and the note's wrap width only —
//! never on the box size or the texture density. That is what makes "resizing
//! never reflows" true by construction: the same `(source, wrap width)` always
//! yields the same line breaks and glyph positions, and the painter scales the
//! shaped glyphs to whatever pixel density the texture needs (cosmic-text's
//! `LayoutGlyph::physical`), instead of re-shaping at a new size.
//!
//! Every paragraph-like block becomes one cosmic-text [`Buffer`] with rich
//! spans; per-span metrics carry `<span style="font-size">`. Inline math is an
//! atom: a single no-break space whose letter spacing makes its advance equal
//! to the equation's width, and whose line height reserves the equation's
//! height. After shaping, the atom's glyph position is where the equation mask
//! is composited. Decorations, code backgrounds, list markers, rules, quote
//! bars and table grids are plain rectangles in the draw list.

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, Weight, Wrap};

use super::math::{MathBox, MathCache};
use super::parse::{Block, Document, Inline, InlineStyle, ListItem, MathAtom};

/// Body text size in world units.
pub const BASE_FONT_SIZE: f32 = 16.0;

/// Line height as a multiple of the font size.
const LINE_HEIGHT: f32 = 1.4;

/// Vertical gap between blocks, in em.
const BLOCK_GAP: f32 = 0.6;

/// Gap between items of a list, in em.
const ITEM_GAP: f32 = 0.25;

/// Indent of list content, in em.
const LIST_INDENT: f32 = 1.6;

/// Indent of quoted content, in em.
const QUOTE_INDENT: f32 = 1.0;

/// Inline-code and code-block text size relative to the surrounding text.
const CODE_SCALE: f32 = 0.92;

/// Heading sizes relative to the base, H1–H6.
const HEADING_SCALE: [f32; 6] = [1.75, 1.5, 1.25, 1.125, 1.0, 0.875];

/// How a glyph, decoration or rectangle is coloured. Resolved against the
/// note palette at paint time, so a background change never re-lays out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    /// The auto-contrast ink at an alpha.
    Ink(f32),
    /// A link: the accent when it contrasts, otherwise the ink.
    Link,
    /// An explicit `<span>` colour; never adjusted.
    Color([u8; 4]),
    /// Equation source shown because it could not be typeset.
    Error,
}

/// What a rectangle is filled with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fill {
    /// A glyph-like paint.
    Paint(Paint),
    /// The translucent panel behind code and table headers.
    Panel,
}

/// The style of one shaped span, indexed by glyph metadata.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphStyle {
    /// Glyph colour.
    pub paint: Paint,
    /// Underline decoration.
    pub underline: bool,
    /// Strikethrough decoration.
    pub strike: bool,
    /// Inline code background.
    pub code: bool,
    /// The equation this placeholder glyph stands for.
    pub math: Option<usize>,
}

/// A shaped text block placed in the note.
pub struct TextItem {
    /// Left edge in world units.
    pub x: f32,
    /// Top edge in world units.
    pub y: f32,
    /// The shaped buffer, in world units.
    pub buffer: Buffer,
    /// Span styles, indexed by `LayoutGlyph::metadata`.
    pub styles: Vec<GlyphStyle>,
    /// Horizontal clip `[x0, x1]` in world units (code blocks).
    pub clip: Option<[f32; 2]>,
}

/// A rectangle in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectItem {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
    /// Fill.
    pub fill: Fill,
    /// Corner radius.
    pub radius: f32,
}

/// An equation placed in the note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MathItem {
    /// Index into [`NoteLayout::atoms`].
    pub atom: usize,
    /// Whether it is typeset in display style.
    pub display: bool,
    /// Left edge in world units.
    pub x: f32,
    /// Baseline in world units.
    pub baseline: f32,
    /// Font size (pixels per em at 100% zoom) in world units.
    pub font_size: f32,
    /// Tint.
    pub paint: Paint,
    /// Metrics, in em.
    pub metrics: MathBox,
}

/// A laid-out note, in world units relative to the content's top-left.
#[derive(Default)]
pub struct NoteLayout {
    /// Content height.
    pub height: f32,
    /// Panels, backgrounds and rules, drawn first.
    pub rects: Vec<RectItem>,
    /// Text blocks.
    pub texts: Vec<TextItem>,
    /// Decorations drawn over the text (strikethrough, underlines).
    pub overlays: Vec<RectItem>,
    /// Equations, drawn last.
    pub math: Vec<MathItem>,
    /// The document's equations.
    pub atoms: Vec<MathAtom>,
}

#[cfg(test)]
impl NoteLayout {
    /// Every glyph's position and line break, in world units, for structural
    /// tests: `(block, line, glyph start byte, x, baseline)`.
    pub fn glyph_positions(&self) -> Vec<(usize, usize, usize, f32, f32)> {
        let mut out = Vec::new();
        for (block, text) in self.texts.iter().enumerate() {
            for run in text.buffer.layout_runs() {
                for glyph in run.glyphs {
                    out.push((
                        block,
                        run.line_i,
                        glyph.start,
                        text.x + glyph.x,
                        text.y + run.line_y,
                    ));
                }
            }
        }
        out
    }

    /// A stable fingerprint of [`Self::glyph_positions`].
    pub fn signature(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut feed = |value: u64| {
            for byte in value.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        };
        for (block, line, start, x, y) in self.glyph_positions() {
            feed(block as u64);
            feed(line as u64);
            feed(start as u64);
            feed(u64::from(x.to_bits()));
            feed(u64::from(y.to_bits()));
        }
        hash
    }
}

/// Lays out `document` at `wrap_width` world units.
pub fn layout(
    fonts: &mut FontSystem,
    math: &mut MathCache,
    document: &Document,
    wrap_width: f32,
) -> NoteLayout {
    let wrap_width = wrap_width.max(1.0);
    let nbsp_em = nbsp_advance_em(fonts);
    let mut context = Context {
        fonts,
        math,
        atoms: &document.math,
        nbsp_em,
        out: NoteLayout {
            atoms: document.math.clone(),
            ..NoteLayout::default()
        },
    };
    let height = context.blocks(&document.blocks, 0.0, 0.0, wrap_width, 1.0, 0);
    let mut out = context.out;
    out.height = height;
    out
}

/// The advance of U+00A0 in em, so a math atom's letter spacing can make up
/// the rest of the equation's width exactly.
fn nbsp_advance_em(fonts: &mut FontSystem) -> f32 {
    let size = 100.0;
    let mut buffer = Buffer::new(fonts, Metrics::new(size, size));
    buffer.set_text("\u{a0}", &Attrs::new(), Shaping::Advanced, None);
    buffer.shape_until_scroll(fonts, false);
    buffer
        .layout_runs()
        .flat_map(|run| run.glyphs.iter())
        .map(|glyph| glyph.w / size)
        .next()
        .unwrap_or(0.25)
}

/// A shaped block before it is placed.
struct Shaped {
    buffer: Buffer,
    styles: Vec<GlyphStyle>,
    width: f32,
    height: f32,
    rects: Vec<RectItem>,
    overlays: Vec<RectItem>,
    math: Vec<MathItem>,
}

struct Context<'a> {
    fonts: &'a mut FontSystem,
    math: &'a mut MathCache,
    atoms: &'a [MathAtom],
    nbsp_em: f32,
    out: NoteLayout,
}

impl Context<'_> {
    /// Lays out a block list starting at `y`; returns the bottom edge.
    fn blocks(
        &mut self,
        blocks: &[Block],
        x: f32,
        mut y: f32,
        width: f32,
        dim: f32,
        depth: usize,
    ) -> f32 {
        for (index, block) in blocks.iter().enumerate() {
            if index > 0 {
                y += BLOCK_GAP * BASE_FONT_SIZE;
            }
            y = self.block(block, x, y, width, dim, depth);
        }
        y
    }

    fn block(&mut self, block: &Block, x: f32, y: f32, width: f32, dim: f32, depth: usize) -> f32 {
        match block {
            Block::Paragraph(inlines) => {
                let shaped = self.shape(inlines, BASE_FONT_SIZE, false, dim, Some(width));
                self.place(shaped, x, y, None)
            }
            Block::Heading(level, inlines) => {
                let index = usize::from(level.saturating_sub(1)).min(5);
                let size = BASE_FONT_SIZE * HEADING_SCALE[index];
                let dim = if *level >= 5 { dim * 0.7 } else { dim };
                let top = if y > 0.0 { 0.3 * size } else { 0.0 };
                let shaped = self.shape(inlines, size, true, dim, Some(width));
                self.place(shaped, x, y + top, None) + 0.15 * size
            }
            Block::Code { lang, text } => self.code_block(lang.as_deref(), text, x, y, width, dim),
            Block::List { start, items } => self.list(*start, items, x, y, width, dim, depth),
            Block::Quote(blocks) => {
                let indent = QUOTE_INDENT * BASE_FONT_SIZE;
                let bottom = self.blocks(
                    blocks,
                    x + indent,
                    y,
                    (width - indent).max(1.0),
                    dim * 0.85,
                    depth + 1,
                );
                self.out.rects.push(RectItem {
                    x: x + 0.15 * BASE_FONT_SIZE,
                    y,
                    w: 3.0,
                    h: (bottom - y).max(BASE_FONT_SIZE),
                    fill: Fill::Paint(Paint::Ink(0.35 * dim)),
                    radius: 1.5,
                });
                bottom.max(y + BASE_FONT_SIZE)
            }
            Block::Rule => {
                let gap = 0.5 * BASE_FONT_SIZE;
                self.out.rects.push(RectItem {
                    x,
                    y: y + gap,
                    w: width,
                    h: 1.0,
                    fill: Fill::Paint(Paint::Ink(0.25 * dim)),
                    radius: 0.0,
                });
                y + 2.0 * gap + 1.0
            }
            Block::Table { header, rows } => self.table(header, rows, x, y, width, dim),
            Block::Math(atom, style) => self.display_math(*atom, *style, x, y, width, dim),
        }
    }

    fn display_math(
        &mut self,
        atom: usize,
        style: InlineStyle,
        x: f32,
        y: f32,
        width: f32,
        dim: f32,
    ) -> f32 {
        let tex = self.atoms[atom].tex.clone();
        let size = style.span.font_size.unwrap_or(BASE_FONT_SIZE);
        match self.math.measure(&tex, true) {
            Ok(metrics) => {
                let w = metrics.width * size;
                let pad = 0.2 * size;
                self.out.math.push(MathItem {
                    atom,
                    display: true,
                    x: x + ((width - w) * 0.5).max(0.0),
                    baseline: y + pad + metrics.height * size,
                    font_size: size,
                    paint: style.span.color.map_or(Paint::Ink(dim), Paint::Color),
                    metrics,
                });
                y + 2.0 * pad + (metrics.height + metrics.depth) * size
            }
            Err(_) => {
                let fallback = [Inline::Text(format!("$${tex}$$"), InlineStyle::default())];
                let mut shaped = self.shape(&fallback, BASE_FONT_SIZE, false, dim, Some(width));
                mark_error(&mut shaped);
                self.place(shaped, x, y, None)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn list(
        &mut self,
        start: Option<u64>,
        items: &[ListItem],
        x: f32,
        mut y: f32,
        width: f32,
        dim: f32,
        depth: usize,
    ) -> f32 {
        let indent = LIST_INDENT * BASE_FONT_SIZE;
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                y += ITEM_GAP * BASE_FONT_SIZE;
            }
            let marker = match (item.task, start) {
                (Some(true), _) => "\u{2611}".to_owned(),
                (Some(false), _) => "\u{2610}".to_owned(),
                (None, Some(first)) => format!("{}.", first.saturating_add(index as u64)),
                (None, None) => ["\u{2022}", "\u{25e6}", "\u{25aa}"][depth % 3].to_owned(),
            };
            let shaped = self.shape(
                &[Inline::Text(marker, InlineStyle::default())],
                BASE_FONT_SIZE,
                false,
                dim,
                None,
            );
            let marker_x = x + indent - shaped.width - 0.35 * BASE_FONT_SIZE;
            self.place(shaped, marker_x.max(x), y, None);
            let bottom = self.blocks(
                &item.blocks,
                x + indent,
                y,
                (width - indent).max(1.0),
                dim,
                depth + 1,
            );
            y = bottom.max(y + BASE_FONT_SIZE * LINE_HEIGHT);
        }
        y
    }

    fn code_block(
        &mut self,
        lang: Option<&str>,
        text: &str,
        x: f32,
        y: f32,
        width: f32,
        dim: f32,
    ) -> f32 {
        let pad = 0.6 * BASE_FONT_SIZE;
        let mut top = pad;
        if let Some(lang) = lang {
            let tag = self.shape(
                &[Inline::Text(lang.to_owned(), InlineStyle::default())],
                0.7 * BASE_FONT_SIZE,
                false,
                0.55 * dim,
                None,
            );
            let tag_x = (x + width - pad - tag.width).max(x + pad);
            top += tag.height;
            self.place(tag, tag_x, y + 0.35 * pad, Some([x, x + width]));
        }
        let style = InlineStyle {
            code: true,
            ..InlineStyle::default()
        };
        let mut shaped = self.shape_with(
            &[Inline::Text(text.to_owned(), style)],
            BASE_FONT_SIZE,
            false,
            dim,
            None,
            false,
        );
        // The panel is the background; per-run inline-code tints would double it.
        shaped.rects.clear();
        let height = shaped.height.max(BASE_FONT_SIZE);
        let panel_index = self.out.rects.len();
        self.place(
            shaped,
            x + pad,
            y + top,
            Some([x + pad * 0.5, x + width - pad * 0.5]),
        );
        // The panel goes under the language tag and the code.
        self.out.rects.insert(
            panel_index.min(self.out.rects.len()),
            RectItem {
                x,
                y,
                w: width,
                h: top + height + pad,
                fill: Fill::Panel,
                radius: 4.0,
            },
        );
        y + top + height + pad
    }

    fn table(
        &mut self,
        header: &[Vec<Inline>],
        rows: &[Vec<Vec<Inline>>],
        x: f32,
        y: f32,
        width: f32,
        dim: f32,
    ) -> f32 {
        let columns = rows
            .iter()
            .map(Vec::len)
            .chain(std::iter::once(header.len()))
            .max()
            .unwrap_or(0);
        if columns == 0 {
            return y;
        }
        let pad = 0.4 * BASE_FONT_SIZE;
        let all_rows: Vec<(&[Vec<Inline>], bool)> = std::iter::once((header, true))
            .chain(rows.iter().map(|row| (row.as_slice(), false)))
            .collect();
        let mut natural = vec![0.0_f32; columns];
        for (row, bold) in &all_rows {
            for (column, cell) in row.iter().enumerate() {
                let shaped = self.shape(cell, BASE_FONT_SIZE, *bold, dim, None);
                natural[column] = natural[column].max(shaped.width);
            }
        }
        let wanted: Vec<f32> = natural.iter().map(|w| w + 2.0 * pad).collect();
        let total: f32 = wanted.iter().sum();
        let widths: Vec<f32> = if total <= width {
            wanted
        } else {
            wanted
                .iter()
                .map(|w| (w / total * width).max(2.0 * pad + BASE_FONT_SIZE))
                .collect()
        };
        let table_width: f32 = widths.iter().sum();
        let mut row_y = y;
        let line = Fill::Paint(Paint::Ink(0.25 * dim));
        for (row, bold) in &all_rows {
            let mut height = BASE_FONT_SIZE * LINE_HEIGHT;
            let mut column_x = x;
            let mut placed = Vec::new();
            for (column, cell_width) in widths.iter().enumerate() {
                if let Some(cell) = row.get(column) {
                    let shaped = self.shape(
                        cell,
                        BASE_FONT_SIZE,
                        *bold,
                        dim,
                        Some((cell_width - 2.0 * pad).max(1.0)),
                    );
                    height = height.max(shaped.height);
                    placed.push((shaped, column_x + pad));
                }
                column_x += cell_width;
            }
            if *bold {
                self.out.rects.push(RectItem {
                    x,
                    y: row_y,
                    w: table_width,
                    h: height + 2.0 * pad,
                    fill: Fill::Panel,
                    radius: 0.0,
                });
            }
            for (shaped, cell_x) in placed {
                self.place(shaped, cell_x, row_y + pad, None);
            }
            row_y += height + 2.0 * pad;
            self.out.rects.push(RectItem {
                x,
                y: row_y,
                w: table_width,
                h: 1.0,
                fill: line,
                radius: 0.0,
            });
        }
        // Outer frame and column separators.
        self.out.rects.push(RectItem {
            x,
            y,
            w: table_width,
            h: 1.0,
            fill: line,
            radius: 0.0,
        });
        let mut column_x = x;
        for cell_width in std::iter::once(0.0).chain(widths.iter().copied()) {
            column_x += cell_width;
            self.out.rects.push(RectItem {
                x: column_x,
                y,
                w: 1.0,
                h: row_y - y + 1.0,
                fill: line,
                radius: 0.0,
            });
        }
        row_y + 1.0
    }

    /// Shapes inline content, wrapped at `width` (or unwrapped for `None`).
    fn shape(
        &mut self,
        inlines: &[Inline],
        base: f32,
        bold: bool,
        dim: f32,
        width: Option<f32>,
    ) -> Shaped {
        self.shape_with(inlines, base, bold, dim, width, true)
    }

    fn shape_with(
        &mut self,
        inlines: &[Inline],
        base: f32,
        bold: bool,
        dim: f32,
        width: Option<f32>,
        wrap: bool,
    ) -> Shaped {
        let mut spans: Vec<(String, Attrs<'static>)> = Vec::new();
        let mut styles: Vec<GlyphStyle> = Vec::new();
        let mut math_metrics: Vec<Option<MathBox>> = Vec::new();
        for inline in inlines {
            match inline {
                Inline::Text(text, style) => {
                    let (attrs, glyph) = self.attrs(*style, base, bold, dim, styles.len());
                    spans.push((text.clone(), attrs));
                    styles.push(glyph);
                    math_metrics.push(None);
                }
                Inline::Math(atom, style) => {
                    let size = style.span.font_size.unwrap_or(base);
                    let tex = self.atoms[*atom].tex.clone();
                    match self.math.measure(&tex, false) {
                        Ok(metrics) => {
                            let extra = 2.0 * metrics.height.max(metrics.depth) + 0.4;
                            let line_height = (size * LINE_HEIGHT).max(extra * size);
                            let attrs = Attrs::new()
                                .metrics(Metrics::new(size, line_height))
                                .letter_spacing((metrics.width - self.nbsp_em).max(-self.nbsp_em))
                                .metadata(styles.len());
                            spans.push(("\u{a0}".to_owned(), attrs));
                            styles.push(GlyphStyle {
                                paint: style.span.color.map_or(Paint::Ink(dim), Paint::Color),
                                underline: false,
                                strike: false,
                                code: false,
                                math: Some(*atom),
                            });
                            math_metrics.push(Some(metrics));
                        }
                        Err(_) => {
                            let display = self.atoms[*atom].display;
                            let source = if display {
                                format!("$${tex}$$")
                            } else {
                                format!("${tex}$")
                            };
                            let error = InlineStyle {
                                code: true,
                                ..*style
                            };
                            let (attrs, mut glyph) =
                                self.attrs(error, base, bold, dim, styles.len());
                            glyph.paint = Paint::Error;
                            glyph.underline = true;
                            spans.push((source, attrs));
                            styles.push(glyph);
                            math_metrics.push(None);
                        }
                    }
                }
            }
        }

        let metrics = Metrics::new(base, base * LINE_HEIGHT);
        let mut buffer = Buffer::new(self.fonts, metrics);
        buffer.set_wrap(if wrap && width.is_some() {
            Wrap::WordOrGlyph
        } else {
            Wrap::None
        });
        buffer.set_size(if wrap { width } else { None }, None);
        let default = Attrs::new().metrics(metrics);
        buffer.set_rich_text(
            spans
                .iter()
                .map(|(text, attrs)| (text.as_str(), attrs.clone())),
            &default,
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(self.fonts, false);

        let mut shaped = Shaped {
            buffer,
            styles,
            width: 0.0,
            height: 0.0,
            rects: Vec::new(),
            overlays: Vec::new(),
            math: Vec::new(),
        };
        let mut bottom = 0.0_f32;
        for run in shaped.buffer.layout_runs() {
            shaped.width = shaped.width.max(run.line_w);
            bottom = bottom.max(run.line_top + run.line_height);
            for glyph in run.glyphs {
                let Some(style) = shaped.styles.get(glyph.metadata) else {
                    continue;
                };
                let size = glyph.font_size;
                if let (Some(atom), Some(Some(metrics))) =
                    (style.math, math_metrics.get(glyph.metadata))
                {
                    shaped.math.push(MathItem {
                        atom,
                        display: false,
                        x: glyph.x,
                        baseline: run.line_y,
                        font_size: size,
                        paint: style.paint,
                        metrics: *metrics,
                    });
                    continue;
                }
                if style.code {
                    push_or_extend(
                        &mut shaped.rects,
                        RectItem {
                            x: glyph.x - 0.15 * size,
                            y: run.line_y - 0.92 * size,
                            w: glyph.w + 0.3 * size,
                            h: 1.22 * size,
                            fill: Fill::Panel,
                            radius: 0.2 * size,
                        },
                    );
                }
                let line = (size / 14.0).max(1.0);
                if style.underline {
                    push_or_extend(
                        &mut shaped.overlays,
                        RectItem {
                            x: glyph.x,
                            y: run.line_y + 0.12 * size,
                            w: glyph.w,
                            h: line,
                            fill: Fill::Paint(style.paint),
                            radius: 0.0,
                        },
                    );
                }
                if style.strike {
                    push_or_extend(
                        &mut shaped.overlays,
                        RectItem {
                            x: glyph.x,
                            y: run.line_y - 0.3 * size,
                            w: glyph.w,
                            h: line,
                            fill: Fill::Paint(style.paint),
                            radius: 0.0,
                        },
                    );
                }
            }
        }
        shaped.height = bottom;
        shaped
    }

    /// The cosmic-text attributes and glyph style for one text span.
    fn attrs(
        &self,
        style: InlineStyle,
        base: f32,
        bold: bool,
        dim: f32,
        index: usize,
    ) -> (Attrs<'static>, GlyphStyle) {
        let mut size = style.span.font_size.unwrap_or(base);
        if style.code {
            size *= CODE_SCALE;
        }
        let mut attrs = Attrs::new()
            .metrics(Metrics::new(size, size * LINE_HEIGHT))
            .metadata(index);
        if style.code {
            attrs = attrs.family(Family::Monospace);
        }
        if bold || style.bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if style.italic {
            attrs = attrs.style(Style::Italic);
        }
        let paint = match style.span.color {
            Some(color) => Paint::Color(color),
            None if style.link => Paint::Link,
            None => Paint::Ink(dim),
        };
        (
            attrs,
            GlyphStyle {
                paint,
                underline: style.link,
                strike: style.strike,
                code: style.code,
                math: None,
            },
        )
    }

    /// Places a shaped block at `x`, `y` and returns its bottom edge.
    fn place(&mut self, shaped: Shaped, x: f32, y: f32, clip: Option<[f32; 2]>) -> f32 {
        let offset = |mut rect: RectItem| {
            rect.x += x;
            rect.y += y;
            rect
        };
        self.out.rects.extend(shaped.rects.into_iter().map(offset));
        self.out
            .overlays
            .extend(shaped.overlays.into_iter().map(offset));
        self.out
            .math
            .extend(shaped.math.into_iter().map(|mut item| {
                item.x += x;
                item.baseline += y;
                item
            }));
        let bottom = y + shaped.height;
        self.out.texts.push(TextItem {
            x,
            y,
            buffer: shaped.buffer,
            styles: shaped.styles,
            clip,
        });
        bottom
    }
}

/// Marks every span of a fallback block as equation-error text.
fn mark_error(shaped: &mut Shaped) {
    for style in &mut shaped.styles {
        style.paint = Paint::Error;
        style.underline = true;
    }
}

/// Extends the last rectangle when `rect` continues it on the same line,
/// otherwise appends it — so a run of code glyphs gets one background.
fn push_or_extend(rects: &mut Vec<RectItem>, rect: RectItem) {
    if let Some(last) = rects.last_mut()
        && (last.y - rect.y).abs() < 0.01
        && (last.h - rect.h).abs() < 0.01
        && last.fill == rect.fill
        && rect.x <= last.x + last.w + 0.5 * rect.h
        && rect.x >= last.x
    {
        last.w = (rect.x + rect.w - last.x).max(last.w);
        return;
    }
    rects.push(rect);
}

#[cfg(test)]
mod tests {
    use super::{BASE_FONT_SIZE, Fill, NoteLayout, Paint, layout};
    use crate::markdown::math::MathCache;
    use crate::markdown::parse::parse;
    use crate::text::TextEngine;

    fn lay(source: &str, width: f32) -> NoteLayout {
        let mut engine = TextEngine::new();
        let mut math = MathCache::new();
        layout(engine.font_system_mut(), &mut math, &parse(source), width)
    }

    fn line_count(layout: &NoteLayout, block: usize) -> usize {
        layout.texts[block].buffer.layout_runs().count()
    }

    #[test]
    fn a_paragraph_wraps_at_the_wrap_width_and_not_beyond() {
        let text = "the quick brown fox jumps over the lazy dog ".repeat(6);
        let narrow = lay(&text, 120.0);
        let wide = lay(&text, 600.0);
        assert!(line_count(&narrow, 0) > line_count(&wide, 0));
        for run in narrow.texts[0].buffer.layout_runs() {
            assert!(run.line_w <= 120.0 + 0.5, "line {} too wide", run.line_w);
        }
        assert!(narrow.height > wide.height);
    }

    #[test]
    fn the_same_source_and_width_give_identical_glyph_positions() {
        let source = "# Head\n\nSome **bold** text with $x^2$ and `code`.\n\n- a\n- b";
        let a = lay(source, 240.0);
        let b = lay(source, 240.0);
        assert_eq!(a.glyph_positions(), b.glyph_positions());
        assert_eq!(a.signature(), b.signature());
        assert_ne!(a.signature(), lay(source, 120.0).signature());
    }

    #[test]
    fn a_24px_span_gets_a_taller_line_than_body_text() {
        let body = lay("plain line", 400.0);
        let big = lay(
            "plain <span style=\"font-size: 24px\">big</span> line",
            400.0,
        );
        let body_height = body.texts[0]
            .buffer
            .layout_runs()
            .next()
            .expect("run")
            .line_height;
        let big_run = big.texts[0].buffer.layout_runs().next().expect("run");
        assert!((body_height - BASE_FONT_SIZE * 1.4).abs() < 0.01);
        assert!(
            (big_run.line_height - 24.0 * 1.4).abs() < 0.01,
            "{}",
            big_run.line_height
        );
        let sizes: Vec<f32> = big_run.glyphs.iter().map(|glyph| glyph.font_size).collect();
        assert!(sizes.contains(&24.0) && sizes.contains(&BASE_FONT_SIZE));
    }

    #[test]
    fn span_colours_and_links_resolve_to_their_paints() {
        let doc = lay(
            "This is <span style=\"color: red;\">red text</span> and [a link](x)",
            600.0,
        );
        let paints: Vec<Paint> = doc.texts[0]
            .styles
            .iter()
            .map(|style| style.paint)
            .collect();
        assert!(paints.contains(&Paint::Color([255, 0, 0, 255])));
        assert!(paints.contains(&Paint::Link));
        assert!(paints.contains(&Paint::Ink(1.0)));
        assert!(!doc.overlays.is_empty(), "the link is underlined");
    }

    #[test]
    fn inline_math_reserves_its_width_on_the_baseline() {
        let doc = lay("before $E=mc^2$ after", 600.0);
        assert_eq!(doc.math.len(), 1);
        let item = doc.math[0];
        assert!(!item.display);
        let run = doc.texts[0].buffer.layout_runs().next().expect("run");
        assert!(
            (item.baseline - run.line_y).abs() < 0.01,
            "sits on the text baseline"
        );
        let after = run
            .glyphs
            .iter()
            .find(|glyph| &run.text[glyph.start..glyph.end] == "a")
            .expect("the 'a' of after");
        let math_right = item.x + item.metrics.width * item.font_size;
        assert!(
            after.x >= math_right - 0.5,
            "text after the equation does not overlap it"
        );
    }

    #[test]
    fn display_math_is_centred_in_its_own_block() {
        let doc = lay("$$\\int_0^1 x^2\\,dx = \\tfrac{1}{3}$$", 400.0);
        assert_eq!(doc.math.len(), 1);
        let item = doc.math[0];
        assert!(item.display);
        let w = item.metrics.width * item.font_size;
        assert!(
            ((item.x + w * 0.5) - 200.0).abs() < 0.5,
            "centred: {}",
            item.x
        );
        assert!(doc.height >= (item.metrics.height + item.metrics.depth) * item.font_size);
    }

    #[test]
    fn invalid_tex_falls_back_to_error_styled_source() {
        let doc = lay("bad $\\frac{a}{$ math", 400.0);
        assert!(doc.math.is_empty());
        assert!(
            doc.texts[0]
                .styles
                .iter()
                .any(|style| style.paint == Paint::Error && style.underline)
        );
    }

    #[test]
    fn the_fixture_covers_every_block_kind() {
        let fixture = "# H1\n## H2\n###### H6\n\nText *it* **b** ~~s~~ `code` [link](u).\n\n```rust\nlet x = 1;\n```\n\n1. one\n2. two\n\n- [x] done\n- [ ] todo\n  - nested\n\n> quote\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |";
        let doc = lay(fixture, 320.0);
        assert!(doc.texts.len() > 10);
        assert!(
            doc.rects.iter().any(|rect| rect.fill == Fill::Panel),
            "code panels"
        );
        assert!(
            doc.rects
                .iter()
                .any(|rect| rect.h == 1.0 && rect.w == 320.0),
            "the rule spans the width"
        );
        assert!(doc.overlays.len() >= 2, "strike and link decorations");
        // Code blocks never wrap.
        let long = lay(&format!("```\n{}\n```", "x".repeat(400)), 200.0);
        let code = long
            .texts
            .iter()
            .find(|text| text.clip.is_some())
            .expect("code text");
        assert_eq!(code.buffer.layout_runs().count(), 1);
    }

    #[test]
    fn tables_shrink_columns_to_fit() {
        let doc = lay(
            "| a long header cell | another long header cell |\n|---|---|\n| short | words that wrap around |",
            160.0,
        );
        let right = doc
            .rects
            .iter()
            .filter(|rect| rect.w == 1.0)
            .map(|rect| rect.x)
            .fold(0.0_f32, f32::max);
        assert!(right <= 161.0, "the grid fits the width: {right}");
    }
}
