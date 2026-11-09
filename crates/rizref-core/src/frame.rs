//! The selection frame and its eight scale handles plus the rotate handle.
//!
//! A single selected item shows its own oriented frame; a multi-item selection
//! shows the axis-aligned union. The frame lives in world units; the app
//! converts handle offsets to screen pixels before hit-testing so the handles
//! stay the same size at 5% and at 3200% zoom.
//!
//! Scaling is defined here rather than in the interaction code so the rules —
//! what the pivot is, what shift does, how a multi-selection is scaled — are
//! unit-tested without a window.

use glam::Vec2;

use crate::geometry::Rect;
use crate::item::Item;
use crate::scene::Scene;
use crate::selection::Selection;
use crate::transform::{MIN_ITEM_SIZE, Transform};

/// A handle on the selection frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Handle {
    /// North-west corner.
    Nw,
    /// North edge.
    N,
    /// North-east corner.
    Ne,
    /// East edge.
    E,
    /// South-east corner.
    Se,
    /// South edge.
    S,
    /// South-west corner.
    Sw,
    /// West edge.
    W,
    /// The rotate handle above the north edge.
    Rotate,
}

impl Handle {
    /// The eight scale handles, standard order.
    pub const SCALE: [Handle; 8] = [
        Handle::Nw,
        Handle::N,
        Handle::Ne,
        Handle::E,
        Handle::Se,
        Handle::S,
        Handle::Sw,
        Handle::W,
    ];

    /// Direction of the handle in frame space, `(0, 0)` for the rotate handle.
    pub const fn signs(self) -> (i32, i32) {
        match self {
            Handle::Nw => (-1, -1),
            Handle::N => (0, -1),
            Handle::Ne => (1, -1),
            Handle::E => (1, 0),
            Handle::Se => (1, 1),
            Handle::S => (0, 1),
            Handle::Sw => (-1, 1),
            Handle::W => (-1, 0),
            Handle::Rotate => (0, 0),
        }
    }

    /// Whether this is one of the four corner handles.
    pub const fn is_corner(self) -> bool {
        matches!(self, Handle::Nw | Handle::Ne | Handle::Se | Handle::Sw)
    }

    /// Whether this is one of the eight scale handles.
    pub const fn is_scale(self) -> bool {
        !matches!(self, Handle::Rotate)
    }

    /// The opposite scale handle.
    pub const fn opposite(self) -> Option<Handle> {
        match self {
            Handle::Nw => Some(Handle::Se),
            Handle::N => Some(Handle::S),
            Handle::Ne => Some(Handle::Sw),
            Handle::E => Some(Handle::W),
            Handle::Se => Some(Handle::Nw),
            Handle::S => Some(Handle::N),
            Handle::Sw => Some(Handle::Ne),
            Handle::W => Some(Handle::E),
            Handle::Rotate => None,
        }
    }
}

/// What scaling keeps fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pivot {
    /// Scale about the selection centre; the dragged handle still follows the
    /// pointer, and the opposite side moves symmetrically (alt).
    Center,
    /// Scale about the opposite corner or edge, the classic canvas behaviour.
    /// The default.
    Opposite,
}

/// How a scale drag behaves for the held modifiers and what is selected.
///
/// The opposite corner/edge stays put unless `alt` scales about the centre.
/// A corner drag keeps the aspect ratio of images (a photo is never meant to
/// be stretched) and `shift` frees it; notes are free text cards, so a corner
/// drag resizes them freely and `shift` locks the ratio. Edge drags change
/// one axis, except that `shift` scales both from an edge too.
pub fn scale_rules(handle: Handle, alt: bool, shift: bool, has_note: bool) -> (bool, Pivot) {
    let pivot = if alt { Pivot::Center } else { Pivot::Opposite };
    let keep_aspect = if handle.is_corner() && !has_note {
        !shift
    } else {
        shift
    };
    (keep_aspect, pivot)
}

/// An oriented box around the selection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionFrame {
    /// Centre in world units.
    pub center: Vec2,
    /// Extent in world units.
    pub size: Vec2,
    /// Rotation in radians.
    pub rotation: f32,
}

impl SelectionFrame {
    /// A frame from its parts.
    pub fn new(center: Vec2, size: Vec2, rotation: f32) -> Self {
        Self {
            center,
            size: size.abs(),
            rotation,
        }
    }

    /// The frame of one item: its own oriented box.
    pub fn of_item(item: &Item) -> Self {
        Self {
            center: item.center(),
            size: item.size(),
            rotation: item.rotation(),
        }
    }

    /// The frame for the current selection.
    ///
    /// One item keeps its orientation. Several items use the axis-aligned
    /// union, because a rotated frame around unrelated items would imply an
    /// orientation the selection does not have.
    pub fn for_selection(scene: &Scene, selection: &Selection) -> Option<Self> {
        let first = selection.iter().next()?;
        let item = scene.item(first)?;
        if selection.len() == 1 {
            return Some(Self::of_item(item));
        }
        let mut union = item.world_rect();
        for id in selection.iter() {
            if let Some(item) = scene.item(id) {
                union = union.union(&item.world_rect());
            }
        }
        Some(Self::new(union.center(), union.size, 0.0))
    }

    /// The axis-aligned world rectangle the frame sweeps out.
    pub fn world_rect(&self) -> Rect {
        let (sin, cos) = self.rotation.sin_cos();
        let (half_w, half_h) = (self.size.x * 0.5, self.size.y * 0.5);
        let extent_x = (cos * half_w).abs() + (sin * half_h).abs();
        let extent_y = (sin * half_w).abs() + (cos * half_h).abs();
        Rect::from_center_size(self.center, Vec2::new(2.0 * extent_x, 2.0 * extent_y))
    }

    /// The four corners in world space, clockwise from north-west.
    pub fn corners(&self) -> [Vec2; 4] {
        let half = self.size * 0.5;
        let (sin, cos) = self.rotation.sin_cos();
        [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(x, y)| {
            let local = Vec2::new(x as f32 * half.x, y as f32 * half.y);
            self.center + Vec2::new(cos * local.x - sin * local.y, sin * local.x + cos * local.y)
        })
    }

    /// A handle's offset from the centre in frame space.
    ///
    /// `rotate_offset` is the gap between the north edge and the rotate handle,
    /// in world units; the app passes `pixels / zoom` so it is screen-constant.
    pub fn handle_local(&self, handle: Handle, rotate_offset: f32) -> Vec2 {
        let half = self.size * 0.5;
        if handle == Handle::Rotate {
            return Vec2::new(0.0, -half.y - rotate_offset.max(0.0));
        }
        let (sx, sy) = handle.signs();
        Vec2::new(sx as f32 * half.x, sy as f32 * half.y)
    }

    /// A handle's world position.
    pub fn handle_position(&self, handle: Handle, rotate_offset: f32) -> Vec2 {
        let local = self.handle_local(handle, rotate_offset);
        let (sin, cos) = self.rotation.sin_cos();
        self.center + Vec2::new(cos * local.x - sin * local.y, sin * local.x + cos * local.y)
    }

    /// The scale or rotate handle within `radius` world units of `point`.
    ///
    /// The rotate handle wins ties by being tested first: it sits away from the
    /// frame, so nothing else is usually near it.
    pub fn hit(&self, point: Vec2, radius: f32, rotate_offset: f32) -> Option<Handle> {
        let rotate = self.handle_position(Handle::Rotate, rotate_offset);
        if (point - rotate).length() <= radius {
            return Some(Handle::Rotate);
        }
        let mut best: Option<(f32, Handle)> = None;
        for handle in Handle::SCALE {
            let distance = (point - self.handle_position(handle, rotate_offset)).length();
            if distance <= radius && best.is_none_or(|(closest, _)| distance < closest) {
                best = Some((distance, handle));
            }
        }
        best.map(|(_, handle)| handle)
    }

    /// Scales the frame by dragging `handle` to `pointer`.
    pub fn scaled(&self, handle: Handle, pointer: Vec2, keep_aspect: bool, pivot: Pivot) -> Self {
        scale(
            self.center,
            self.size,
            self.rotation,
            handle,
            pointer,
            keep_aspect,
            pivot,
        )
    }
}

/// Scales an item's transform by dragging `handle` to `pointer`.
pub fn scale_item(
    transform: Transform,
    handle: Handle,
    pointer: Vec2,
    keep_aspect: bool,
    pivot: Pivot,
) -> Transform {
    let scaled = scale(
        transform.center(),
        transform.size(),
        transform.rotation(),
        handle,
        pointer,
        keep_aspect,
        pivot,
    );
    Transform::new(scaled.center, scaled.size).with_rotation(scaled.rotation)
}

/// The shared scaling rule for a frame and for one item.
fn scale(
    center: Vec2,
    size: Vec2,
    rotation: f32,
    handle: Handle,
    pointer: Vec2,
    keep_aspect: bool,
    pivot: Pivot,
) -> SelectionFrame {
    let half = size * 0.5;
    let (sin, cos) = rotation.sin_cos();
    let offset = pointer - center;
    let local = Vec2::new(
        cos * offset.x + sin * offset.y,
        -sin * offset.x + cos * offset.y,
    );
    let (sx, sy) = handle.signs();
    let (mut new_hw, mut new_hh) = (half.x, half.y);

    match pivot {
        Pivot::Center => {
            if sx != 0 {
                new_hw = local.x.abs();
            }
            if sy != 0 {
                new_hh = local.y.abs();
            }
            if keep_aspect {
                let ratio = aspect_ratio(new_hw, new_hh, half, sx, sy);
                new_hw = half.x * ratio;
                new_hh = half.y * ratio;
            }
        }
        Pivot::Opposite => {
            let anchor = Vec2::new(-sx as f32 * half.x, -sy as f32 * half.y);
            if sx != 0 {
                new_hw = ((local.x - anchor.x) * sx as f32).abs() * 0.5;
            }
            if sy != 0 {
                new_hh = ((local.y - anchor.y) * sy as f32).abs() * 0.5;
            }
            if keep_aspect {
                let ratio = aspect_ratio(new_hw, new_hh, half, sx, sy);
                new_hw = half.x * ratio;
                new_hh = half.y * ratio;
            }
        }
    }

    let min_half = MIN_ITEM_SIZE * 0.5;
    new_hw = new_hw.max(min_half);
    new_hh = new_hh.max(min_half);

    let new_center_local = match pivot {
        Pivot::Center => Vec2::ZERO,
        Pivot::Opposite => Vec2::new(
            if sx == 0 {
                0.0
            } else {
                (-sx as f32 * half.x) + sx as f32 * new_hw
            },
            if sy == 0 {
                0.0
            } else {
                (-sy as f32 * half.y) + sy as f32 * new_hh
            },
        ),
    };
    let new_center = center
        + Vec2::new(
            cos * new_center_local.x - sin * new_center_local.y,
            sin * new_center_local.x + cos * new_center_local.y,
        );

    SelectionFrame {
        center: new_center,
        size: Vec2::new(2.0 * new_hw, 2.0 * new_hh),
        rotation,
    }
}

/// Uniform scale factor for shift-scaling, taken from the most-changed axis.
fn aspect_ratio(new_hw: f32, new_hh: f32, half: Vec2, sx: i32, sy: i32) -> f32 {
    let ratio_x = if sx != 0 && half.x > 0.0 {
        new_hw / half.x
    } else {
        0.0
    };
    let ratio_y = if sy != 0 && half.y > 0.0 {
        new_hh / half.y
    } else {
        0.0
    };
    ratio_x.max(ratio_y).max(0.001)
}

/// Maps one item from the old selection frame to a new one.
///
/// The frame is axis-aligned for a multi-selection, so the item's centre scales
/// with the frame and its size takes the frame's factors expressed in the
/// item's own axes. A rotated item therefore stays a rectangle: the shear a
/// non-uniform world scale would imply is dropped deliberately.
pub fn scale_item_in_frame(
    transform: Transform,
    old: SelectionFrame,
    new: SelectionFrame,
) -> Transform {
    let fx = if old.size.x.abs() > f32::EPSILON {
        new.size.x / old.size.x
    } else {
        1.0
    };
    let fy = if old.size.y.abs() > f32::EPSILON {
        new.size.y / old.size.y
    } else {
        1.0
    };
    let offset = transform.center() - old.center;
    let center = new.center + Vec2::new(offset.x * fx, offset.y * fy);

    let (sin, cos) = transform.rotation().sin_cos();
    let local_x = fx * cos * cos + fy * sin * sin;
    let local_y = fx * sin * sin + fy * cos * cos;
    let size = (transform.size() * Vec2::new(local_x, local_y))
        .abs()
        .max(Vec2::splat(MIN_ITEM_SIZE));

    Transform::new(center, size).with_rotation(transform.rotation())
}

#[cfg(test)]
mod tests {
    use super::{Handle, Pivot, SelectionFrame, scale_item, scale_item_in_frame, scale_rules};

    #[test]
    fn the_opposite_corner_is_the_default_pivot_and_alt_uses_the_centre() {
        assert_eq!(
            scale_rules(Handle::Se, false, false, false).1,
            Pivot::Opposite
        );
        assert_eq!(
            scale_rules(Handle::E, false, false, true).1,
            Pivot::Opposite
        );
        assert_eq!(scale_rules(Handle::Se, true, false, false).1, Pivot::Center);
    }

    #[test]
    fn image_corners_keep_the_aspect_and_notes_resize_freely() {
        // Images: corners locked, shift frees; edges free, shift locks.
        assert!(scale_rules(Handle::Nw, false, false, false).0);
        assert!(!scale_rules(Handle::Nw, false, true, false).0);
        assert!(!scale_rules(Handle::N, false, false, false).0);
        assert!(scale_rules(Handle::N, false, true, false).0);
        // Notes: free everywhere, shift locks.
        assert!(!scale_rules(Handle::Se, false, false, true).0);
        assert!(scale_rules(Handle::Se, false, true, true).0);
    }

    #[test]
    fn a_default_corner_drag_on_an_image_keeps_its_aspect_exactly() {
        // The manual-test case: a 4000 x 3000 image dragged far and askew
        // from its south-east corner must stay exactly 4:3, with its
        // north-west corner fixed.
        let transform = Transform::new(Vec2::new(2000.0, 1500.0), Vec2::new(4000.0, 3000.0));
        let (keep_aspect, pivot) = scale_rules(Handle::Se, false, false, false);

        let scaled = scale_item(
            transform,
            Handle::Se,
            Vec2::new(6975.0, 5198.0),
            keep_aspect,
            pivot,
        );

        let size = scaled.size();
        assert!((size.x / size.y - 4.0 / 3.0).abs() < 1e-5, "{size}");
        let nw = scaled.center() - size * 0.5;
        assert!(nw.length() < 1e-2, "the opposite corner moved: {nw}");
    }
    use crate::scene::{Scene, SceneId};
    use crate::selection::Selection;
    use crate::transform::{MIN_ITEM_SIZE, Transform};
    use glam::Vec2;
    use std::f32::consts::FRAC_PI_2;

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1.0e-3
    }

    fn frame() -> SelectionFrame {
        SelectionFrame::new(Vec2::new(100.0, 50.0), Vec2::new(80.0, 40.0), 0.0)
    }

    #[test]
    fn a_multi_selection_frame_is_the_axis_aligned_union() {
        let mut scene = Scene::new(SceneId::from_bytes([1; 16]));
        let a = scene.insert_note("a", Vec2::ZERO, Vec2::splat(20.0));
        let b = scene.insert_note("b", Vec2::new(100.0, 0.0), Vec2::splat(20.0));
        let selection: Selection = [a, b].into_iter().collect();

        let frame = SelectionFrame::for_selection(&scene, &selection).expect("two items");

        assert!(close(frame.center, Vec2::new(50.0, 0.0)));
        assert!(close(frame.size, Vec2::new(120.0, 20.0)));
        assert_eq!(frame.rotation, 0.0);
    }

    #[test]
    fn a_single_selection_frame_keeps_the_item_rotation() {
        let mut scene = Scene::new(SceneId::from_bytes([2; 16]));
        let a = scene.insert_note("a", Vec2::new(10.0, 20.0), Vec2::new(30.0, 10.0));
        if let Some(item) = scene.item_mut(a) {
            item.set_transform(
                Transform::new(Vec2::new(10.0, 20.0), Vec2::new(30.0, 10.0)).with_rotation(0.4),
            );
        }
        let selection: Selection = [a].into_iter().collect();

        let frame = SelectionFrame::for_selection(&scene, &selection).expect("one item");

        assert!((frame.rotation - 0.4).abs() < 1.0e-6);
        assert!(close(frame.center, Vec2::new(10.0, 20.0)));
    }

    #[test]
    fn handles_sit_on_the_corners_edges_and_above_the_top() {
        let frame = frame();

        assert!(close(
            frame.handle_position(Handle::Nw, 20.0),
            Vec2::new(60.0, 30.0)
        ));
        assert!(close(
            frame.handle_position(Handle::E, 20.0),
            Vec2::new(140.0, 50.0)
        ));
        assert!(close(
            frame.handle_position(Handle::Rotate, 20.0),
            Vec2::new(100.0, 10.0)
        ));
    }

    #[test]
    fn handles_rotate_with_the_frame() {
        let frame = SelectionFrame::new(Vec2::ZERO, Vec2::splat(20.0), FRAC_PI_2);

        // The west handle of a quarter-turned square ends up above the centre.
        assert!(close(
            frame.handle_position(Handle::W, 0.0),
            Vec2::new(0.0, -10.0)
        ));
    }

    #[test]
    fn hit_prefers_the_rotate_handle_and_reports_the_nearest_scale_handle() {
        let frame = frame();

        assert_eq!(
            frame.hit(Vec2::new(100.0, 10.0), 6.0, 20.0),
            Some(Handle::Rotate)
        );
        assert_eq!(
            frame.hit(Vec2::new(62.0, 31.0), 6.0, 20.0),
            Some(Handle::Nw)
        );
        assert_eq!(frame.hit(Vec2::new(0.0, 0.0), 6.0, 20.0), None);
    }

    #[test]
    fn centre_scaling_keeps_the_centre_and_follows_the_pointer() {
        let frame = frame();

        let scaled = frame.scaled(Handle::Se, Vec2::new(140.0, 70.0), false, Pivot::Center);

        assert!(close(scaled.center, frame.center));
        assert!(close(scaled.size, Vec2::new(80.0, 40.0)));
        assert!(close(
            scaled.handle_position(Handle::Se, 20.0),
            Vec2::new(140.0, 70.0)
        ));
    }

    #[test]
    fn opposite_scaling_anchors_the_opposite_corner() {
        let frame = frame();

        let scaled = frame.scaled(Handle::Se, Vec2::new(180.0, 90.0), false, Pivot::Opposite);

        assert!(close(
            scaled.handle_position(Handle::Nw, 20.0),
            Vec2::new(60.0, 30.0)
        ));
        assert!(close(
            scaled.handle_position(Handle::Se, 20.0),
            Vec2::new(180.0, 90.0)
        ));
        assert!(close(scaled.size, Vec2::new(120.0, 60.0)));
    }

    #[test]
    fn edge_scaling_changes_one_axis_only() {
        let frame = frame();

        let scaled = frame.scaled(Handle::E, Vec2::new(160.0, 50.0), false, Pivot::Opposite);

        assert!(close(
            scaled.handle_position(Handle::W, 20.0),
            Vec2::new(60.0, 50.0)
        ));
        assert!((scaled.size.y - 40.0).abs() < 1.0e-4);
        assert!((scaled.size.x - 100.0).abs() < 1.0e-4);
    }

    #[test]
    fn shift_keeps_the_aspect_ratio() {
        let frame = frame();
        let aspect = frame.size.x / frame.size.y;

        let scaled = frame.scaled(Handle::Se, Vec2::new(300.0, 80.0), true, Pivot::Center);

        assert!((scaled.size.x / scaled.size.y - aspect).abs() < 1.0e-4);
    }

    #[test]
    fn scaling_clamps_to_the_minimum_size() {
        let frame = frame();

        let scaled = frame.scaled(Handle::Se, frame.center, false, Pivot::Center);

        assert_eq!(scaled.size, Vec2::splat(MIN_ITEM_SIZE));
    }

    #[test]
    fn a_rotated_item_scales_in_its_own_frame() {
        let transform = Transform::new(Vec2::ZERO, Vec2::new(100.0, 50.0)).with_rotation(FRAC_PI_2);
        let frame = SelectionFrame::new(transform.center(), transform.size(), transform.rotation());

        // Dragging the east handle of the quarter-turned item moves along the
        // item's local x, which points down the screen.
        let scaled = scale_item(
            transform,
            Handle::E,
            Vec2::new(0.0, 75.0),
            false,
            Pivot::Center,
        );

        assert!(close(scaled.size(), Vec2::new(150.0, 50.0)));
        assert!((scaled.rotation() - FRAC_PI_2).abs() < 1.0e-4);
        let _ = frame;
    }

    #[test]
    fn multi_item_scaling_is_exact_for_unrotated_items() {
        let old = SelectionFrame::new(Vec2::ZERO, Vec2::new(100.0, 100.0), 0.0);
        let new = SelectionFrame::new(Vec2::ZERO, Vec2::new(200.0, 50.0), 0.0);
        let item = Transform::new(Vec2::new(30.0, 20.0), Vec2::splat(20.0));

        let scaled = scale_item_in_frame(item, old, new);

        assert!(close(scaled.center(), Vec2::new(60.0, 10.0)));
        assert!(close(scaled.size(), Vec2::new(40.0, 10.0)));
    }

    #[test]
    fn multi_item_scaling_keeps_rotated_items_rectangular() {
        let old = SelectionFrame::new(Vec2::ZERO, Vec2::new(100.0, 100.0), 0.0);
        let new = SelectionFrame::new(Vec2::ZERO, Vec2::new(200.0, 500.0), 0.0);
        let item = Transform::new(Vec2::new(10.0, 0.0), Vec2::splat(10.0)).with_rotation(0.3);

        let scaled = scale_item_in_frame(item, old, new);

        assert!((scaled.rotation() - 0.3).abs() < 1.0e-6);
        assert!(scaled.size().x > 0.0 && scaled.size().y > 0.0);
    }
}
