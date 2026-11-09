//! The product shell around the canvas: start view, menus, shortcut and
//! settings overlays, toasts and mode indicators.
//!
//! The shell is a small state machine ([`Shell`]) plus a cached CPU-raster
//! layer ([`UiLayer`]). The canvas HUD keeps its tiny per-frame texture; the
//! shell layer is viewport-sized and re-uploaded only when something in it
//! changed, which is what keeps an idle board at zero CPU and a menu hover from
//! costing a full-screen upload every mouse event.

pub mod image_info;
pub mod menu;
pub mod note_color;
pub mod paint;
pub mod panels;
pub mod start;
pub mod titlebar;

use std::time::{Duration, Instant};

use glam::DVec2;
use tiny_skia::Pixmap;

use crate::hud::straight_rgba;
use crate::platform::window_modes::WindowModes;
use crate::recents::Recents;
use crate::settings::Settings;
use crate::shortcuts::Shortcuts;
use crate::text::TextEngine;

use self::paint::{Painter, UiRect};

/// How long a toast stays on screen.
pub const TOAST_LIFETIME: Duration = Duration::from_millis(3200);

/// Whether the shell is showing the launch state or the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The start view.
    Start,
    /// The board.
    Canvas,
}

/// A short message at the bottom of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    /// The message.
    pub text: String,
    /// When it disappears.
    pub until: Instant,
}

/// All interactive shell state.
#[derive(Debug)]
pub struct Shell {
    mode: Mode,
    /// The open context menu, if any.
    pub menu: Option<menu::MenuState>,
    /// The open F1 overlay, if any.
    pub shortcuts: Option<panels::ShortcutsPanel>,
    /// The open settings dialog, if any.
    pub settings: Option<panels::SettingsPanel>,
    /// The modal unsaved-changes dialog, if closing would lose work.
    pub confirm_close: Option<panels::ConfirmClose>,
    /// The modal image information dialog, if open.
    pub image_info: Option<image_info::ImageInfoPanel>,
    /// The current toast, if any.
    pub toast: Option<Toast>,
    /// The note background popover, if open.
    pub note_color: Option<note_color::NoteColorPopover>,
    /// The hover-toolbar tooltip: its text and logical anchor.
    pub tooltip: Option<(String, DVec2)>,
    /// Whether the colour picker is armed.
    pub picker: bool,
    /// Whether item editing is locked.
    pub locked: bool,
    /// The auto-hiding title bar.
    pub title_bar: titlebar::TitleBar,
    /// Bumped whenever the rendered shell changes.
    revision: u64,
}

impl Shell {
    /// A shell in `mode`.
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            menu: None,
            shortcuts: None,
            settings: None,
            confirm_close: None,
            image_info: None,
            toast: None,
            note_color: None,
            tooltip: None,
            picker: false,
            locked: false,
            title_bar: titlebar::TitleBar::new(),
            revision: 1,
        }
    }

    /// The current mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Switches modes and closes every overlay.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.menu = None;
        self.shortcuts = None;
        self.settings = None;
        self.image_info = None;
        self.note_color = None;
        self.tooltip = None;
        self.picker = false;
        self.title_bar.hide();
        self.touch();
    }

    /// Marks the shell layer dirty.
    pub fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// The current revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether any overlay currently owns input.
    pub fn has_overlay(&self) -> bool {
        self.menu.is_some()
            || self.note_color.is_some()
            || self.shortcuts.is_some()
            || self.settings.is_some()
            || self.confirm_close.is_some()
            || self.image_info.is_some()
    }

    /// Opens the image information dialog, closing the other overlays.
    pub fn open_image_info(&mut self, panel: image_info::ImageInfoPanel) {
        self.menu = None;
        self.note_color = None;
        self.shortcuts = None;
        self.settings = None;
        self.tooltip = None;
        self.picker = false;
        self.image_info = Some(panel);
        self.touch();
    }

    /// Closes the image information dialog. Returns whether it was open.
    pub fn close_image_info(&mut self) -> bool {
        if self.image_info.take().is_some() {
            self.touch();
            true
        } else {
            false
        }
    }

    /// Opens the close guard for `title`, closing the other overlays.
    ///
    /// Repeated close requests keep the existing dialog instead of resetting
    /// the highlighted button under the user's pointer.
    pub fn open_confirm_close(&mut self, title: String) {
        self.open_confirm(
            title,
            "Close without saving",
            "Closing now will discard them.",
        );
    }

    /// Opens the crash-recovery offer over whatever is showing.
    pub fn open_recovery(&mut self, dialog: panels::ConfirmClose) {
        self.menu = None;
        self.shortcuts = None;
        self.settings = None;
        self.image_info = None;
        self.confirm_close = Some(dialog);
        self.touch();
    }

    /// Opens the unsaved-changes guard for a non-close destructive step.
    pub fn open_confirm(&mut self, title: String, discard_label: &str, note: &str) {
        if self.confirm_close.is_some() {
            return;
        }
        self.menu = None;
        self.shortcuts = None;
        self.settings = None;
        self.image_info = None;
        self.confirm_close = Some(panels::ConfirmClose::for_action(title, discard_label, note));
        self.touch();
    }

    /// Dismisses the close guard without closing. Returns whether it was open.
    pub fn close_confirm_close(&mut self) -> bool {
        if self.confirm_close.take().is_some() {
            self.touch();
            true
        } else {
            false
        }
    }

    /// Opens a menu and closes the other overlays.
    pub fn open_menu(&mut self, entries: Vec<menu::MenuEntry>, position: DVec2, viewport: DVec2) {
        self.menu = Some(menu::MenuState::open(entries, position, viewport));
        self.note_color = None;
        self.shortcuts = None;
        self.settings = None;
        self.touch();
    }

    /// Closes the menu if one is open.
    pub fn close_menu(&mut self) -> bool {
        if self.menu.take().is_some() {
            self.touch();
            true
        } else {
            false
        }
    }

    /// Toggles the F1 overlay.
    pub fn toggle_shortcuts(&mut self) {
        if self.shortcuts.take().is_some() {
            self.touch();
        } else {
            self.shortcuts = Some(panels::ShortcutsPanel::new());
            self.settings = None;
            self.menu = None;
            self.touch();
        }
    }

    /// Toggles the settings dialog.
    pub fn toggle_settings(&mut self) {
        if self.settings.take().is_some() {
            self.touch();
        } else {
            self.settings = Some(panels::SettingsPanel::new());
            self.shortcuts = None;
            self.menu = None;
            self.touch();
        }
    }

    /// Shows a toast until `now + TOAST_LIFETIME`.
    pub fn toast(&mut self, text: impl Into<String>, now: Instant) {
        self.toast = Some(Toast {
            text: text.into(),
            until: now + TOAST_LIFETIME,
        });
        self.touch();
    }

    /// Removes an expired toast. Returns whether anything changed.
    pub fn expire_toast(&mut self, now: Instant) -> bool {
        if self.toast.as_ref().is_some_and(|toast| toast.until <= now) {
            self.toast = None;
            self.touch();
            true
        } else {
            false
        }
    }

    /// When the next toast expiry (or the ghost hotkey poll) should wake the
    /// event loop.
    pub fn next_wake(&self) -> Option<Instant> {
        self.toast.as_ref().map(|toast| toast.until)
    }

    /// Closes whichever overlay is open. Returns whether one was open.
    pub fn escape(&mut self) -> bool {
        let closed = self.confirm_close.take().is_some()
            || self.image_info.take().is_some()
            || self.menu.take().is_some()
            || self.note_color.take().is_some()
            || self.shortcuts.take().is_some()
            || self.settings.take().is_some();
        if closed {
            self.touch();
            true
        } else if self.picker {
            self.picker = false;
            self.touch();
            true
        } else {
            false
        }
    }
}

/// The cached screen-space shell layer.
pub struct UiLayer {
    pixmap: Option<Pixmap>,
    width: u32,
    height: u32,
    scale: f32,
    built_revision: u64,
}

impl Default for UiLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl UiLayer {
    /// An empty layer; the first [`needs_paint`](Self::needs_paint) is true.
    pub fn new() -> Self {
        Self {
            pixmap: None,
            width: 0,
            height: 0,
            scale: 1.0,
            built_revision: 0,
        }
    }

    /// Forces the next [`needs_paint`](Self::needs_paint) to say yes.
    ///
    /// The GPU attaches after the CPU frames have already painted (and
    /// consumed) the layer; without this, its first frame would `Keep` a UI
    /// texture that was never uploaded and show a bare canvas until the next
    /// shell change (phase 07 manual test: a blank start view until a click).
    pub fn invalidate(&mut self) {
        self.pixmap = None;
    }

    /// Whether the layer must be repainted for this size/scale/revision.
    pub fn needs_paint(&self, viewport: DVec2, scale: f64, revision: u64) -> bool {
        let width = (viewport.x * scale).round().max(1.0) as u32;
        let height = (viewport.y * scale).round().max(1.0) as u32;
        self.pixmap.is_none()
            || self.width != width
            || self.height != height
            || (self.scale - scale as f32).abs() > f32::EPSILON
            || self.built_revision != revision
    }

    /// Repaints the layer. Must be called before [`upload`](Self::upload), and
    /// only when [`needs_paint`](Self::needs_paint) says so.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        shell: &Shell,
        settings: &Settings,
        shortcuts: &Shortcuts,
        recents: &Recents,
        modes: WindowModes,
        viewport: DVec2,
        scale: f64,
        cursor: Option<DVec2>,
        text: &mut TextEngine,
    ) {
        let width = (viewport.x * scale).round().max(1.0) as u32;
        let height = (viewport.y * scale).round().max(1.0) as u32;
        if self.width != width || self.height != height || self.pixmap.is_none() {
            self.pixmap = Pixmap::new(width, height);
            self.width = width;
            self.height = height;
        }
        let Some(pixmap) = self.pixmap.as_mut() else {
            return;
        };
        self.scale = scale as f32;
        self.built_revision = shell.revision();

        let mut painter = Painter::new(pixmap, text, scale as f32);
        if shell.mode() == Mode::Start {
            let layout = start::StartLayout::new(viewport, recents.entries.len());
            start::paint(&mut painter, viewport, recents, &layout);
        } else {
            painter.clear([0, 0, 0, 0]);
        }
        shell.title_bar.paint(&mut painter, viewport);
        paint_mode_indicator(&mut painter, viewport, modes);

        if let Some(panel) = &shell.shortcuts {
            panel.paint(&mut painter, viewport, shortcuts);
        }
        if let Some(panel) = &shell.settings {
            panel.paint(&mut painter, viewport, settings);
        }
        if let Some(dialog) = &shell.image_info {
            dialog.paint(&mut painter, viewport);
        }
        // Above the info dialog's backdrop, so "Path copied" stays visible.
        if let Some(toast) = &shell.toast {
            paint_toast(&mut painter, viewport, &toast.text);
        }
        if shell.picker
            && let Some(cursor) = cursor
        {
            paint_picker_hint(&mut painter, viewport, cursor);
        }
        if let Some((text, anchor)) = &shell.tooltip {
            paint_tooltip(&mut painter, viewport, text, *anchor);
        }
        if let Some(popover) = &shell.note_color {
            popover.paint(&mut painter);
        }
        if let Some(menu) = &shell.menu {
            menu.paint(&mut painter);
        }
        if let Some(dialog) = &shell.confirm_close {
            dialog.paint(&mut painter, viewport);
        }
    }

    /// The straight-alpha RGBA bytes and device size of the layer.
    pub fn upload(&self) -> Option<(Vec<u8>, u32, u32)> {
        let pixmap = self.pixmap.as_ref()?;
        Some((straight_rgba(pixmap), pixmap.width(), pixmap.height()))
    }
}

/// The accent border and HUD note that say a window is ignoring the mouse.
fn paint_mode_indicator(painter: &mut Painter, viewport: DVec2, modes: WindowModes) {
    let (message, alpha) = if modes.ghost() {
        ("GHOST MODE · Ctrl+Shift+G to restore", paint::GHOST_ACCENT)
    } else if modes.effective_click_through() {
        (
            "CLICK-THROUGH · Ctrl+Shift+X to restore",
            paint::GHOST_ACCENT,
        )
    } else if modes.opacity_percent < 100 {
        (
            "TRANSPARENT WINDOW",
            [
                paint::DIM_TEXT[0],
                paint::DIM_TEXT[1],
                paint::DIM_TEXT[2],
                180,
            ],
        )
    } else {
        return;
    };
    let width = viewport.x as f32;
    let height = viewport.y as f32;
    let inset = 3.0;
    // Four thin bars rather than a stroked path: crisper at fractional scale.
    painter.fill_rect(UiRect::new(inset, inset, width - inset * 2.0, 2.0), alpha);
    painter.fill_rect(
        UiRect::new(inset, height - inset - 2.0, width - inset * 2.0, 2.0),
        alpha,
    );
    painter.fill_rect(UiRect::new(inset, inset, 2.0, height - inset * 2.0), alpha);
    painter.fill_rect(
        UiRect::new(width - inset - 2.0, inset, 2.0, height - inset * 2.0),
        alpha,
    );

    let text_width = painter.measure(message, 11.0);
    let pill = UiRect::new(
        (width - text_width - 28.0) * 0.5,
        height - 42.0,
        text_width + 28.0,
        24.0,
    );
    painter.fill_round_rect(pill, 6.0, [18, 18, 22, 220]);
    painter.text_centered(message, pill, pill.y + 5.0, 11.0, alpha);
}

/// Paints a small tooltip below `anchor`, kept inside the viewport.
fn paint_tooltip(painter: &mut Painter, viewport: DVec2, text: &str, anchor: DVec2) {
    let width = painter.measure(text, 11.0) + 16.0;
    let x = (anchor.x as f32).clamp(4.0, (viewport.x as f32 - width - 4.0).max(4.0));
    let y = (anchor.y as f32 + 6.0).min(viewport.y as f32 - 26.0);
    let pill = UiRect::new(x, y, width, 22.0);
    painter.fill_round_rect(pill, 5.0, [18, 18, 22, 235]);
    painter.stroke_round_rect(pill, 5.0, 1.0, paint::PANEL_BORDER);
    painter.text(text, x + 8.0, y + 5.0, 11.0, paint::TEXT);
}

/// Paints a toast at the bottom centre.
fn paint_toast(painter: &mut Painter, viewport: DVec2, text: &str) {
    let width = painter.measure(text, 12.0) + 34.0;
    let pill = UiRect::new(
        ((viewport.x as f32 - width) * 0.5).max(8.0),
        viewport.y as f32 - 78.0,
        width.min(viewport.x as f32 - 16.0),
        28.0,
    );
    painter.fill_round_rect(pill, 8.0, [20, 20, 25, 238]);
    painter.stroke_round_rect(pill, 8.0, 1.0, paint::PANEL_BORDER);
    painter.text_centered(text, pill, pill.y + 6.0, 12.0, paint::TEXT);
}

/// Paints the colour-picker hint near the cursor.
fn paint_picker_hint(painter: &mut Painter, viewport: DVec2, cursor: DVec2) {
    let x = (cursor.x as f32 + 18.0).min(viewport.x as f32 - 180.0);
    let y = (cursor.y as f32 + 18.0).min(viewport.y as f32 - 60.0);
    // Crosshair.
    painter.fill_rect(
        UiRect::new(x - 22.0, cursor.y as f32, 10.0, 1.0),
        [255, 255, 255, 200],
    );
    painter.fill_rect(
        UiRect::new(x - 6.0, cursor.y as f32, 10.0, 1.0),
        [255, 255, 255, 200],
    );
    painter.fill_rect(
        UiRect::new(cursor.x as f32, y - 22.0, 1.0, 10.0),
        [255, 255, 255, 200],
    );
    painter.fill_rect(
        UiRect::new(cursor.x as f32, y - 6.0, 1.0, 10.0),
        [255, 255, 255, 200],
    );
    let pill = UiRect::new(x, y, 168.0, 24.0);
    painter.fill_round_rect(pill, 5.0, [18, 18, 22, 225]);
    painter.text(
        "Click a pixel · Esc cancels",
        x + 9.0,
        y + 5.0,
        10.5,
        paint::TEXT,
    );
}

#[cfg(test)]
mod tests {
    use super::{Mode, Shell, TOAST_LIFETIME, UiLayer, panels};
    use glam::DVec2;
    use std::time::{Duration, Instant};

    #[test]
    fn a_new_shell_is_in_the_requested_mode_and_dirty() {
        let shell = Shell::new(Mode::Start);
        assert_eq!(shell.mode(), Mode::Start);
        assert!(shell.revision() > 0);
        assert!(!shell.has_overlay());
    }

    #[test]
    fn toasts_expire_once_and_bump_the_revision() {
        let mut shell = Shell::new(Mode::Canvas);
        let now = Instant::now();
        shell.toast("hello", now);
        let revision = shell.revision();
        assert!(!shell.expire_toast(now + Duration::from_millis(10)));
        assert!(shell.expire_toast(now + TOAST_LIFETIME));
        assert!(shell.toast.is_none());
        assert_ne!(shell.revision(), revision);
        assert!(!shell.expire_toast(now + TOAST_LIFETIME));
    }

    #[test]
    fn the_close_guard_is_modal_and_escape_cancels_it() {
        let mut shell = Shell::new(Mode::Canvas);
        shell.open_confirm_close("Document1*.rizref".to_owned());
        assert!(shell.has_overlay());
        assert!(shell.confirm_close.is_some());
        assert!(shell.escape());
        assert!(shell.confirm_close.is_none());
        assert!(!shell.has_overlay());
    }

    #[test]
    fn the_image_info_dialog_is_modal_and_escape_closes_it() {
        use super::image_info::{ImageInfo, ImageInfoPanel};
        use glam::Vec2;
        use rizref_codecs::{ImageBuffer, ImageFormat};
        use rizref_core::{Asset, Scene, SceneId};

        let mut scene = Scene::new(SceneId::random());
        let image = ImageBuffer::filled(2, 2, [0, 0, 0, 255]);
        let asset = Asset::from_decoded(
            std::sync::Arc::from(vec![1u8; 4]),
            ImageFormat::Png,
            &image,
            image.clone(),
        );
        let (asset, _) = scene.insert_asset(asset);
        let item = scene
            .insert_image(asset, Vec2::ZERO, Vec2::ONE)
            .expect("asset");
        let info = ImageInfo::from_scene(&scene, item, None).expect("image");

        let mut shell = Shell::new(Mode::Canvas);
        shell.toggle_settings();
        shell.open_image_info(ImageInfoPanel::new(info, None));
        assert!(shell.settings.is_none(), "other overlays close");
        assert!(shell.has_overlay());
        assert!(shell.escape());
        assert!(shell.image_info.is_none());
        assert!(!shell.has_overlay());
        assert!(!shell.close_image_info());
    }

    #[test]
    fn escape_closes_overlays_before_the_picker() {
        let mut shell = Shell::new(Mode::Canvas);
        shell.toggle_shortcuts();
        shell.picker = true;
        assert!(shell.escape());
        assert!(shell.shortcuts.is_none());
        assert!(shell.picker, "the picker survives the first escape");
        assert!(shell.escape());
        assert!(!shell.picker);
        assert!(!shell.escape());
    }

    #[test]
    fn switching_modes_closes_every_overlay() {
        let mut shell = Shell::new(Mode::Canvas);
        shell.toggle_settings();
        shell.picker = true;
        shell
            .title_bar
            .pointer_moved(Some(DVec2::new(400.0, 2.0)), DVec2::new(800.0, 600.0), true);
        assert!(shell.title_bar.visible());
        shell.set_mode(Mode::Start);
        assert_eq!(shell.mode(), Mode::Start);
        assert!(!shell.has_overlay());
        assert!(!shell.picker);
        assert!(!shell.title_bar.visible());
    }

    #[test]
    fn the_layer_repaints_only_when_inputs_change() {
        let mut layer = UiLayer::new();
        let viewport = DVec2::new(800.0, 600.0);
        assert!(layer.needs_paint(viewport, 1.0, 1));
        layer.built_revision = 1;
        layer.width = 800;
        layer.height = 600;
        layer.scale = 1.0;
        // A pixmap is still required; the dummy values model a built layer.
        assert!(layer.needs_paint(viewport, 1.0, 1), "no pixmap yet");
    }

    #[test]
    fn invalidating_a_built_layer_forces_a_repaint() {
        // The GPU attaches after the CPU frames painted the layer: its first
        // frame must repaint and upload it, or the start view is blank.
        let mut layer = UiLayer::new();
        let viewport = DVec2::new(800.0, 600.0);
        layer.pixmap = tiny_skia::Pixmap::new(800, 600);
        layer.built_revision = 1;
        layer.width = 800;
        layer.height = 600;
        layer.scale = 1.0;
        assert!(!layer.needs_paint(viewport, 1.0, 1), "built and current");

        layer.invalidate();

        assert!(layer.needs_paint(viewport, 1.0, 1));
    }

    #[test]
    fn the_recovery_offer_is_modal_and_replaces_other_overlays() {
        let mut shell = Shell::new(Mode::Start);
        shell.open_recovery(panels::ConfirmClose::recovery(
            "\"Board\" (3 items) was not saved.".to_owned(),
            "It belongs to x.rizref.".to_owned(),
        ));

        assert!(shell.has_overlay());
        let dialog = shell.confirm_close.as_ref().expect("open");
        assert_eq!(dialog.heading, "Recover unsaved work");
        assert_eq!(dialog.cancel_label, "Restore");
    }
}
