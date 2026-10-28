//! The item transform: centre, extent and rotation on the board.
//!
//! This is the one representation every manipulation path uses. It matches the
//! renderer's instanced quad exactly — centre plus extent plus an angle — so a
//! transform change is a direct instance-buffer update with no matrix
//! decomposition, and hit-testing is one rotation of the point instead of four
//! of the rectangle.
//!
//! All maths is `f32` world units. Scaling, rotation and snapping are computed
//! here rather than in the app so the interaction rules are unit-testable
//! without a window, and so the renderer stays a pure consumer of geometry.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::geometry::Rect;

/// Smallest extent an item may be scaled to, in world units.
///
/// A zero-width item cannot be grabbed again, so scaling clamps rather than
/// collapsing; the value is small enough to be invisible at 100% and large
/// enough that the handles still overlap the item.
pub const MIN_ITEM_SIZE: f32 = 8.0;

/// Rotation applied to a local offset (clockwise on screen, y down).
fn rotate(offset: Vec2, radians: f32) -> Vec2 {
    let (sin, cos) = radians.sin_cos();
    Vec2::new(
        cos * offset.x - sin * offset.y,
        sin * offset.x + cos * offset.y,
    )
}

/// The inverse of [`rotate`].
fn unrotate(offset: Vec2, radians: f32) -> Vec2 {
    let (sin, cos) = radians.sin_cos();
    Vec2::new(
        cos * offset.x + sin * offset.y,
        -sin * offset.x + cos * offset.y,
    )
}

/// Where and how large an item sits on the board.
///
/// An unrotated transform with centre `c` and size `s` covers
/// `c ± s/2` exactly; rotating by `rotation` turns the quad about `c`. The size
/// is always non-negative, so a transform can never be inside-out.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    center: Vec2,
    size: Vec2,
    rotation: f32,
}

impl Transform {
    /// An unrotated transform centred on `center`.
    pub fn new(center: Vec2, size: Vec2) -> Self {
        Self {
            center,
            size: size.abs(),
            rotation: 0.0,
        }
    }

    /// The same transform turned by `rotation` radians.
    pub fn with_rotation(mut self, rotation: f32) -> Self {
        self.rotation = rotation;
        self
    }

    /// Centre in world units.
    pub const fn center(&self) -> Vec2 {
        self.center
    }

    /// Extent in world units; always non-negative.
    pub const fn size(&self) -> Vec2 {
        self.size
    }

    /// Rotation in radians, clockwise on screen.
    pub const fn rotation(&self) -> f32 {
        self.rotation
    }

    /// Moves the centre without touching size or rotation.
    pub fn set_center(&mut self, center: Vec2) {
        self.center = center;
    }

    /// Sets the extent, normalising it and clamping to [`MIN_ITEM_SIZE`].
    pub fn set_size(&mut self, size: Vec2) {
        self.size = size.abs().max(Vec2::splat(MIN_ITEM_SIZE));
    }

    /// Sets the rotation in radians.
    pub fn set_rotation(&mut self, rotation: f32) {
        self.rotation = rotation;
    }

    /// The transform moved by `delta`.
    pub fn translated(&self, delta: Vec2) -> Self {
        Self {
            center: self.center + delta,
            ..*self
        }
    }

    /// The transform turned by `delta` about its own centre.
    pub fn rotated(&self, delta: f32) -> Self {
        Self {
            rotation: self.rotation + delta,
            ..*self
        }
    }

    /// The transform turned by `delta` about `pivot`.
    ///
    /// The centre orbits the pivot and the item's own angle advances by the
    /// same amount, which is what rotating a multi-item selection means.
    pub fn rotated_about(&self, pivot: Vec2, delta: f32) -> Self {
        Self {
            center: pivot + rotate(self.center - pivot, delta),
            rotation: self.rotation + delta,
            size: self.size,
        }
    }

    /// The transform scaled by `factors` about `pivot`, along the world axes.
    ///
    /// Used for multi-item scaling, where the selection frame is axis-aligned;
    /// a rotated item's size is scaled in its own frame as a fixed-scale
    /// approximation, which keeps the quad a rectangle rather than shearing it.
    pub fn scaled_about(&self, pivot: Vec2, factors: Vec2) -> Self {
        let offset = self.center - pivot;
        let (sin, cos) = self.rotation.sin_cos();
        let local_x = factors.x * cos * cos + factors.y * sin * sin;
        let local_y = factors.x * sin * sin + factors.y * cos * cos;
        Self {
            center: pivot + Vec2::new(offset.x * factors.x, offset.y * factors.y),
            size: (self.size * Vec2::new(local_x, local_y))
                .abs()
                .max(Vec2::splat(MIN_ITEM_SIZE)),
            rotation: self.rotation,
        }
    }

    /// The axis-aligned world rectangle the rotated quad sweeps out.
    pub fn world_rect(&self) -> Rect {
        let (sin, cos) = self.rotation.sin_cos();
        let (half_w, half_h) = (self.size.x * 0.5, self.size.y * 0.5);
        let extent_x = (cos * half_w).abs() + (sin * half_h).abs();
        let extent_y = (sin * half_w).abs() + (cos * half_h).abs();
        Rect::from_center_size(self.center, Vec2::new(2.0 * extent_x, 2.0 * extent_y))
    }

    /// Whether `point` lies inside the oriented rectangle.
    pub fn hit_test(&self, point: Vec2) -> bool {
        let local = unrotate(point - self.center, self.rotation);
        local.x.abs() <= self.size.x * 0.5 && local.y.abs() <= self.size.y * 0.5
    }

    /// The four corners in world space: north-west, north-east, south-east,
    /// south-west, in clockwise screen order.
    pub fn corners(&self) -> [Vec2; 4] {
        let half = self.size * 0.5;
        [
            self.from_local(Vec2::new(-half.x, -half.y)),
            self.from_local(Vec2::new(half.x, -half.y)),
            self.from_local(Vec2::new(half.x, half.y)),
            self.from_local(Vec2::new(-half.x, half.y)),
        ]
    }

    /// Maps an offset from the centre in item space to world space.
    pub fn from_local(&self, local: Vec2) -> Vec2 {
        self.center + rotate(local, self.rotation)
    }

    /// Maps a world point to an offset from the centre in item space.
    pub fn to_local(&self, world: Vec2) -> Vec2 {
        unrotate(world - self.center, self.rotation)
    }

    /// Whether the oriented rectangle shares any point with `rect`.
    ///
    /// Separating-axis test over the four relevant axes (the rectangle's two
    /// world axes and the item's two local axes). Edges that merely touch count
    /// as intersecting: a marquee that grazes an item should select it.
    pub fn intersects_rect(&self, rect: Rect) -> bool {
        if rect.is_empty() || self.size.x <= 0.0 || self.size.y <= 0.0 {
            return false;
        }
        let item = self.corners();
        let other = [
            rect.min,
            Vec2::new(rect.max().x, rect.min.y),
            rect.max(),
            Vec2::new(rect.min.x, rect.max().y),
        ];
        let (sin, cos) = self.rotation.sin_cos();
        let axes = [Vec2::X, Vec2::Y, Vec2::new(cos, sin), Vec2::new(-sin, cos)];
        axes.iter().all(|axis| overlaps(&item, &other, *axis))
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::new(Vec2::ZERO, Vec2::splat(MIN_ITEM_SIZE))
    }
}

/// Whether the projections of two convex polygons onto `axis` overlap.
fn overlaps(a: &[Vec2; 4], b: &[Vec2; 4], axis: Vec2) -> bool {
    if axis.length_squared() < 1.0e-12 {
        return true;
    }
    let project = |points: &[Vec2; 4]| {
        points
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), point| {
                let value = point.dot(axis);
                (min.min(value), max.max(value))
            })
    };
    let (a_min, a_max) = project(a);
    let (b_min, b_max) = project(b);
    a_min <= b_max && b_min <= a_max
}

#[cfg(test)]
mod tests {
    use super::{MIN_ITEM_SIZE, Transform};
    use crate::geometry::Rect;
    use glam::Vec2;
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1.0e-4
    }

    fn item() -> Transform {
        Transform::new(Vec2::new(10.0, 20.0), Vec2::new(100.0, 50.0))
    }

    #[test]
    fn a_new_transform_is_axis_aligned_and_normalises_its_size() {
        let transform = Transform::new(Vec2::ZERO, Vec2::new(-4.0, -8.0));

        assert_eq!(transform.size(), Vec2::new(4.0, 8.0));
        assert_eq!(transform.rotation(), 0.0);
    }

    #[test]
    fn the_world_rect_is_the_quad_and_grows_with_rotation() {
        let mut transform = item();
        assert!(close(transform.world_rect().min, Vec2::new(-40.0, -5.0)));

        transform.set_rotation(FRAC_PI_2);
        let rect = transform.world_rect();
        assert!(close(rect.min, Vec2::new(-15.0, -30.0)));
        assert!(close(rect.max(), Vec2::new(35.0, 70.0)));
    }

    #[test]
    fn hit_testing_respects_rotation() {
        let mut transform = item();
        assert!(transform.hit_test(Vec2::new(55.0, 20.0)));
        assert!(!transform.hit_test(Vec2::new(70.0, 20.0)));

        transform.set_rotation(FRAC_PI_2);
        assert!(transform.hit_test(Vec2::new(10.0, 60.0)));
        assert!(!transform.hit_test(Vec2::new(55.0, 20.0)));
    }

    #[test]
    fn corners_are_clockwise_and_round_trip_through_hit_testing() {
        let transform =
            Transform::new(Vec2::new(3.0, -4.0), Vec2::new(20.0, 10.0)).with_rotation(FRAC_PI_4);

        let corners = transform.corners();
        assert!(close(
            corners[0],
            transform.from_local(Vec2::new(-10.0, -5.0))
        ));
        for corner in corners {
            assert!(
                transform.hit_test(corner * 0.999 + transform.center() * 0.001),
                "a corner pulled slightly inwards is inside"
            );
        }
    }

    #[test]
    fn local_and_world_space_round_trip() {
        let transform = item().with_rotation(0.7);
        let local = Vec2::new(12.0, -7.0);

        assert!(close(
            transform.to_local(transform.from_local(local)),
            local
        ));
    }

    #[test]
    fn rotating_about_a_pivot_orbits_the_centre_and_keeps_the_size() {
        let transform = item();
        let rotated = transform.rotated_about(Vec2::ZERO, FRAC_PI_2);

        assert!(close(rotated.center(), Vec2::new(-20.0, 10.0)));
        assert_eq!(rotated.size(), transform.size());
        assert!((rotated.rotation() - FRAC_PI_2).abs() < 1.0e-6);
    }

    #[test]
    fn axis_scaling_moves_the_centre_and_uses_the_local_frame() {
        let transform = item().with_rotation(FRAC_PI_2);
        let scaled = transform.scaled_about(Vec2::ZERO, Vec2::new(2.0, 2.0));

        assert!(close(scaled.center(), Vec2::new(20.0, 40.0)));
        assert!(close(scaled.size(), Vec2::new(200.0, 100.0)));

        // Non-uniform world scaling of a quarter-turned item: world x hits the
        // item's height, so the height takes the x factor.
        let stretched = transform.scaled_about(Vec2::ZERO, Vec2::new(3.0, 1.0));
        assert!(close(stretched.size(), Vec2::new(100.0, 150.0)));
    }

    #[test]
    fn scaling_clamps_to_the_minimum_size() {
        let transform = item();
        let squashed = transform.scaled_about(transform.center(), Vec2::splat(0.0));

        assert_eq!(squashed.size(), Vec2::splat(MIN_ITEM_SIZE));
    }

    #[test]
    fn rect_intersection_is_exact_for_rotation() {
        let transform = Transform::new(Vec2::ZERO, Vec2::new(100.0, 10.0)).with_rotation(FRAC_PI_4);

        // A small box far along the diagonal: inside the rotated bar.
        let along = Rect::from_center_size(Vec2::new(30.0, 30.0), Vec2::splat(4.0));
        assert!(transform.intersects_rect(along));

        // A box at the same distance along +x is outside the bar.
        let across = Rect::from_center_size(Vec2::new(60.0, 0.0), Vec2::splat(4.0));
        assert!(!transform.intersects_rect(across));
    }

    #[test]
    fn rect_intersection_accepts_a_touching_edge() {
        let transform = Transform::new(Vec2::ZERO, Vec2::new(10.0, 10.0));

        let touching = Rect::from_min_max(Vec2::new(5.0, -10.0), Vec2::new(15.0, 10.0));
        assert!(transform.intersects_rect(touching));
    }

    #[test]
    fn rect_intersection_of_unrotated_items_matches_rect_math() {
        let transform = Transform::new(Vec2::ZERO, Vec2::new(10.0, 10.0));
        let overlapping = Rect::from_min_max(Vec2::new(4.0, 4.0), Vec2::new(8.0, 8.0));
        let disjoint = Rect::from_min_max(Vec2::new(6.0, 6.0), Vec2::new(8.0, 8.0));

        assert!(transform.intersects_rect(overlapping));
        assert!(!transform.intersects_rect(disjoint));
    }
}
