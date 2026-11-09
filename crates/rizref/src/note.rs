//! Note rendering and the in-place source editor.
//!
//! A note is an ordinary item whose payload is text. The app rasterises it into
//! a pixmap, uploads that as a texture and draws it like any image.
//!
//! * **Markdown notes** (phase 06a) are parsed and laid out in world units at
//!   their wrap width by [`crate::markdown`], then painted here at the texture
//!   density. The layout is cached by `(source, wrap width)`, so a resize only
//!   re-rasterises; it never re-shapes, and never moves a line break.
//! * **Plain notes** (phase-04 boards) keep the original path: one word-wrapped
//!   block at the box's inner width, in the auto-contrast ink.
//!
//! Editing keeps a cosmic-text [`Editor`] over the **source** for the session,
//! so the caret, arrow keys and selection are handled by the shaping library.
//! Its wrap width is the note's wrap width, so the source wraps where the
//! rendered document does and resizing the box never rewraps it either.
//!
//! Textures are content-addressed (`rizref-note-v2`): the id hashes everything
//! that changes pixels, so identical notes share one GPU texture and an edited
//! note simply gets a new id.

use std::collections::HashMap;

use cosmic_text::{
    Action, Attrs, Buffer, Cursor, Edit as _, Editor, Metrics, Selection as TextSelection, Shaping,
    Wrap,
};
use glam::Vec2;
use rizref_codecs::ImageBuffer;
use rizref_core::{AssetId, Item, ItemId, NoteContent, NoteFormat, checksum};
use tiny_skia::{Color, FillRule, Paint as SkPaint, Path, PathBuilder, Pixmap, Stroke, Transform};

use crate::markdown::layout::{BASE_FONT_SIZE, Fill, NoteLayout, Paint, layout};
use crate::markdown::math::MathCache;
use crate::markdown::parse::parse;
use crate::markdown::style::NotePalette;
use crate::text::TextEngine;

/// Texture pixels per world unit for a note.
pub const NOTE_SUPERSAMPLE: f32 = 2.0;

/// Longest note texture side, in pixels.
pub const NOTE_MAX_SIDE: f32 = 2048.0;

/// Distance between the note's border and its text, in world units.
pub const NOTE_PADDING: f32 = rizref_core::NOTE_PADDING;

/// Text size in world units.
pub const NOTE_FONT_SIZE: f32 = BASE_FONT_SIZE;

/// Corner radius in world units.
pub const NOTE_RADIUS: f32 = 10.0;

/// Background opacity of the card (the phase-04 card was 242/255).
const CARD_ALPHA: u8 = 242;

/// Error colour for equations that could not be typeset.
const ERROR_COLOUR: [u8; 3] = [0xe0, 0x4f, 0x3f];

/// Most cached layouts before the cache is cleared.
const LAYOUT_CACHE_LIMIT: usize = 256;

/// The palette a note's content asks for.
pub fn palette(content: &NoteContent) -> NotePalette {
    NotePalette::for_background(content.background_or_default())
}

/// The width a note's content is laid out at.
pub fn wrap_width(content: &NoteContent, size: Vec2) -> f32 {
    content
        .wrap_width
        .unwrap_or_else(|| rizref_core::inner_note_width(size.x))
}

/// One note-editing session.
///
/// The editor owns its shaped buffer; the font system stays in the shared
/// [`TextEngine`], which is why the methods take the engine back in.
pub struct NoteSession {
    item: ItemId,
    editor: Editor<'static>,
    original: NoteContent,
    wrap_width: f32,
    pixels_per_world: f32,
    provisional: bool,
    was_clean: bool,
}

impl NoteSession {
    /// Starts editing `item`, or `None` when it is not a note.
    ///
    /// `provisional` marks a freshly created note that is not in the history
    /// yet; `was_clean` remembers whether the document was saved, so a cancel
    /// can restore the title's "not dirty" state.
    pub fn begin(
        engine: &mut TextEngine,
        item: &Item,
        provisional: bool,
        was_clean: bool,
    ) -> Option<Self> {
        let original = item.note_content()?;
        let wrap = wrap_width(&original, item.size());
        let pixels_per_world = pixels_per_world(item.size());
        let font_size = NOTE_FONT_SIZE * pixels_per_world;
        let mut buffer = Buffer::new(
            engine.font_system_mut(),
            Metrics::new(font_size, (font_size * 1.4).ceil()),
        );
        buffer.set_wrap(Wrap::WordOrGlyph);
        let (_, height) = text_area(item.size(), pixels_per_world);
        buffer.set_size(Some(wrap * pixels_per_world), Some(height));
        buffer.set_text(&original.text, &Attrs::new(), Shaping::Advanced, None);
        Some(Self {
            item: item.id(),
            editor: Editor::new(buffer),
            original,
            wrap_width: wrap,
            pixels_per_world,
            provisional,
            was_clean,
        })
    }

    /// The note being edited.
    pub const fn item(&self) -> ItemId {
        self.item
    }

    /// The content as it was when the session began.
    pub const fn original(&self) -> &NoteContent {
        &self.original
    }

    /// Whether the note was created by this session and is not in history.
    pub const fn is_provisional(&self) -> bool {
        self.provisional
    }

    /// Whether the document was saved when the session began.
    pub const fn was_clean(&self) -> bool {
        self.was_clean
    }

    /// Pixels per world unit this session was shaped at.
    pub const fn pixels_per_world(&self) -> f32 {
        self.pixels_per_world
    }

    /// The current source text.
    pub fn text(&self) -> String {
        self.editor.with_buffer(buffer_text)
    }

    /// The content a commit writes: the edited source as markdown, the
    /// original background, and the session's wrap width persisted.
    pub fn content(&self) -> NoteContent {
        NoteContent {
            text: self.text(),
            format: NoteFormat::Markdown,
            background: self.original.background,
            wrap_width: Some(self.wrap_width),
        }
    }

    /// The content the live preview writes while typing: the original
    /// format and wrap width, so a cancelled edit leaves nothing behind.
    pub fn live_content(&self) -> NoteContent {
        NoteContent {
            text: self.text(),
            ..self.original.clone()
        }
    }

    /// Updates the visible height after the box was resized; the wrap width
    /// (and so every line break) is unchanged.
    pub fn resize(&mut self, size: Vec2) {
        let (_, height) = text_area(size, self.pixels_per_world);
        let width = self.wrap_width * self.pixels_per_world;
        self.editor.with_buffer_mut(|buffer| {
            buffer.set_size(Some(width), Some(height));
        });
    }

    /// Applies one editor action.
    pub fn action(&mut self, engine: &mut TextEngine, action: Action) {
        self.editor
            .borrow_with(engine.font_system_mut())
            .action(action);
    }

    /// Places the caret at a note-local pixel position.
    pub fn click(&mut self, engine: &mut TextEngine, x: f32, y: f32) {
        self.action(
            engine,
            Action::Click {
                x: x.round() as i32,
                y: y.round() as i32,
            },
        );
    }

    /// Inserts text, one character per action so newlines stay lines.
    pub fn insert_text(&mut self, engine: &mut TextEngine, text: &str) {
        for character in text.chars() {
            if character.is_control() && character != '\n' {
                continue;
            }
            self.action(
                engine,
                if character == '\n' {
                    Action::Enter
                } else {
                    Action::Insert(character)
                },
            );
        }
    }

    /// Selects the whole note, so the next typed character replaces it.
    pub fn select_all(&mut self) {
        let (line, index) = self.editor.with_buffer(|buffer| {
            let line = buffer.lines.len().saturating_sub(1);
            let index = buffer.lines.last().map_or(0, |line| line.text().len());
            (line, index)
        });
        self.editor
            .set_selection(TextSelection::Normal(Cursor::new(0, 0)));
        self.editor.set_cursor(Cursor::new(line, index));
    }
}

/// Collects a buffer's lines back into one string.
fn buffer_text(buffer: &Buffer) -> String {
    let mut out = String::new();
    for (index, line) in buffer.lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(line.text());
    }
    out
}

/// Texture pixels per world unit for a note of `size`.
///
/// Large notes are rasterised at a lower density rather than refusing to grow a
/// texture past [`NOTE_MAX_SIDE`]; the text stays sharp because the displayed
/// size stays below the texture size in the common case.
pub fn pixels_per_world(size: Vec2) -> f32 {
    let longest = size.x.max(size.y).max(1.0);
    NOTE_SUPERSAMPLE.min(NOTE_MAX_SIDE / longest).max(0.05)
}

/// The text area of a note in texture pixels.
fn text_area(size: Vec2, pixels_per_world: f32) -> (f32, f32) {
    let (width, height) = texture_size(size, pixels_per_world);
    let padding = NOTE_PADDING * pixels_per_world;
    (
        (width as f32 - 2.0 * padding).max(8.0),
        (height as f32 - 2.0 * padding).max(8.0),
    )
}

/// The texture dimensions for a note of `size`.
fn texture_size(size: Vec2, pixels_per_world: f32) -> (u32, u32) {
    (
        (size.x * pixels_per_world)
            .round()
            .clamp(8.0, NOTE_MAX_SIDE) as u32,
        (size.y * pixels_per_world)
            .round()
            .clamp(8.0, NOTE_MAX_SIDE) as u32,
    )
}

/// Parsed-and-laid-out notes plus the math cache, shared by every note.
#[derive(Default)]
pub struct NoteRenderer {
    math: MathCache,
    layouts: HashMap<(String, u32), NoteLayout>,
}

impl NoteRenderer {
    /// An empty renderer.
    pub fn new() -> Self {
        Self::default()
    }

    /// The layout of `text` at `wrap` world units, cached.
    pub fn layout(&mut self, engine: &mut TextEngine, text: &str, wrap: f32) -> &NoteLayout {
        let key = (text.to_owned(), wrap.to_bits());
        if !self.layouts.contains_key(&key) {
            if self.layouts.len() >= LAYOUT_CACHE_LIMIT {
                self.layouts.clear();
            }
            let document = parse(text);
            let laid_out = layout(engine.font_system_mut(), &mut self.math, &document, wrap);
            self.layouts.insert(key.clone(), laid_out);
        }
        &self.layouts[&key]
    }

    /// The laid-out content height of a markdown note, in world units.
    pub fn content_height(
        &mut self,
        engine: &mut TextEngine,
        content: &NoteContent,
        size: Vec2,
    ) -> f32 {
        let wrap = wrap_width(content, size);
        match content.format {
            NoteFormat::Markdown => self.layout(engine, &content.text, wrap).height,
            NoteFormat::Plain => plain_height(engine, &content.text, wrap),
        }
    }

    /// Rasterises a note, with the live editor when one is active.
    pub fn render(
        &mut self,
        engine: &mut TextEngine,
        content: &NoteContent,
        size: Vec2,
        session: Option<&mut NoteSession>,
    ) -> Option<ImageBuffer> {
        let colours = palette(content);
        let pixels_per_world = match &session {
            Some(session) => session.pixels_per_world,
            None => pixels_per_world(size),
        };
        let (width, height) = texture_size(size, pixels_per_world);
        let mut pixmap = Pixmap::new(width, height)?;
        draw_background(&mut pixmap, pixels_per_world, &colours);
        let padding = NOTE_PADDING * pixels_per_world;
        let clip = [
            padding.floor() as i32,
            padding.floor() as i32,
            (width as f32 - padding).ceil() as i32,
            (height as f32 - padding).ceil() as i32,
        ];
        let ink = rgba(colours.ink, 255);

        match (session, content.format) {
            (Some(session), _) => engine.draw_editor(
                &mut pixmap,
                &mut session.editor,
                padding,
                padding,
                ink,
                rgba(colours.caret, 255),
                colours.selection,
                ink,
                clip,
            ),
            (None, NoteFormat::Plain) => engine.draw_block(
                &mut pixmap,
                &content.text,
                padding,
                padding,
                width as f32 - 2.0 * padding,
                height as f32 - 2.0 * padding,
                NOTE_FONT_SIZE * pixels_per_world,
                ink,
            ),
            (None, NoteFormat::Markdown) => {
                let wrap = wrap_width(content, size);
                // Split the borrow: the layout lives in `self.layouts`, the
                // math masks in `self.math`.
                let key = (content.text.clone(), wrap.to_bits());
                self.layout(engine, &content.text, wrap);
                let laid_out = &self.layouts[&key];
                paint_layout(
                    engine,
                    &mut self.math,
                    &mut pixmap,
                    laid_out,
                    &colours,
                    padding,
                    pixels_per_world,
                    clip,
                );
            }
        }

        ImageBuffer::from_rgba(width, height, straight_rgba(&pixmap))
    }
}

/// The content height of a plain note wrapped at `wrap` world units.
fn plain_height(engine: &mut TextEngine, text: &str, wrap: f32) -> f32 {
    let fonts = engine.font_system_mut();
    let metrics = Metrics::new(NOTE_FONT_SIZE, (NOTE_FONT_SIZE * 1.4).ceil());
    let mut buffer = Buffer::new(fonts, metrics);
    buffer.set_wrap(Wrap::Word);
    buffer.set_size(Some(wrap), None);
    buffer.set_text(text, &Attrs::new(), Shaping::Advanced, None);
    buffer.shape_until_scroll(fonts, false);
    buffer
        .layout_runs()
        .map(|run| run.line_top + run.line_height)
        .fold(0.0, f32::max)
}

const fn rgba(rgb: [u8; 3], alpha: u8) -> [u8; 4] {
    [rgb[0], rgb[1], rgb[2], alpha]
}

/// Resolves a layout paint against the note palette.
fn resolve(paint: Paint, colours: &NotePalette) -> [u8; 4] {
    match paint {
        Paint::Ink(alpha) => colours.ink_alpha(alpha),
        Paint::Link => colours
            .link
            .map_or(rgba(colours.ink, 255), |link| rgba(link, 255)),
        Paint::Color(color) => color,
        Paint::Error => rgba(ERROR_COLOUR, 255),
    }
}

/// Paints a laid-out markdown note at `scale` pixels per world unit, with
/// the content's top-left at `padding` pixels.
#[allow(clippy::too_many_arguments)]
fn paint_layout(
    engine: &mut TextEngine,
    math: &mut MathCache,
    pixmap: &mut Pixmap,
    laid_out: &NoteLayout,
    colours: &NotePalette,
    padding: f32,
    scale: f32,
    clip: [i32; 4],
) {
    let panel = if colours.light_ink {
        [255, 255, 255, 26]
    } else {
        [0, 0, 0, 20]
    };
    let to_px = |value: f32| padding + value * scale;
    let mask = clip_mask(pixmap, clip);
    for rect in &laid_out.rects {
        let fill = match rect.fill {
            Fill::Panel => panel,
            Fill::Paint(paint) => resolve(paint, colours),
        };
        fill_rect(
            pixmap,
            [to_px(rect.x), to_px(rect.y), rect.w * scale, rect.h * scale],
            rect.radius * scale,
            fill,
            mask.as_ref(),
        );
    }
    for text in &laid_out.texts {
        let text_clip = match text.clip {
            Some([x0, x1]) => [
                clip[0].max(to_px(x0).floor() as i32),
                clip[1],
                clip[2].min(to_px(x1).ceil() as i32),
                clip[3],
            ],
            None => clip,
        };
        engine.draw_scaled(
            pixmap,
            &text.buffer,
            (to_px(text.x), to_px(text.y)),
            scale,
            text_clip,
            |metadata| {
                let style = text.styles.get(metadata)?;
                // Math placeholders are composited separately.
                style.math.is_none().then(|| resolve(style.paint, colours))
            },
        );
    }
    for rect in &laid_out.overlays {
        let fill = match rect.fill {
            Fill::Panel => panel,
            Fill::Paint(paint) => resolve(paint, colours),
        };
        fill_rect(
            pixmap,
            [to_px(rect.x), to_px(rect.y), rect.w * scale, rect.h * scale],
            rect.radius * scale,
            fill,
            mask.as_ref(),
        );
    }
    for item in &laid_out.math {
        let atom = &laid_out.atoms[item.atom];
        let Some(equation) = math.mask(&atom.tex, item.display, item.font_size * scale) else {
            continue;
        };
        let colour = resolve(item.paint, colours);
        let left = to_px(item.x).round() as i32;
        let top = (to_px(item.baseline) - equation.baseline as f32).round() as i32;
        composite_mask(
            pixmap,
            &equation.alpha,
            equation.width,
            equation.height,
            left,
            top,
            colour,
            clip,
        );
    }
}

/// A tiny-skia mask for the content rectangle, when it is smaller than the
/// pixmap.
fn clip_mask(pixmap: &Pixmap, clip: [i32; 4]) -> Option<tiny_skia::Mask> {
    let mut mask = tiny_skia::Mask::new(pixmap.width(), pixmap.height())?;
    let rect = tiny_skia::Rect::from_ltrb(
        clip[0] as f32,
        clip[1] as f32,
        clip[2] as f32,
        clip[3] as f32,
    )?;
    mask.fill_path(
        &PathBuilder::from_rect(rect),
        FillRule::Winding,
        false,
        Transform::identity(),
    );
    Some(mask)
}

fn fill_rect(
    pixmap: &mut Pixmap,
    [x, y, w, h]: [f32; 4],
    radius: f32,
    colour: [u8; 4],
    mask: Option<&tiny_skia::Mask>,
) {
    let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w.max(1.0), h.max(1.0)) else {
        return;
    };
    let mut paint = SkPaint {
        anti_alias: true,
        ..SkPaint::default()
    };
    paint.set_color(Color::from_rgba8(
        colour[0], colour[1], colour[2], colour[3],
    ));
    if radius > 0.5 {
        if let Some(path) = rounded_rect(rect, radius) {
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                mask,
            );
        }
    } else {
        pixmap.fill_rect(rect, &paint, Transform::identity(), mask);
    }
}

/// Composites a coverage mask tinted with `colour` at `left`, `top`.
#[allow(clippy::too_many_arguments)]
fn composite_mask(
    pixmap: &mut Pixmap,
    alpha: &[u8],
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    colour: [u8; 4],
    clip: [i32; 4],
) {
    let target_width = pixmap.width() as i32;
    let target_height = pixmap.height() as i32;
    let data = pixmap.data_mut();
    for row in 0..height as i32 {
        let y = top + row;
        if y < clip[1].max(0) || y >= clip[3].min(target_height) {
            continue;
        }
        for column in 0..width as i32 {
            let x = left + column;
            if x < clip[0].max(0) || x >= clip[2].min(target_width) {
                continue;
            }
            let coverage = alpha[(row as u32 * width + column as u32) as usize];
            if coverage == 0 {
                continue;
            }
            let a = (u16::from(coverage) * u16::from(colour[3]) / 255) as u8;
            let offset = (y as usize * target_width as usize + x as usize) * 4;
            blend(data, offset, colour, a);
        }
    }
}

/// Straight-alpha source over premultiplied destination.
fn blend(data: &mut [u8], offset: usize, colour: [u8; 4], alpha: u8) {
    let alpha = u16::from(alpha);
    let inverse = 255 - alpha;
    for channel in 0..3 {
        let src = u16::from(colour[channel]) * alpha / 255;
        data[offset + channel] =
            (src + u16::from(data[offset + channel]) * inverse / 255).min(255) as u8;
    }
    data[offset + 3] = (alpha + u16::from(data[offset + 3]) * inverse / 255).min(255) as u8;
}

/// The content-addressed texture id for a note.
pub fn texture_id(content: &NoteContent, size: Vec2, pixels_per_world: f32) -> AssetId {
    let mut bytes = Vec::with_capacity(content.text.len() + 48);
    bytes.extend_from_slice(b"rizref-note-v2\0");
    bytes.extend_from_slice(content.text.as_bytes());
    bytes.push(0);
    bytes.push(match content.format {
        NoteFormat::Plain => 0,
        NoteFormat::Markdown => 1,
    });
    bytes.extend_from_slice(&content.background_or_default());
    bytes.extend_from_slice(&wrap_width(content, size).to_bits().to_le_bytes());
    bytes.extend_from_slice(&size.x.to_bits().to_le_bytes());
    bytes.extend_from_slice(&size.y.to_bits().to_le_bytes());
    bytes.extend_from_slice(&pixels_per_world.to_bits().to_le_bytes());
    AssetId::from_checksum(checksum(&bytes))
}

/// Paints the rounded card and its border.
fn draw_background(pixmap: &mut Pixmap, pixels_per_world: f32, colours: &NotePalette) {
    let width = pixmap.width() as f32;
    let height = pixmap.height() as f32;
    let inset = (pixels_per_world * 1.5).max(1.0);
    let Some(rect) = tiny_skia::Rect::from_xywh(
        inset,
        inset,
        (width - 2.0 * inset).max(1.0),
        (height - 2.0 * inset).max(1.0),
    ) else {
        return;
    };
    let radius = (NOTE_RADIUS * pixels_per_world).min(rect.width().min(rect.height()) * 0.5);
    let Some(path) = rounded_rect(rect, radius) else {
        return;
    };

    let mut paint = SkPaint {
        anti_alias: true,
        ..SkPaint::default()
    };
    let [r, g, b] = colours.background;
    paint.set_color(Color::from_rgba8(r, g, b, CARD_ALPHA));
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    let [r, g, b] = colours.border;
    paint.set_color(Color::from_rgba8(r, g, b, 255));
    let stroke = Stroke {
        width: (1.5 * pixels_per_world).max(1.0),
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// Builds a rounded rectangle path.
fn rounded_rect(rect: tiny_skia::Rect, radius: f32) -> Option<Path> {
    let (x, y, width, height) = (rect.x(), rect.y(), rect.width(), rect.height());
    let radius = radius.max(0.0).min(width * 0.5).min(height * 0.5);
    let mut builder = PathBuilder::new();
    builder.move_to(x + radius, y);
    builder.line_to(x + width - radius, y);
    builder.quad_to(x + width, y, x + width, y + radius);
    builder.line_to(x + width, y + height - radius);
    builder.quad_to(x + width, y + height, x + width - radius, y + height);
    builder.line_to(x + radius, y + height);
    builder.quad_to(x, y + height, x, y + height - radius);
    builder.line_to(x, y + radius);
    builder.quad_to(x, y, x + radius, y);
    builder.close();
    builder.finish()
}

/// Converts tiny-skia's premultiplied pixels to straight-alpha RGBA.
fn straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = pixmap.data().to_vec();
    for pixel in out.as_chunks_mut::<4>().0 {
        let alpha = u16::from(pixel[3]);
        match alpha {
            0 => pixel[..3].fill(0),
            255 => {}
            _ => {
                for channel in pixel.iter_mut().take(3) {
                    *channel = ((u16::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{NOTE_PADDING, NoteRenderer, NoteSession, pixels_per_world, texture_id};
    use crate::markdown::style::{DARK_INK, LIGHT_INK};
    use crate::text::TextEngine;
    use cosmic_text::Action;
    use glam::Vec2;
    use rizref_codecs::ImageBuffer;
    use rizref_core::{Item, ItemId, NoteContent, NoteFormat};

    fn note(text: &str) -> Item {
        Item::note(
            ItemId::from_raw(1),
            text,
            Vec2::ZERO,
            Vec2::new(200.0, 120.0),
        )
    }

    fn content(text: &str, background: Option<[u8; 3]>) -> NoteContent {
        NoteContent {
            background,
            ..NoteContent::markdown(text)
        }
    }

    /// Pixels close (within `tolerance`) to `rgb` and nearly opaque.
    fn count_colour(image: &ImageBuffer, rgb: [u8; 3], tolerance: u8) -> usize {
        image
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| {
                pixel[3] > 200 && (0..3).all(|c| pixel[c].abs_diff(rgb[c]) <= tolerance)
            })
            .count()
    }

    #[test]
    fn a_rendered_note_is_non_transparent_and_sized_to_the_note() {
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let image = renderer
            .render(
                &mut engine,
                &content("study this", None),
                Vec2::new(200.0, 120.0),
                None,
            )
            .expect("the note rasterises");

        // 2x supersampling: 400 x 240.
        assert_eq!((image.width(), image.height()), (400, 240));
        let opaque = image
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[3] > 200)
            .count();
        assert!(opaque > 1000, "the note has a visible body");
    }

    #[test]
    fn the_requested_span_colours_reach_the_pixels() {
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let size = Vec2::new(420.0, 120.0);
        for background in [None, Some([250, 248, 240])] {
            let red = renderer
                .render(
                    &mut engine,
                    &content(
                        "This is <span style=\"color: red;\">red text</span> in a sentence",
                        background,
                    ),
                    size,
                    None,
                )
                .expect("renders");
            assert!(count_colour(&red, [255, 0, 0], 8) > 40, "red glyph pixels");
            let hex = renderer
                .render(
                    &mut engine,
                    &content(
                        "This is <span style=\"color: #ff5733;\">Custom color</span>",
                        background,
                    ),
                    size,
                    None,
                )
                .expect("renders");
            assert!(
                count_colour(&hex, [0xff, 0x57, 0x33], 8) > 40,
                "hex glyph pixels"
            );
        }
    }

    #[test]
    fn the_ink_flips_with_the_background_and_spans_do_not() {
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let size = Vec2::new(300.0, 100.0);
        let text = "**Heavy ink here** <span style=\"color: #00c000\">green</span>";
        let dark = renderer
            .render(&mut engine, &content(text, Some([60, 30, 90])), size, None)
            .expect("renders");
        let light = renderer
            .render(
                &mut engine,
                &content(text, Some([250, 250, 245])),
                size,
                None,
            )
            .expect("renders");
        assert!(count_colour(&dark, LIGHT_INK, 10) > 30);
        assert_eq!(count_colour(&dark, DARK_INK, 10), 0);
        assert!(count_colour(&light, DARK_INK, 10) > 30);
        assert!(count_colour(&dark, [0, 0xc0, 0], 10) > 10);
        assert!(count_colour(&light, [0, 0xc0, 0], 10) > 10);
    }

    #[test]
    fn math_is_composited_in_its_span_tint() {
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let image = renderer
            .render(
                &mut engine,
                &content(
                    "<span style=\"color: #0000ff\">$E=mc^2$</span>\n\n$$\\int_0^1 x^2\\,dx = \\tfrac{1}{3}$$",
                    Some([255, 255, 255]),
                ),
                Vec2::new(300.0, 160.0),
                None,
            )
            .expect("renders");
        assert!(
            count_colour(&image, [0, 0, 255], 12) > 20,
            "tinted inline math"
        );
    }

    #[test]
    fn resizing_changes_pixels_but_never_the_layout() {
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let mut note = content(&"words that wrap around the note ".repeat(8), None);
        note.wrap_width = Some(200.0);
        let wrap = super::wrap_width(&note, Vec2::new(228.0, 300.0));
        let before = renderer.layout(&mut engine, &note.text, wrap).signature();
        // A different box: the wrap width is stored, so the layout key is the same.
        let resized = super::wrap_width(&note, Vec2::new(600.0, 90.0));
        assert_eq!(wrap, resized);
        let after = renderer
            .layout(&mut engine, &note.text, resized)
            .signature();
        assert_eq!(before, after);
        let image = renderer
            .render(&mut engine, &note, Vec2::new(600.0, 90.0), None)
            .expect("renders");
        assert_eq!(
            image.width(),
            (600.0 * pixels_per_world(Vec2::new(600.0, 90.0))) as u32
        );
    }

    #[test]
    fn plain_notes_keep_the_phase_04_path() {
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let mut plain = content("**not bold**", None);
        plain.format = NoteFormat::Plain;
        let height = renderer.content_height(&mut engine, &plain, Vec2::new(200.0, 100.0));
        assert!(height > 10.0 && height < 30.0, "{height}");
        assert!(
            renderer
                .render(&mut engine, &plain, Vec2::new(200.0, 100.0), None)
                .is_some()
        );
    }

    #[test]
    fn texture_ids_are_content_addressed() {
        let size = Vec2::splat(100.0);
        let a = texture_id(&content("hello", None), size, 2.0);
        assert_eq!(a, texture_id(&content("hello", None), size, 2.0));
        assert_ne!(a, texture_id(&content("hello!", None), size, 2.0));
        assert_ne!(
            a,
            texture_id(&content("hello", None), Vec2::splat(200.0), 2.0)
        );
        assert_ne!(a, texture_id(&content("hello", Some([1, 2, 3])), size, 2.0));
        let mut wrapped = content("hello", None);
        wrapped.wrap_width = Some(50.0);
        assert_ne!(a, texture_id(&wrapped, size, 2.0));
    }

    #[test]
    fn an_editor_session_edits_the_source_and_commits_markdown() {
        let mut engine = TextEngine::new();
        let mut item = note("ab");
        let mut plain = item.note_content().expect("note");
        plain.format = NoteFormat::Plain;
        item.set_note(&plain);
        let mut session = NoteSession::begin(&mut engine, &item, false, true).expect("a note");

        session.click(&mut engine, 0.0, 0.0);
        session.select_all();
        session.insert_text(&mut engine, "hi\nthere");
        assert_eq!(session.text(), "hi\nthere");
        assert_eq!(session.original().text, "ab");
        assert_eq!(session.live_content().format, NoteFormat::Plain);
        let committed = session.content();
        assert_eq!(
            committed.format,
            NoteFormat::Markdown,
            "an edit upgrades plain"
        );
        assert_eq!(committed.wrap_width, Some(200.0 - 2.0 * NOTE_PADDING));

        session.action(&mut engine, Action::Backspace);
        assert_eq!(session.text(), "hi\nther");
        assert!(session.was_clean() && !session.is_provisional());
    }

    #[test]
    fn a_session_renders_its_caret() {
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let item = note("");
        let mut session = NoteSession::begin(&mut engine, &item, true, false).expect("a note");

        let image = renderer
            .render(
                &mut engine,
                &content("", None),
                Vec2::new(200.0, 120.0),
                Some(&mut session),
            )
            .expect("renders");

        // The caret uses the warm accent on the dark default card.
        let padding = (NOTE_PADDING * session.pixels_per_world()) as usize;
        let width = image.width() as usize;
        let mut warm = 0;
        for y in padding..padding + 30 {
            for x in padding..padding + 6 {
                let offset = (y * width + x) * 4;
                let pixel = &image.pixels()[offset..offset + 4];
                if pixel[0] > 200 && pixel[1] > 150 {
                    warm += 1;
                }
            }
        }
        assert!(warm > 0, "the caret is visible at the text origin");
    }

    /// The phase-06a budgets, measured rather than asserted (timings vary by
    /// machine). Run with
    /// `cargo test -p rizref --release -- --ignored --nocapture note_budgets`.
    #[test]
    #[ignore = "a timing report, not a correctness check"]
    fn note_budgets() {
        use std::time::Instant;
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let paragraph = "Some **bold** text, *italic* text, `code` and a [link](x). ";
        let mut source = String::from("# Study notes\n\n");
        while source.len() < 4000 {
            source.push_str(paragraph);
            source.push_str("\n\n- a list item\n- another one\n\n");
        }
        let mut note = content(&source, None);
        note.wrap_width = Some(400.0);
        let size = Vec2::new(428.0, 1400.0);

        let start = Instant::now();
        renderer.layout(&mut engine, &note.text, 400.0);
        let cold_layout = start.elapsed();
        renderer.render(&mut engine, &note, size, None);
        let start = Instant::now();
        renderer.render(&mut engine, &note, size, None);
        let warm_render = start.elapsed();
        let start = Instant::now();
        let resized = Vec2::new(600.0, 900.0);
        let _ = renderer.layout(&mut engine, &note.text, super::wrap_width(&note, resized));
        let resize_layout = start.elapsed();

        let maths: String = (0..20)
            .map(|i| format!("$x_{i}^2 + \\frac{{{i}}}{{y}}$ "))
            .collect();
        let maths = content(&maths, None);
        let math_size = Vec2::new(400.0, 400.0);
        let start = Instant::now();
        renderer.render(&mut engine, &maths, math_size, None);
        let math_cold = start.elapsed();
        let start = Instant::now();
        renderer.render(&mut engine, &maths, math_size, None);
        let math_warm = start.elapsed();

        println!(
            "4,000-char note: cold layout {cold_layout:?}, warm re-render {warm_render:?}; \
             resize (layout cache hit) {resize_layout:?}; 20 equations: cold {math_cold:?}, \
             cached {math_warm:?}"
        );
    }
}
