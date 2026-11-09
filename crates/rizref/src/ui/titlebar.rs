//! The custom, auto-hiding title bar of the frameless window.
//!
//! The window keeps the whole client area for the board and only reveals its
//! window controls when the pointer reaches the top edge. Everything here is a
//! pure function of the viewport and pointer position; the application owns the
//! actual window operations (`drag_window`, `set_minimized`, …), which keeps the
//! layout and visibility rules testable without a desktop.

use std::time::{Duration, Instant};

use glam::DVec2;

use crate::shortcuts::ShortcutId;

use super::paint::{ACCENT, ACCENT_SOFT, Painter, TEXT, UiRect};

/// Height of the title bar in logical pixels.
pub const HEIGHT: f32 = 34.0;

/// Width of the pin, minimize and maximize buttons.
pub const BUTTON_WIDTH: f32 = 44.0;

/// Width of the close button.
pub const CLOSE_WIDTH: f32 = 46.0;

/// How long the pointer rests on a button before its tooltip appears.
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(500);

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

/// The title-bar controls: the main-menu button at the left edge, then the
/// four window controls at the right, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleBarButton {
    /// ☰: opens the full command menu with a left click.
    Menu,
    /// Toggle always-on-top.
    Pin,
    /// Minimize the window.
    Minimize,
    /// Maximize or restore the window.
    Maximize,
    /// Close the window.
    Close,
}

impl TitleBarButton {
    /// The command a button shares with the keyboard, for its tooltip.
    pub const fn shortcut(self) -> Option<ShortcutId> {
        match self {
            Self::Pin => Some(ShortcutId::ToggleAlwaysOnTop),
            Self::Close => Some(ShortcutId::Quit),
            Self::Menu | Self::Minimize | Self::Maximize => None,
        }
    }

    /// The tooltip: what a click does now (the pin and maximize buttons
    /// toggle, so their wording follows the state), with the live shortcut
    /// when the command has one.
    pub fn tooltip(self, pinned: bool, maximized: bool, shortcut: Option<&str>) -> String {
        let action = match self {
            Self::Menu => "Main menu",
            Self::Pin if pinned => "Unpin",
            Self::Pin => "Pin on top",
            Self::Minimize => "Minimize",
            Self::Maximize if maximized => "Restore",
            Self::Maximize => "Maximize",
            Self::Close => "Close",
        };
        match shortcut {
            Some(chord) => format!("{action} ({chord})"),
            None => action.to_owned(),
        }
    }
}

/// The title-bar geometry for one viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TitleBarLayout {
    /// The whole bar.
    pub bar: UiRect,
    /// The ☰ main-menu button.
    pub menu: UiRect,
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
        // The menu button never overlaps the window controls on a very
        // narrow window: it shrinks to whatever is left of the pin.
        let menu_width = BUTTON_WIDTH.min(pin_x);

        Self {
            bar: UiRect::new(0.0, 0.0, width, HEIGHT),
            menu: UiRect::new(0.0, 0.0, menu_width, HEIGHT),
            pin: UiRect::new(pin_x, 0.0, pin_width, HEIGHT),
            minimize: UiRect::new(minimize_x, 0.0, minimize_width, HEIGHT),
            maximize: UiRect::new(maximize_x, 0.0, maximize_width, HEIGHT),
            close: UiRect::new(close_x, 0.0, close_width, HEIGHT),
            drag: UiRect::new(menu_width, 0.0, (pin_x - menu_width).max(0.0), HEIGHT),
        }
    }

    /// The rectangle of one button.
    pub fn rect(&self, button: TitleBarButton) -> UiRect {
        match button {
            TitleBarButton::Menu => self.menu,
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
            TitleBarButton::Menu,
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
    /// When the pointer arrived on the hovered button.
    hovered_since: Option<Instant>,
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
            hovered_since: None,
        }
    }

    /// The button under the pointer, if any.
    pub const fn hovered(&self) -> Option<TitleBarButton> {
        self.hovered
    }

    /// When the hovered button's tooltip is due.
    pub fn hint_due(&self) -> Option<Instant> {
        self.hovered_since.map(|since| since + TOOLTIP_DELAY)
    }

    /// The button whose tooltip should show at `now`: the pointer has
    /// rested on it for [`TOOLTIP_DELAY`] and the bar is visible.
    pub fn hint(&self, now: Instant) -> Option<TitleBarButton> {
        let due = self.hint_due()?;
        (self.visible && now >= due).then_some(self.hovered?)
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
        self.hovered_since = None;
        changed
    }

    /// Updates visibility and hover for the pointer.
    ///
    /// `allowed` is false in modes where the bar must never appear (ghost,
    /// click-through, borderless fullscreen, colour picker). Returns whether
    /// the painted state changed.
    pub fn pointer_moved(&mut self, cursor: Option<DVec2>, viewport: DVec2, allowed: bool) -> bool {
        self.pointer_moved_at(cursor, viewport, allowed, Instant::now())
    }

    /// [`pointer_moved`](Self::pointer_moved) at an explicit time, so the
    /// tooltip delay is testable.
    pub fn pointer_moved_at(
        &mut self,
        cursor: Option<DVec2>,
        viewport: DVec2,
        allowed: bool,
        now: Instant,
    ) -> bool {
        let before = (self.visible, self.hovered);
        let changed = self.track_pointer(cursor, viewport, allowed, before);
        if self.hovered != before.1 {
            // A new button (or none): the delay starts again, so moving
            // along the bar never flashes one tooltip after another.
            self.hovered_since = self.hovered.map(|_| now);
        }
        changed
    }

    fn track_pointer(
        &mut self,
        cursor: Option<DVec2>,
        viewport: DVec2,
        allowed: bool,
        before: (bool, Option<TitleBarButton>),
    ) -> bool {
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
            let left = layout.menu.right() + 6.0;
            let available = (layout.pin.x - left - 12.0).max(0.0);
            if available >= 24.0 {
                let shown = elide(painter, title, available, 12.0);
                painter.text(&shown, left, 10.0, 12.0, TITLE_TEXT);
            }
        }

        for button in [
            TitleBarButton::Menu,
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
        TitleBarButton::Menu => {
            for dy in [-4.0, 0.0, 4.0] {
                painter.line((cx - 6.0, cy + dy), (cx + 6.0, cy + dy), 1.3, rgba);
            }
        }
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
    fn the_menu_button_sits_at_the_left_and_is_not_a_drag_area() {
        let layout = TitleBarLayout::new(viewport());
        assert_eq!(layout.menu.x, 0.0);
        assert_eq!(layout.menu.width, BUTTON_WIDTH);
        assert_eq!(layout.drag.x, layout.menu.right());
        assert_eq!(
            layout.button_at(DVec2::new(20.0, 17.0)),
            Some(TitleBarButton::Menu)
        );
        // A window narrower than the controls never overlaps them.
        let narrow = TitleBarLayout::new(DVec2::new(150.0, 400.0));
        assert!(narrow.menu.right() <= narrow.pin.x);
        assert!(narrow.drag.width >= 0.0);
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
        assert_eq!(layout.button_at(DVec2::new(140.0, 12.0)), None);
        assert!(layout.contains(DVec2::new(140.0, 12.0)));
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

    #[test]
    fn a_button_tooltip_waits_for_the_delay_and_follows_the_state() {
        use super::{TOOLTIP_DELAY, TitleBar, TitleBarButton, TitleBarLayout};
        use std::time::Instant;
        let viewport = DVec2::new(600.0, 400.0);
        let layout = TitleBarLayout::new(viewport);
        let pin = layout.pin;
        let on_pin = DVec2::new(f64::from(pin.x + 5.0), 10.0);
        let mut bar = TitleBar::new();
        let start = Instant::now();

        bar.pointer_moved_at(Some(on_pin), viewport, true, start);
        assert_eq!(bar.hovered(), Some(TitleBarButton::Pin));
        assert_eq!(bar.hint(start), None, "not before the delay");
        assert_eq!(bar.hint(start + TOOLTIP_DELAY), Some(TitleBarButton::Pin));

        // Moving within the same button keeps the timer.
        let still_on_pin = DVec2::new(f64::from(pin.x + 9.0), 12.0);
        bar.pointer_moved_at(Some(still_on_pin), viewport, true, start + TOOLTIP_DELAY);
        assert_eq!(bar.hint(start + TOOLTIP_DELAY), Some(TitleBarButton::Pin));

        // Another button restarts it; leaving the bar clears it.
        let on_close = DVec2::new(f64::from(layout.close.x + 5.0), 10.0);
        let later = start + TOOLTIP_DELAY * 2;
        bar.pointer_moved_at(Some(on_close), viewport, true, later);
        assert_eq!(bar.hint(later), None);
        bar.pointer_moved_at(Some(DVec2::new(300.0, 300.0)), viewport, true, later);
        assert_eq!(bar.hint(later + TOOLTIP_DELAY), None);
        assert_eq!(bar.hint_due(), None);

        assert_eq!(
            TitleBarButton::Pin.tooltip(false, false, Some("Ctrl+Shift+T")),
            "Pin on top (Ctrl+Shift+T)"
        );
        assert_eq!(TitleBarButton::Pin.tooltip(true, false, None), "Unpin");
        assert_eq!(
            TitleBarButton::Maximize.tooltip(false, true, None),
            "Restore"
        );
        assert_eq!(
            TitleBarButton::Maximize.tooltip(false, false, None),
            "Maximize"
        );
        assert_eq!(
            TitleBarButton::Menu.tooltip(false, false, None),
            "Main menu"
        );
    }
}
