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
//!
//! Phase 07 performance: the text, decorations and equations of a markdown
//! note are painted once into a cached layer (keyed by source, wrap width,
//! density and palette) and blitted onto a freshly drawn card, so a re-render
//! that keeps the wrap width costs the card and a copy; the card is drawn
//! analytically and the straight-alpha conversion is table-driven. Renders run
//! on the note worker (`note_worker`), never on the UI thread, except the
//! live editor's.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use cosmic_text::{
    Action, Attrs, Buffer, Cursor, Edit as _, Editor, Metrics, Selection as TextSelection, Shaping,
    Wrap,
};
use glam::Vec2;
use rizref_codecs::ImageBuffer;
use rizref_core::{AssetId, Item, ItemId, ItemState, NoteContent, NoteFormat, checksum};
use tiny_skia::{Color, FillRule, Paint as SkPaint, Path, PathBuilder, Pixmap, Transform};

use crate::markdown::layout::{BASE_FONT_SIZE, Fill, NoteLayout, Paint, layout};
use crate::markdown::math::MathCache;
use crate::markdown::parse::parse;
use crate::markdown::style::NotePalette;
use crate::rich_text::{self, RichState, StyleToggle};
use crate::text::TextEngine;
use rizref_core::rich::TextStyle;

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
    /// The styled-editing state of a text item; `None` for a note.
    rich: Option<RichState>,
    /// The caret's last drawn rectangle in texture pixels: `[x, top,
    /// height]`, set by every render of the session.
    caret: Option<[f32; 3]>,
    /// The item's placement when the session began (a text item's box
    /// grows as it is typed into; cancel and undo put it back).
    original_state: ItemState,
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
        if original.is_rich() {
            return Some(Self::begin_rich(
                engine,
                item,
                original,
                provisional,
                was_clean,
            ));
        }
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
            rich: None,
            original_state: item.state(),
            caret: None,
        })
    }

    /// Starts editing a text item at the scale its box shows it at (1 for a
    /// settled item), so the glyphs sit exactly where the rendered item had
    /// them. Lines break only at Enter.
    fn begin_rich(
        engine: &mut TextEngine,
        item: &Item,
        original: NoteContent,
        provisional: bool,
        was_clean: bool,
    ) -> Self {
        let density = pixels_per_world(item.size());
        let natural = rich_text::natural_size(engine, &original, density);
        // An empty item has no text to scale: its box was sized for the size
        // about to be typed, not for the default size an empty buffer measures
        // at (reading that as a scale made new text at 18% zoom 3.25x too big).
        let scale = if original.text.is_empty() {
            1.0
        } else {
            rich_text::scale_for(natural, item.size())
        };
        let pixels = density * scale;
        let mut buffer = rich_text::new_buffer(engine, pixels);
        rich_text::set_buffer_text(&mut buffer, &original.text, &original.spans, pixels);
        let mut editor = Editor::new(buffer);
        // The caret starts at the end, ready to keep typing.
        let end = editor.with_buffer(|buffer| {
            let line = buffer.lines.len().saturating_sub(1);
            Cursor::new(line, buffer.lines.last().map_or(0, |l| l.text().len()))
        });
        editor.set_cursor(end);
        let typing = RichState::style_at_caret(&editor, pixels);
        let mut session = Self {
            item: item.id(),
            editor,
            original,
            wrap_width: 0.0,
            pixels_per_world: density,
            provisional,
            was_clean,
            rich: Some(RichState {
                typing,
                shown: typing,
                scale,
            }),
            original_state: item.state(),
            caret: None,
        };
        session.sync_empty_metrics();
        session.resize(item.size());
        session
    }

    /// Pixels per text unit: the texture density times the item's scale.
    /// Every font size of a text item is converted with this.
    fn text_pixels(&self) -> f32 {
        self.pixels_per_world * self.rich.map_or(1.0, |rich| rich.scale)
    }

    /// An empty text item's single line takes the typing size, so its caret
    /// and box are as tall as what is about to be typed; otherwise the
    /// buffer's default size is the default text size (unstyled characters
    /// rely on it).
    fn sync_empty_metrics(&mut self) {
        let Some(rich) = self.rich else {
            return;
        };
        let pixels = self.text_pixels();
        let empty = self
            .editor
            .with_buffer(|buffer| buffer.lines.iter().all(|line| line.text().is_empty()));
        let size = if empty {
            rich.typing.size_or_default()
        } else {
            rizref_core::DEFAULT_TEXT_SIZE
        };
        let font = (f32::from(size) * pixels).max(1.0);
        let metrics = Metrics::new(font, font * rich_text::TEXT_LINE_HEIGHT);
        self.editor
            .with_buffer_mut(|buffer| buffer.set_metrics(metrics));
    }

    /// Whether this session edits a text item rather than a note.
    pub const fn is_rich(&self) -> bool {
        self.rich.is_some()
    }

    /// Sets the style a new, empty text item starts typing in.
    pub fn set_typing(&mut self, style: TextStyle) {
        if let Some(rich) = self.rich.as_mut() {
            rich.typing = style;
            rich.shown = style;
        }
        self.sync_empty_metrics();
    }

    /// The box a text item needs now, in world units: its text's natural box
    /// at the session's scale. The box hugs the text while it is typed.
    pub fn rich_size(&mut self, engine: &mut TextEngine) -> Option<Vec2> {
        let scale = self.rich?.scale;
        let pixels = self.text_pixels();
        let mut editor = self.editor.borrow_with(engine.font_system_mut());
        editor.shape_as_needed(false);
        let natural = editor.with_buffer(|buffer| rich_text::buffer_natural(buffer, pixels));
        Some(natural * scale)
    }

    /// The content padding in texture pixels.
    fn padding_px(&self) -> f32 {
        match self.rich {
            Some(_) => rizref_core::TEXT_PADDING * self.text_pixels(),
            None => NOTE_PADDING * self.pixels_per_world,
        }
    }

    /// The caret as a note-local segment in world units: its top point and
    /// its height, with `(0, 0)` the item's centre and before any mirroring.
    /// `None` until the session has been drawn once.
    pub fn caret_local(&self, size: Vec2) -> Option<(Vec2, f32)> {
        let [x, top, height] = self.caret?;
        let padding = self.padding_px();
        let ppw = self.pixels_per_world;
        let local = Vec2::new((padding + x) / ppw, (padding + top) / ppw) - size * 0.5;
        Some((local, height / ppw))
    }

    /// The colour the caret of a text item is drawn in: its typing colour.
    pub fn caret_colour(&self) -> Option<[u8; 3]> {
        self.rich
            .map(|rich| rich.typing.color.unwrap_or(rich_text::DEFAULT_TEXT_INK))
    }

    /// Whether any text is selected.
    #[cfg(test)]
    pub fn has_selection(&self) -> bool {
        self.editor.selection_bounds().is_some()
    }

    /// The style the format toolbar shows: for a selection, a property is on
    /// when every selected character has it (the colour and size are the
    /// first character's); otherwise the typing style. Cached: free to call
    /// every frame and on every pointer move.
    pub fn display_style(&self) -> Option<TextStyle> {
        self.rich.map(|rich| rich.shown)
    }

    /// Recomputes [`Self::display_style`] after anything that can change it.
    fn refresh_shown(&mut self) {
        let Some(typing) = self.rich.map(|rich| rich.typing) else {
            return;
        };
        let shown = match self.selection_spans() {
            None => typing,
            Some((text, spans, range)) => {
                let all = |toggle: StyleToggle| {
                    rizref_core::rich::all_in_range(&text, &spans, range.clone(), |s| toggle.get(s))
                };
                let first = rizref_core::rich::style_at(&spans, range.start);
                TextStyle {
                    bold: all(StyleToggle::Bold),
                    italic: all(StyleToggle::Italic),
                    underline: all(StyleToggle::Underline),
                    color: first.color,
                    size: first.size,
                }
            }
        };
        if let Some(rich) = self.rich.as_mut() {
            rich.shown = shown;
        }
    }

    /// The joined text, its spans and the selected byte range, when there is
    /// a selection.
    fn selection_spans(
        &self,
    ) -> Option<(String, Vec<rizref_core::TextSpan>, std::ops::Range<usize>)> {
        let (start, end) = self.editor.selection_bounds()?;
        let pixels = self.text_pixels();
        self.editor.with_buffer(|buffer| {
            let (text, spans) = rich_text::buffer_content(buffer, pixels);
            let range = rich_text::offset_of(buffer, start)..rich_text::offset_of(buffer, end);
            (range.start < range.end).then_some((text, spans, range))
        })
    }

    /// Restyles the selection with `change`, or, with nothing selected, the
    /// characters typed next.
    fn restyle(&mut self, change: impl Fn(&mut TextStyle)) {
        let Some(rich) = self.rich.as_mut() else {
            return;
        };
        change(&mut rich.typing);
        if let Some((text, spans, range)) = self.selection_spans() {
            let spans = rizref_core::rich::apply_style(&text, &spans, range, change);
            let pixels = self.text_pixels();
            self.editor
                .with_buffer_mut(|buffer| rich_text::write_buffer_spans(buffer, &spans, pixels));
            self.editor.set_redraw(true);
        }
        self.refresh_shown();
        self.sync_empty_metrics();
    }

    /// Steps the font size of the selection (or of what is typed next) to the
    /// next size up or down; returns the new size.
    pub fn step_size(&mut self, larger: bool) -> Option<u16> {
        let current = self.display_style()?.size_or_default();
        let size = rizref_core::rich::step_text_size(current, larger);
        let stored = rizref_core::rich::normalize_size(Some(size));
        self.restyle(|style| style.size = stored);
        Some(size)
    }

    /// Toggles bold, italic or underline: off when the whole selection (or
    /// the typing style) already has it, on otherwise.
    pub fn toggle(&mut self, toggle: StyleToggle) {
        let Some(shown) = self.display_style() else {
            return;
        };
        let on = !toggle.get(&shown);
        self.restyle(|style| toggle.set(style, on));
    }

    /// Sets the text colour; the default ink is stored as no colour.
    pub fn set_colour(&mut self, rgb: [u8; 3]) {
        let colour = (rgb != rich_text::DEFAULT_TEXT_INK).then_some(rgb);
        self.restyle(|style| style.color = colour);
    }

    /// The note being edited.
    pub const fn item(&self) -> ItemId {
        self.item
    }

    /// The content as it was when the session began.
    pub const fn original(&self) -> &NoteContent {
        &self.original
    }

    /// The item's placement as it was when the session began.
    pub const fn original_state(&self) -> ItemState {
        self.original_state
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
        if self.rich.is_some() {
            return self.live_content();
        }
        NoteContent {
            text: self.text(),
            format: NoteFormat::Markdown,
            background: self.original.background,
            wrap_width: Some(self.wrap_width),
            spans: Vec::new(),
        }
    }

    /// The content the live preview writes while typing: the original
    /// format and wrap width, so a cancelled edit leaves nothing behind.
    pub fn live_content(&self) -> NoteContent {
        if self.rich.is_some() {
            let pixels = self.text_pixels();
            let (text, spans) = self
                .editor
                .with_buffer(|buffer| rich_text::buffer_content(buffer, pixels));
            return NoteContent {
                text,
                spans,
                ..self.original.clone()
            };
        }
        NoteContent {
            text: self.text(),
            ..self.original.clone()
        }
    }

    /// Updates the visible height after the box was resized; the wrap width
    /// (and so every line break) is unchanged.
    pub fn resize(&mut self, size: Vec2) {
        if self.rich.is_some() {
            self.adapt_density(size);
            // Text items never wrap (the box follows the text), but the
            // buffer still gets the box's text width: a multi-line selection
            // highlights each line out to the buffer width, and with no width
            // every line but the last lost its highlight.
            let width = (size.x * self.pixels_per_world - 2.0 * self.padding_px()).max(1.0);
            self.editor.with_buffer_mut(|buffer| {
                if buffer.size().0 != Some(width) {
                    buffer.set_size(Some(width), None);
                }
            });
            return;
        }
        let (_, height) = text_area(size, self.pixels_per_world);
        let width = self.wrap_width * self.pixels_per_world;
        self.editor.with_buffer_mut(|buffer| {
            buffer.set_size(Some(width), Some(height));
        });
    }

    /// Applies one editor action.
    pub fn action(&mut self, engine: &mut TextEngine, action: Action) {
        let moves = !matches!(action, Action::Insert(_) | Action::Enter);
        self.editor
            .borrow_with(engine.font_system_mut())
            .action(action);
        if moves {
            self.refresh_typing();
        } else {
            self.refresh_shown();
        }
        self.sync_empty_metrics();
    }

    /// Moves the caret, extending the selection when `extend` is set (shift
    /// held) and dropping it otherwise.
    pub fn motion(&mut self, engine: &mut TextEngine, motion: cosmic_text::Motion, extend: bool) {
        if extend {
            if self.editor.selection() == TextSelection::None {
                let cursor = self.editor.cursor();
                self.editor.set_selection(TextSelection::Normal(cursor));
            }
        } else if let Some((start, end)) = self.editor.selection_bounds() {
            // Without shift, left and right first collapse the selection to
            // its edge, as every text field does.
            self.editor.set_selection(TextSelection::None);
            match motion {
                cosmic_text::Motion::Left => {
                    self.editor.set_cursor(start);
                    self.refresh_typing();
                    return;
                }
                cosmic_text::Motion::Right => {
                    self.editor.set_cursor(end);
                    self.refresh_typing();
                    return;
                }
                _ => {}
            }
        } else {
            self.editor.set_selection(TextSelection::None);
        }
        self.action(engine, Action::Motion(motion));
    }

    /// Extends the selection to a note-local pixel position (a drag).
    pub fn drag(&mut self, engine: &mut TextEngine, x: f32, y: f32) {
        self.action(
            engine,
            Action::Drag {
                x: x.round() as i32,
                y: y.round() as i32,
            },
        );
    }

    /// Selects the word at a note-local pixel position (a double-click).
    pub fn select_word(&mut self, engine: &mut TextEngine, x: f32, y: f32) {
        self.action(
            engine,
            Action::DoubleClick {
                x: x.round() as i32,
                y: y.round() as i32,
            },
        );
    }

    /// The selected text, if any.
    pub fn selected_text(&self) -> Option<String> {
        self.editor.copy_selection().filter(|text| !text.is_empty())
    }

    /// Removes the selection, returning whether there was one.
    pub fn delete_selection(&mut self) -> bool {
        let deleted = self.editor.delete_selection();
        self.refresh_typing();
        deleted
    }

    fn refresh_typing(&mut self) {
        let pixels = self.text_pixels();
        // With no characters left there is nothing to continue the style
        // of: keep what was being typed (deleting the first letter of a new
        // text item at 17% zoom used to drop its size back to the default,
        // so the rest came out tiny).
        let empty = self
            .editor
            .with_buffer(|buffer| buffer.lines.iter().all(|line| line.text().is_empty()));
        if let Some(rich) = self.rich.as_mut()
            && !empty
        {
            rich.typing = RichState::style_at_caret(&self.editor, pixels);
        }
        self.refresh_shown();
    }

    /// Re-shapes a text item at the texture density its box now needs.
    ///
    /// The density is capped so the texture's longest side stays within
    /// [`NOTE_MAX_SIDE`]. A text item grows while it is typed, and at a low
    /// zoom its type is large in world units, so it soon passes that cap:
    /// kept at the density it started with, the texture was clipped at the
    /// cap and stretched across the box (the typed words showed as a few
    /// smeared letters until the edit ended).
    fn adapt_density(&mut self, size: Vec2) {
        if self.rich.is_none() {
            return;
        }
        let target = pixels_per_world(size);
        if (target - self.pixels_per_world).abs() <= self.pixels_per_world * 0.01 {
            return;
        }
        let old = self.text_pixels();
        let (text, spans) = self
            .editor
            .with_buffer(|buffer| rich_text::buffer_content(buffer, old));
        let cursor = self.editor.cursor();
        let selection = self.editor.selection();
        self.pixels_per_world = target;
        let new = self.text_pixels();
        self.editor
            .with_buffer_mut(|buffer| rich_text::set_buffer_text(buffer, &text, &spans, new));
        // The text is unchanged, so the caret and selection still address
        // the same characters.
        self.editor.set_cursor(cursor);
        self.editor.set_selection(selection);
        self.sync_empty_metrics();
    }

    /// Converts a note-local world position (`(0, 0)` the centre, already
    /// un-mirrored) into editor pixels.
    pub fn local_to_editor(&self, local: Vec2, size: Vec2) -> (f32, f32) {
        let padding = self.padding_px();
        let point = (local + size * 0.5) * self.pixels_per_world - Vec2::splat(padding);
        (point.x, point.y)
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
        if let Some(rich) = self.rich {
            self.insert_styled(engine, text, rich.typing);
            return;
        }
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

    /// Inserts text in the typing style of a text item.
    fn insert_styled(&mut self, engine: &mut TextEngine, text: &str, style: TextStyle) {
        if self.editor.selection_bounds().is_some() {
            self.editor.delete_selection();
        }
        let attrs = rich_text::attrs_for(&style, self.text_pixels());
        for character in text.chars() {
            if character == '\n' {
                self.action(engine, Action::Enter);
                continue;
            }
            if character.is_control() {
                continue;
            }
            let before = self.editor.cursor();
            self.action(engine, Action::Insert(character));
            let after = self.editor.cursor();
            if after.line == before.line && after.index > before.index {
                self.editor.with_buffer_mut(|buffer| {
                    let line = &mut buffer.lines[after.line];
                    let mut list = line.attrs_list().clone();
                    list.add_span(before.index..after.index, &attrs);
                    line.set_attrs_list(list);
                });
            }
        }
        if let Some(rich) = self.rich.as_mut() {
            rich.typing = style;
            rich.shown = style;
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
        self.refresh_shown();
    }

    /// Draws the session into `pixmap` (glyphs, selection and decorations,
    /// but not the caret: the overlay draws that so it can blink and stays
    /// a readable width at any zoom) and records where the caret is.
    fn draw(
        &mut self,
        engine: &mut TextEngine,
        pixmap: &mut Pixmap,
        ink: [u8; 4],
        selection: [u8; 4],
        clip: [i32; 4],
    ) {
        let padding = self.padding_px();
        engine.draw_editor(
            pixmap,
            &mut self.editor,
            padding,
            padding,
            ink,
            [0, 0, 0, 0],
            selection,
            ink,
            clip,
        );
        // The caret is as tall as the line it is on (its largest text), so it
        // follows the text size; on an empty line of a text item, as tall as
        // what is about to be typed there.
        let cursor = self.editor.cursor();
        let typing_line = self.rich.map(|rich| {
            f32::from(rich.typing.size_or_default())
                * self.text_pixels()
                * rich_text::TEXT_LINE_HEIGHT
        });
        self.caret = self.editor.with_buffer(|buffer| {
            let empty = buffer
                .lines
                .get(cursor.line)
                .is_some_and(|line| line.text().is_empty());
            buffer
                .layout_runs()
                .filter(|run| run.line_i == cursor.line)
                .find_map(|run| {
                    let x = run.cursor_position(&cursor)?;
                    let height = match typing_line {
                        Some(typing) if empty => typing,
                        _ => run.line_height,
                    };
                    Some([x, run.line_top, height])
                })
        });
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
pub(crate) fn text_area(size: Vec2, pixels_per_world: f32) -> (f32, f32) {
    let (width, height) = texture_size(size, pixels_per_world);
    let padding = NOTE_PADDING * pixels_per_world;
    (
        (width as f32 - 2.0 * padding).max(8.0),
        (height as f32 - 2.0 * padding).max(8.0),
    )
}

/// The texture dimensions for a note of `size`.
pub(crate) fn texture_size(size: Vec2, pixels_per_world: f32) -> (u32, u32) {
    (
        (size.x * pixels_per_world)
            .round()
            .clamp(8.0, NOTE_MAX_SIDE) as u32,
        (size.y * pixels_per_world)
            .round()
            .clamp(8.0, NOTE_MAX_SIDE) as u32,
    )
}

/// Bytes of rasterised text layers kept for re-renders.
const LAYER_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// What a rasterised text layer depends on: the source, how it wraps, the
/// texture density and the palette (auto-contrast ink follows the
/// background). The box size is not part of it — a resize that keeps the
/// wrap width re-uses the layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LayerKey {
    source: u64,
    source_len: usize,
    wrap: u32,
    density: u32,
    background: [u8; 3],
}

/// A markdown note's text, math and decorations, painted once onto a
/// transparent premultiplied pixmap. `origin` is the integer pixel the
/// layer's top-left goes to in the note texture; the fractional part of the
/// padding is baked into the layer, so the blit is an exact copy.
struct TextLayer {
    pixmap: Pixmap,
    origin: i32,
    /// Per row, the `[first, last)` columns holding any ink: most rows are
    /// the gaps between lines, and the blit skips them outright.
    spans: Vec<(u32, u32)>,
}

/// The inked column range of every row of `pixmap`; `(0, 0)` for an empty row.
fn ink_spans(pixmap: &Pixmap) -> Vec<(u32, u32)> {
    let width = pixmap.width() as usize;
    pixmap
        .data()
        .chunks_exact(width * 4)
        .map(|row| {
            let pixels = row.as_chunks::<4>().0;
            let inked = |pixel: &[u8; 4]| pixel[3] != 0;
            match pixels.iter().position(inked) {
                Some(first) => {
                    let last = pixels.iter().rposition(inked).unwrap_or(first);
                    (first as u32, last as u32 + 1)
                }
                None => (0, 0),
            }
        })
        .collect()
}

/// Parsed-and-laid-out notes, the math cache and the rasterised text layers,
/// shared by every note.
///
/// Phase 07: a re-render of an unchanged note — a resize that keeps the wrap
/// width, a background change of another note, a texture evicted and wanted
/// again — blits the cached text layer onto a fresh card instead of
/// re-painting every glyph and equation (35 ms for 4,000 characters before).
#[derive(Default)]
pub struct NoteRenderer {
    math: MathCache,
    layouts: HashMap<(String, u32), NoteLayout>,
    layers: HashMap<LayerKey, (u64, Arc<TextLayer>)>,
    layer_bytes: usize,
    layer_clock: u64,
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
        if content.is_rich() {
            let density = pixels_per_world(size);
            let natural = rich_text::natural_size(engine, content, density);
            return (natural.y - 2.0 * rizref_core::TEXT_PADDING)
                * rich_text::scale_for(natural, size);
        }
        let wrap = wrap_width(content, size);
        match content.format {
            NoteFormat::Markdown => self.layout(engine, &content.text, wrap).height,
            NoteFormat::Plain | NoteFormat::Rich => plain_height(engine, &content.text, wrap),
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
        if content.is_rich() {
            return match session {
                Some(session) => {
                    let (width, height) = texture_size(size, session.pixels_per_world);
                    let mut pixmap = Pixmap::new(width, height)?;
                    let ink = rich_text::DEFAULT_TEXT_INK;
                    session.draw(
                        engine,
                        &mut pixmap,
                        [ink[0], ink[1], ink[2], 255],
                        [0x53, 0x9b, 0xf5, 110],
                        [0, 0, width as i32, height as i32],
                    );
                    ImageBuffer::from_rgba(width, height, straight_rgba(&pixmap))
                }
                None => rich_text::render(engine, content, size, pixels_per_world(size)),
            };
        }
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
            (Some(session), _) => session.draw(engine, &mut pixmap, ink, colours.selection, clip),
            (None, NoteFormat::Plain | NoteFormat::Rich) => engine.draw_block(
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
                let layer = self.text_layer(engine, content, size, pixels_per_world, &colours);
                if let Some(layer) = layer {
                    blit(&mut pixmap, &layer.pixmap, &layer.spans, layer.origin, clip);
                }
            }
        }

        ImageBuffer::from_rgba(width, height, straight_rgba(&pixmap))
    }
}

impl NoteRenderer {
    /// The cached text layer for a markdown note, painting it on a miss.
    fn text_layer(
        &mut self,
        engine: &mut TextEngine,
        content: &NoteContent,
        size: Vec2,
        pixels_per_world: f32,
        colours: &NotePalette,
    ) -> Option<Arc<TextLayer>> {
        let wrap = wrap_width(content, size);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.text.hash(&mut hasher);
        let key = LayerKey {
            source: hasher.finish(),
            source_len: content.text.len(),
            wrap: wrap.to_bits(),
            density: pixels_per_world.to_bits(),
            background: colours.background,
        };
        self.layer_clock += 1;
        let now = self.layer_clock;
        if let Some((used, layer)) = self.layers.get_mut(&key) {
            *used = now;
            return Some(Arc::clone(layer));
        }

        // Split the borrow: the layout lives in `self.layouts`, the math
        // masks in `self.math`.
        let layout_key = (content.text.clone(), wrap.to_bits());
        self.layout(engine, &content.text, wrap);
        let laid_out = &self.layouts[&layout_key];
        let padding = NOTE_PADDING * pixels_per_world;
        let origin = padding.floor();
        let fraction = padding - origin;
        // Everything the layout draws lies within its wrap width and height;
        // the slack covers italic overhang and a trailing equation.
        let slack = 8.0 * pixels_per_world;
        let width = (wrap * pixels_per_world + fraction + slack)
            .ceil()
            .clamp(1.0, NOTE_MAX_SIDE * 2.0) as u32;
        let height = (laid_out.height * pixels_per_world + fraction + slack)
            .ceil()
            .clamp(1.0, NOTE_MAX_SIDE * 8.0) as u32;
        let mut pixmap = Pixmap::new(width, height)?;
        paint_layout(
            engine,
            &mut self.math,
            &mut pixmap,
            laid_out,
            colours,
            fraction,
            pixels_per_world,
            [0, 0, width as i32, height as i32],
        );
        let spans = ink_spans(&pixmap);
        let layer = Arc::new(TextLayer {
            pixmap,
            origin: origin as i32,
            spans,
        });

        let bytes = layer.pixmap.data().len();
        while self.layer_bytes + bytes > LAYER_CACHE_BYTES && !self.layers.is_empty() {
            let oldest = self
                .layers
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(key, _)| *key)
                .expect("non-empty");
            if let Some((_, gone)) = self.layers.remove(&oldest) {
                self.layer_bytes -= gone.pixmap.data().len();
            }
        }
        self.layer_bytes += bytes;
        self.layers.insert(key, (now, Arc::clone(&layer)));
        Some(layer)
    }

    /// Number of cached text layers.
    #[cfg(test)]
    fn layer_count(&self) -> usize {
        self.layers.len()
    }
}

/// Composites premultiplied `layer` over `pixmap` with its top-left at
/// `(origin, origin)`, inside `clip` — an integer copy, never resampled.
fn blit(pixmap: &mut Pixmap, layer: &Pixmap, spans: &[(u32, u32)], origin: i32, clip: [i32; 4]) {
    let target_width = pixmap.width() as i32;
    let target_height = pixmap.height() as i32;
    let x0 = clip[0].max(0).max(origin);
    let y0 = clip[1].max(0).max(origin);
    let x1 = clip[2].min(target_width).min(origin + layer.width() as i32);
    let y1 = clip[3]
        .min(target_height)
        .min(origin + layer.height() as i32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let source = layer.data();
    let source_width = layer.width() as usize;
    let data = pixmap.data_mut();
    for y in y0..y1 {
        let (first, last) = spans
            .get((y - origin) as usize)
            .copied()
            .unwrap_or((0, layer.width()));
        if first >= last {
            continue;
        }
        let source_row = (y - origin) as usize * source_width;
        let target_row = y as usize * target_width as usize;
        let row_start = x0.max(origin + first as i32);
        let row_end = x1.min(origin + last as i32);
        for x in row_start..row_end {
            let s = (source_row + (x - origin) as usize) * 4;
            let alpha = source[s + 3];
            if alpha == 0 {
                continue;
            }
            let d = (target_row + x as usize) * 4;
            if alpha == 255 {
                data[d..d + 4].copy_from_slice(&source[s..s + 4]);
                continue;
            }
            let inverse = 255 - u32::from(alpha);
            for channel in 0..4 {
                let under = u32::from(data[d + channel]);
                data[d + channel] =
                    (u32::from(source[s + channel]) + (under * inverse + 127) / 255).min(255) as u8;
            }
        }
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

/// Longest side of a note's placeholder and of its low-resolution copy.
pub const NOTE_LOW_SIDE: f32 = 256.0;

/// The card alone — background and border, no text — at a low density: what
/// a note shows for the moment between appearing and its worker render
/// arriving. Costs well under a millisecond, so it is safe on the UI thread.
pub fn render_placeholder(content: &NoteContent, size: Vec2) -> Option<ImageBuffer> {
    if content.is_rich() {
        return rich_text::render_placeholder(size);
    }
    let density = (NOTE_LOW_SIDE / size.x.max(size.y).max(1.0)).min(NOTE_SUPERSAMPLE);
    let (width, height) = texture_size(size, density);
    let mut pixmap = Pixmap::new(width, height)?;
    draw_background(&mut pixmap, density, &palette(content));
    ImageBuffer::from_rgba(width, height, straight_rgba(&pixmap))
}

/// A note texture downscaled to [`NOTE_LOW_SIDE`]: kept on the CPU so an
/// evicted note can show it, slightly soft, until its full texture is back.
pub fn low_resolution(image: &ImageBuffer) -> ImageBuffer {
    match rizref_core::fit_to_side(image, NOTE_LOW_SIDE as u32) {
        rizref_core::Fitted::Original(original) => original.clone(),
        rizref_core::Fitted::Resized(resized) => resized,
    }
}

/// A texture id derived from `texture` for one of its stand-ins (`tag`
/// names which), so a placeholder or low-resolution copy never shares an id
/// with a full render.
pub fn derived_texture_id(texture: AssetId, tag: &[u8]) -> AssetId {
    let mut bytes = Vec::with_capacity(48);
    bytes.extend_from_slice(b"rizref-note-derived\0");
    bytes.extend_from_slice(tag);
    bytes.extend_from_slice(texture.as_bytes());
    AssetId::from_checksum(checksum(&bytes))
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
        NoteFormat::Rich => 2,
    });
    for span in &content.spans {
        bytes.extend_from_slice(&(span.start as u64).to_le_bytes());
        bytes.extend_from_slice(&(span.end as u64).to_le_bytes());
        let style = span.style;
        bytes.push(
            u8::from(style.bold) | (u8::from(style.italic) << 1) | (u8::from(style.underline) << 2),
        );
        bytes.extend_from_slice(&style.color.map_or([0; 4], |[r, g, b]| [1, r, g, b]));
    }
    bytes.extend_from_slice(&content.background_or_default());
    bytes.extend_from_slice(&wrap_width(content, size).to_bits().to_le_bytes());
    bytes.extend_from_slice(&size.x.to_bits().to_le_bytes());
    bytes.extend_from_slice(&size.y.to_bits().to_le_bytes());
    bytes.extend_from_slice(&pixels_per_world.to_bits().to_le_bytes());
    AssetId::from_checksum(checksum(&bytes))
}

/// Paints the rounded card and its border.
///
/// Drawn analytically rather than through tiny-skia's anti-aliased path
/// fill and stroke, which cost 33 ms for a 626 x 2048 card — most of a warm
/// re-render. Interior runs of each row are a plain fill; only pixels near
/// the outline evaluate the rounded rectangle's signed distance, which gives
/// the same anti-aliased edge (coverage = distance clamped to one pixel).
fn draw_background(pixmap: &mut Pixmap, pixels_per_world: f32, colours: &NotePalette) {
    let width = pixmap.width() as usize;
    let height = pixmap.height() as usize;
    let inset = (pixels_per_world * 1.5).max(1.0);
    let rect_w = (width as f32 - 2.0 * inset).max(1.0);
    let rect_h = (height as f32 - 2.0 * inset).max(1.0);
    let radius = (NOTE_RADIUS * pixels_per_world)
        .min(rect_w.min(rect_h) * 0.5)
        .max(0.0);
    let half_border = (1.5 * pixels_per_world).max(1.0) * 0.5;
    let centre = (inset + rect_w * 0.5, inset + rect_h * 0.5);
    let half = (rect_w * 0.5, rect_h * 0.5);

    let premultiply = |rgb: [u8; 3], alpha: u8| -> [u8; 4] {
        let a = u16::from(alpha);
        [
            ((u16::from(rgb[0]) * a + 127) / 255) as u8,
            ((u16::from(rgb[1]) * a + 127) / 255) as u8,
            ((u16::from(rgb[2]) * a + 127) / 255) as u8,
            alpha,
        ]
    };
    let card = premultiply(colours.background, CARD_ALPHA);
    let border = colours.border;

    // Signed distance from a pixel centre to the rounded rectangle's edge,
    // negative inside.
    let distance = |x: f32, y: f32| -> f32 {
        let qx = (x - centre.0).abs() - (half.0 - radius);
        let qy = (y - centre.1).abs() - (half.1 - radius);
        let outside = qx.max(0.0).hypot(qy.max(0.0));
        outside + qx.max(qy).min(0.0) - radius
    };
    // One pixel of the card at signed distance `d`: the fill, then the
    // border stroke centred on the edge over it (source-over, premultiplied).
    let shade = |d: f32| -> [u8; 4] {
        let fill = (0.5 - d).clamp(0.0, 1.0);
        let stroke = (half_border + 0.5 - d.abs()).clamp(0.0, 1.0);
        let mut out = [0.0_f32; 4];
        for channel in 0..4 {
            out[channel] = f32::from(card[channel]) * fill;
        }
        let a = stroke;
        for channel in 0..3 {
            out[channel] = f32::from(border[channel]) * a + out[channel] * (1.0 - a);
        }
        out[3] = 255.0 * a + out[3] * (1.0 - a);
        out.map(|value| value.round().clamp(0.0, 255.0) as u8)
    };

    // Pixels farther than this inside the edge are plain card.
    let solid = half_border + 1.0;
    // One row of plain card, copied in a single memcpy per row.
    let card_row: Vec<u8> = card.iter().copied().cycle().take(width * 4).collect();
    let data = pixmap.data_mut();
    for y in 0..height {
        let py = y as f32 + 0.5;
        let row = &mut data[y * width * 4..(y + 1) * width * 4];
        // Away from the corner rows, the edge is the rectangle's side: the
        // interior run between the side bands is plain card.
        let in_straight_rows = (py - centre.1).abs() <= half.1 - radius - solid;
        let (run_start, run_end) = if in_straight_rows {
            (
                (inset + solid).ceil() as usize,
                ((inset + rect_w - solid).floor() as usize).min(width),
            )
        } else {
            (0, 0)
        };
        if run_end > run_start {
            row[run_start * 4..run_end * 4].copy_from_slice(&card_row[run_start * 4..run_end * 4]);
        }
        for x in 0..width {
            if x >= run_start && x < run_end {
                continue;
            }
            let pixel = &mut row[x * 4..x * 4 + 4];
            let d = distance(x as f32 + 0.5, py);
            if d > half_border + 0.5 {
                continue;
            }
            pixel.copy_from_slice(&shade(d));
        }
    }
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
///
/// Through a table of every `(alpha, channel)` result, built once: the
/// per-channel division cost 17 ms on a 626 x 2048 note. The table holds the
/// exact rounded quotient, so the output is unchanged.
pub(crate) fn straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    static TABLE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = vec![0_u8; 256 * 256];
        for alpha in 1..256_u16 {
            for channel in 0..256_u16 {
                table[usize::from(alpha) * 256 + usize::from(channel)] =
                    ((channel * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
        table
    });
    let mut out = pixmap.data().to_vec();
    // A card is mostly one colour: a pixel equal to the previous input takes
    // the previous output without a lookup.
    let (mut last_in, mut last_out) = ([0_u8; 4], [0_u8; 4]);
    for pixel in out.as_chunks_mut::<4>().0 {
        // Opaque pixels are already straight, and a transparent text item
        // is mostly empty pixels: both skip the cache and the table.
        match pixel[3] {
            255 => continue,
            0 => {
                if pixel[..3] != [0, 0, 0] {
                    pixel[..3].fill(0);
                }
                continue;
            }
            _ => {}
        }
        if *pixel == last_in {
            *pixel = last_out;
            continue;
        }
        last_in = *pixel;
        match pixel[3] {
            0 => pixel[..3].fill(0),
            255 => {}
            alpha => {
                let row = &table[usize::from(alpha) * 256..usize::from(alpha) * 256 + 256];
                for channel in pixel.iter_mut().take(3) {
                    *channel = row[usize::from(*channel)];
                }
            }
        }
        last_out = *pixel;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        NOTE_MAX_SIDE, NOTE_PADDING, NoteRenderer, NoteSession, pixels_per_world, texture_id,
    };
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
    fn a_resize_that_keeps_the_wrap_reuses_the_painted_layer() {
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let mut note = content(&"Some **bold** words that wrap. ".repeat(10), None);
        note.wrap_width = Some(200.0);

        let small = renderer
            .render(&mut engine, &note, Vec2::new(228.0, 300.0), None)
            .expect("renders");
        assert_eq!(renderer.layer_count(), 1);
        let large = renderer
            .render(&mut engine, &note, Vec2::new(400.0, 500.0), None)
            .expect("renders");
        assert_eq!(
            renderer.layer_count(),
            1,
            "same wrap and density: one layer"
        );

        // The text sits at the same pixels in both: compare a band of the
        // first line, inside the smaller card.
        let density = pixels_per_world(Vec2::new(228.0, 300.0));
        assert_eq!(density, pixels_per_world(Vec2::new(400.0, 500.0)));
        let row = ((NOTE_PADDING + 8.0) * density) as u32;
        let band = |image: &ImageBuffer| {
            let start = ((row * image.width()) * 4) as usize;
            let len = (150.0 * density) as usize * 4;
            image.pixels()[start..start + len].to_vec()
        };
        assert_eq!(band(&small), band(&large));
        assert_ne!(
            band(&small).iter().max(),
            band(&small).iter().min(),
            "the band holds text, not just card"
        );
    }

    #[test]
    fn the_layer_cache_matches_a_direct_paint() {
        // The cached path blits a layer painted at the padding's fraction;
        // it must match painting straight into the card.
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let note = content("# Title\n\nSome *text* and $x^2$ here.", Some([30, 30, 60]));
        let size = Vec2::new(300.0, 160.0);
        let cached = renderer
            .render(&mut engine, &note, size, None)
            .expect("renders");

        let density = pixels_per_world(size);
        let (width, height) = super::texture_size(size, density);
        let colours = super::palette(&note);
        let mut direct = tiny_skia::Pixmap::new(width, height).expect("pixmap");
        super::draw_background(&mut direct, density, &colours);
        let padding = NOTE_PADDING * density;
        let clip = [
            padding.floor() as i32,
            padding.floor() as i32,
            (width as f32 - padding).ceil() as i32,
            (height as f32 - padding).ceil() as i32,
        ];
        let wrap = super::wrap_width(&note, size);
        let key = (note.text.clone(), wrap.to_bits());
        let mut fresh = NoteRenderer::new();
        fresh.layout(&mut engine, &note.text, wrap);
        let laid_out = &fresh.layouts[&key];
        let mut math = crate::markdown::math::MathCache::default();
        super::paint_layout(
            &mut engine,
            &mut math,
            &mut direct,
            laid_out,
            &colours,
            padding,
            density,
            clip,
        );
        let direct = super::straight_rgba(&direct);

        let differing = cached
            .pixels()
            .iter()
            .zip(&direct)
            .filter(|(a, b)| a.abs_diff(**b) > 2)
            .count();
        assert!(
            differing * 1000 < direct.len(),
            "{differing} of {} bytes differ by more than rounding",
            direct.len()
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

        let size = Vec2::new(200.0, 120.0);
        assert_eq!(session.caret_local(size), None, "not drawn yet");
        let image = renderer
            .render(&mut engine, &content("", None), size, Some(&mut session))
            .expect("renders");

        // The caret is not baked into the texture (the overlay draws it, so
        // it can blink): no warm accent pixels at the text origin.
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
        assert_eq!(warm, 0, "the texture holds no caret");

        // Instead the session reports where it is: at the text origin, one
        // line tall, in note-local world units.
        let (top, height) = session.caret_local(size).expect("drawn");
        let origin = Vec2::splat(NOTE_PADDING) - size * 0.5;
        assert!((top - origin).length() < 1.0, "{top:?} vs {origin:?}");
        assert!(height > 15.0 && height < 30.0, "{height}");
    }

    /// A text item whose box is its text's natural size (scale 1).
    fn rich_item(text: &str) -> Item {
        let mut engine = TextEngine::new();
        let content = NoteContent::rich(text);
        let size = crate::rich_text::natural_size(&mut engine, &content, 2.0);
        Item::note_with(ItemId::from_raw(5), content, Vec2::ZERO, size)
    }

    #[test]
    fn a_text_session_styles_the_selection_and_the_typing() {
        use crate::rich_text::StyleToggle;
        let mut engine = TextEngine::new();
        let item = rich_item("hello");
        let mut session = NoteSession::begin(&mut engine, &item, false, true).expect("rich");
        assert!(session.is_rich());

        // Nothing selected: bold applies to what is typed next.
        session.toggle(StyleToggle::Bold);
        assert!(session.display_style().expect("rich").bold);
        session.insert_text(&mut engine, " world");
        let content = session.content();
        assert_eq!(content.text, "hello world");
        assert_eq!(content.format, NoteFormat::Rich);
        assert_eq!(content.spans.len(), 1);
        assert_eq!((content.spans[0].start, content.spans[0].end), (5, 11));
        assert!(content.spans[0].style.bold);

        // Select everything, underline and colour it.
        session.select_all();
        session.toggle(StyleToggle::Underline);
        session.set_colour([255, 0, 0]);
        let content = session.content();
        assert!(
            content
                .spans
                .iter()
                .all(|s| s.style.underline && s.style.color == Some([255, 0, 0]))
        );
        assert_eq!(content.spans.first().map(|s| s.start), Some(0));
        assert_eq!(content.spans.last().map(|s| s.end), Some(11));
        // Mixed bold: the toolbar shows bold off, and a toggle makes it all bold.
        assert!(!session.display_style().expect("rich").bold);
        session.toggle(StyleToggle::Bold);
        assert!(session.content().spans.iter().all(|s| s.style.bold));
        session.toggle(StyleToggle::Bold);
        assert!(session.content().spans.iter().all(|s| !s.style.bold));
    }

    #[test]
    fn toggles_turn_off_again_across_several_lines() {
        use crate::rich_text::StyleToggle;
        let mut engine = TextEngine::new();
        let item = rich_item("Roses are red,\nRoses are blue");
        let mut session = NoteSession::begin(&mut engine, &item, false, true).expect("rich");
        session.select_all();
        for toggle in [
            StyleToggle::Bold,
            StyleToggle::Italic,
            StyleToggle::Underline,
        ] {
            session.toggle(toggle);
            assert!(
                toggle.get(&session.display_style().expect("rich")),
                "{toggle:?} on"
            );
            assert!(
                session.content().spans.iter().all(|s| toggle.get(&s.style)),
                "{toggle:?} on everywhere"
            );
            session.toggle(toggle);
            assert!(
                !toggle.get(&session.display_style().expect("rich")),
                "{toggle:?} off"
            );
            assert!(
                session
                    .content()
                    .spans
                    .iter()
                    .all(|s| !toggle.get(&s.style)),
                "{toggle:?} off everywhere"
            );
        }
        assert!(session.content().spans.is_empty());
    }

    #[test]
    fn shift_motion_selects_and_a_plain_motion_collapses() {
        use cosmic_text::Motion;
        let mut engine = TextEngine::new();
        let item = rich_item("abc");
        let mut session = NoteSession::begin(&mut engine, &item, false, true).expect("rich");
        session.motion(&mut engine, Motion::Left, true);
        session.motion(&mut engine, Motion::Left, true);
        assert_eq!(session.selected_text().as_deref(), Some("bc"));
        session.motion(&mut engine, Motion::Left, false);
        assert!(!session.has_selection());
        session.insert_text(&mut engine, "X");
        assert_eq!(session.text(), "aXbc");
    }

    #[test]
    fn a_text_item_renders_without_a_card_while_editing() {
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let item = rich_item("Text");
        let mut session = NoteSession::begin(&mut engine, &item, false, true).expect("rich");
        let content = item.note_content().expect("note");
        let image = renderer
            .render(&mut engine, &content, item.size(), Some(&mut session))
            .expect("renders");
        assert_eq!(image.pixels()[3], 0, "transparent corner");
        assert!(session.caret_local(item.size()).is_some());
        assert!(session.caret_colour().is_some());
    }

    #[test]
    fn the_box_hugs_the_text_while_typing_and_a_size_step_restyles_the_selection() {
        let mut engine = TextEngine::new();
        let item = rich_item("");
        let mut session = NoteSession::begin(&mut engine, &item, true, false).expect("rich");
        let empty = session.rich_size(&mut engine).expect("rich");
        assert!((empty - item.size()).abs().max_element() < 1.0, "{empty}");
        session.insert_text(&mut engine, "one two three");
        let one_line = session.rich_size(&mut engine).expect("rich");
        assert!(one_line.x > empty.x * 3.0, "grows to the right: {one_line}");
        assert!((one_line.y - empty.y).abs() < 1.0);
        session.insert_text(&mut engine, "\nfour\nfive");
        let three = session.rich_size(&mut engine).expect("rich");
        assert!(three.y > one_line.y * 2.0 && (three.x - one_line.x).abs() < 1.0);

        session.select_all();
        assert_eq!(session.step_size(true), Some(28));
        let content = session.content();
        assert!(content.spans.iter().all(|s| s.style.size == Some(28)));
        assert_eq!(session.display_style().and_then(|s| s.size), Some(28));
        // Bigger type, bigger box both ways.
        let bigger = session.rich_size(&mut engine).expect("rich");
        assert!(bigger.x > three.x && bigger.y > three.y);
        // Deleting shrinks it back.
        session.select_all();
        session.insert_text(&mut engine, "x");
        assert!(session.rich_size(&mut engine).expect("rich").x < one_line.x);
    }

    /// What the app does after every keystroke: the box follows the text,
    /// then the edited item renders with the session.
    fn type_and_render(
        engine: &mut TextEngine,
        renderer: &mut NoteRenderer,
        session: &mut NoteSession,
        text: &str,
    ) -> (Vec2, ImageBuffer) {
        session.insert_text(engine, text);
        let size = session.rich_size(engine).expect("rich");
        session.resize(size);
        let content = session.live_content();
        let image = renderer
            .render(engine, &content, size, Some(session))
            .expect("renders");
        (size, image)
    }

    #[test]
    fn large_type_grown_past_the_texture_cap_is_not_clipped_or_stretched() {
        // The video's case: Add text at 17% zoom types at ~141 world units,
        // so a line of 21 letters is several thousand units wide.
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let item = rich_item("");
        let mut session = NoteSession::begin(&mut engine, &item, true, false).expect("rich");
        session.set_typing(rizref_core::TextStyle {
            size: Some(141),
            ..rizref_core::TextStyle::default()
        });
        let mut last = None;
        for letter in "abcdefghijklmnopqrstu".chars() {
            last = Some(type_and_render(
                &mut engine,
                &mut renderer,
                &mut session,
                &letter.to_string(),
            ));
        }
        let (size, image) = last.expect("typed");
        assert!(
            size.x * 2.0 > NOTE_MAX_SIDE,
            "past the cap at density 2: {size}"
        );
        // The texture has the box's proportions (not clipped on one axis).
        let texture = image.width() as f32 / image.height() as f32;
        assert!(
            (texture - size.x / size.y).abs() / (size.x / size.y) < 0.02,
            "texture {}x{} for box {size}",
            image.width(),
            image.height()
        );
        // And the ink reaches the far end: the last letters are drawn.
        let width = image.width() as usize;
        let rightmost = image
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, p)| p[3] > 128)
            .map(|(i, _)| i % width)
            .max()
            .unwrap_or(0);
        assert!(
            rightmost as f32 > width as f32 * 0.9,
            "ink ends at {rightmost} of {width}"
        );
        assert_eq!(session.content().text, "abcdefghijklmnopqrstu");
        assert!(
            session
                .content()
                .spans
                .iter()
                .all(|span| span.style.size == Some(141))
        );
    }

    #[test]
    fn a_new_text_item_at_low_zoom_types_at_its_size_not_scaled_up() {
        // Add text at 18% zoom: the app sizes the empty box for 133-unit
        // type and sets that as the typing size.
        let mut engine = TextEngine::new();
        let item = Item::note_with(
            ItemId::from_raw(6),
            NoteContent::rich(""),
            Vec2::ZERO,
            crate::rich_text::new_box(133),
        );
        let mut session = NoteSession::begin(&mut engine, &item, true, false).expect("rich");
        session.set_typing(rizref_core::TextStyle {
            size: Some(133),
            ..rizref_core::TextStyle::default()
        });
        let empty = session.rich_size(&mut engine).expect("rich");
        assert!(
            (empty - item.size()).abs().max_element() < 1.0,
            "{empty} vs {}",
            item.size()
        );
        session.insert_text(&mut engine, "abc");
        let size = session.rich_size(&mut engine).expect("rich");
        let one_line = 133.0 * crate::rich_text::TEXT_LINE_HEIGHT + 2.0 * rizref_core::TEXT_PADDING;
        assert!((size.y - one_line).abs() < 2.0, "{size} vs {one_line}");
        assert!(
            session
                .content()
                .spans
                .iter()
                .all(|span| span.style.size == Some(133))
        );
    }

    #[test]
    fn deleting_back_to_empty_keeps_the_typing_size() {
        let mut engine = TextEngine::new();
        let item = rich_item("");
        let mut session = NoteSession::begin(&mut engine, &item, true, false).expect("rich");
        session.set_typing(rizref_core::TextStyle {
            size: Some(141),
            bold: true,
            ..rizref_core::TextStyle::default()
        });
        session.insert_text(&mut engine, "h");
        session.action(&mut engine, Action::Backspace);
        assert_eq!(session.text(), "");
        session.insert_text(&mut engine, "abc");
        let spans = session.content().spans;
        assert_eq!(spans.len(), 1, "{spans:?}");
        assert_eq!(spans[0].style.size, Some(141));
        assert!(spans[0].style.bold);
    }

    #[test]
    fn select_all_highlights_every_line_of_a_text_item() {
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let item = rich_item("first line\nsecond");
        let mut session = NoteSession::begin(&mut engine, &item, false, true).expect("rich");
        session.select_all();
        let image = renderer
            .render(
                &mut engine,
                &item.note_content().expect("text"),
                item.size(),
                Some(&mut session),
            )
            .expect("renders");
        // The highlight is a translucent blue; count it in each half.
        let width = image.width() as usize;
        let half = image.height() as usize / 2;
        let highlighted = |rows: std::ops::Range<usize>| {
            rows.flat_map(|y| (0..width).map(move |x| (y * width + x) * 4))
                .filter(|&i| {
                    let p = &image.pixels()[i..i + 4];
                    p[3] > 60 && p[3] < 200 && p[2] > p[0]
                })
                .count()
        };
        assert!(highlighted(0..half) > 200, "first line highlighted");
        assert!(highlighted(half..2 * half) > 200, "second line highlighted");
    }

    #[test]
    fn the_caret_is_as_tall_as_the_text_it_is_in() {
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let item = rich_item("");
        let mut session = NoteSession::begin(&mut engine, &item, true, false).expect("rich");
        let mut caret_height = |session: &mut NoteSession, engine: &mut TextEngine| {
            let content = session.live_content();
            let size = session.rich_size(engine).expect("rich");
            renderer
                .render(engine, &content, size, Some(session))
                .expect("renders");
            session.caret_local(size).expect("drawn").1
        };
        session.insert_text(&mut engine, "small");
        let small = caret_height(&mut session, &mut engine);
        session.select_all();
        for _ in 0..4 {
            session.step_size(true);
        }
        session.motion(&mut engine, cosmic_text::Motion::End, false);
        let big = caret_height(&mut session, &mut engine);
        assert!(big > small * 1.5, "{big} vs {small}");
        // An empty item takes the typing size before anything is typed.
        let empty = rich_item("");
        let mut fresh = NoteSession::begin(&mut engine, &empty, true, false).expect("rich");
        fresh.set_typing(rizref_core::TextStyle {
            size: Some(72),
            ..rizref_core::TextStyle::default()
        });
        let tall = caret_height(&mut fresh, &mut engine);
        assert!(tall > 72.0, "{tall}");
    }

    /// What one keystroke into a text item costs on the UI thread: the edit,
    /// reading the content back, measuring the box and re-rendering. Run
    /// with `cargo test -p rizref --release -- --ignored --nocapture
    /// text_keystroke_budget`.
    #[test]
    #[ignore = "a timing report, not a correctness check"]
    fn text_keystroke_budget() {
        use std::time::Instant;
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let mut item = rich_item("");
        let mut transform = item.transform();
        transform.set_size(Vec2::new(600.0, 400.0));
        item.set_transform(transform);
        let mut session = NoteSession::begin(&mut engine, &item, false, true).expect("rich");
        let sentence = "The quick brown fox jumps over the lazy dog. ";
        let mut worst = std::time::Duration::ZERO;
        let mut total = std::time::Duration::ZERO;
        let mut parts = [std::time::Duration::ZERO; 4];
        let mut count = 0;
        for character in sentence.repeat(6).chars() {
            let start = Instant::now();
            session.insert_text(&mut engine, &character.to_string());
            let edited = Instant::now();
            let content = session.live_content();
            let _ = session.display_style();
            let read = Instant::now();
            let size = session.rich_size(&mut engine).expect("rich");
            session.resize(size);
            let measured = Instant::now();
            renderer.render(&mut engine, &content, size, Some(&mut session));
            let done = Instant::now();
            parts[0] += edited - start;
            parts[1] += read - edited;
            parts[2] += measured - read;
            parts[3] += done - measured;
            let spent = done - start;
            worst = worst.max(spent);
            total += spent;
            count += 1;
        }
        println!(
            "text keystroke: mean {:?}, worst {worst:?} over {count} keys;              edit {:?}, read back {:?}, measure {:?}, render {:?}",
            total / count,
            parts[0] / count,
            parts[1] / count,
            parts[2] / count,
            parts[3] / count,
        );
    }

    /// The note budgets (phase 06a, met in phase 07), measured rather than
    /// asserted (timings vary by machine). Run with
    /// `cargo test -p rizref --release -- --ignored --nocapture note_budgets`.
    #[test]
    #[ignore = "a timing report, not a correctness check"]
    fn note_budgets() {
        use std::time::{Duration, Instant};
        let best = |runs: u32, mut work: Box<dyn FnMut() + '_>| {
            (0..runs)
                .map(|_| {
                    let start = Instant::now();
                    work();
                    start.elapsed()
                })
                .min()
                .unwrap_or(Duration::ZERO)
        };
        let paragraph = "Some **bold** text, *italic* text, `code` and a [link](x). ";
        let mut source = String::from("# Study notes\n\n");
        while source.len() < 4000 {
            source.push_str(paragraph);
            source.push_str("\n\n- a list item\n- another one\n\n");
        }
        let mut note = content(&source, None);
        note.wrap_width = Some(400.0);
        let size = Vec2::new(428.0, 1400.0);

        // Cold: a fresh renderer (no layout, no masks, no layer).
        let mut engine = TextEngine::new();
        let mut renderer = NoteRenderer::new();
        let start = Instant::now();
        renderer.layout(&mut engine, &note.text, 400.0);
        let cold_layout = start.elapsed();
        let start = Instant::now();
        renderer.render(&mut engine, &note, size, None);
        let cold_paint = start.elapsed();
        // Warm: the same note again (an evicted texture coming back), and a
        // resize that keeps the wrap width.
        let warm_render = best(
            5,
            Box::new(|| {
                renderer.render(&mut engine, &note, size, None);
            }),
        );
        let resized = Vec2::new(600.0, 900.0);
        let resize_render = best(
            5,
            Box::new(|| {
                renderer.render(&mut engine, &note, resized, None);
            }),
        );

        let maths: String = (0..20)
            .map(|i| format!("$x_{i}^2 + \\frac{{{i}}}{{y}}$ "))
            .collect();
        let maths = content(&maths, None);
        let math_size = Vec2::new(400.0, 400.0);
        let start = Instant::now();
        renderer.render(&mut engine, &maths, math_size, None);
        let math_cold = start.elapsed();
        let math_warm = best(
            5,
            Box::new(|| {
                renderer.render(&mut engine, &maths, math_size, None);
            }),
        );

        // What the UI thread pays for a note whose render is on the worker.
        let placeholder = best(
            5,
            Box::new(|| {
                super::render_placeholder(&note, size);
            }),
        );

        println!(
            "4,000-char note: cold layout {cold_layout:?}, cold paint {cold_paint:?}, \
             warm re-render {warm_render:?}, resize re-render {resize_render:?}; \
             20 equations: cold {math_cold:?}, warm {math_warm:?}; \
             UI-thread placeholder {placeholder:?}"
        );
    }
}
