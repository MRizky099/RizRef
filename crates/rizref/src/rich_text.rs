//! Free-standing text items ("Add text"): shaping, rendering and the styled
//! editing helpers.
//!
//! A text item is a note whose format is [`NoteFormat::Rich`]: plain words
//! plus [`TextSpan`] runs of bold, italic, underline, colour and font size.
//! Unlike a note it has no card and no backdrop — only the glyphs are
//! painted, on a transparent texture.
//!
//! Its box is its text. While it is edited the box hugs the words, growing
//! and shrinking as they are typed; lines break only at Enter. Once it is
//! done, the box has only corner handles, and dragging one scales the text
//! itself: the item is drawn at the scale of its box over the text's natural
//! size, and when the drag ends that scale is baked into the font sizes, so
//! the size the format bar shows is the size on the board.
//!
//! [`NoteFormat::Rich`]: rizref_core::NoteFormat::Rich

use cosmic_text::{
    Attrs, AttrsList, Buffer, CacheMetrics, Color, Cursor, Edit as _, Editor, Metrics, Shaping,
    Style, TextDecoration, UnderlineStyle, Weight, Wrap,
};
use glam::Vec2;
use rizref_codecs::ImageBuffer;
use rizref_core::rich::{self, DEFAULT_TEXT_SIZE, TextSpan, TextStyle, normalize_size};
use rizref_core::{NoteContent, TEXT_PADDING};
use tiny_skia::Pixmap;

use crate::markdown::style::LIGHT_INK;
use crate::text::TextEngine;

/// Line height as a multiple of the font size.
pub const TEXT_LINE_HEIGHT: f32 = 1.3;

/// The ink of characters without an explicit colour: the warm light ink of
/// the dark canvas.
pub const DEFAULT_TEXT_INK: [u8; 3] = LIGHT_INK;

/// The colours offered for text, first the default ink.
pub const TEXT_PRESETS: [[u8; 3]; 12] = [
    DEFAULT_TEXT_INK,
    [0xff, 0xff, 0xff],
    [0x9a, 0x9a, 0x9a],
    [0x1b, 0x1b, 0x1b],
    [0xe5, 0x53, 0x4b],
    [0xf0, 0x88, 0x3e],
    [0xf2, 0xcc, 0x60],
    [0x57, 0xab, 0x5a],
    [0x39, 0xc5, 0xbb],
    [0x53, 0x9b, 0xf5],
    [0x98, 0x6e, 0xe2],
    [0xe2, 0x75, 0xad],
];

/// Shaping metrics for `size` world units at `pixels` per world unit.
fn metrics(size: f32, pixels: f32) -> Metrics {
    let font = (size * pixels).max(1.0);
    Metrics::new(font, font * TEXT_LINE_HEIGHT)
}

/// The cosmic-text attributes for one style at `pixels` per world unit.
pub fn attrs_for(style: &TextStyle, pixels: f32) -> Attrs<'static> {
    let mut attrs = Attrs::new();
    if style.bold {
        attrs = attrs.weight(Weight::BOLD);
    }
    if style.italic {
        attrs = attrs.style(Style::Italic);
    }
    if style.underline {
        attrs.text_decoration = TextDecoration {
            underline: UnderlineStyle::Single,
            ..TextDecoration::new()
        };
    }
    if let Some([r, g, b]) = style.color {
        attrs = attrs.color(Color::rgb(r, g, b));
    }
    if let Some(size) = style.size {
        attrs = attrs.metrics(metrics(f32::from(size), pixels));
    }
    attrs
}

/// The style cosmic-text attributes describe at `pixels` per world unit
/// (the inverse of [`attrs_for`]).
pub fn style_of(attrs: &Attrs, pixels: f32) -> TextStyle {
    let size = attrs.metrics_opt.map(|cached: CacheMetrics| {
        let font = Metrics::from(cached).font_size / pixels.max(1.0e-6);
        font.round().clamp(0.0, f32::from(u16::MAX)) as u16
    });
    TextStyle {
        bold: attrs.weight.0 >= 600,
        italic: attrs.style != Style::Normal,
        underline: attrs.text_decoration.underline != UnderlineStyle::None,
        color: attrs
            .color_opt
            .map(|color| [color.r(), color.g(), color.b()]),
        size: normalize_size(size),
    }
}

/// A buffer for text items at `pixels` per text unit: default-size lines
/// that break only where the author pressed Enter.
pub fn new_buffer(engine: &mut TextEngine, pixels: f32) -> Buffer {
    let mut buffer = Buffer::new(
        engine.font_system_mut(),
        metrics(f32::from(DEFAULT_TEXT_SIZE), pixels),
    );
    buffer.set_wrap(Wrap::None);
    buffer.set_size(None, None);
    buffer
}

/// Fills `buffer` with `text` styled by `spans`.
pub fn set_buffer_text(buffer: &mut Buffer, text: &str, spans: &[TextSpan], pixels: f32) {
    let mut pieces: Vec<(&str, Attrs)> = Vec::new();
    let mut at = 0;
    for span in spans {
        if span.start > at {
            pieces.push((&text[at..span.start], Attrs::new()));
        }
        pieces.push((&text[span.start..span.end], attrs_for(&span.style, pixels)));
        at = span.end;
    }
    if at < text.len() || pieces.is_empty() {
        pieces.push((&text[at..], Attrs::new()));
    }
    buffer.set_rich_text(pieces, &Attrs::new(), Shaping::Advanced, None);
}

/// Reads a buffer back into plain text and normalised spans.
pub fn buffer_content(buffer: &Buffer, pixels: f32) -> (String, Vec<TextSpan>) {
    let mut text = String::new();
    let mut spans = Vec::new();
    for (index, line) in buffer.lines.iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let start = text.len();
        text.push_str(line.text());
        let defaults = style_of(&line.attrs_list().defaults(), pixels);
        if !defaults.is_default() && !line.text().is_empty() {
            spans.push(TextSpan {
                start,
                end: start + line.text().len(),
                style: defaults,
            });
        }
        for (range, attrs) in line.attrs_list().spans_iter() {
            spans.push(TextSpan {
                start: start + range.start,
                end: start + range.end,
                style: style_of(&attrs.as_attrs(), pixels),
            });
        }
    }
    let spans = rich::normalize(&text, spans);
    (text, spans)
}

/// Writes `spans` (over the buffer's joined text) onto its lines' attribute
/// lists.
pub fn write_buffer_spans(buffer: &mut Buffer, spans: &[TextSpan], pixels: f32) {
    let mut start = 0;
    for line in &mut buffer.lines {
        let end = start + line.text().len();
        let mut list = AttrsList::new(&Attrs::new());
        for span in spans {
            let s = span.start.max(start);
            let e = span.end.min(end);
            if s < e {
                list.add_span(s - start..e - start, &attrs_for(&span.style, pixels));
            }
        }
        line.set_attrs_list(list);
        start = end + 1;
    }
}

/// Byte offset of a cursor in the buffer's joined text.
pub fn offset_of(buffer: &Buffer, cursor: Cursor) -> usize {
    buffer
        .lines
        .iter()
        .take(cursor.line)
        .map(|line| line.text().len() + 1)
        .sum::<usize>()
        + cursor.index
}

/// The height of a shaped buffer's text in pixels: at least one line of the
/// default size, so an empty item keeps room for the caret.
pub fn buffer_height(buffer: &Buffer) -> f32 {
    buffer
        .layout_runs()
        .map(|run| run.line_top + run.line_height)
        .fold(buffer.metrics().line_height, f32::max)
}

/// The width of a shaped buffer's widest line in pixels, never below
/// `minimum` (room for the caret in an empty item).
pub fn buffer_width(buffer: &Buffer, minimum: f32) -> f32 {
    buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(minimum, f32::max)
}

/// The natural box of a shaped buffer in text units (`pixels` per unit): its
/// text plus the padding on every side.
pub fn buffer_natural(buffer: &Buffer, pixels: f32) -> Vec2 {
    let caret_room = buffer.metrics().font_size * 0.5;
    Vec2::new(buffer_width(buffer, caret_room), buffer_height(buffer)) / pixels
        + Vec2::splat(2.0 * TEXT_PADDING)
}

/// The natural (unscaled) box of `content` in text units: its lines at their
/// font sizes plus the padding, shaped at `pixels` per unit (the density it
/// renders at, so the measure is the rendered one).
pub fn natural_size(engine: &mut TextEngine, content: &NoteContent, pixels: f32) -> Vec2 {
    let mut buffer = new_buffer(engine, pixels);
    set_buffer_text(&mut buffer, &content.text, &content.spans, pixels);
    buffer.shape_until_scroll(engine.font_system_mut(), false);
    buffer_natural(&buffer, pixels)
}

/// How much a box of `size` scales text whose natural box is `natural`: the
/// smaller axis, so the text always fits inside its box. Right after a
/// corner drag the scale is baked into the font sizes, so this is 1 for a
/// settled item and only differs while the box is being dragged.
pub fn scale_for(natural: Vec2, size: Vec2) -> f32 {
    (size.x / natural.x.max(1.0e-3))
        .min(size.y / natural.y.max(1.0e-3))
        .clamp(1.0e-3, 1.0e3)
}

/// The box of a new, empty text item typed at `size` world units: one line,
/// with room for the caret.
pub fn new_box(size: u16) -> Vec2 {
    let size = f32::from(size);
    Vec2::new(size * 0.5, size * TEXT_LINE_HEIGHT) + Vec2::splat(2.0 * TEXT_PADDING)
}

/// Rasterises a text item that is not being edited: transparent, with the
/// glyphs, decorations, colours and sizes of its spans, scaled to its box.
pub fn render(
    engine: &mut TextEngine,
    content: &NoteContent,
    size: Vec2,
    pixels: f32,
) -> Option<ImageBuffer> {
    let (width, height) = crate::note::texture_size(size, pixels);
    let mut pixmap = Pixmap::new(width, height)?;
    let natural = natural_size(engine, content, pixels);
    let pixels = pixels * scale_for(natural, size);
    let mut buffer = new_buffer(engine, pixels);
    set_buffer_text(&mut buffer, &content.text, &content.spans, pixels);
    let padding = TEXT_PADDING * pixels;
    let ink = DEFAULT_TEXT_INK;
    engine.draw_buffer(
        &mut pixmap,
        &mut buffer,
        padding,
        padding,
        [ink[0], ink[1], ink[2], 255],
    );
    ImageBuffer::from_rgba(width, height, crate::note::straight_rgba(&pixmap))
}

/// A transparent stand-in: what a text item shows before its first render.
pub fn render_placeholder(size: Vec2) -> Option<ImageBuffer> {
    let density = (crate::note::NOTE_LOW_SIDE / size.x.max(size.y).max(1.0))
        .min(crate::note::NOTE_SUPERSAMPLE);
    let (width, height) = crate::note::texture_size(size, density);
    let pixmap = Pixmap::new(width, height)?;
    ImageBuffer::from_rgba(width, height, crate::note::straight_rgba(&pixmap))
}

/// Which on/off property a toolbar button or shortcut toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleToggle {
    /// Bold.
    Bold,
    /// Italic.
    Italic,
    /// Underline.
    Underline,
}

impl StyleToggle {
    /// Reads the property.
    pub const fn get(self, style: &TextStyle) -> bool {
        match self {
            Self::Bold => style.bold,
            Self::Italic => style.italic,
            Self::Underline => style.underline,
        }
    }

    /// Writes the property.
    pub fn set(self, style: &mut TextStyle, on: bool) {
        match self {
            Self::Bold => style.bold = on,
            Self::Italic => style.italic = on,
            Self::Underline => style.underline = on,
        }
    }
}

/// The styled-editing state of a text session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RichState {
    /// What the next typed character looks like.
    pub typing: TextStyle,
    /// What the format bar shows: the typing style, or the selection's
    /// common style. Cached after every edit, so drawing and hovering the
    /// bar never re-reads the buffer.
    pub shown: TextStyle,
    /// The item's scale (its box over its text's natural size) when the
    /// session began, kept while typing: 1 for a settled item.
    pub scale: f32,
}

impl RichState {
    /// The style typing continues with at the editor's caret: the character
    /// before it, or at the start of a line the first character of that line
    /// (else the end of the line above).
    pub fn style_at_caret(editor: &Editor<'static>, pixels: f32) -> TextStyle {
        let cursor = editor.cursor();
        editor.with_buffer(|buffer| {
            let style_before = |line: usize, index: usize| -> Option<TextStyle> {
                let text = buffer.lines.get(line)?.text();
                let previous = text[..index.min(text.len())].char_indices().next_back()?.0;
                Some(style_of(
                    &buffer.lines[line].attrs_list().get_span(previous),
                    pixels,
                ))
            };
            style_before(cursor.line, cursor.index)
                .or_else(|| {
                    let line = buffer.lines.get(cursor.line)?;
                    (!line.text().is_empty())
                        .then(|| style_of(&line.attrs_list().get_span(0), pixels))
                })
                .or_else(|| {
                    let above = cursor.line.checked_sub(1)?;
                    style_before(above, buffer.lines[above].text().len())
                })
                .unwrap_or_default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{attrs_for, buffer_content, natural_size, new_box, render, scale_for, style_of};
    use crate::text::TextEngine;
    use cosmic_text::{Buffer, Metrics};
    use glam::Vec2;
    use rizref_core::NoteContent;
    use rizref_core::rich::{TextSpan, TextStyle};

    #[test]
    fn attrs_and_styles_convert_both_ways_at_any_density() {
        let style = TextStyle {
            bold: true,
            italic: true,
            underline: true,
            color: Some([10, 20, 30]),
            size: Some(48),
        };
        for pixels in [0.5, 1.0, 2.0] {
            assert_eq!(style_of(&attrs_for(&style, pixels), pixels), style);
        }
        assert!(style_of(&attrs_for(&TextStyle::default(), 2.0), 2.0).is_default());
    }

    #[test]
    fn a_buffer_round_trips_text_and_spans_across_lines() {
        let mut engine = TextEngine::new();
        let mut content = NoteContent::rich("one\ntwo three");
        content.spans = vec![TextSpan {
            start: 2,
            end: 7,
            style: TextStyle {
                bold: true,
                ..TextStyle::default()
            },
        }];
        let mut buffer = Buffer::new(engine.font_system_mut(), Metrics::new(16.0, 20.0));
        super::set_buffer_text(&mut buffer, &content.text, &content.spans, 2.0);
        let (text, spans) = buffer_content(&buffer, 2.0);
        assert_eq!(text, content.text);
        // The newline is not styled, so the run splits around it.
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start, spans[0].end), (2, 3));
        assert_eq!((spans[1].start, spans[1].end), (4, 7));
    }

    #[test]
    fn the_natural_box_follows_lines_and_sizes_and_the_box_scales_it() {
        let mut engine = TextEngine::new();
        let one = natural_size(&mut engine, &NoteContent::rich("a few words"), 2.0);
        let two = natural_size(&mut engine, &NoteContent::rich("a few words\nmore"), 2.0);
        assert!((two.x - one.x).abs() < 0.5, "never wraps: same widest line");
        assert!(two.y > one.y * 1.5);
        let mut big = NoteContent::rich("a few words");
        big.spans = vec![TextSpan {
            start: 0,
            end: big.text.len(),
            style: TextStyle {
                size: Some(72),
                ..TextStyle::default()
            },
        }];
        let large = natural_size(&mut engine, &big, 2.0);
        // Three times the type: three times the text, plus the same padding.
        let pad = Vec2::splat(2.0 * rizref_core::TEXT_PADDING);
        let ratio = (large - pad) / (one - pad);
        assert!(ratio.x > 2.7 && ratio.y > 2.9, "{ratio}");
        assert!((scale_for(one, one * 3.0) - 3.0).abs() < 1.0e-4);
        // An empty item is one default line with room for the caret.
        let empty = natural_size(&mut engine, &NoteContent::rich(""), 2.0);
        assert!((empty - new_box(24)).abs().max_element() < 1.0, "{empty}");
    }

    #[test]
    fn underlined_text_draws_a_continuous_line() {
        // Narrow letters leave gaps between glyphs; only an underline gives
        // a long unbroken run of ink on one row.
        let longest_run = |content: &NoteContent| {
            let mut engine = TextEngine::new();
            let image = render(&mut engine, content, Vec2::new(300.0, 60.0), 2.0).expect("renders");
            let width = image.width() as usize;
            image
                .pixels()
                .chunks_exact(width * 4)
                .map(|row| {
                    let mut best = 0;
                    let mut run = 0;
                    for pixel in row.as_chunks::<4>().0 {
                        run = if pixel[3] > 128 { run + 1 } else { 0 };
                        best = best.max(run);
                    }
                    best
                })
                .max()
                .unwrap_or(0)
        };
        let plain = NoteContent::rich("iiiiiiii");
        let mut underlined = plain.clone();
        underlined.spans = vec![TextSpan {
            start: 0,
            end: 8,
            style: TextStyle {
                underline: true,
                ..TextStyle::default()
            },
        }];
        assert!(longest_run(&plain) < 20, "{}", longest_run(&plain));
        assert!(
            longest_run(&underlined) > 60,
            "{}",
            longest_run(&underlined)
        );
    }

    #[test]
    fn a_rendered_text_item_is_transparent_except_its_glyphs() {
        let mut engine = TextEngine::new();
        let mut content = NoteContent::rich("Red");
        content.spans = vec![TextSpan {
            start: 0,
            end: 3,
            style: TextStyle {
                color: Some([255, 0, 0]),
                ..TextStyle::default()
            },
        }];
        let image = render(&mut engine, &content, Vec2::new(200.0, 60.0), 2.0).expect("renders");
        let pixels = image.pixels().as_chunks::<4>().0;
        assert_eq!(pixels[0][3], 0, "no card behind the text");
        let red = pixels
            .iter()
            .filter(|p| p[3] > 200 && p[0] > 220 && p[1] < 40 && p[2] < 40)
            .count();
        assert!(red > 40, "red glyph pixels: {red}");
    }
}
