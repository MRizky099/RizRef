//! Edge and centre snapping with smart guides.
//!
//! While items are dragged, their bounding rectangle is offered to this engine
//! together with the rectangles it may snap to — every unselected item and the
//! scene rect. The engine returns the smallest correction that puts one of the
//! moving edges or centres onto a target line, plus the guides to draw.
//!
//! Tolerance is passed in world units: the app divides its 3-pixel screen
//! tolerance by the zoom, so snapping feels the same at 5% and at 3200%. The
//! correction never overshoots: it lands exactly on the target line, and a
//! candidate farther than the tolerance is ignored rather than half-applied.

use glam::Vec2;

use crate::geometry::Rect;

/// Axis a guide is perpendicular to.
///
/// `Axis::X` means a vertical guide at a constant world x; `Axis::Y` means a
/// horizontal guide at a constant world y.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Vertical guide line.
    X,
    /// Horizontal guide line.
    Y,
}

/// One guide line to draw while dragging.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guide {
    /// Whether the line is vertical or horizontal.
    pub axis: Axis,
    /// Constant coordinate of the line.
    pub position: f32,
    /// Start of the drawn span along the other axis.
    pub start: f32,
    /// End of the drawn span along the other axis.
    pub end: f32,
}

/// The correction to apply and the guides it produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnapResult {
    /// World-space translation that lands the moving rectangle on its guides.
    pub offset: Vec2,
    /// Guide lines to draw, at most one per axis.
    pub guides: Vec<Guide>,
}

impl SnapResult {
    /// Whether anything snapped.
    pub fn is_snapped(&self) -> bool {
        self.offset != Vec2::ZERO || !self.guides.is_empty()
    }
}

/// One axis' winning alignment.
#[derive(Debug, Clone, Copy)]
struct Match {
    delta: f32,
    line: f32,
    span: (f32, f32),
}

/// Snaps `moving` to the edges and centres of `targets`.
///
/// `tolerance` is in world units. The returned offset is the translation to add
/// to `moving`; guides describe where the alignment landed.
pub fn snap_rect(moving: Rect, targets: &[Rect], tolerance: f32) -> SnapResult {
    if tolerance <= 0.0 || moving.is_empty() {
        return SnapResult::default();
    }
    let mut result = SnapResult::default();

    if let Some(best) = best_on_axis(moving, targets, Axis::X, tolerance) {
        result.offset.x = best.delta;
        result.guides.push(Guide {
            axis: Axis::X,
            position: best.line,
            start: best.span.0,
            end: best.span.1,
        });
    }
    if let Some(best) = best_on_axis(moving, targets, Axis::Y, tolerance) {
        result.offset.y = best.delta;
        result.guides.push(Guide {
            axis: Axis::Y,
            position: best.line,
            start: best.span.0,
            end: best.span.1,
        });
    }
    result
}

/// The closest alignment of `moving`'s three reference lines to a target's
/// three reference lines on one axis.
fn best_on_axis(moving: Rect, targets: &[Rect], axis: Axis, tolerance: f32) -> Option<Match> {
    let moving_lines = reference_lines(moving, axis);
    let mut best: Option<Match> = None;
    for target in targets {
        for line in reference_lines(*target, axis) {
            for moving_line in moving_lines {
                let delta = line - moving_line;
                if delta.abs() > tolerance {
                    continue;
                }
                if best.is_none_or(|current| delta.abs() < current.delta.abs()) {
                    best = Some(Match {
                        delta,
                        line,
                        span: guide_span(moving, *target, axis),
                    });
                }
            }
        }
    }
    best
}

/// The min, centre and max of a rectangle along one axis.
fn reference_lines(rect: Rect, axis: Axis) -> [f32; 3] {
    let (min, max) = match axis {
        Axis::X => (rect.min.x, rect.max().x),
        Axis::Y => (rect.min.y, rect.max().y),
    };
    [min, (min + max) * 0.5, max]
}

/// How far a guide has to be drawn to touch both rectangles.
fn guide_span(moving: Rect, target: Rect, axis: Axis) -> (f32, f32) {
    match axis {
        // A vertical guide spans y.
        Axis::X => (
            moving.min.y.min(target.min.y),
            moving.max().y.max(target.max().y),
        ),
        Axis::Y => (
            moving.min.x.min(target.min.x),
            moving.max().x.max(target.max().x),
        ),
    }
}

/// The world tolerance that corresponds to `pixels` screen pixels at `zoom`.
pub fn screen_tolerance(pixels: f32, zoom: f64) -> f32 {
    if zoom <= 0.0 {
        return 0.0;
    }
    pixels / zoom as f32
}

#[cfg(test)]
mod tests {
    use super::{Axis, screen_tolerance, snap_rect};
    use crate::geometry::Rect;
    use glam::Vec2;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Vec2::new(x, y), Vec2::new(w, h))
    }

    #[test]
    fn an_edge_snaps_exactly_onto_the_target_edge() {
        let moving = rect(0.0, 0.0, 10.0, 10.0);
        let target = rect(12.0, 40.0, 10.0, 10.0);

        let result = snap_rect(moving, &[target], 3.0);

        assert_eq!(
            result.offset.x, 2.0,
            "the right edge lands on the left edge"
        );
        assert_eq!(result.offset.y, 0.0, "nothing within tolerance on y");
        assert!(
            result
                .guides
                .iter()
                .any(|guide| guide.axis == Axis::X && (guide.position - 12.0).abs() < 1.0e-6)
        );
    }

    #[test]
    fn centres_snap_to_each_other() {
        let moving = rect(0.0, 0.0, 10.0, 10.0);
        // A wide target whose edges are far away but whose centre is 0.4 from
        // the moving centre: only the centre alignment is inside tolerance.
        let target = rect(-4.6, 0.0, 20.0, 10.0);

        let result = snap_rect(moving, &[target], 0.5);

        assert!((result.offset.x - 0.4).abs() < 1.0e-6);
        assert!(
            result
                .guides
                .iter()
                .any(|guide| guide.axis == Axis::X && (guide.position - 5.4).abs() < 1.0e-6)
        );
    }

    #[test]
    fn a_gap_outside_the_tolerance_is_left_alone() {
        let moving = rect(0.0, 0.0, 10.0, 10.0);
        let target = rect(10.0 + 3.01, 100.0, 10.0, 10.0);

        let result = snap_rect(moving, &[target], 3.0);

        assert_eq!(result.offset.x, 0.0, "nothing within tolerance on x");
        assert!(
            !result.guides.iter().any(|guide| guide.axis == Axis::X),
            "and no x guide is drawn"
        );
    }

    #[test]
    fn a_correction_never_overshoots_at_any_zoom() {
        let target = rect(1000.0, 1000.0, 200.0, 200.0);
        let moving = rect(199.0, 500.0, 100.0, 100.0);

        for zoom in [0.05, 0.1, 0.5, 1.0, 4.0, 8.0, 32.0] {
            let tolerance = screen_tolerance(3.0, zoom);
            let result = snap_rect(moving, &[target], tolerance);
            let snapped = moving.translated(result.offset);
            if result.offset.x != 0.0 {
                assert!(
                    (snapped.max().x - 1000.0).abs() < 1.0e-3,
                    "at {zoom:.2}x the right edge landed on the target edge"
                );
                assert!(
                    result.offset.x.abs() <= tolerance + 1.0e-4,
                    "and the correction stayed inside the tolerance"
                );
            } else {
                assert!(
                    (1000.0 - moving.max().x).abs() > tolerance,
                    "no correction means the gap was outside the tolerance at {zoom:.2}x"
                );
            }
        }
    }

    #[test]
    fn tolerance_shrinks_as_the_view_zooms_in() {
        assert_eq!(screen_tolerance(3.0, 0.05), 60.0);
        assert!((screen_tolerance(3.0, 32.0) - 0.09375).abs() < 1.0e-6);

        // At 5% a 50-unit gap snaps; at 3200% the same gap is far away.
        let moving = rect(0.0, 0.0, 10.0, 10.0);
        let target = rect(60.0, 0.0, 10.0, 10.0);
        assert_eq!(
            snap_rect(moving, &[target], screen_tolerance(3.0, 0.05))
                .offset
                .x,
            50.0
        );
        assert_eq!(
            snap_rect(moving, &[target], screen_tolerance(3.0, 32.0))
                .offset
                .x,
            0.0
        );
    }

    #[test]
    fn the_closest_of_several_candidates_wins() {
        let moving = rect(0.0, 0.0, 10.0, 10.0);
        let near = rect(11.0, 100.0, 10.0, 10.0);
        let nearer = rect(12.5, 200.0, 10.0, 10.0);

        // The right edge (10) is 1 from the near edge and 2.5 from the other.
        let result = snap_rect(moving, &[nearer, near], 3.0);
        assert_eq!(result.offset.x, 1.0);
    }

    #[test]
    fn guides_span_both_rectangles() {
        let moving = rect(0.0, 0.0, 10.0, 10.0);
        let target = rect(0.0, 100.0, 10.0, 10.0);

        let result = snap_rect(moving, &[target], 1.0);
        let guide = result
            .guides
            .iter()
            .find(|guide| guide.axis == Axis::X)
            .expect("the left edges align");

        assert_eq!(guide.start, 0.0);
        assert_eq!(guide.end, 110.0);
    }

    #[test]
    fn a_zero_tolerance_snaps_nothing() {
        let moving = rect(0.0, 0.0, 10.0, 10.0);
        let target = rect(0.0, 0.0, 10.0, 10.0);

        assert!(!snap_rect(moving, &[target], 0.0).is_snapped());
    }
}
