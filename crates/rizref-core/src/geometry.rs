//! Axis-aligned geometry shared by the canvas, hit-testing and export.
//!
//! Everything here works in `f32` world units. Screen-space rounding happens at
//! presentation time, never in the model.

use glam::Vec2;
use serde::{Deserialize, Serialize};

/// An axis-aligned rectangle.
///
/// The rectangle is stored as a corner plus a non-negative extent, so it can
/// never be inverted: the constructors normalise whatever they are given. It is
/// used both for world-space item bounds and for viewport-relative regions and
/// carries no unit of its own — the caller knows which space it is in.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// Corner with the smallest coordinates.
    pub min: Vec2,
    /// Extent from [`Rect::min`], always non-negative.
    pub size: Vec2,
}

impl Rect {
    /// A zero-sized rectangle at the world origin.
    pub const ZERO: Self = Self {
        min: Vec2::ZERO,
        size: Vec2::ZERO,
    };

    /// Builds a rectangle from its smallest corner and an extent.
    ///
    /// A negative component of `size` is folded into the corner rather than
    /// kept, so the result always satisfies `max() >= min()`: a caller that
    /// passes a negative extent meant to name the opposite corner, and silently
    /// keeping an inverted rectangle is how hit-tests start missing items.
    pub fn from_min_size(min: Vec2, size: Vec2) -> Self {
        Self {
            min: min + size.min(Vec2::ZERO),
            size: size.abs(),
        }
    }

    /// Builds a rectangle from two opposite corners given in any order.
    pub fn from_min_max(a: Vec2, b: Vec2) -> Self {
        let min = a.min(b);
        Self {
            min,
            size: a.max(b) - min,
        }
    }

    /// Builds a rectangle centred on `center` with the given extent.
    pub fn from_center_size(center: Vec2, size: Vec2) -> Self {
        Self::from_min_size(center - size.abs() * 0.5, size)
    }

    /// Corner with the largest coordinates.
    pub fn max(&self) -> Vec2 {
        self.min + self.size
    }

    /// Centre point of the rectangle.
    pub fn center(&self) -> Vec2 {
        self.min + self.size * 0.5
    }

    /// Extent along x.
    pub fn width(&self) -> f32 {
        self.size.x
    }

    /// Extent along y.
    pub fn height(&self) -> f32 {
        self.size.y
    }

    /// Surface area; zero for a degenerate rectangle.
    pub fn area(&self) -> f32 {
        self.size.x * self.size.y
    }

    /// Whether either extent is zero, i.e. the rectangle covers no point.
    pub fn is_empty(&self) -> bool {
        self.size.x <= 0.0 || self.size.y <= 0.0
    }

    /// Whether `point` lies inside the rectangle.
    ///
    /// The region is half-open: points on the `min` edges are inside, points on
    /// the `max` edges are not. That makes rectangles tile without overlapping,
    /// which is what hit-testing neighbouring items needs.
    pub fn contains(&self, point: Vec2) -> bool {
        point.x >= self.min.x
            && point.y >= self.min.y
            && point.x < self.max().x
            && point.y < self.max().y
    }

    /// Whether the two rectangles share at least one point.
    pub fn intersects(&self, other: &Self) -> bool {
        self.intersection(other).is_some()
    }

    /// The overlapping region, or `None` when the rectangles are disjoint.
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let min = self.min.max(other.min);
        let max = self.max().min(other.max());
        if max.x <= min.x || max.y <= min.y {
            return None;
        }
        Some(Self::from_min_size(min, max - min))
    }

    /// The smallest rectangle containing both.
    pub fn union(&self, other: &Self) -> Self {
        Self::from_min_max(self.min.min(other.min), self.max().max(other.max()))
    }

    /// Moves the rectangle by `by`.
    pub fn translated(&self, by: Vec2) -> Self {
        Self {
            min: self.min + by,
            size: self.size,
        }
    }

    /// Scales the extent, keeping [`Rect::min`] fixed.
    pub fn scaled_size(&self, factor: Vec2) -> Self {
        Self {
            min: self.min,
            size: self.size * factor.abs(),
        }
    }

    /// Scales the rectangle about its own centre, as a resize gizmo does.
    pub fn scaled_from_center(&self, factor: Vec2) -> Self {
        Self::from_center_size(self.center(), self.size * factor.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::Rect;
    use glam::Vec2;

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    #[test]
    fn from_min_max_accepts_corners_in_any_order() {
        let forward = Rect::from_min_max(v(1.0, 2.0), v(4.0, 6.0));
        let reversed = Rect::from_min_max(v(4.0, 6.0), v(1.0, 2.0));

        assert_eq!(forward, reversed);
        assert_eq!(forward.min, v(1.0, 2.0));
        assert_eq!(forward.size, v(3.0, 4.0));
    }

    #[test]
    fn negative_size_is_folded_into_the_corner() {
        let rect = Rect::from_min_size(v(5.0, 5.0), v(-2.0, -3.0));

        assert_eq!(rect.min, v(3.0, 2.0));
        assert_eq!(rect.max(), v(5.0, 5.0));
    }

    #[test]
    fn from_center_size_is_symmetric() {
        let rect = Rect::from_center_size(v(10.0, 10.0), v(4.0, 2.0));

        assert_eq!(rect.min, v(8.0, 9.0));
        assert_eq!(rect.max(), v(12.0, 11.0));
        assert_eq!(rect.center(), v(10.0, 10.0));
    }

    #[test]
    fn contains_is_half_open_on_the_max_edges() {
        let rect = Rect::from_min_max(v(0.0, 0.0), v(10.0, 10.0));

        assert!(rect.contains(v(0.0, 0.0)), "the min corner is inside");
        assert!(rect.contains(v(5.0, 5.0)), "the interior is inside");
        assert!(!rect.contains(v(10.0, 5.0)), "the max x edge is outside");
        assert!(!rect.contains(v(5.0, 10.0)), "the max y edge is outside");
        assert!(
            !rect.contains(v(-0.001, 5.0)),
            "left of the rect is outside"
        );
    }

    #[test]
    fn intersection_of_touching_edges_is_empty() {
        let left = Rect::from_min_max(v(0.0, 0.0), v(10.0, 10.0));
        let right = Rect::from_min_max(v(10.0, 0.0), v(20.0, 10.0));

        assert!(left.intersection(&right).is_none());
        assert!(!left.intersects(&right));
    }

    #[test]
    fn intersection_reports_the_overlap() {
        let a = Rect::from_min_max(v(0.0, 0.0), v(10.0, 10.0));
        let b = Rect::from_min_max(v(6.0, 8.0), v(20.0, 20.0));

        let overlap = a.intersection(&b).expect("rectangles overlap");
        assert_eq!(overlap.min, v(6.0, 8.0));
        assert_eq!(overlap.size, v(4.0, 2.0));
        assert_eq!(a.intersection(&b), b.intersection(&a));
    }

    #[test]
    fn union_covers_both_operands() {
        let a = Rect::from_min_max(v(0.0, 0.0), v(2.0, 2.0));
        let b = Rect::from_min_max(v(5.0, 1.0), v(6.0, 9.0));

        let union = a.union(&b);
        assert_eq!(union.min, v(0.0, 0.0));
        assert_eq!(union.max(), v(6.0, 9.0));
        assert!(union.contains(a.min) && union.contains(b.min));
    }

    #[test]
    fn empty_rectangles_cover_nothing() {
        let degenerate = Rect::from_min_max(v(3.0, 3.0), v(3.0, 7.0));

        assert!(degenerate.is_empty());
        assert_eq!(degenerate.area(), 0.0);
        assert!(!degenerate.contains(v(3.0, 5.0)));
    }

    #[test]
    fn scaling_keeps_the_anchors_the_caller_expects() {
        let rect = Rect::from_min_max(v(2.0, 2.0), v(6.0, 6.0));

        assert_eq!(rect.scaled_size(v(2.0, 2.0)).min, v(2.0, 2.0));
        assert_eq!(rect.scaled_size(v(2.0, 2.0)).size, v(8.0, 8.0));
        assert_eq!(rect.scaled_from_center(v(2.0, 2.0)).center(), rect.center());
    }
}
