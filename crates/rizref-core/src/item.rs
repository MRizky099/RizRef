//! An image placed on the board.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::asset::AssetId;
use crate::geometry::Rect;
use crate::scene::ItemId;

/// One image item: which asset it shows and where it sits on the board.
///
/// The transform is centre-plus-extent-plus-rotation, the same representation
/// the renderer's instanced quads use, so drawing an item is a direct mapping
/// with no matrix decomposition. `size` is the image's on-board extent in world
/// units; an unrotated item covers `size` exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageItem {
    id: ItemId,
    asset: AssetId,
    center: Vec2,
    size: Vec2,
    rotation: f32,
    opacity: f32,
}

impl ImageItem {
    /// An opaque, unrotated item centred on `center`.
    pub fn new(id: ItemId, asset: AssetId, center: Vec2, size: Vec2) -> Self {
        Self {
            id,
            asset,
            center,
            size: size.abs(),
            rotation: 0.0,
            opacity: 1.0,
        }
    }

    /// The item's identity within its scene.
    pub fn id(&self) -> ItemId {
        self.id
    }

    /// The asset this item shows.
    pub fn asset(&self) -> AssetId {
        self.asset
    }

    /// Centre in world units.
    pub fn center(&self) -> Vec2 {
        self.center
    }

    /// Extent in world units; always non-negative.
    pub fn size(&self) -> Vec2 {
        self.size
    }

    /// Rotation in radians, clockwise on screen.
    pub fn rotation(&self) -> f32 {
        self.rotation
    }

    /// Opacity in `0..=1`.
    pub fn opacity(&self) -> f32 {
        self.opacity
    }

    /// Moves the item so its centre lands on `center`.
    pub fn set_center(&mut self, center: Vec2) {
        self.center = center;
    }

    /// Sets the on-board extent, normalising a negative size.
    pub fn set_size(&mut self, size: Vec2) {
        self.size = size.abs();
    }

    /// Sets the rotation in radians.
    pub fn set_rotation(&mut self, rotation: f32) {
        self.rotation = rotation;
    }

    /// Sets the opacity, clamped to `0..=1`.
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.0, 1.0);
    }

    /// The axis-aligned world rectangle the rotated item sweeps out.
    pub fn world_rect(&self) -> Rect {
        let (sin, cos) = self.rotation.sin_cos();
        let (half_w, half_h) = (self.size.x * 0.5, self.size.y * 0.5);
        let extent_x = (cos * half_w).abs() + (sin * half_h).abs();
        let extent_y = (sin * half_w).abs() + (cos * half_h).abs();
        Rect::from_min_size(
            Vec2::new(self.center.x - extent_x, self.center.y - extent_y),
            Vec2::new(2.0 * extent_x, 2.0 * extent_y),
        )
    }

    /// Whether `point` lies inside the item's oriented rectangle.
    ///
    /// The point is rotated into item space rather than the rectangle into
    /// world space: one rotation instead of four corners, and the comparison is
    /// a plain half-extent check.
    pub fn hit_test(&self, point: Vec2) -> bool {
        let local = point - self.center;
        let (sin, cos) = self.rotation.sin_cos();
        let x = cos * local.x + sin * local.y;
        let y = -sin * local.x + cos * local.y;
        x.abs() <= self.size.x * 0.5 && y.abs() <= self.size.y * 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::ImageItem;
    use crate::asset::AssetId;
    use crate::scene::ItemId;
    use glam::Vec2;
    use std::f32::consts::FRAC_PI_2;

    fn item() -> ImageItem {
        ImageItem::new(
            ItemId::from_raw(1),
            AssetId::from_checksum([1; 32]),
            Vec2::new(10.0, 20.0),
            Vec2::new(100.0, 50.0),
        )
    }

    #[test]
    fn a_new_item_is_opaque_and_unrotated() {
        let item = item();

        assert_eq!(item.rotation(), 0.0);
        assert_eq!(item.opacity(), 1.0);
        assert_eq!(item.center(), Vec2::new(10.0, 20.0));
        assert_eq!(item.size(), Vec2::new(100.0, 50.0));
    }

    #[test]
    fn the_world_rect_is_the_quad_and_grows_with_rotation() {
        let mut item = item();
        let rect = item.world_rect();
        assert_eq!(rect.min, Vec2::new(-40.0, -5.0));
        assert_eq!(rect.max(), Vec2::new(60.0, 45.0));

        item.set_rotation(FRAC_PI_2);
        let rect = item.world_rect();
        assert!((rect.min.x - (-15.0)).abs() < 1.0e-4);
        assert!((rect.min.y - (-30.0)).abs() < 1.0e-4);
        assert!((rect.max().x - 35.0).abs() < 1.0e-4);
        assert!((rect.max().y - 70.0).abs() < 1.0e-4);
    }

    #[test]
    fn hit_testing_respects_rotation() {
        let mut item = item();
        // The edge at 45 world units right of centre is inside.
        assert!(item.hit_test(Vec2::new(55.0, 20.0)));
        // Outside the long side.
        assert!(!item.hit_test(Vec2::new(70.0, 20.0)));

        item.set_rotation(FRAC_PI_2);
        // After a quarter turn the long axis is vertical.
        assert!(item.hit_test(Vec2::new(10.0, 60.0)));
        assert!(!item.hit_test(Vec2::new(55.0, 20.0)));
    }

    #[test]
    fn size_is_normalised_and_opacity_is_clamped() {
        let mut item = item();

        item.set_size(Vec2::new(-30.0, -40.0));
        assert_eq!(item.size(), Vec2::new(30.0, 40.0));

        item.set_opacity(2.0);
        assert_eq!(item.opacity(), 1.0);
        item.set_opacity(-1.0);
        assert_eq!(item.opacity(), 0.0);
    }
}
