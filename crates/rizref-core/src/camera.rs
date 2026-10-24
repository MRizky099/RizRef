//! The canvas view transform.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::geometry::Rect;

/// The view transform of the infinite canvas.
///
/// The mapping is `screen = (world - center) * zoom + viewport / 2`, with the
/// origin of both spaces in the top-left corner. The viewport size is passed in
/// per call instead of being stored: that way a resize cannot change what the
/// user is looking at, and the camera stays a value type that is cheap to copy,
/// compare and test.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// World-space point drawn at the centre of the viewport.
    pub center: Vec2,
    /// Device pixels per world unit, always inside [`Camera::MIN_ZOOM`]`..=`[`Camera::MAX_ZOOM`].
    pub zoom: f32,
}

impl Camera {
    /// Smallest supported zoom: one world unit covers 0.01 px.
    pub const MIN_ZOOM: f32 = 0.01;

    /// Largest supported zoom: one world unit covers 200 px.
    pub const MAX_ZOOM: f32 = 200.0;

    /// A camera looking at `center` at `zoom`, with the zoom clamped to the supported range.
    pub fn new(center: Vec2, zoom: f32) -> Self {
        Self {
            center,
            zoom: Self::clamp_zoom(zoom),
        }
    }

    /// Clamps a zoom factor to the supported range.
    ///
    /// The range is wide but finite on purpose: below it a board would collapse
    /// into sub-pixel noise, above it f32 precision starts losing item edges.
    pub fn clamp_zoom(zoom: f32) -> f32 {
        zoom.clamp(Self::MIN_ZOOM, Self::MAX_ZOOM)
    }

    /// Maps a world-space point to a viewport pixel.
    pub fn world_to_screen(&self, world: Vec2, viewport: Vec2) -> Vec2 {
        (world - self.center) * self.zoom + viewport * 0.5
    }

    /// Maps a viewport pixel back to world space.
    pub fn screen_to_world(&self, screen: Vec2, viewport: Vec2) -> Vec2 {
        (screen - viewport * 0.5) / self.zoom + self.center
    }

    /// Multiplies the zoom by `factor` while keeping the world point under
    /// `anchor` (a viewport pixel, normally the mouse cursor) exactly in place.
    ///
    /// This is the "zoom towards the cursor" behaviour of every canvas app; the
    /// anchor stays put even when the requested zoom is clamped.
    pub fn zoom_at(&mut self, anchor: Vec2, factor: f32, viewport: Vec2) {
        let anchored_world = self.screen_to_world(anchor, viewport);
        self.zoom = Self::clamp_zoom(self.zoom * factor);
        self.center = anchored_world - (anchor - viewport * 0.5) / self.zoom;
    }

    /// Pans by a viewport-space delta, so the board follows the cursor one-to-one.
    pub fn pan_by_screen(&mut self, delta: Vec2) {
        self.center -= delta / self.zoom;
    }

    /// The world-space region currently visible in `viewport`.
    pub fn visible_world_rect(&self, viewport: Vec2) -> Rect {
        Rect::from_min_size(
            self.screen_to_world(Vec2::ZERO, viewport),
            viewport / self.zoom,
        )
    }
}

impl Default for Camera {
    /// A camera at the world origin showing one world unit per device pixel.
    fn default() -> Self {
        Self {
            center: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Camera;
    use glam::Vec2;

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    fn assert_close(actual: Vec2, expected: Vec2) {
        assert!(
            (actual - expected).length() < 1.0e-3,
            "expected {expected:?}, got {actual:?}"
        );
    }

    #[test]
    fn default_camera_maps_the_centre_to_the_origin() {
        let camera = Camera::default();
        let viewport = v(800.0, 600.0);

        assert_eq!(
            camera.world_to_screen(Vec2::ZERO, viewport),
            v(400.0, 300.0)
        );
        assert_eq!(
            camera.screen_to_world(v(400.0, 300.0), viewport),
            Vec2::ZERO
        );
    }

    #[test]
    fn screen_and_world_round_trip() {
        let camera = Camera::new(v(120.0, -45.0), 3.5);
        let viewport = v(1920.0, 1080.0);
        let world = v(431.5, 77.25);

        let round_tripped =
            camera.screen_to_world(camera.world_to_screen(world, viewport), viewport);

        assert_close(round_tripped, world);
    }

    #[test]
    fn zoom_keeps_the_anchor_under_the_cursor() {
        let viewport = v(1000.0, 700.0);
        let anchor = v(250.0, 300.0);
        let mut camera = Camera::new(v(10.0, 20.0), 1.0);
        let anchored_world = camera.screen_to_world(anchor, viewport);

        camera.zoom_at(anchor, 2.0, viewport);

        assert_eq!(camera.zoom, 2.0);
        assert_close(camera.screen_to_world(anchor, viewport), anchored_world);
    }

    #[test]
    fn zoom_clamping_also_keeps_the_anchor() {
        let viewport = v(1000.0, 700.0);
        let anchor = v(900.0, 120.0);
        let mut camera = Camera::new(v(0.0, 0.0), Camera::MAX_ZOOM);
        let anchored_world = camera.screen_to_world(anchor, viewport);

        camera.zoom_at(anchor, 4.0, viewport);

        assert_eq!(camera.zoom, Camera::MAX_ZOOM);
        assert_close(camera.screen_to_world(anchor, viewport), anchored_world);
    }

    #[test]
    fn zoom_stays_inside_the_supported_range() {
        let mut camera = Camera::default();

        camera.zoom_at(Vec2::ZERO, 1.0e12, v(100.0, 100.0));
        assert_eq!(camera.zoom, Camera::MAX_ZOOM);

        camera.zoom_at(Vec2::ZERO, 1.0e-12, v(100.0, 100.0));
        assert_eq!(camera.zoom, Camera::MIN_ZOOM);
    }

    #[test]
    fn panning_follows_the_cursor() {
        let viewport = v(800.0, 600.0);
        let mut camera = Camera::new(Vec2::ZERO, 2.0);

        camera.pan_by_screen(v(100.0, 0.0));

        // Dragging right by 100 px moves the viewport 50 world units left.
        assert_eq!(camera.center, v(-50.0, 0.0));
        assert_eq!(
            camera.world_to_screen(Vec2::ZERO, viewport),
            v(500.0, 300.0)
        );
    }

    #[test]
    fn visible_rect_matches_the_viewport_corners() {
        let viewport = v(800.0, 400.0);
        let camera = Camera::new(v(0.0, 0.0), 2.0);

        let visible = camera.visible_world_rect(viewport);

        assert_eq!(visible.min, v(-200.0, -100.0));
        assert_eq!(visible.max(), v(200.0, 100.0));
    }
}
