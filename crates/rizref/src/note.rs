//! Note rendering and the in-place text editor.
//!
//! A note is an ordinary item whose payload is text. The app rasterises it with
//! cosmic-text into a pixmap, uploads that as a texture, and draws it like any
//! image. Editing keeps a cosmic-text [`Editor`] alive for the duration of the
//! session, so the caret, arrow keys and selection are handled by the shaping
//! library rather than by a hand-rolled text buffer.
//!
//! Textures are content-addressed: the id is the hash of the note's text and
//! size, so identical notes share one GPU texture through the existing texture
//! cache and an edited note simply gets a new id.

use cosmic_text::{
    Action, Attrs, Buffer, Cursor, Edit as _, Editor, Metrics, Selection as TextSelection, Shaping,
    Wrap,
};
use glam::Vec2;
use rizref_codecs::ImageBuffer;
use rizref_core::{AssetId, Item, ItemId, checksum};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

use crate::text::TextEngine;

/// Texture pixels per world unit for a note.
pub const NOTE_SUPERSAMPLE: f32 = 2.0;

/// Longest note texture side, in pixels.
pub const NOTE_MAX_SIDE: f32 = 2048.0;

/// Distance between the note's border and its text, in world units.
pub const NOTE_PADDING: f32 = 14.0;

/// Text size in world units.
pub const NOTE_FONT_SIZE: f32 = 16.0;

/// Corner radius in world units.
pub const NOTE_RADIUS: f32 = 10.0;

/// Note background.
pub const NOTE_BG: [u8; 4] = [48, 44, 37, 242];

/// Note border.
pub const NOTE_BORDER: [u8; 4] = [122, 106, 78, 255];

/// Note text.
pub const NOTE_TEXT: [u8; 4] = [238, 232, 220, 255];

/// Caret colour while editing.
pub const NOTE_CURSOR: [u8; 4] = [255, 206, 110, 255];

/// Selection highlight while editing.
pub const NOTE_SELECTION: [u8; 4] = [66, 92, 136, 220];

/// One note-editing session.
///
/// The editor owns its shaped buffer; the font system stays in the shared
/// [`TextEngine`], which is why the methods take the engine back in.
pub struct NoteSession {
    item: ItemId,
    editor: Editor<'static>,
    original: String,
    pixels_per_world: f32,
}

impl NoteSession {
    /// Starts editing `item`, or `None` when it is not a note.
    pub fn begin(engine: &mut TextEngine, item: &Item) -> Option<Self> {
        let text = item.text()?.to_owned();
        let pixels_per_world = pixels_per_world(item.size());
        let (width, height) = text_area(item.size(), pixels_per_world);
        let font_size = NOTE_FONT_SIZE * pixels_per_world;
        let mut buffer = Buffer::new(
            engine.font_system_mut(),
            Metrics::new(font_size, (font_size * 1.4).ceil()),
        );
        buffer.set_wrap(Wrap::Word);
        buffer.set_size(Some(width), Some(height));
        buffer.set_text(&text, &Attrs::new(), Shaping::Advanced, None);
        Some(Self {
            item: item.id(),
            editor: Editor::new(buffer),
            original: text,
            pixels_per_world,
        })
    }

    /// The note being edited.
    pub const fn item(&self) -> ItemId {
        self.item
    }

    /// The text as it was when the session began, for the undo command.
    pub fn original(&self) -> &str {
        &self.original
    }

    /// Pixels per world unit this session was shaped at.
    pub const fn pixels_per_world(&self) -> f32 {
        self.pixels_per_world
    }

    /// The current text.
    pub fn text(&self) -> String {
        self.editor.with_buffer(buffer_text)
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

/// Rasterises a note, with the live editor when one is active.
pub fn render(
    engine: &mut TextEngine,
    text: &str,
    size: Vec2,
    session: Option<&mut NoteSession>,
) -> Option<ImageBuffer> {
    let pixels_per_world = pixels_per_world(size);
    let (width, height) = texture_size(size, pixels_per_world);
    let mut pixmap = Pixmap::new(width, height)?;
    draw_background(&mut pixmap, pixels_per_world);
    let padding = NOTE_PADDING * pixels_per_world;

    match session {
        Some(session) => engine.draw_editor(
            &mut pixmap,
            &mut session.editor,
            padding,
            padding,
            NOTE_TEXT,
            NOTE_CURSOR,
            NOTE_SELECTION,
            NOTE_TEXT,
        ),
        None => engine.draw_block(
            &mut pixmap,
            text,
            padding,
            padding,
            width as f32 - 2.0 * padding,
            height as f32 - 2.0 * padding,
            NOTE_FONT_SIZE * pixels_per_world,
            NOTE_TEXT,
        ),
    }

    ImageBuffer::from_rgba(width, height, straight_rgba(&pixmap))
}

/// The content-addressed texture id for a note.
pub fn texture_id(text: &str, size: Vec2) -> AssetId {
    let mut bytes = Vec::with_capacity(text.len() + 24);
    bytes.extend_from_slice(b"rizref-note-v1\0");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(&size.x.to_bits().to_le_bytes());
    bytes.extend_from_slice(&size.y.to_bits().to_le_bytes());
    AssetId::from_checksum(checksum(&bytes))
}

/// Paints the rounded background and its border.
fn draw_background(pixmap: &mut Pixmap, pixels_per_world: f32) {
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

    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color(Color::from_rgba8(
        NOTE_BG[0], NOTE_BG[1], NOTE_BG[2], NOTE_BG[3],
    ));
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    paint.set_color(Color::from_rgba8(
        NOTE_BORDER[0],
        NOTE_BORDER[1],
        NOTE_BORDER[2],
        NOTE_BORDER[3],
    ));
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
    use super::{NOTE_PADDING, NOTE_TEXT, NoteSession, render, texture_id};
    use crate::text::TextEngine;
    use cosmic_text::Action;
    use glam::Vec2;
    use rizref_core::{Item, ItemId};

    fn note(text: &str) -> Item {
        Item::note(
            ItemId::from_raw(1),
            text,
            Vec2::ZERO,
            Vec2::new(200.0, 120.0),
        )
    }

    #[test]
    fn a_rendered_note_is_non_transparent_and_sized_to_the_note() {
        let mut engine = TextEngine::new();

        let image = render(&mut engine, "study this", Vec2::new(200.0, 120.0), None)
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
    fn texture_ids_are_content_addressed() {
        let a = texture_id("hello", Vec2::splat(100.0));
        let b = texture_id("hello", Vec2::splat(100.0));
        let c = texture_id("hello!", Vec2::splat(100.0));
        let d = texture_id("hello", Vec2::splat(200.0));

        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
    }

    #[test]
    fn an_editor_session_inserts_and_deletes_text() {
        let mut engine = TextEngine::new();
        let item = note("ab");
        let mut session = NoteSession::begin(&mut engine, &item).expect("a note");

        session.click(&mut engine, 0.0, 0.0);
        session.select_all();
        session.insert_text(&mut engine, "hi\nthere");
        assert_eq!(session.text(), "hi\nthere");
        assert_eq!(session.original(), "ab");

        session.action(&mut engine, Action::Backspace);
        assert_eq!(session.text(), "hi\nther");
    }

    #[test]
    fn a_session_renders_its_caret() {
        let mut engine = TextEngine::new();
        let item = note("");
        let mut session = NoteSession::begin(&mut engine, &item).expect("a note");

        let image =
            render(&mut engine, "", Vec2::new(200.0, 120.0), Some(&mut session)).expect("renders");

        // The caret colour is warm; the empty note body is dark. Some pixels
        // near the text origin are warmer than the background.
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
        assert_eq!(NOTE_TEXT[3], 255);
    }
}
