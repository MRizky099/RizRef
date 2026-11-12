//! One thing placed on the board: an image, or a text note.
//!
//! Every item carries the same transform, opacity and mirror flags; only the
//! payload differs. Keeping one item type means selection, hit-testing,
//! manipulation, ordering and grouping are written once and work for notes as
//! soon as they work for images.
//!
//! The transform is centre-plus-extent-plus-rotation, the same representation
//! the renderer's instanced quads use, so drawing an item is a direct mapping
//! with no matrix decomposition. Mirrored items keep a positive extent and flip
//! the sampled UV rectangle instead, which is why a flip never invents a
//! negative scale for later maths to trip over.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::asset::AssetId;
use crate::geometry::Rect;
use crate::rich::{TextSpan, normalize};
use crate::scene::ItemId;
use crate::transform::Transform;

/// Default on-board size of a freshly created note, in world units.
pub const DEFAULT_NOTE_SIZE: Vec2 = Vec2::new(360.0, 220.0);

/// Inner padding between a note's frame and its content, in world units.
pub const NOTE_PADDING: f32 = 14.0;

/// The default note card colour, `#302c25`.
pub const DEFAULT_NOTE_BACKGROUND: [u8; 3] = [0x30, 0x2c, 0x25];

/// Inner padding between a text item's box and its glyphs, in unscaled
/// text units (the box scales with the text, so this scales too).
pub const TEXT_PADDING: f32 = 6.0;

/// Smallest stored note wrap width, in world units.
pub const MIN_WRAP_WIDTH: f32 = 16.0;

/// Largest stored note wrap width, in world units.
pub const MAX_WRAP_WIDTH: f32 = 10_000.0;

/// Clamps a stored wrap width to [`MIN_WRAP_WIDTH`]`..=`[`MAX_WRAP_WIDTH`].
///
/// A non-finite or non-positive value is not a wrap width at all and yields
/// `None`, which means "use the note's inner box width".
pub fn clamp_wrap_width(width: f32) -> Option<f32> {
    (width.is_finite() && width > 0.0).then(|| width.clamp(MIN_WRAP_WIDTH, MAX_WRAP_WIDTH))
}

/// How a note's text is interpreted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteFormat {
    /// Plain UTF-8 with `\n` line breaks: the phase-04 meaning, and the default
    /// for any payload that does not say otherwise.
    #[default]
    Plain,
    /// The markdown subset rendered by the application.
    Markdown,
    /// A free-standing text item: plain `text` styled by `spans` (bold,
    /// italic, underline, colour), with no card behind it. Its box scales
    /// the text: resizing the item resizes the glyphs.
    Rich,
}

/// Everything that describes a note's content, as one before/after unit.
///
/// The edit command stores two of these, so text, format, background and wrap
/// width all move through undo together.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NoteContent {
    /// The note's source text.
    pub text: String,
    /// How `text` is interpreted.
    pub format: NoteFormat,
    /// An opaque background override; `None` is the default card colour.
    pub background: Option<[u8; 3]>,
    /// The width the content is laid out at, in world units; `None` uses the
    /// note's inner box width.
    pub wrap_width: Option<f32>,
    /// Styled runs over `text`, for [`NoteFormat::Rich`]; empty otherwise.
    pub spans: Vec<TextSpan>,
}

impl NoteContent {
    /// An unstyled text item.
    pub fn rich(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            format: NoteFormat::Rich,
            background: None,
            wrap_width: None,
            spans: Vec::new(),
        }
    }

    /// Whether this is a free-standing text item rather than a note card.
    pub fn is_rich(&self) -> bool {
        self.format == NoteFormat::Rich
    }

    /// A markdown note with the given source and no overrides.
    pub fn markdown(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            format: NoteFormat::Markdown,
            background: None,
            wrap_width: None,
            spans: Vec::new(),
        }
    }

    /// The background colour to paint.
    pub fn background_or_default(&self) -> [u8; 3] {
        self.background.unwrap_or(DEFAULT_NOTE_BACKGROUND)
    }
}

/// The inner (content) width of a note box `width` wide, never below
/// [`MIN_WRAP_WIDTH`].
pub fn inner_note_width(width: f32) -> f32 {
    (width - 2.0 * NOTE_PADDING).max(MIN_WRAP_WIDTH)
}

/// What an item shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemBody {
    /// A shared image asset.
    Image {
        /// The asset whose texture this item samples.
        asset: AssetId,
    },
    /// A text note drawn by the application.
    Note {
        /// The note's source text; see `format`.
        text: String,
        /// How `text` is interpreted; absent in older payloads, meaning plain.
        #[serde(default)]
        format: NoteFormat,
        /// An opaque background override.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        background: Option<[u8; 3]>,
        /// The content wrap width in world units.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wrap_width: Option<f32>,
        /// Styled runs over `text` (rich text items only).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        spans: Vec<TextSpan>,
    },
}

/// An item's transform plus the flags that belong to its quad.
///
/// This is the unit commands store as their before/after inverse: the payload
/// (asset or text) is not a transform and is edited by its own command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemState {
    /// Position, extent and rotation.
    pub transform: Transform,
    /// Whether the sampled texture is mirrored horizontally.
    pub flip_x: bool,
    /// Whether the sampled texture is mirrored vertically.
    pub flip_y: bool,
}

/// One item on the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    id: ItemId,
    transform: Transform,
    opacity: f32,
    #[serde(default)]
    flip_x: bool,
    #[serde(default)]
    flip_y: bool,
    body: ItemBody,
}

impl Item {
    /// An opaque, unrotated image item centred on `center`.
    pub fn image(id: ItemId, asset: AssetId, center: Vec2, size: Vec2) -> Self {
        Self {
            id,
            transform: Transform::new(center, size),
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
            body: ItemBody::Image { asset },
        }
    }

    /// An opaque, unrotated markdown note centred on `center`.
    pub fn note(id: ItemId, text: impl Into<String>, center: Vec2, size: Vec2) -> Self {
        Self::note_with(id, NoteContent::markdown(text), center, size)
    }

    /// An opaque, unrotated note with exactly `content`, as a reader builds it.
    pub fn note_with(id: ItemId, content: NoteContent, center: Vec2, size: Vec2) -> Self {
        let NoteContent {
            text,
            format,
            background,
            wrap_width,
            spans,
        } = content;
        let spans = normalize(&text, spans);
        Self {
            id,
            transform: Transform::new(center, size),
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
            body: ItemBody::Note {
                text,
                format,
                background,
                wrap_width: wrap_width.and_then(clamp_wrap_width),
                spans,
            },
        }
    }

    /// The item's identity within its scene.
    pub fn id(&self) -> ItemId {
        self.id
    }

    /// A copy of this item with a new identity, as duplicate makes.
    pub fn with_id(&self, id: ItemId) -> Self {
        Self { id, ..self.clone() }
    }

    /// What the item shows.
    pub fn body(&self) -> &ItemBody {
        &self.body
    }

    /// The asset this item shows, when it is an image.
    pub fn asset(&self) -> Option<AssetId> {
        match self.body {
            ItemBody::Image { asset } => Some(asset),
            ItemBody::Note { .. } => None,
        }
    }

    /// The note text, when this item is a note.
    pub fn text(&self) -> Option<&str> {
        match &self.body {
            ItemBody::Note { text, .. } => Some(text),
            ItemBody::Image { .. } => None,
        }
    }

    /// The note's full content, when this item is a note.
    pub fn note_content(&self) -> Option<NoteContent> {
        match &self.body {
            ItemBody::Note {
                text,
                format,
                background,
                wrap_width,
                spans,
            } => Some(NoteContent {
                text: text.clone(),
                format: *format,
                background: *background,
                wrap_width: *wrap_width,
                spans: spans.clone(),
            }),
            ItemBody::Image { .. } => None,
        }
    }

    /// The note's text format, when this item is a note.
    pub fn note_format(&self) -> Option<NoteFormat> {
        match &self.body {
            ItemBody::Note { format, .. } => Some(*format),
            ItemBody::Image { .. } => None,
        }
    }

    /// The width a note's content is laid out at: the stored wrap width, or the
    /// inner box width for a note that has none yet.
    pub fn note_wrap_width(&self) -> Option<f32> {
        match &self.body {
            ItemBody::Note { wrap_width, .. } => {
                Some(wrap_width.unwrap_or_else(|| inner_note_width(self.size().x)))
            }
            ItemBody::Image { .. } => None,
        }
    }

    /// Replaces the whole note content, returning whether anything changed.
    ///
    /// The wrap width is clamped on the way in, so a stored note never carries
    /// a value the format would reject.
    pub fn set_note(&mut self, content: &NoteContent) -> bool {
        match &mut self.body {
            ItemBody::Note {
                text,
                format,
                background,
                wrap_width,
                spans,
            } => {
                let new_wrap = content.wrap_width.and_then(clamp_wrap_width);
                let new_spans = normalize(&content.text, content.spans.clone());
                let changed = *text != content.text
                    || *format != content.format
                    || *background != content.background
                    || *wrap_width != new_wrap
                    || *spans != new_spans;
                if changed {
                    text.clone_from(&content.text);
                    *format = content.format;
                    *background = content.background;
                    *wrap_width = new_wrap;
                    *spans = new_spans;
                }
                changed
            }
            ItemBody::Image { .. } => false,
        }
    }

    /// Replaces the note text, returning whether it changed.
    pub fn set_text(&mut self, new_text: &str) -> bool {
        match &mut self.body {
            ItemBody::Note { text, spans, .. } => {
                if text == new_text {
                    false
                } else {
                    text.clear();
                    text.push_str(new_text);
                    *spans = normalize(text, std::mem::take(spans));
                    true
                }
            }
            ItemBody::Image { .. } => false,
        }
    }

    /// Whether the item is a note.
    pub fn is_note(&self) -> bool {
        matches!(self.body, ItemBody::Note { .. })
    }

    /// Whether the rotate gesture may turn the item: images and text items
    /// yes, note cards no (a note is a card of reading text and stays
    /// upright).
    pub fn is_rotatable(&self) -> bool {
        !self.is_note() || self.is_rich_text()
    }

    /// Whether the item is a free-standing rich text item.
    pub fn is_rich_text(&self) -> bool {
        matches!(
            self.body,
            ItemBody::Note {
                format: NoteFormat::Rich,
                ..
            }
        )
    }

    /// The item's placement and quad flags.
    pub const fn state(&self) -> ItemState {
        ItemState {
            transform: self.transform,
            flip_x: self.flip_x,
            flip_y: self.flip_y,
        }
    }

    /// Restores a placement and quad flags, as undo does.
    pub fn set_state(&mut self, state: ItemState) {
        self.transform = state.transform;
        self.flip_x = state.flip_x;
        self.flip_y = state.flip_y;
    }

    /// The item's transform.
    pub const fn transform(&self) -> Transform {
        self.transform
    }

    /// Replaces the transform.
    pub fn set_transform(&mut self, transform: Transform) {
        self.transform = transform;
    }

    /// Centre in world units.
    pub fn center(&self) -> Vec2 {
        self.transform.center()
    }

    /// Extent in world units; always non-negative.
    pub fn size(&self) -> Vec2 {
        self.transform.size()
    }

    /// Rotation in radians, clockwise on screen.
    pub fn rotation(&self) -> f32 {
        self.transform.rotation()
    }

    /// Whether the image is mirrored horizontally.
    pub const fn flip_x(&self) -> bool {
        self.flip_x
    }

    /// Whether the image is mirrored vertically.
    pub const fn flip_y(&self) -> bool {
        self.flip_y
    }

    /// Sets the mirror flags.
    pub fn set_flip(&mut self, flip_x: bool, flip_y: bool) {
        self.flip_x = flip_x;
        self.flip_y = flip_y;
    }

    /// Opacity in `0..=1`.
    pub fn opacity(&self) -> f32 {
        self.opacity
    }

    /// Sets the opacity, clamped to `0..=1`.
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.0, 1.0);
    }

    /// The axis-aligned world rectangle the rotated item sweeps out.
    pub fn world_rect(&self) -> Rect {
        self.transform.world_rect()
    }

    /// Whether `point` lies inside the item's oriented rectangle.
    pub fn hit_test(&self, point: Vec2) -> bool {
        self.transform.hit_test(point)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_NOTE_BACKGROUND, DEFAULT_NOTE_SIZE, Item, ItemBody, ItemState, MAX_WRAP_WIDTH,
        MIN_WRAP_WIDTH, NoteContent, NoteFormat, clamp_wrap_width,
    };
    use crate::asset::AssetId;
    use crate::scene::ItemId;
    use crate::transform::Transform;
    use glam::Vec2;
    use std::f32::consts::FRAC_PI_2;

    fn image() -> Item {
        Item::image(
            ItemId::from_raw(1),
            AssetId::from_checksum([1; 32]),
            Vec2::new(10.0, 20.0),
            Vec2::new(100.0, 50.0),
        )
    }

    #[test]
    fn a_new_image_is_opaque_unflipped_and_unrotated() {
        let item = image();

        assert_eq!(item.rotation(), 0.0);
        assert_eq!(item.opacity(), 1.0);
        assert_eq!(item.center(), Vec2::new(10.0, 20.0));
        assert_eq!(item.size(), Vec2::new(100.0, 50.0));
        assert!(!item.flip_x() && !item.flip_y());
        assert!(!item.is_note());
        assert!(item.asset().is_some());
        assert_eq!(item.text(), None);
    }

    #[test]
    fn a_note_carries_text_and_no_asset() {
        let mut item = Item::note(
            ItemId::from_raw(2),
            "warm light",
            Vec2::ZERO,
            DEFAULT_NOTE_SIZE,
        );

        assert!(item.is_note());
        assert_eq!(item.text(), Some("warm light"));
        assert_eq!(item.asset(), None);
        assert!(item.set_text("cool light"));
        assert!(!item.set_text("cool light"), "same text is not a change");
        assert_eq!(item.text(), Some("cool light"));
        assert!(matches!(item.body(), ItemBody::Note { .. }));
    }

    #[test]
    fn image_text_cannot_be_set() {
        let mut item = image();

        assert!(!item.set_text("not a note"));
    }

    #[test]
    fn the_world_rect_is_the_quad_and_grows_with_rotation() {
        let mut item = image();
        let rect = item.world_rect();
        assert_eq!(rect.min, Vec2::new(-40.0, -5.0));
        assert_eq!(rect.max(), Vec2::new(60.0, 45.0));

        let mut transform = item.transform().with_rotation(FRAC_PI_2);
        transform.set_center(item.center());
        item.set_transform(transform);
        let rect = item.world_rect();
        assert!((rect.min.x - (-15.0)).abs() < 1.0e-4);
        assert!((rect.min.y - (-30.0)).abs() < 1.0e-4);
        assert!((rect.max().x - 35.0).abs() < 1.0e-4);
        assert!((rect.max().y - 70.0).abs() < 1.0e-4);
    }

    #[test]
    fn hit_testing_respects_rotation() {
        let mut item = image();
        assert!(item.hit_test(Vec2::new(55.0, 20.0)));
        assert!(!item.hit_test(Vec2::new(70.0, 20.0)));

        let mut transform = item.transform().with_rotation(FRAC_PI_2);
        transform.set_center(item.center());
        item.set_transform(transform);
        assert!(item.hit_test(Vec2::new(10.0, 60.0)));
        assert!(!item.hit_test(Vec2::new(55.0, 20.0)));
    }

    #[test]
    fn size_is_normalised_and_opacity_is_clamped() {
        let mut item = image();
        let mut transform = item.transform();
        transform.set_size(Vec2::new(-30.0, -40.0));
        item.set_transform(transform);
        assert_eq!(item.size(), Vec2::new(30.0, 40.0));

        item.set_opacity(2.0);
        assert_eq!(item.opacity(), 1.0);
        item.set_opacity(-1.0);
        assert_eq!(item.opacity(), 0.0);
    }

    #[test]
    fn state_round_trips_transform_and_flips() {
        let mut item = image();
        let state = ItemState {
            transform: Transform::new(Vec2::splat(5.0), Vec2::splat(20.0)).with_rotation(0.4),
            flip_x: true,
            flip_y: true,
        };

        item.set_state(state);

        assert_eq!(item.state(), state);
        assert_eq!(item.center(), Vec2::splat(5.0));
        assert!(item.flip_x() && item.flip_y());
    }

    #[test]
    fn a_new_note_is_markdown_with_no_overrides() {
        let item = Item::note(ItemId::from_raw(4), "**hi**", Vec2::ZERO, DEFAULT_NOTE_SIZE);
        let content = item.note_content().expect("a note");

        assert_eq!(content.format, NoteFormat::Markdown);
        assert_eq!(content.background, None);
        assert_eq!(content.wrap_width, None);
        assert_eq!(content.background_or_default(), DEFAULT_NOTE_BACKGROUND);
        assert_eq!(item.note_wrap_width(), Some(360.0 - 28.0));
        assert_eq!(image().note_content(), None);
    }

    #[test]
    fn set_note_reports_changes_field_by_field() {
        let mut item = Item::note(ItemId::from_raw(4), "a", Vec2::ZERO, DEFAULT_NOTE_SIZE);
        let mut content = item.note_content().expect("a note");

        assert!(
            !item.set_note(&content),
            "identical content is not a change"
        );
        content.background = Some([255, 255, 255]);
        assert!(item.set_note(&content));
        content.wrap_width = Some(200.0);
        assert!(item.set_note(&content));
        assert_eq!(item.note_wrap_width(), Some(200.0));
        content.format = NoteFormat::Plain;
        assert!(item.set_note(&content));
        assert_eq!(item.note_content(), Some(content));
        assert!(!image().set_note(&NoteContent::default()));
    }

    #[test]
    fn wrap_width_is_clamped_and_rejects_non_numbers() {
        assert_eq!(clamp_wrap_width(1.0), Some(MIN_WRAP_WIDTH));
        assert_eq!(clamp_wrap_width(1.0e9), Some(MAX_WRAP_WIDTH));
        assert_eq!(clamp_wrap_width(300.0), Some(300.0));
        assert_eq!(clamp_wrap_width(0.0), None);
        assert_eq!(clamp_wrap_width(-5.0), None);
        assert_eq!(clamp_wrap_width(f32::NAN), None);
        assert_eq!(clamp_wrap_width(f32::INFINITY), None);

        let mut content = NoteContent::markdown("x");
        content.wrap_width = Some(4.0);
        let item = Item::note_with(ItemId::from_raw(1), content, Vec2::ZERO, Vec2::splat(50.0));
        assert_eq!(item.note_wrap_width(), Some(MIN_WRAP_WIDTH));
    }

    #[test]
    fn a_legacy_payload_without_format_reads_as_plain() {
        let body: ItemBody =
            serde_json::from_str(r#"{"kind":"note","text":"old"}"#).expect("parses");
        assert_eq!(
            body,
            ItemBody::Note {
                text: "old".into(),
                format: NoteFormat::Plain,
                background: None,
                wrap_width: None,
                spans: Vec::new(),
            }
        );
        let json = serde_json::to_string(&ItemBody::Note {
            text: "new".into(),
            format: NoteFormat::Markdown,
            background: None,
            wrap_width: None,
            spans: Vec::new(),
        })
        .expect("serialises");
        assert_eq!(json, r#"{"kind":"note","text":"new","format":"markdown"}"#);
    }

    #[test]
    fn a_rich_item_keeps_normalised_spans() {
        use crate::rich::{TextSpan, TextStyle};
        let bold = TextStyle {
            bold: true,
            ..TextStyle::default()
        };
        let span = |start, end| TextSpan {
            start,
            end,
            style: bold,
        };
        let mut content = NoteContent::rich("hello");
        content.spans = vec![span(0, 2), span(2, 99)];
        let mut item = Item::note_with(ItemId::from_raw(9), content.clone(), Vec2::ZERO, Vec2::ONE);
        assert!(item.is_rich_text() && item.is_note());
        assert_eq!(item.note_content().expect("note").spans, vec![span(0, 5)]);
        assert!(!item.set_note(&content), "normalises to the same spans");
        assert!(item.set_text("hi"));
        assert_eq!(item.note_content().expect("note").spans, vec![span(0, 2)]);
    }
}
