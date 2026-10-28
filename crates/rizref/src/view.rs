//! The view state: the camera, the active gesture, and the zoom animation.
//!
//! The view state is the application-side owner of the [`Camera`]: input
//! commands from [`crate::input`] land here and become camera changes, and the
//! renderer reads the camera out once per frame. Nothing here touches winit or
//! wgpu, so the whole behaviour — including the smooth-zoom timing — is
//! unit-testable with an injected clock.

use std::time::{Duration, Instant};

use glam::DVec2;
use rizref_core::{Camera, Rect};

/// Which gesture is driving a pan.
///
/// Kept as data rather than a bare bool so the HUD could later show it and
/// phase 05's rebinding can distinguish the gestures when it needs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanKind {
    /// Middle-button drag, allowed anywhere on the canvas.
    MiddleDrag,
    /// Left-button drag while space is held.
    SpaceDrag,
}

/// Zoom multiplier applied per wheel notch.
///
/// Exponential stepping — the plan's `z *= 1.1` — so N notches always zoom the
/// same total factor regardless of the current zoom level.
pub const ZOOM_NOTCH_FACTOR: f64 = 1.1;

/// Duration of the smooth zoom animation.
pub const ZOOM_ANIMATION: Duration = Duration::from_millis(80);

/// Degrees turned per alt+wheel notch.
pub const ROTATE_STEP_DEGREES: f64 = 15.0;

/// Padding, in viewport pixels, left around the content when framing it.
pub const FRAME_PADDING: f64 = 48.0;

/// The zoom animation in flight, if any.
#[derive(Debug)]
struct ZoomAnimation {
    /// Zoom the animation started from.
    from: f64,
    /// Zoom it is heading towards, already clamped.
    target: f64,
    /// Viewport pixel the zoom is anchored at.
    anchor: DVec2,
    /// When the animation began.
    started: Instant,
}

/// Camera plus gesture state for one window.
#[derive(Debug)]
pub struct ViewState {
    /// The view transform, the single source of truth for rendering.
    pub camera: Camera,
    /// Viewport size in logical pixels.
    viewport: DVec2,
    /// Cursor position in logical pixels, `None` until it first enters.
    cursor: Option<DVec2>,
    /// World point under the cursor, shown in the HUD.
    cursor_world: DVec2,
    /// The pan gesture in progress, if any.
    pan: Option<PanKind>,
    /// Whether space is held (the left-drag pan modifier).
    space_held: bool,
    /// Whether ctrl is held (instant zoom, no animation).
    ctrl_held: bool,
    /// The smooth zoom animation, if any.
    zoom_animation: Option<ZoomAnimation>,
}

impl ViewState {
    /// A view state for a camera and a viewport size in logical pixels.
    pub fn new(camera: Camera, viewport: DVec2) -> Self {
        Self {
            camera,
            viewport,
            cursor: None,
            cursor_world: DVec2::ZERO,
            pan: None,
            space_held: false,
            ctrl_held: false,
            zoom_animation: None,
        }
    }

    /// Applies a resize or DPI change. The camera is untouched: a resize
    /// re-frames nothing, the user keeps looking at the same world point.
    pub fn set_viewport(&mut self, viewport: DVec2) {
        self.viewport = viewport;
    }

    /// Reports the cursor at `screen` (logical pixels) and refreshes the world
    /// position derived from it.
    pub fn set_cursor(&mut self, screen: DVec2) {
        self.cursor = Some(screen);
        self.cursor_world = self.camera.screen_to_world(screen, self.viewport);
    }

    /// Whether the cursor has been seen at all.
    pub fn has_cursor(&self) -> bool {
        self.cursor.is_some()
    }

    /// The world point under the cursor, for the HUD readout.
    pub fn cursor_world(&self) -> DVec2 {
        self.cursor_world
    }

    /// Tracks the space modifier (the left-drag pan key).
    pub fn set_space_held(&mut self, held: bool) {
        self.space_held = held;
        // Releasing space mid-drag ends the gesture: the modifier that allowed
        // it is gone, and a pan stuck on after key-up feels like a bug.
        if !held && self.pan == Some(PanKind::SpaceDrag) {
            self.pan = None;
        }
    }

    /// Tracks the ctrl modifier (instant zoom, no animation).
    pub fn set_ctrl_held(&mut self, held: bool) {
        self.ctrl_held = held;
    }

    /// Whether a pan drag is currently active.
    pub fn pan_active(&self) -> bool {
        self.pan.is_some()
    }

    /// The viewport size in logical pixels.
    pub fn viewport(&self) -> DVec2 {
        self.viewport
    }

    /// The cursor position in logical pixels, if it has entered the window.
    pub fn cursor_position(&self) -> Option<DVec2> {
        self.cursor
    }

    /// Whether space is currently held.
    pub fn space_held(&self) -> bool {
        self.space_held
    }

    /// Applies a cursor drag delta in viewport pixels while a pan is active.
    ///
    /// A no-op without an active pan: selection dragging (phase 04) must not
    /// move the camera.
    pub fn drag_pan(&mut self, screen_delta: DVec2) {
        if self.pan.is_some() {
            self.camera.pan_by_screen(screen_delta);
            if let Some(cursor) = self.cursor {
                self.cursor_world = self.camera.screen_to_world(cursor, self.viewport);
            }
        }
    }

    /// Pans by a trackpad two-finger scroll delta in viewport pixels.
    pub fn scroll_pan(&mut self, screen_delta: DVec2) {
        self.camera.pan_by_screen(screen_delta);
        if let Some(cursor) = self.cursor {
            self.cursor_world = self.camera.screen_to_world(cursor, self.viewport);
        }
    }

    /// Applies `notches` wheel zoom notches at the cursor, starting or
    /// extending the smooth animation unless ctrl is held (then it is
    /// instant). Negative notches zoom out; the factor is exponential, so N
    /// notches always move the zoom the same multiplicative distance.
    pub fn zoom_notches(&mut self, notches: f64, now: Instant) {
        let factor = ZOOM_NOTCH_FACTOR.powf(notches);
        let anchor = self.cursor.unwrap_or(self.viewport * 0.5);
        if self.ctrl_held {
            self.zoom_to_at(anchor, self.camera.zoom * factor);
            return;
        }

        // Extend any running animation: more notches multiply the target
        // rather than restarting from a stale `from`.
        let base = self
            .zoom_animation
            .as_ref()
            .map_or(self.camera.zoom, |anim| anim.target);
        self.zoom_animation = Some(ZoomAnimation {
            from: self.camera.zoom,
            target: Camera::clamp_zoom(base * factor),
            anchor,
            started: now,
        });
    }

    /// Sets the zoom immediately, keeping the world point under `anchor`
    /// exactly in place.
    fn zoom_to_at(&mut self, anchor: DVec2, zoom: f64) {
        let factor = Camera::clamp_zoom(zoom) / self.camera.zoom;
        self.camera.zoom_at(anchor, factor, self.viewport);
        self.refresh_cursor_world();
    }

    /// Advances the smooth zoom animation to `now`.
    ///
    /// Returns `true` while the animation is still running, so the caller
    /// knows to keep requesting frames.
    pub fn advance(&mut self, now: Instant) -> bool {
        let Some(anim) = self.zoom_animation.as_ref() else {
            return false;
        };
        let (from, target, anchor, started) = (anim.from, anim.target, anim.anchor, anim.started);
        let progress = (now - started).as_secs_f64() / ZOOM_ANIMATION.as_secs_f64();
        if progress >= 1.0 {
            self.zoom_animation = None;
            self.zoom_to_at(anchor, target);
            return false;
        }
        // Smoothstep eases both ends of the 80 ms; the intermediate zoom is
        // applied through the anchored path, so the cursor stays put exactly
        // as it does for the final value.
        let eased = progress * progress * (3.0 - 2.0 * progress);
        self.zoom_to_at(anchor, from + (target - from) * eased);
        true
    }

    /// Whether a zoom animation is in flight.
    #[cfg(test)]
    pub fn zooming(&self) -> bool {
        self.zoom_animation.is_some()
    }

    /// Applies a resolved [`Command`](crate::input::Command).
    ///
    /// `scene_rect` is the tracked bounding box of the board, needed by the
    /// fit commands; an empty board resets to the default camera instead.
    pub fn apply_command(&mut self, command: crate::input::Command, scene_rect: Option<Rect>) {
        use crate::input::Command;
        match command {
            Command::BeginPan(kind) => {
                if self.pan.is_none() {
                    self.pan = Some(kind);
                }
            }
            Command::EndPan => self.pan = None,
            Command::ZoomNotch(notches) => self.zoom_notches(f64::from(notches), Instant::now()),
            Command::RotateNotch(notches) => {
                self.camera
                    .rotate_by(f64::from(notches) * ROTATE_STEP_DEGREES.to_radians());
                self.refresh_cursor_world();
            }
            Command::FitContent => match scene_rect {
                Some(rect) => self.camera.frame(rect, self.viewport, FRAME_PADDING),
                None => self.camera = Camera::default(),
            },
            Command::ZoomHundred => {
                let anchor = self.cursor.unwrap_or(self.viewport * 0.5);
                self.zoom_to_at(anchor, 1.0);
            }
            Command::ResetRotation => {
                self.camera.set_rotation(0.0);
                self.refresh_cursor_world();
            }
            // Item commands are dispatched by the board, never routed here.
            _ => {}
        }
    }

    /// Re-derives the world point under the cursor after a camera change.
    fn refresh_cursor_world(&mut self) {
        if let Some(cursor) = self.cursor {
            self.cursor_world = self.camera.screen_to_world(cursor, self.viewport);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PanKind, ROTATE_STEP_DEGREES, ViewState, ZOOM_ANIMATION, ZOOM_NOTCH_FACTOR};
    use crate::input::Command;
    use glam::DVec2;
    use rizref_core::{Camera, Rect};
    use std::time::{Duration, Instant};

    fn view() -> ViewState {
        ViewState::new(Camera::default(), DVec2::new(800.0, 600.0))
    }

    fn rect() -> Rect {
        Rect::from_min_max(glam::Vec2::ZERO, glam::Vec2::new(400.0, 200.0))
    }

    #[test]
    fn zoom_notches_accumulate_exponentially() {
        let mut view = view();
        view.set_cursor(DVec2::new(400.0, 300.0));

        view.apply_command(Command::ZoomNotch(1), None);
        view.apply_command(Command::ZoomNotch(1), None);

        let anim = view.zoom_animation.as_ref().expect("animation running");
        assert!(
            (anim.target - ZOOM_NOTCH_FACTOR * ZOOM_NOTCH_FACTOR).abs() < 1.0e-12,
            "two notches square the factor, got {}",
            anim.target
        );
    }

    #[test]
    fn the_zoom_animation_sets_and_settles_the_target() {
        let mut view = view();
        view.set_cursor(DVec2::new(400.0, 300.0));

        view.apply_command(Command::ZoomNotch(1), None);
        let settled_clock = Instant::now();
        assert!(
            view.advance(settled_clock),
            "still animating immediately after"
        );
        assert!(
            (view.camera.zoom - 1.0).abs() < ZOOM_NOTCH_FACTOR,
            "barely moved at t=0"
        );

        assert!(
            !view.advance(settled_clock + ZOOM_ANIMATION + Duration::from_millis(1)),
            "finished at the duration"
        );
        assert!(
            (view.camera.zoom - ZOOM_NOTCH_FACTOR).abs() < 1.0e-9,
            "settled exactly on the target, got {}",
            view.camera.zoom
        );
    }

    #[test]
    fn ctrl_zoom_is_instant() {
        let mut view = view();
        view.set_cursor(DVec2::new(400.0, 300.0));
        view.set_ctrl_held(true);

        view.apply_command(Command::ZoomNotch(1), None);

        assert!(!view.zooming());
        assert!((view.camera.zoom - ZOOM_NOTCH_FACTOR).abs() < 1.0e-12);
    }

    #[test]
    fn zooming_keeps_the_cursor_anchored_throughout() {
        let mut view = view();
        let anchor = DVec2::new(250.0, 310.0);
        view.set_cursor(anchor);
        let anchored_world = view.camera.screen_to_world(anchor, view.viewport);
        let start = Instant::now();

        view.apply_command(Command::ZoomNotch(1), None);
        let mut still_running = true;
        let mut elapsed = Duration::ZERO;
        while still_running {
            elapsed += Duration::from_millis(10);
            still_running = view.advance(start + elapsed);
        }

        assert!(
            (view.camera.screen_to_world(anchor, view.viewport) - anchored_world).length() < 1.0e-9,
            "the world point under the cursor never moves while zooming"
        );
    }

    #[test]
    fn pan_needs_a_gesture_and_follows_the_cursor() {
        let mut view = view();

        view.drag_pan(DVec2::new(100.0, 0.0));
        assert_eq!(view.camera, Camera::default(), "no gesture, no pan");

        view.apply_command(Command::BeginPan(PanKind::MiddleDrag), None);
        assert!(view.pan_active());
        view.drag_pan(DVec2::new(100.0, 0.0));
        assert_eq!(view.camera.eye, DVec2::new(-100.0, 0.0));

        view.apply_command(Command::EndPan, None);
        assert!(!view.pan_active());
        view.drag_pan(DVec2::new(100.0, 0.0));
        assert_eq!(view.camera.eye, DVec2::new(-100.0, 0.0), "pan ended");
    }

    #[test]
    fn releasing_space_ends_a_space_pan() {
        let mut view = view();
        view.set_space_held(true);
        view.apply_command(Command::BeginPan(PanKind::SpaceDrag), None);

        view.set_space_held(false);

        assert!(!view.pan_active());
    }

    #[test]
    fn fit_content_frames_the_scene_rect() {
        let mut view = view();

        view.apply_command(Command::FitContent, Some(rect()));

        assert!((view.camera.eye.x - 200.0).abs() < 1.0e-9);
        assert!((view.camera.eye.y - 100.0).abs() < 1.0e-9);
        assert!(
            view.camera.zoom > 1.0,
            "the 400-unit board fits with room to spare"
        );
    }

    #[test]
    fn fitting_an_empty_board_resets_the_camera() {
        let mut view = view();
        view.camera.eye = DVec2::new(500.0, 500.0);
        view.camera.zoom = 4.0;

        view.apply_command(Command::FitContent, None);

        assert_eq!(view.camera, Camera::default());
    }

    #[test]
    fn zoom_hundred_anchors_at_the_cursor() {
        let mut view = view();
        view.camera.zoom = 4.0;
        view.set_cursor(DVec2::new(300.0, 200.0));
        let anchored = view
            .camera
            .screen_to_world(DVec2::new(300.0, 200.0), view.viewport);

        view.apply_command(Command::ZoomHundred, None);

        assert!((view.camera.zoom - 1.0).abs() < 1.0e-9);
        assert!(
            (view
                .camera
                .screen_to_world(DVec2::new(300.0, 200.0), view.viewport)
                - anchored)
                .length()
                < 1.0e-9
        );
    }

    #[test]
    fn rotation_notches_turn_fifteen_degrees_and_reset_to_zero() {
        let mut view = view();

        view.apply_command(Command::RotateNotch(1), None);
        assert!(
            (view.camera.rotation - ROTATE_STEP_DEGREES.to_radians()).abs() < 1.0e-12,
            "one notch, one step, got {} rad",
            view.camera.rotation
        );

        view.apply_command(Command::ResetRotation, None);
        assert_eq!(view.camera.rotation, 0.0);
    }

    #[test]
    fn a_second_pan_gesture_does_not_stomp_the_first() {
        let mut view = view();
        view.apply_command(Command::BeginPan(PanKind::MiddleDrag), None);

        view.apply_command(Command::BeginPan(PanKind::SpaceDrag), None);

        assert_eq!(view.pan, Some(super::PanKind::MiddleDrag));
    }
}
