//! The canvas view transform: the camera the board is seen through.
//!
//! The camera maps an unbounded world plane onto the viewport, optionally
//! rotated. All camera math is `f64`: at 3200% zoom over a board placed tens of
//! thousands of world units from the origin, an `f32` accumulator starts
//! visibly jittering item edges, and the jitter is exactly the kind of bug that
//! only shows up on someone else's board. Narrowing to `f32` happens once, at
//! draw time, relative to the eye — where the values are small again.
//!
//! The mapping is `screen = viewport/2 + R(rotation) · (world − eye) · zoom`
//! with the origin of both spaces in the top-left corner and y pointing down.
//! The viewport size is passed in per call instead of being stored: that way a
//! resize cannot change what the user is looking at, and the camera stays a
//! value type that is cheap to copy, compare, serialise and test.

use glam::{DMat2, DVec2};
use serde::{Deserialize, Serialize};

use crate::geometry::Rect;

/// The view transform of the infinite canvas.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// World-space point drawn at the centre of the viewport.
    pub eye: DVec2,
    /// Viewport pixels per world unit, always inside
    /// [`Camera::MIN_ZOOM`]`..=`[`Camera::MAX_ZOOM`].
    ///
    /// The zoom percentage of the readout is this factor times 100: `1.0` is
    /// 100%, one world unit (one image pixel) per screen pixel.
    pub zoom: f64,
    /// View rotation in radians, the angle the world is turned clockwise on
    /// screen. The viewport centre — [`Camera::eye`] — stays fixed under
    /// rotation.
    pub rotation: f64,
}

impl Camera {
    /// Smallest supported zoom: 5%, one world unit covers 0.05 px.
    pub const MIN_ZOOM: f64 = 0.05;

    /// Largest supported zoom: 3200%, one world unit covers 32 px.
    pub const MAX_ZOOM: f64 = 32.0;

    /// A camera looking at `eye` at `zoom`, turned by `rotation`, with the
    /// zoom clamped to the supported range.
    pub fn new(eye: DVec2, zoom: f64, rotation: f64) -> Self {
        Self {
            eye,
            zoom: Self::clamp_zoom(zoom),
            rotation,
        }
    }

    /// Clamps a zoom factor to the supported range.
    ///
    /// The range is wide but finite on purpose: below it a board collapses into
    /// sub-pixel noise, above it f32 draw precision starts losing item edges.
    pub const fn clamp_zoom(zoom: f64) -> f64 {
        if zoom < Self::MIN_ZOOM {
            Self::MIN_ZOOM
        } else if zoom > Self::MAX_ZOOM {
            Self::MAX_ZOOM
        } else {
            zoom
        }
    }

    /// The 2×2 rotation-scale block of the view transform: world deltas in,
    /// screen deltas out.
    fn view_linear(&self) -> DMat2 {
        DMat2::from_angle(self.rotation) * self.zoom
    }

    /// Maps a world-space point to a viewport pixel.
    pub fn world_to_screen(&self, world: DVec2, viewport: DVec2) -> DVec2 {
        self.view_linear() * (world - self.eye) + viewport * 0.5
    }

    /// Maps a viewport pixel back to world space.
    pub fn screen_to_world(&self, screen: DVec2, viewport: DVec2) -> DVec2 {
        self.view_linear().inverse() * (screen - viewport * 0.5) + self.eye
    }

    /// Multiplies the zoom by `factor` while keeping the world point under
    /// `anchor` (a viewport pixel, normally the mouse cursor) exactly in place.
    ///
    /// This is the "zoom towards the cursor" behaviour of every canvas app; the
    /// anchor stays put even when the requested zoom is clamped.
    pub fn zoom_at(&mut self, anchor: DVec2, factor: f64, viewport: DVec2) {
        let anchored_world = self.screen_to_world(anchor, viewport);
        self.zoom = Self::clamp_zoom(self.zoom * factor);
        self.eye = anchored_world - self.view_linear().inverse() * (anchor - viewport * 0.5);
    }

    /// Pans by a viewport-space delta, so the board follows the cursor
    /// one-to-one regardless of zoom or rotation.
    pub fn pan_by_screen(&mut self, delta: DVec2) {
        self.eye -= self.view_linear().inverse() * delta;
    }

    /// Turns the view by `delta` radians about the viewport centre.
    ///
    /// The viewport centre always maps to [`Camera::eye`], so rotating about
    /// the centre is exactly rotating about the eye: nothing else moves. This
    /// is the alt+wheel behaviour of the reference application; a free rotate
    /// gizmo arrives with the manipulation phase.
    pub fn rotate_by(&mut self, delta: f64) {
        self.rotation += delta;
    }

    /// Sets the rotation about the viewport centre directly.
    pub fn set_rotation(&mut self, rotation: f64) {
        self.rotation = rotation;
    }

    /// A camera that shows all of `rect` with `padding` viewport pixels to
    /// spare, unrotated: the `F` "frame all" behaviour on a board the user has
    /// not turned.
    ///
    /// An empty or degenerate rect yields the default camera rather than
    /// dividing by zero. To fit into a view the user has rotated, use
    /// [`Camera::frame`], which preserves the current rotation.
    pub fn fit_rect(rect: Rect, viewport: DVec2, padding: f64) -> Self {
        let mut camera = Self::default();
        camera.frame(rect, viewport, padding);
        camera
    }

    /// Adjusts `self` to show all of `rect` with `padding` viewport pixels to
    /// spare, keeping the current rotation.
    ///
    /// Under rotation the rectangle sweeps out a larger on-screen box; the fit
    /// uses that rotated bounding box, so the content fits whatever the view
    /// rotation is. Rotating a rectangle about its own centre keeps the
    /// bounding box centred on it, so the eye lands on the rect centre exactly.
    pub fn frame(&mut self, rect: Rect, viewport: DVec2, padding: f64) {
        if rect.is_empty() {
            return;
        }
        let center = rect.center().as_dvec2();
        let linear = DMat2::from_angle(self.rotation);
        let mut min = DVec2::splat(f64::INFINITY);
        let mut max = DVec2::splat(f64::NEG_INFINITY);
        for corner in [
            rect.min.as_dvec2(),
            DVec2::new(f64::from(rect.max().x), f64::from(rect.min.y)),
            DVec2::new(f64::from(rect.min.x), f64::from(rect.max().y)),
            rect.max().as_dvec2(),
        ] {
            let rotated = linear * (corner - center);
            min = min.min(rotated);
            max = max.max(rotated);
        }
        let extent = (max - min).max(DVec2::splat(1.0e-6));
        let available = (viewport - padding * 2.0).max(DVec2::splat(1.0));
        self.zoom = Self::clamp_zoom((available.x / extent.x).min(available.y / extent.y));
        self.eye = center;
    }

    /// The world-space region currently visible in `viewport`, as the
    /// axis-aligned box containing the (possibly rotated) viewport corners.
    pub fn visible_world_rect(&self, viewport: DVec2) -> Rect {
        let corners = [
            self.screen_to_world(DVec2::ZERO, viewport),
            self.screen_to_world(DVec2::new(viewport.x, 0.0), viewport),
            self.screen_to_world(DVec2::new(0.0, viewport.y), viewport),
            self.screen_to_world(viewport, viewport),
        ];
        let min = corners
            .iter()
            .fold(DVec2::splat(f64::INFINITY), |acc, c| acc.min(*c));
        let max = corners
            .iter()
            .fold(DVec2::splat(f64::NEG_INFINITY), |acc, c| acc.max(*c));
        Rect::from_min_max(min.as_vec2(), max.as_vec2())
    }
}

impl Default for Camera {
    /// A camera at the world origin, no rotation, one world unit per viewport
    /// pixel: 100%.
    fn default() -> Self {
        Self {
            eye: DVec2::ZERO,
            zoom: 1.0,
            rotation: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Camera;
    use crate::geometry::Rect;
    use glam::DVec2;

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    fn assert_close(actual: DVec2, expected: DVec2) {
        assert!(
            (actual - expected).length() < 1.0e-9,
            "expected {expected:?}, got {actual:?}"
        );
    }

    #[test]
    fn default_camera_maps_the_centre_to_the_origin() {
        let camera = Camera::default();
        let viewport = v(800.0, 600.0);

        assert_eq!(
            camera.world_to_screen(DVec2::ZERO, viewport),
            v(400.0, 300.0)
        );
        assert_eq!(
            camera.screen_to_world(v(400.0, 300.0), viewport),
            DVec2::ZERO
        );
    }

    #[test]
    fn screen_and_world_round_trip_at_every_rotation() {
        let viewport = v(1920.0, 1080.0);
        let world = v(431.5, 77.25);

        for quarter in 0..8 {
            let rotation = quarter as f64 * std::f64::consts::FRAC_PI_8;
            let camera = Camera::new(v(120.0, -45.0), 3.5, rotation);

            let round_tripped =
                camera.screen_to_world(camera.world_to_screen(world, viewport), viewport);

            assert_close(round_tripped, world);
        }
    }

    #[test]
    fn rotation_turns_the_axes_as_expected() {
        // A quarter turn clockwise on screen (y down): the world +x axis ends
        // up pointing down the viewport.
        let camera = Camera::new(DVec2::ZERO, 1.0, std::f64::consts::FRAC_PI_2);
        let viewport = v(200.0, 100.0);

        let screen = camera.world_to_screen(v(50.0, 0.0), viewport);

        // Centre of the viewport plus the rotated axis: (100, 50) + (0, 50).
        assert_close(screen, v(100.0, 100.0));
    }

    #[test]
    fn zoom_keeps_the_anchor_under_the_cursor() {
        let viewport = v(1000.0, 700.0);
        let anchor = v(250.0, 300.0);
        let mut camera = Camera::new(v(10.0, 20.0), 1.0, 0.3);
        let anchored_world = camera.screen_to_world(anchor, viewport);

        camera.zoom_at(anchor, 2.0, viewport);

        assert!((camera.zoom - 2.0).abs() < 1.0e-12);
        assert_close(camera.screen_to_world(anchor, viewport), anchored_world);
    }

    #[test]
    fn zoom_clamping_also_keeps_the_anchor() {
        let viewport = v(1000.0, 700.0);
        let anchor = v(900.0, 120.0);
        let mut camera = Camera::new(v(0.0, 0.0), Camera::MAX_ZOOM, -0.7);
        let anchored_world = camera.screen_to_world(anchor, viewport);

        camera.zoom_at(anchor, 4.0, viewport);

        assert_eq!(camera.zoom, Camera::MAX_ZOOM);
        assert_close(camera.screen_to_world(anchor, viewport), anchored_world);
    }

    #[test]
    fn zoom_stays_inside_the_supported_range() {
        let mut camera = Camera::default();

        camera.zoom_at(v(50.0, 50.0), 1.0e12, v(100.0, 100.0));
        assert_eq!(camera.zoom, Camera::MAX_ZOOM);

        camera.zoom_at(v(50.0, 50.0), 1.0e-12, v(100.0, 100.0));
        assert_eq!(camera.zoom, Camera::MIN_ZOOM);
    }

    #[test]
    fn the_zoom_range_is_five_percent_to_3200_percent() {
        assert_eq!(Camera::MIN_ZOOM, 0.05);
        assert_eq!(Camera::MAX_ZOOM, 32.0);
        assert_eq!(Camera::clamp_zoom(0.04), Camera::MIN_ZOOM);
        assert_eq!(Camera::clamp_zoom(1.0), 1.0, "100% is inside the range");
        assert_eq!(Camera::clamp_zoom(100.0), Camera::MAX_ZOOM);
    }

    #[test]
    fn panning_follows_the_cursor_through_any_rotation() {
        let viewport = v(800.0, 600.0);
        for quarter in 0..4 {
            let rotation = quarter as f64 * std::f64::consts::FRAC_PI_2;
            let mut camera = Camera::new(DVec2::ZERO, 2.0, rotation);

            // Whatever world point sat under screen x=0 keeps sitting 100 px
            // to the right after a 100 px drag: the board follows the cursor
            // one to one, at every rotation.
            let under_left_edge = camera.screen_to_world(v(0.0, 300.0), viewport);
            camera.pan_by_screen(v(100.0, 0.0));

            assert_close(
                camera.world_to_screen(under_left_edge, viewport),
                v(100.0, 300.0),
            );
        }
    }

    #[test]
    fn rotation_keeps_the_centre_looking_at_the_eye() {
        let viewport = v(600.0, 400.0);
        let mut camera = Camera::new(v(11.0, 22.0), 1.5, 0.25);

        camera.rotate_by(0.75);
        camera.rotate_by(-2.0);

        assert_close(
            camera.screen_to_world(viewport * 0.5, viewport),
            v(11.0, 22.0),
        );
    }

    #[test]
    fn visible_rect_matches_the_viewport_corners_when_unrotated() {
        let viewport = v(800.0, 400.0);
        let camera = Camera::new(DVec2::ZERO, 2.0, 0.0);

        let visible = camera.visible_world_rect(viewport);

        assert_eq!(visible.min, v(-200.0, -100.0).as_vec2());
        assert_eq!(visible.max().as_dvec2(), v(200.0, 100.0));
    }

    #[test]
    fn the_visible_rect_covers_the_rotated_viewport_corners() {
        let viewport = v(400.0, 200.0);
        let camera = Camera::new(DVec2::ZERO, 1.0, std::f64::consts::FRAC_PI_4);

        let visible = camera.visible_world_rect(viewport);

        // The AABB is the box swept by the rotated corners; every corner
        // round-trips onto its boundary. The rect is stored in f32 while the
        // world point is f64, so the tolerance is f32-scaled.
        let min = visible.min.as_dvec2();
        let max = visible.max().as_dvec2();
        for corner in [
            DVec2::ZERO,
            v(viewport.x, 0.0),
            v(0.0, viewport.y),
            viewport,
        ] {
            let world = camera.screen_to_world(corner, viewport);
            assert!(
                world.x >= min.x - 1.0e-3
                    && world.y >= min.y - 1.0e-3
                    && world.x <= max.x + 1.0e-3
                    && world.y <= max.y + 1.0e-3,
                "corner {corner:?} -> {world:?} should be inside {visible:?}"
            );
        }
    }

    #[test]
    fn fit_rect_shows_everything_with_padding() {
        let viewport = v(1000.0, 800.0);
        let rect = Rect::from_min_max(glam::Vec2::ZERO, glam::Vec2::new(400.0, 200.0));

        let camera = Camera::fit_rect(rect, viewport, 50.0);

        // 900/400 = 2.25 on x, 700/200 = 3.5 on y: the x fit wins.
        assert!((camera.zoom - 2.25).abs() < 1.0e-9);
        assert_close(camera.eye, v(200.0, 100.0));
        // Every corner of the rect lands inside the padded viewport.
        let padded = DVec2::splat(50.0);
        for corner in [v(0.0, 0.0), v(400.0, 0.0), v(0.0, 200.0), v(400.0, 200.0)] {
            let screen = camera.world_to_screen(corner, viewport);
            assert!(
                screen.x >= padded.x - 1.0e-9
                    && screen.y >= padded.y - 1.0e-9
                    && screen.x <= viewport.x - padded.x + 1.0e-9
                    && screen.y <= viewport.y - padded.y + 1.0e-9,
                "corner {corner:?} at {screen:?} is outside the padded viewport"
            );
        }
    }

    #[test]
    fn frame_preserves_rotation_and_still_fits() {
        let viewport = v(1000.0, 800.0);
        let rect = Rect::from_min_max(glam::Vec2::ZERO, glam::Vec2::new(400.0, 200.0));

        let mut camera = Camera::new(DVec2::ZERO, 1.0, std::f64::consts::FRAC_PI_4);
        camera.frame(rect, viewport, 50.0);

        assert_eq!(camera.rotation, std::f64::consts::FRAC_PI_4);
        // A 45° turn of a 400 x 200 rect sweeps out a (400+200)/√2 square, so
        // the zoom must be tighter than the unrotated fit of 2.25.
        let unrotated = Camera::fit_rect(rect, viewport, 50.0);
        assert!(
            camera.zoom < unrotated.zoom,
            "rotated fit {} must be tighter than unrotated {}",
            camera.zoom,
            unrotated.zoom
        );
        assert_close(camera.eye, v(200.0, 100.0));
        // And the content still fits: every corner is inside the viewport.
        for corner in [v(0.0, 0.0), v(400.0, 0.0), v(0.0, 200.0), v(400.0, 200.0)] {
            let screen = camera.world_to_screen(corner, viewport);
            assert!(
                screen.x >= 0.0
                    && screen.y >= 0.0
                    && screen.x <= viewport.x
                    && screen.y <= viewport.y,
                "corner {corner:?} at {screen:?} is outside the viewport"
            );
        }
    }

    #[test]
    fn fitting_an_empty_rect_resets_the_camera() {
        let camera = Camera::fit_rect(Rect::ZERO, v(800.0, 600.0), 50.0);

        assert_eq!(camera, Camera::default());
    }
}
