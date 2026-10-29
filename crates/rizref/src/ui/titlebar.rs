//! The custom, auto-hiding title bar of the frameless window.
//!
//! The window keeps the whole client area for the board and only reveals its
//! window controls when the pointer reaches the top edge. Everything here is a
//! pure function of the viewport and pointer position; the application owns the
//! actual window operations (`drag_window`, `set_minimized`, …), which keeps the
//! layout and visibility rules testable without a desktop.

use glam::DVec2;

use super::paint::{ACCENT, ACCENT_SOFT, Painter, TEXT, UiRect};

/// Height of the title bar in logical pixels.
pub const HEIGHT: f32 = 34.0;

/// Width of the pin, minimize and maximize buttons.
pub const BUTTON_WIDTH: f32 = 44.0;

/// Width of the close button.
pub const CLOSE_WIDTH: f32 = 46.0;

/// Bar fill.
const BAR_BG: [u8; 4] = [23, 24, 29, 255];
/// Hairline under the bar.
const BAR_BORDER: [u8; 4] = [55, 58, 67, 255];
/// Hover feedback for pin/minimize/maximize.
const HOVER_BG: [u8; 4] = [255, 255, 255, 18];
/// Hover feedback for close (the Windows close-button red).
const CLOSE_HOVER: [u8; 4] = [196, 43, 28, 255];
/// Icon colour for a hovered close button.
const CLOSE_ICON: [u8; 4] = [255, 255, 255, 255];
/// Document title text.
const TITLE_TEXT: [u8; 4] = [206, 209, 218, 255];

/// The four window controls, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleBarButton {
    /// Toggle always-on-top.
    Pin,
    /// Minimize the window.
    Minimize,
    /// Maximize or restore the window.
    Maximize,
    /// Close the window.
    Close,
}

/// The title-bar geometry for one viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TitleBarLayout {
    /// The whole bar.
    pub bar: UiRect,
    /// Always-on-top pin.
    pub pin: UiRect,
    /// Minimize.
    pub minimize: UiRect,
    /// Maximize/restore.
    pub maximize: UiRect,
    /// Close.
    pub close: UiRect,
    /// The empty area that drags the window.
    pub drag: UiRect,
}

impl TitleBarLayout {
    /// Lays the bar out at the top of `viewport`.
    pub fn new(viewport: DVec2) -> Self {
        let width = viewport.x.max(0.0) as f32;
        let close_x = (width - CLOSE_WIDTH).max(0.0);
        let close_width = (width - close_x).max(0.0);
        let maximize_x = (close_x - BUTTON_WIDTH).max(0.0);
        let maximize_width = (close_x - maximize_x).max(0.0);
        let minimize_x = (maximize_x - BUTTON_WIDTH).max(0.0);
        let minimize_width = (maximize_x - minimize_x).max(0.0);
        let pin_x = (minimize_x - BUTTON_WIDTH).max(0.0);
        let pin_width = (minimize_x - pin_x).max(0.0);

        Self {
            bar: UiRect::new(0.0, 0.0, width, HEIGHT),
            pin: UiRect::new(pin_x, 0.0, pin_width, HEIGHT),
            minimize: UiRect::new(minimize_x, 0.0, minimize_width, HEIGHT),
            maximize: UiRect::new(maximize_x, 0.0, maximize_width, HEIGHT),
            close: UiRect::new(close_x, 0.0, close_width, HEIGHT),
            drag: UiRect::new(0.0, 0.0, pin_x, HEIGHT),
        }
    }

    /// The rectangle of one button.
    pub fn rect(&self, button: TitleBarButton) -> UiRect {
        match button {
            TitleBarButton::Pin => self.pin,
            TitleBarButton::Minimize => self.minimize,
            TitleBarButton::Maximize => self.maximize,
            TitleBarButton::Close => self.close,
        }
    }

    /// Whether a logical point is on the bar.
    pub fn contains(&self, point: DVec2) -> bool {
        self.bar.contains(point.x as f32, point.y as f32)
    }

    /// The button under a logical point, if any.
    pub fn button_at(&self, point: DVec2) -> Option<TitleBarButton> {
        let (x, y) = (point.x as f32, point.y as f32);
        [
            TitleBarButton::Pin,
            TitleBarButton::Minimize,
            TitleBarButton::Maximize,
            TitleBarButton::Close,
        ]
        .into_iter()
        .find(|button| self.rect(*button).contains(x, y))
    }
}

/// The bar's visibility, hover state and the two toggles it mirrors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleBar {
    visible: bool,
    hovered: Option<TitleBarButton>,
    pinned: bool,
    maximized: bool,
    title: Option<String>,
}

impl Default for TitleBar {
    fn default() -> Self {
        Self::new()
    }
}

impl TitleBar {
    /// A hidden bar with no state latched.
    pub const fn new() -> Self {
        Self {
            visible: false,
            hovered: None,
            pinned: false,
            maximized: false,
            title: None,
        }
    }

    /// Sets the document title shown at the left, or `None` for a blank left
    /// side (the start view). Returns whether the painted title changed.
    pub fn set_title(&mut self, title: Option<String>) -> bool {
        if self.title == title {
            return false;
        }
        self.title = title;
        true
    }

    /// The current document title, if one is open.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Whether the bar is currently painted.
    pub const fn visible(&self) -> bool {
        self.visible
    }

    /// Whether always-on-top is on (pin icon state).
    pub const fn pinned(&self) -> bool {
        self.pinned
    }

    /// Whether the window is maximized (maximize/restore icon).
    pub const fn maximized(&self) -> bool {
        self.maximized
    }

    /// Mirrors the window state. Returns whether the painted icons changed.
    pub fn set_state(&mut self, pinned: bool, maximized: bool) -> bool {
        let changed = self.pinned != pinned || self.maximized != maximized;
        self.pinned = pinned;
        self.maximized = maximized;
        changed
    }

    /// Hides the bar. Returns whether it was showing.
    pub fn hide(&mut self) -> bool {
        let changed = self.visible || self.hovered.is_some();
        self.visible = false;
        self.hovered = None;
        changed
    }

    /// Updates visibility and hover for the pointer.
    ///
    /// `allowed` is false in modes where the bar must never appear (ghost,
    /// click-through, borderless fullscreen, colour picker). Returns whether
    /// the painted state changed.
    pub fn pointer_moved(&mut self, cursor: Option<DVec2>, viewport: DVec2, allowed: bool) -> bool {
        let before = (self.visible, self.hovered);
        if !allowed {
            self.visible = false;
            self.hovered = None;
        } else if let Some(cursor) = cursor {
            let layout = TitleBarLayout::new(viewport);
            if self.visible {
                if layout.contains(cursor) {
                    self.hovered = layout.button_at(cursor);
                } else {
                    self.visible = false;
                    self.hovered = None;
                }
            } else if cursor.x >= 0.0
                && cursor.x <= viewport.x
                && cursor.y >= 0.0
                && cursor.y <= f64::from(HEIGHT)
            {
                self.visible = true;
                self.hovered = layout.button_at(cursor);
            }
        } else {
            self.visible = false;
            self.hovered = None;
        }
        (self.visible, self.hovered) != before
    }

    /// Hides the bar when the pointer leaves the window.
    pub fn pointer_left(&mut self) -> bool {
        self.hide()
    }

    /// Paints the bar into the shell layer.
    pub fn paint(&self, painter: &mut Painter, viewport: DVec2) {
        if !self.visible {
            return;
        }
        let layout = TitleBarLayout::new(viewport);
        painter.fill_rect(layout.bar, BAR_BG);
        painter.fill_rect(
            UiRect::new(0.0, HEIGHT - 1.0, layout.bar.width, 1.0),
            BAR_BORDER,
        );

        if let Some(title) = &self.title {
            let available = (layout.pin.x - 24.0).max(0.0);
            if available >= 24.0 {
                let shown = elide(painter, title, available, 12.0);
                painter.text(&shown, 12.0, 10.0, 12.0, TITLE_TEXT);
            }
        }

        for button in [
            TitleBarButton::Pin,
            TitleBarButton::Minimize,
            TitleBarButton::Maximize,
            TitleBarButton::Close,
        ] {
            let rect = layout.rect(button);
            let hovered = self.hovered == Some(button);
            if hovered {
                let background = if button == TitleBarButton::Close {
                    CLOSE_HOVER
                } else if button == TitleBarButton::Pin && self.pinned {
                    ACCENT_SOFT
                } else {
                    HOVER_BG
                };
                painter.fill_round_rect(rect.inset(4.0), 4.0, background);
            }
            let color = if button == TitleBarButton::Close && hovered {
                CLOSE_ICON
            } else if button == TitleBarButton::Pin && self.pinned {
                ACCENT
            } else {
                TEXT
            };
            draw_icon(painter, button, rect, color, self.maximized);
        }
    }
}

/// Draws one control's glyph, centred in `rect`.
fn draw_icon(
    painter: &mut Painter,
    button: TitleBarButton,
    rect: UiRect,
    rgba: [u8; 4],
    maximized: bool,
) {
    let cx = rect.x + rect.width * 0.5;
    let cy = rect.y + rect.height * 0.5;
    match button {
        TitleBarButton::Minimize => painter.line((cx - 5.0, cy), (cx + 5.0, cy), 1.1, rgba),
        TitleBarButton::Close => {
            painter.line((cx - 4.6, cy - 4.6), (cx + 4.6, cy + 4.6), 1.1, rgba);
            painter.line((cx + 4.6, cy - 4.6), (cx - 4.6, cy + 4.6), 1.1, rgba);
        }
        TitleBarButton::Maximize => {
            if maximized {
                // Restore: a back square peeking out above-right of the front one.
                painter.line((cx - 3.0, cy - 4.5), (cx + 4.5, cy - 4.5), 1.0, rgba);
                painter.line((cx + 4.5, cy - 4.5), (cx + 4.5, cy + 3.0), 1.0, rgba);
                painter.line((cx - 4.5, cy - 3.0), (cx + 3.0, cy - 3.0), 1.0, rgba);
                painter.line((cx + 3.0, cy - 3.0), (cx + 3.0, cy + 4.5), 1.0, rgba);
                painter.line((cx - 4.5, cy + 4.5), (cx + 3.0, cy + 4.5), 1.0, rgba);
                painter.line((cx - 4.5, cy - 3.0), (cx - 4.5, cy + 4.5), 1.0, rgba);
            } else {
                let half = 4.5;
                painter.line((cx - half, cy - half), (cx + half, cy - half), 1.0, rgba);
                painter.line((cx + half, cy - half), (cx + half, cy + half), 1.0, rgba);
                painter.line((cx + half, cy + half), (cx - half, cy + half), 1.0, rgba);
                painter.line((cx - half, cy + half), (cx - half, cy - half), 1.0, rgba);
            }
        }
        TitleBarButton::Pin => {
            // A thumbtack: wide head, narrow body, hooked needle.
            painter.line((cx - 4.2, cy - 5.0), (cx + 4.2, cy - 5.0), 1.9, rgba);
            painter.line((cx, cy - 5.0), (cx, cy + 0.8), 2.4, rgba);
            painter.line((cx - 2.4, cy + 1.6), (cx + 2.4, cy + 1.6), 1.4, rgba);
            painter.line((cx, cy + 1.6), (cx + 4.0, cy + 5.0), 1.1, rgba);
        }
    }
}

/// Shortens `text` with an ellipsis until it fits `max_width`.
fn elide(painter: &mut Painter, text: &str, max_width: f32, size: f32) -> String {
    if painter.measure(text, size) <= max_width {
        return text.to_owned();
    }
    for end in text.char_indices().map(|(index, _)| index).rev() {
        let candidate = format!("{}…", &text[..end]);
        if painter.measure(&candidate, size) <= max_width {
            return candidate;
        }
    }
    String::from("…")
}

#[cfg(test)]
mod tests {
    use super::{BUTTON_WIDTH, CLOSE_WIDTH, HEIGHT, TitleBar, TitleBarButton, TitleBarLayout};
    use glam::DVec2;

    fn viewport() -> DVec2 {
        DVec2::new(800.0, 600.0)
    }

    #[test]
    fn controls_are_anchored_to_the_right_edge_in_order() {
        let layout = TitleBarLayout::new(viewport());
        assert_eq!(layout.close.right(), 800.0);
        assert_eq!(layout.close.width, CLOSE_WIDTH);
        assert_eq!(layout.maximize.right(), layout.close.x);
        assert_eq!(layout.minimize.right(), layout.maximize.x);
        assert_eq!(layout.pin.right(), layout.minimize.x);
        assert_eq!(layout.pin.width, BUTTON_WIDTH);
        assert_eq!(layout.drag.right(), layout.pin.x);
        assert_eq!(layout.bar.height, HEIGHT);
    }

    #[test]
    fn hit_testing_matches_the_buttons() {
        let layout = TitleBarLayout::new(viewport());
        let center = |rect: super::UiRect| {
            DVec2::new(
                f64::from(rect.x + rect.width * 0.5),
                f64::from(rect.y + rect.height * 0.5),
            )
        };
        assert_eq!(
            layout.button_at(center(layout.pin)),
            Some(TitleBarButton::Pin)
        );
        assert_eq!(
            layout.button_at(center(layout.close)),
            Some(TitleBarButton::Close)
        );
        assert_eq!(layout.button_at(DVec2::new(40.0, 12.0)), None);
        assert!(layout.contains(DVec2::new(40.0, 12.0)));
        assert!(!layout.contains(DVec2::new(40.0, f64::from(HEIGHT) + 1.0)));
    }

    #[test]
    fn the_bar_strip_reveals_and_leaving_hides() {
        let mut bar = TitleBar::new();
        assert!(!bar.visible());
        // The whole bar area is a hover target, not only the very top edge.
        assert!(bar.pointer_moved(Some(DVec2::new(300.0, 20.0)), viewport(), true));
        assert!(bar.visible());
        assert!(!bar.pointer_moved(
            Some(DVec2::new(300.0, f64::from(HEIGHT) - 1.0)),
            viewport(),
            true
        ));
        assert!(bar.visible());
        assert!(bar.pointer_moved(Some(DVec2::new(300.0, 80.0)), viewport(), true));
        assert!(!bar.visible());
        assert!(!bar.pointer_moved(Some(DVec2::new(300.0, 80.0)), viewport(), true));
    }

    #[test]
    fn blocked_modes_never_show_the_bar() {
        let mut bar = TitleBar::new();
        assert!(!bar.pointer_moved(Some(DVec2::new(300.0, 2.0)), viewport(), false));
        assert!(!bar.visible());
        bar.pointer_moved(Some(DVec2::new(300.0, 2.0)), viewport(), true);
        assert!(bar.visible());
        assert!(bar.pointer_moved(Some(DVec2::new(300.0, 2.0)), viewport(), false));
        assert!(!bar.visible());
    }

    #[test]
    fn hover_tracks_the_button_under_the_pointer() {
        let mut bar = TitleBar::new();
        let layout = TitleBarLayout::new(viewport());
        let close = DVec2::new(f64::from(layout.close.x + layout.close.width * 0.5), 17.0);
        bar.pointer_moved(Some(DVec2::new(300.0, 2.0)), viewport(), true);
        assert!(bar.pointer_moved(Some(close), viewport(), true));
        assert_eq!(bar.hovered, Some(TitleBarButton::Close));
    }

    #[test]
    fn the_document_title_is_optional_and_changes_once() {
        let mut bar = TitleBar::new();
        assert_eq!(bar.title(), None);
        assert!(bar.set_title(Some("Document1*.rizref".to_owned())));
        assert_eq!(bar.title(), Some("Document1*.rizref"));
        assert!(!bar.set_title(Some("Document1*.rizref".to_owned())));
        assert!(bar.set_title(None));
        assert_eq!(bar.title(), None);
    }

    #[test]
    fn state_changes_are_reported_once() {
        let mut bar = TitleBar::new();
        assert!(bar.set_state(true, false));
        assert!(!bar.set_state(true, false));
        assert!(bar.pinned());
        assert!(bar.set_state(true, true));
        assert!(bar.maximized());
        assert!(!bar.hide(), "the bar was not showing yet");
        bar.pointer_moved(Some(DVec2::new(300.0, 2.0)), viewport(), true);
        assert!(bar.hide());
        assert!(!bar.hide());
    }
}
