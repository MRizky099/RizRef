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
use crate::scene::ItemId;
use crate::transform::Transform;

/// Default on-board size of a freshly created note, in world units.
pub const DEFAULT_NOTE_SIZE: Vec2 = Vec2::new(360.0, 220.0);

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
        /// The note's text, plain UTF-8 with `\n` line breaks.
        text: String,
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

    /// An opaque, unrotated note centred on `center`.
    pub fn note(id: ItemId, text: impl Into<String>, center: Vec2, size: Vec2) -> Self {
        Self {
            id,
            transform: Transform::new(center, size),
            opacity: 1.0,
            flip_x: false,
            flip_y: false,
            body: ItemBody::Note { text: text.into() },
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
            ItemBody::Note { text } => Some(text),
            ItemBody::Image { .. } => None,
        }
    }

    /// Replaces the note text, returning whether it changed.
    pub fn set_text(&mut self, new_text: &str) -> bool {
        match &mut self.body {
            ItemBody::Note { text } => {
                if text == new_text {
                    false
                } else {
                    text.clear();
                    text.push_str(new_text);
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
    use super::{DEFAULT_NOTE_SIZE, Item, ItemBody, ItemState};
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
}
