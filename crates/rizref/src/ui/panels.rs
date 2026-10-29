//! The two keyboard-first overlays: the F1 shortcut list (with rebinding) and
//! the settings dialog.
//!
//! Both are state machines over a list. The application supplies the live
//! [`Shortcuts`]/[`Settings`] for rendering, and receives a [`PanelResult`] for
//! anything that changes them — the panels never own persisted state, so a
//! failed save can be reported without leaving the UI out of sync.

use glam::DVec2;

use crate::settings::Settings;
use crate::shortcuts::{Key, KeyChord, ShortcutId, Shortcuts};
use crate::ui::paint::{
    ACCENT, ACCENT_SOFT, DIM_TEXT, DISABLED_TEXT, GHOST_ACCENT, HOVER_BG, PANEL_BORDER, Painter,
    TEXT, UiRect,
};

/// Row height in both panels.
const ROW_HEIGHT: f32 = 27.0;
/// Outer padding.
const PADDING: f32 = 22.0;

/// What a panel wants the application to do.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelResult {
    /// Nothing.
    None,
    /// The panel state changed.
    Changed,
    /// Close the panel.
    Close,
    /// Rebind this action to this chord.
    Bind(ShortcutId, KeyChord),
    /// Restore this action's default chord.
    Reset(ShortcutId),
    /// Change one setting by a direction (`+1`/`-1`) or toggle it.
    Adjust(SettingId, i32),
}

/// One row of the settings dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingId {
    /// Grid visibility.
    Grid,
    /// Snapping.
    Snap,
    /// HUD readout.
    Hud,
    /// Scene border.
    Border,
    /// Checkerboard.
    Checkerboard,
    /// Grayscale.
    Grayscale,
    /// Always-on-top.
    AlwaysOnTop,
    /// Ordinary opacity.
    Opacity,
    /// Ghost opacity.
    GhostOpacity,
    /// Decoded-bitmap cache cap.
    DecodeCap,
}

impl SettingId {
    /// Every row, in display order.
    pub const ALL: [Self; 10] = [
        Self::Grid,
        Self::Snap,
        Self::Hud,
        Self::Border,
        Self::Checkerboard,
        Self::Grayscale,
        Self::AlwaysOnTop,
        Self::Opacity,
        Self::GhostOpacity,
        Self::DecodeCap,
    ];

    /// The row label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Grid => "Canvas grid",
            Self::Snap => "Snap to items",
            Self::Hud => "HUD readout",
            Self::Border => "Scene border",
            Self::Checkerboard => "Checkerboard",
            Self::Grayscale => "Grayscale items",
            Self::AlwaysOnTop => "Always on top",
            Self::Opacity => "Window opacity",
            Self::GhostOpacity => "Ghost opacity",
            Self::DecodeCap => "Decode cache limit",
        }
    }

    /// The current value rendered on the right.
    pub fn value(self, settings: &Settings) -> String {
        let on_off = |value: bool| if value { "On" } else { "Off" }.to_owned();
        match self {
            Self::Grid => on_off(settings.grid_visible),
            Self::Snap => on_off(settings.snap_enabled),
            Self::Hud => on_off(settings.hud_visible),
            Self::Border => on_off(settings.scene_border),
            Self::Checkerboard => on_off(settings.checkerboard),
            Self::Grayscale => on_off(settings.grayscale),
            Self::AlwaysOnTop => on_off(settings.always_on_top),
            Self::Opacity => format!("{} %", settings.opacity_percent),
            Self::GhostOpacity => format!("{} %", settings.ghost_opacity_percent),
            Self::DecodeCap => format!("{} MiB", settings.max_decode_bytes / (1024 * 1024)),
        }
    }

    /// Whether the row is a boolean toggle.
    pub const fn is_toggle(self) -> bool {
        matches!(
            self,
            Self::Grid
                | Self::Snap
                | Self::Hud
                | Self::Border
                | Self::Checkerboard
                | Self::Grayscale
                | Self::AlwaysOnTop
        )
    }
}

/// The F1 overlay state.
#[derive(Debug, Clone, Default)]
pub struct ShortcutsPanel {
    /// Case-insensitive filter text.
    pub filter: String,
    /// Selected index into the filtered list.
    pub selected: usize,
    /// First visible row.
    pub scroll: usize,
    /// The action currently waiting for a chord, if any.
    pub rebind: Option<ShortcutId>,
    /// Inline error (a conflicting chord, for example).
    pub error: Option<String>,
}

impl ShortcutsPanel {
    /// A fresh panel.
    pub fn new() -> Self {
        Self::default()
    }

    /// The filtered action list.
    pub fn filtered(&self, shortcuts: &Shortcuts) -> Vec<ShortcutId> {
        let needle = self.filter.trim().to_ascii_lowercase();
        ShortcutId::ALL
            .into_iter()
            .filter(|id| {
                if needle.is_empty() {
                    return true;
                }
                let chord = shortcuts
                    .chord_for(*id)
                    .map(|chord| chord.to_string().to_ascii_lowercase())
                    .unwrap_or_default();
                id.label().to_ascii_lowercase().contains(&needle) || chord.contains(&needle)
            })
            .collect()
    }

    /// The currently selected action, if the list is not empty.
    pub fn selected_action(&self, shortcuts: &Shortcuts) -> Option<ShortcutId> {
        let filtered = self.filtered(shortcuts);
        filtered
            .get(self.selected.min(filtered.len().saturating_sub(1)))
            .copied()
    }

    /// Handles one key. `chord` is the event's key plus modifiers.
    pub fn key(&mut self, chord: KeyChord, shortcuts: &Shortcuts, viewport: DVec2) -> PanelResult {
        if let Some(action) = self.rebind {
            if chord.key == Key::Escape {
                self.rebind = None;
                self.error = None;
                return PanelResult::Changed;
            }
            if chord.key == Key::Enter || chord.key == Key::Space {
                return PanelResult::None;
            }
            self.rebind = None;
            self.error = None;
            return PanelResult::Bind(action, chord);
        }

        match chord.key {
            Key::Escape => return PanelResult::Close,
            Key::Up => {
                let len = self.filtered(shortcuts).len();
                if len > 0 {
                    self.selected = if self.selected == 0 {
                        len - 1
                    } else {
                        self.selected - 1
                    };
                }
                self.keep_visible(shortcuts, viewport);
                return PanelResult::Changed;
            }
            Key::Down => {
                let len = self.filtered(shortcuts).len();
                if len > 0 {
                    self.selected = (self.selected + 1) % len;
                }
                self.keep_visible(shortcuts, viewport);
                return PanelResult::Changed;
            }
            Key::PageUp => {
                self.scroll = self.scroll.saturating_sub(self.visible_rows(viewport));
                self.selected = self.scroll;
                return PanelResult::Changed;
            }
            Key::PageDown => {
                self.scroll += self.visible_rows(viewport);
                self.selected = self
                    .selected
                    .max(self.scroll)
                    .min(self.filtered(shortcuts).len().saturating_sub(1));
                self.keep_visible(shortcuts, viewport);
                return PanelResult::Changed;
            }
            Key::Enter => {
                if let Some(action) = self.selected_action(shortcuts) {
                    self.rebind = Some(action);
                    self.error = None;
                    return PanelResult::Changed;
                }
            }
            Key::Delete => {
                if let Some(action) = self.selected_action(shortcuts) {
                    return PanelResult::Reset(action);
                }
            }
            Key::Backspace if self.filter.pop().is_some() => {
                self.selected = 0;
                self.scroll = 0;
                return PanelResult::Changed;
            }
            _ => {}
        }

        // Plain printable keys extend the search filter.
        if !chord.ctrl && !chord.alt {
            let name = chord.key.name();
            if name.len() == 1 {
                self.filter.push_str(&name.to_ascii_lowercase());
                self.selected = 0;
                self.scroll = 0;
                return PanelResult::Changed;
            }
        }
        PanelResult::None
    }

    /// Number of list rows the panel can show at once.
    pub fn visible_rows(&self, viewport: DVec2) -> usize {
        let panel = panel_rect(viewport);
        let list_top = panel.y + 104.0;
        let list_bottom = panel.bottom() - 56.0;
        (((list_bottom - list_top) / ROW_HEIGHT).floor() as usize).max(3)
    }

    /// Scrolls so the selected row is visible.
    pub fn keep_visible(&mut self, shortcuts: &Shortcuts, viewport: DVec2) {
        let visible = self.visible_rows(viewport);
        let len = self.filtered(shortcuts).len();
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = self.selected.min(len - 1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
        let max_scroll = len.saturating_sub(visible);
        self.scroll = self.scroll.min(max_scroll);
    }

    /// Paints the overlay.
    pub fn paint(&self, painter: &mut Painter, viewport: DVec2, shortcuts: &Shortcuts) {
        dim_backdrop(painter, viewport);
        let panel = panel_rect(viewport);
        painter.panel(panel, 10.0);

        painter.text(
            "Keyboard shortcuts",
            panel.x + PADDING,
            panel.y + 16.0,
            18.0,
            TEXT,
        );
        painter.text_right(
            &format!("{} actions", shortcuts.bound_count()),
            panel.right() - PADDING,
            panel.y + 21.0,
            11.0,
            DIM_TEXT,
        );

        let search = UiRect::new(
            panel.x + PADDING,
            panel.y + 54.0,
            panel.width - PADDING * 2.0,
            32.0,
        );
        painter.fill_round_rect(search, 6.0, [27, 28, 34, 255]);
        painter.stroke_round_rect(search, 6.0, 1.0, PANEL_BORDER);
        let query = if self.filter.is_empty() {
            "Search actions…".to_owned()
        } else {
            self.filter.clone()
        };
        painter.text(
            &query,
            search.x + 10.0,
            search.y + 8.0,
            12.5,
            if self.filter.is_empty() {
                DISABLED_TEXT
            } else {
                TEXT
            },
        );

        let filtered = self.filtered(shortcuts);
        let visible = self.visible_rows(viewport);
        let list_top = panel.y + 104.0;
        for (slot, action) in filtered.iter().skip(self.scroll).take(visible).enumerate() {
            let index = self.scroll + slot;
            let row = UiRect::new(
                panel.x + PADDING - 6.0,
                list_top + slot as f32 * ROW_HEIGHT,
                panel.width - PADDING * 2.0 + 12.0,
                ROW_HEIGHT,
            );
            let selected = index == self.selected;
            if selected {
                painter.fill_round_rect(row.inset(2.0), 5.0, HOVER_BG);
                painter.fill_rect(
                    UiRect::new(row.x + 1.0, row.y + 5.0, 2.0, ROW_HEIGHT - 10.0),
                    ACCENT,
                );
            }
            let y = row.y + (ROW_HEIGHT - 17.0) * 0.5;
            if self.rebind == Some(*action) {
                painter.text("Press a chord…", row.x + 12.0, y, 13.0, GHOST_ACCENT);
            } else {
                painter.text(action.label(), row.x + 12.0, y, 13.0, TEXT);
                let chord = shortcuts
                    .chord_for(*action)
                    .map_or_else(|| "—".to_owned(), |chord| chord.to_string());
                painter.text_right(
                    &chord,
                    row.right() - 12.0,
                    y,
                    12.0,
                    if shortcuts.chord_for(*action).is_some() {
                        DIM_TEXT
                    } else {
                        DISABLED_TEXT
                    },
                );
            }
        }

        if filtered.is_empty() {
            painter.text(
                "No action matches that search.",
                panel.x + PADDING,
                list_top + 6.0,
                12.5,
                DIM_TEXT,
            );
            painter.fill_round_rect(
                UiRect::new(
                    panel.x + PADDING,
                    list_top,
                    panel.width - PADDING * 2.0,
                    ROW_HEIGHT * 2.0,
                ),
                6.0,
                ACCENT_SOFT,
            );
        }

        if let Some(error) = &self.error {
            painter.text(
                error,
                panel.x + PADDING,
                panel.bottom() - 42.0,
                11.5,
                [255, 140, 140, 255],
            );
        }
        painter.text(
            "Type to search · ↑↓ select · Enter rebind · Del restore default · Esc close",
            panel.x + PADDING,
            panel.bottom() - 24.0,
            10.5,
            DIM_TEXT,
        );
    }
}

/// The settings dialog state.
#[derive(Debug, Clone, Default)]
pub struct SettingsPanel {
    /// Selected row index.
    pub selected: usize,
}

impl SettingsPanel {
    /// A fresh dialog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Handles one key.
    pub fn key(&mut self, chord: KeyChord) -> PanelResult {
        match chord.key {
            Key::Escape => PanelResult::Close,
            Key::Up => {
                self.selected = self
                    .selected
                    .checked_sub(1)
                    .unwrap_or(SettingId::ALL.len() - 1);
                PanelResult::Changed
            }
            Key::Down => {
                self.selected = (self.selected + 1) % SettingId::ALL.len();
                PanelResult::Changed
            }
            Key::Left => {
                let id = SettingId::ALL[self.selected.min(SettingId::ALL.len() - 1)];
                PanelResult::Adjust(id, -1)
            }
            Key::Right => {
                let id = SettingId::ALL[self.selected.min(SettingId::ALL.len() - 1)];
                PanelResult::Adjust(id, 1)
            }
            Key::Enter | Key::Space => {
                let id = SettingId::ALL[self.selected.min(SettingId::ALL.len() - 1)];
                PanelResult::Adjust(id, 1)
            }
            _ => PanelResult::None,
        }
    }

    /// Paints the dialog.
    pub fn paint(&self, painter: &mut Painter, viewport: DVec2, settings: &Settings) {
        dim_backdrop(painter, viewport);
        let panel = settings_rect(viewport);
        painter.panel(panel, 10.0);
        painter.text("Settings", panel.x + PADDING, panel.y + 16.0, 18.0, TEXT);
        painter.text(
            "Changes apply immediately and are saved to settings.json",
            panel.x + PADDING,
            panel.y + 44.0,
            11.0,
            DIM_TEXT,
        );

        let list_top = panel.y + 78.0;
        for (index, id) in SettingId::ALL.iter().enumerate() {
            let row = UiRect::new(
                panel.x + PADDING - 6.0,
                list_top + index as f32 * ROW_HEIGHT,
                panel.width - PADDING * 2.0 + 12.0,
                ROW_HEIGHT,
            );
            if index == self.selected {
                painter.fill_round_rect(row.inset(2.0), 5.0, HOVER_BG);
                painter.fill_rect(
                    UiRect::new(row.x + 1.0, row.y + 5.0, 2.0, ROW_HEIGHT - 10.0),
                    ACCENT,
                );
            }
            let y = row.y + (ROW_HEIGHT - 17.0) * 0.5;
            painter.text(id.label(), row.x + 12.0, y, 13.0, TEXT);
            painter.text_right(&id.value(settings), row.right() - 12.0, y, 12.0, DIM_TEXT);
            if index == self.selected {
                painter.text_right(
                    if id.is_toggle() {
                        "Enter toggles"
                    } else {
                        "← →"
                    },
                    row.right() - 76.0,
                    y,
                    10.0,
                    DISABLED_TEXT,
                );
            }
        }

        painter.text(
            "↑↓ select · Enter/← → change · Esc close",
            panel.x + PADDING,
            panel.bottom() - 24.0,
            10.5,
            DIM_TEXT,
        );
        painter.text(
            &format!("Canvas: {}", settings.canvas_theme.label()),
            panel.right() - PADDING - 100.0,
            panel.bottom() - 24.0,
            10.5,
            DIM_TEXT,
        );
    }
}

/// What the close-confirmation dialog wants the application to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseChoice {
    /// Nothing.
    None,
    /// The highlighted button changed.
    Changed,
    /// Keep the document open.
    Cancel,
    /// Close the window and discard the unsaved changes.
    Discard,
}

/// The modal "unsaved changes" dialog shown before a destructive step.
#[derive(Debug, Clone)]
pub struct ConfirmClose {
    /// Document title including the unsaved marker.
    pub title: String,
    /// Label of the destructive button.
    pub discard_label: String,
    /// One line explaining what continuing will do.
    pub note: String,
    /// 0 = Cancel, 1 = the destructive choice.
    selected: usize,
}

impl ConfirmClose {
    /// A dialog for closing `title`, with Cancel selected by default.
    #[allow(dead_code)]
    pub fn new(title: String) -> Self {
        Self::for_action(
            title,
            "Close without saving",
            "Closing now will discard them.",
        )
    }

    /// A dialog for a destructive step that is not a close.
    pub fn for_action(
        title: String,
        discard_label: impl Into<String>,
        note: impl Into<String>,
    ) -> Self {
        Self {
            title,
            discard_label: discard_label.into(),
            note: note.into(),
            selected: 0,
        }
    }

    /// Handles one key.
    pub fn key(&mut self, chord: KeyChord) -> CloseChoice {
        match chord.key {
            Key::Escape => CloseChoice::Cancel,
            Key::Left | Key::Right | Key::Up | Key::Down | Key::Tab => {
                self.selected = 1 - self.selected;
                CloseChoice::Changed
            }
            Key::Enter | Key::Space => self.choice(self.selected),
            _ => CloseChoice::None,
        }
    }

    /// Updates the highlighted button under the pointer.
    pub fn pointer_move(&mut self, position: DVec2, viewport: DVec2) -> bool {
        let Some(index) = self.hit_button(position, viewport) else {
            return false;
        };
        if self.selected == index {
            return false;
        }
        self.selected = index;
        true
    }

    /// Handles a click. Clicks outside the buttons are ignored: the dialog is
    /// modal, so a stray click must not discard the document.
    pub fn pointer_click(&mut self, position: DVec2, viewport: DVec2) -> CloseChoice {
        match self.hit_button(position, viewport) {
            Some(index) => {
                self.selected = index;
                self.choice(index)
            }
            None => CloseChoice::None,
        }
    }

    /// The button index under `position`, if any.
    pub fn hit_button(&self, position: DVec2, viewport: DVec2) -> Option<usize> {
        let (x, y) = (position.x as f32, position.y as f32);
        confirm_buttons(viewport)
            .iter()
            .position(|(rect, _)| rect.contains(x, y))
    }

    fn choice(&self, index: usize) -> CloseChoice {
        if index == 0 {
            CloseChoice::Cancel
        } else {
            CloseChoice::Discard
        }
    }

    /// Paints the dialog above everything else.
    pub fn paint(&self, painter: &mut Painter, viewport: DVec2) {
        dim_backdrop(painter, viewport);
        let panel = confirm_rect(viewport);
        painter.panel(panel, 10.0);
        painter.text(
            "Unsaved changes",
            panel.x + PADDING,
            panel.y + 16.0,
            18.0,
            TEXT,
        );
        painter.text(
            &format!("{} has unsaved changes.", self.title),
            panel.x + PADDING,
            panel.y + 48.0,
            13.0,
            TEXT,
        );
        painter.text(
            &self.note,
            panel.x + PADDING,
            panel.y + 70.0,
            11.5,
            DIM_TEXT,
        );
        painter.text(
            "Esc cancels · ← → switch · Enter confirms",
            panel.x + PADDING,
            panel.bottom() - 68.0,
            10.5,
            DIM_TEXT,
        );

        for (index, (rect, choice)) in confirm_buttons(viewport).iter().enumerate() {
            let selected = index == self.selected;
            if *choice == CloseChoice::Discard {
                painter.fill_round_rect(
                    *rect,
                    6.0,
                    if selected {
                        [206, 48, 32, 255]
                    } else {
                        [166, 40, 29, 255]
                    },
                );
                painter.text_centered(
                    &self.discard_label,
                    *rect,
                    rect.y + 8.0,
                    12.0,
                    [255, 255, 255, 255],
                );
            } else {
                if selected {
                    painter.fill_round_rect(*rect, 6.0, HOVER_BG);
                }
                painter.stroke_round_rect(
                    *rect,
                    6.0,
                    1.0,
                    if selected { ACCENT } else { PANEL_BORDER },
                );
                painter.text_centered("Cancel", *rect, rect.y + 8.0, 12.0, TEXT);
            }
        }
    }
}

/// Whether `position` is inside the shortcut panel.
pub fn shortcuts_contains(viewport: DVec2, position: DVec2) -> bool {
    panel_rect(viewport).contains(position.x as f32, position.y as f32)
}

/// Whether `position` is inside the settings panel.
pub fn settings_contains(viewport: DVec2, position: DVec2) -> bool {
    settings_rect(viewport).contains(position.x as f32, position.y as f32)
}

/// The shortcut panel rectangle, centred and clamped.
fn panel_rect(viewport: DVec2) -> UiRect {
    let width = (viewport.x - 100.0).clamp(420.0, 720.0);
    let height = (viewport.y - 100.0).clamp(320.0, 600.0);
    UiRect::new(
        ((viewport.x - width) * 0.5) as f32,
        ((viewport.y - height) * 0.5) as f32,
        width as f32,
        height as f32,
    )
}

/// The settings panel rectangle.
fn settings_rect(viewport: DVec2) -> UiRect {
    let width = (viewport.x - 140.0).clamp(360.0, 520.0);
    let height = 78.0 + SettingId::ALL.len() as f32 * ROW_HEIGHT + 46.0;
    UiRect::new(
        ((viewport.x - width) * 0.5) as f32,
        ((viewport.y - f64::from(height)).max(40.0) * 0.5) as f32,
        width as f32,
        height,
    )
}

/// The close-confirmation dialog rectangle.
fn confirm_rect(viewport: DVec2) -> UiRect {
    let width = (viewport.x - 80.0).clamp(360.0, 480.0);
    let height = 206.0;
    UiRect::new(
        ((viewport.x - width) * 0.5) as f32,
        ((viewport.y - f64::from(height)) * 0.5).max(8.0) as f32,
        width as f32,
        height,
    )
}

/// The dialog's two buttons: Cancel, then the destructive action.
fn confirm_buttons(viewport: DVec2) -> [(UiRect, CloseChoice); 2] {
    let panel = confirm_rect(viewport);
    let height = 32.0;
    let y = panel.bottom() - PADDING - height;
    let discard_width = 176.0;
    let cancel_width = 96.0;
    let discard = UiRect::new(
        panel.right() - PADDING - discard_width,
        y,
        discard_width,
        height,
    );
    let cancel = UiRect::new(discard.x - 10.0 - cancel_width, y, cancel_width, height);
    [
        (cancel, CloseChoice::Cancel),
        (discard, CloseChoice::Discard),
    ]
}

/// Dims the content behind a modal panel.
fn dim_backdrop(painter: &mut Painter, viewport: DVec2) {
    painter.fill_rect(
        UiRect::new(0.0, 0.0, viewport.x as f32, viewport.y as f32),
        [0, 0, 0, 150],
    );
}

/// The settings value steps for the decode-cache row.
pub const DECODE_CAP_STEPS: [u64; 7] = [
    512 * 1024 * 1024,
    1 << 30,
    2 << 30,
    3 << 30,
    4 << 30,
    6 << 30,
    8 << 30,
];

/// Cycles a numeric setting by one step in `direction`.
pub fn cycle_decode_cap(current: u64, direction: i32) -> u64 {
    let index = DECODE_CAP_STEPS
        .iter()
        .position(|value| *value >= current)
        .unwrap_or(DECODE_CAP_STEPS.len() - 1) as i32;
    let next = (index + direction).clamp(0, DECODE_CAP_STEPS.len() as i32 - 1) as usize;
    DECODE_CAP_STEPS[next]
}

/// Cycles a byte-valued percentage by one step in `direction`.
pub fn cycle_percent(current: u8, direction: i32, min: u8, max: u8, step: u8) -> u8 {
    if direction >= 0 {
        if current >= max {
            min
        } else {
            current.saturating_add(step).min(max)
        }
    } else if current <= min {
        max
    } else {
        current.saturating_sub(step).max(min)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CloseChoice, ConfirmClose, PanelResult, SettingId, SettingsPanel, ShortcutsPanel,
        confirm_buttons, confirm_rect, cycle_decode_cap, cycle_percent,
    };
    use crate::settings::Settings;
    use crate::shortcuts::{Key, KeyChord, ShortcutId, Shortcuts};
    use glam::DVec2;

    fn viewport() -> DVec2 {
        DVec2::new(1280.0, 800.0)
    }

    #[test]
    fn searching_filters_by_label_and_chord() {
        let shortcuts = Shortcuts::new();
        let mut panel = ShortcutsPanel::new();
        panel.filter = "zoom".to_owned();
        let filtered = panel.filtered(&shortcuts);
        assert!(filtered.contains(&ShortcutId::ZoomHundred));
        assert!(!filtered.contains(&ShortcutId::Undo));

        panel.filter = "ctrl+y".to_owned();
        assert_eq!(panel.filtered(&shortcuts), vec![ShortcutId::Redo]);
    }

    #[test]
    fn enter_arms_rebinding_and_the_next_chord_is_returned() {
        let shortcuts = Shortcuts::new();
        let mut panel = ShortcutsPanel::new();
        panel.filter = "undo".to_owned();

        assert!(matches!(
            panel.key(KeyChord::plain(Key::Enter), &shortcuts, viewport()),
            PanelResult::Changed
        ));
        assert_eq!(panel.rebind, Some(ShortcutId::Undo));
        assert_eq!(
            panel.key(KeyChord::ctrl(Key::U), &shortcuts, viewport()),
            PanelResult::Bind(ShortcutId::Undo, KeyChord::ctrl(Key::U))
        );
        assert_eq!(panel.rebind, None);
    }

    #[test]
    fn escape_cancels_a_rebind_before_closing_the_panel() {
        let shortcuts = Shortcuts::new();
        let mut panel = ShortcutsPanel::new();
        panel.rebind = Some(ShortcutId::Undo);

        assert!(matches!(
            panel.key(KeyChord::plain(Key::Escape), &shortcuts, viewport()),
            PanelResult::Changed
        ));
        assert!(matches!(
            panel.key(KeyChord::plain(Key::Escape), &shortcuts, viewport()),
            PanelResult::Close
        ));
    }

    #[test]
    fn typing_extends_the_filter_and_backspace_shrinks_it() {
        let shortcuts = Shortcuts::new();
        let mut panel = ShortcutsPanel::new();
        panel.key(KeyChord::plain(Key::U), &shortcuts, viewport());
        panel.key(KeyChord::plain(Key::N), &shortcuts, viewport());
        assert_eq!(panel.filter, "un");
        assert!(matches!(
            panel.key(KeyChord::plain(Key::Backspace), &shortcuts, viewport()),
            PanelResult::Changed
        ));
        assert_eq!(panel.filter, "u");
    }

    #[test]
    fn scrolling_keeps_the_selection_visible() {
        let shortcuts = Shortcuts::new();
        let mut panel = ShortcutsPanel::new();
        let visible = panel.visible_rows(viewport());
        for _ in 0..visible + 5 {
            panel.key(KeyChord::plain(Key::Down), &shortcuts, viewport());
        }
        assert!(panel.scroll <= panel.selected);
        assert!(panel.selected < panel.scroll + visible);
    }

    #[test]
    fn settings_toggle_and_adjust_return_the_right_row() {
        let mut panel = SettingsPanel::new();
        let first = SettingId::ALL[0];
        assert_eq!(
            panel.key(KeyChord::plain(Key::Enter)),
            PanelResult::Adjust(first, 1)
        );
        panel.key(KeyChord::plain(Key::Down));
        assert_eq!(
            panel.key(KeyChord::plain(Key::Left)),
            PanelResult::Adjust(SettingId::ALL[1], -1)
        );
    }

    #[test]
    fn numeric_steps_wrap_and_clamp() {
        assert_eq!(cycle_percent(100, 1, 20, 100, 10), 20);
        assert_eq!(cycle_percent(20, -1, 20, 100, 10), 100);
        assert_eq!(cycle_percent(60, 1, 20, 100, 10), 70);
        assert_eq!(cycle_decode_cap(2 << 30, 1), 3 << 30);
        assert_eq!(cycle_decode_cap(2 << 30, -1), 1 << 30);
    }

    #[test]
    fn the_value_column_names_every_setting() {
        let settings = Settings::default();
        for id in SettingId::ALL {
            assert!(!id.label().is_empty());
            assert!(!id.value(&settings).is_empty());
        }
    }

    #[test]
    fn the_close_dialog_defaults_to_cancel() {
        let mut dialog = ConfirmClose::new("Document1*.rizref".to_owned());
        assert_eq!(dialog.key(KeyChord::plain(Key::Enter)), CloseChoice::Cancel);
        assert_eq!(
            dialog.key(KeyChord::plain(Key::Escape)),
            CloseChoice::Cancel
        );
    }

    #[test]
    fn the_close_dialog_switches_to_discard() {
        let mut dialog = ConfirmClose::new("Document1*.rizref".to_owned());
        assert_eq!(
            dialog.key(KeyChord::plain(Key::Right)),
            CloseChoice::Changed
        );
        assert_eq!(
            dialog.key(KeyChord::plain(Key::Enter)),
            CloseChoice::Discard
        );
    }

    #[test]
    fn the_close_dialog_hit_tests_its_buttons() {
        let mut dialog = ConfirmClose::new("Document1*.rizref".to_owned());
        let panel = confirm_rect(viewport());
        let buttons = confirm_buttons(viewport());
        let center = |rect: super::UiRect| {
            DVec2::new(
                f64::from(rect.x + rect.width * 0.5),
                f64::from(rect.y + rect.height * 0.5),
            )
        };
        assert_eq!(dialog.hit_button(center(buttons[0].0), viewport()), Some(0));
        assert_eq!(
            dialog.pointer_click(center(buttons[1].0), viewport()),
            CloseChoice::Discard
        );
        assert!(dialog.pointer_move(center(buttons[0].0), viewport()));
        assert_eq!(
            dialog.pointer_click(
                DVec2::new(f64::from(panel.x) + 5.0, f64::from(panel.y) + 5.0),
                viewport()
            ),
            CloseChoice::None
        );
    }
}
