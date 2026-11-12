//! The note background popover: a 6 × 2 palette, a `#rrggbb` field with live
//! validation, and "Pick from screen" (the existing GPU-readback eyedropper).
//!
//! It is a shell overlay like the menu: while open it owns input, board
//! shortcuts are suppressed, click-outside and `Esc` close it without applying,
//! and the arrow keys move through the palette. Applying (a palette click, or
//! `Enter` with a valid field) is one undo step, run by the application.

use glam::DVec2;
use rizref_core::{DEFAULT_NOTE_BACKGROUND, ItemId};

use crate::shortcuts::Key;
use crate::ui::paint::{ACCENT, DIM_TEXT, HOVER_BG, PANEL_BORDER, Painter, TEXT, UiRect};

/// Popover width in logical pixels.
pub const WIDTH: f32 = 240.0;

/// Palette columns.
const COLUMNS: usize = 6;

/// Side of one palette swatch.
const SWATCH: f32 = 30.0;

/// Gap between swatches.
const GAP: f32 = 6.0;

/// Inner padding.
const PAD: f32 = 12.0;

/// Height of the hex field and the pick button.
const FIELD_HEIGHT: f32 = 26.0;

/// The palette: the default card, three neutrals and eight hues.
pub const PRESETS: [[u8; 3]; 12] = [
    DEFAULT_NOTE_BACKGROUND,
    [0x1e, 0x1e, 0x22],
    [0x6b, 0x6b, 0x6b],
    [0xf7, 0xf4, 0xec],
    [0xe5, 0x53, 0x4b],
    [0xf0, 0x88, 0x3e],
    [0xf2, 0xcc, 0x60],
    [0x57, 0xab, 0x5a],
    [0x39, 0xc5, 0xbb],
    [0x53, 0x9b, 0xf5],
    [0x98, 0x6e, 0xe2],
    [0xe2, 0x75, 0xad],
];

/// What a click or key on the popover asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopoverResult {
    /// Nothing to do.
    None,
    /// The popover repainted (hover, typing, focus).
    Changed,
    /// Close without applying.
    Closed,
    /// Apply this colour (and close).
    Apply([u8; 3]),
    /// Arm the eyedropper for this note.
    Pick,
}

/// Something on the popover under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopoverHit {
    /// A palette swatch.
    Swatch(usize),
    /// The hex text field.
    Field,
    /// "Pick from screen".
    Pick,
    /// The panel background.
    Panel,
}

/// The open popover.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteColorPopover {
    /// The note being recoloured.
    pub note: ItemId,
    /// The note's colour when the popover opened.
    pub current: [u8; 3],
    /// The hex field's text.
    pub field: String,
    /// The palette swatch with keyboard focus.
    pub focus: Option<usize>,
    /// What the pointer is over.
    pub hovered: Option<PopoverHit>,
    /// Whether this picks the colour of a text item's characters rather
    /// than a note's background.
    pub text: bool,
    presets: &'static [[u8; 3]; 12],
    bounds: UiRect,
}

impl NoteColorPopover {
    /// Opens under `anchor` (the swatch's bottom-left), clamped into the
    /// viewport.
    pub fn open(note: ItemId, current: [u8; 3], anchor: DVec2, viewport: DVec2) -> Self {
        Self::open_with(note, current, anchor, viewport, &PRESETS, false)
    }

    /// Opens the text colour picker of the text item being edited.
    pub fn open_text(note: ItemId, current: [u8; 3], anchor: DVec2, viewport: DVec2) -> Self {
        Self::open_with(
            note,
            current,
            anchor,
            viewport,
            &crate::rich_text::TEXT_PRESETS,
            true,
        )
    }

    fn open_with(
        note: ItemId,
        current: [u8; 3],
        anchor: DVec2,
        viewport: DVec2,
        presets: &'static [[u8; 3]; 12],
        text: bool,
    ) -> Self {
        let height = Self::height();
        let x = (anchor.x as f32).clamp(4.0, (viewport.x as f32 - WIDTH - 4.0).max(4.0));
        let mut y = anchor.y as f32 + 4.0;
        if y + height > viewport.y as f32 - 4.0 {
            y = (viewport.y as f32 - height - 4.0).max(4.0);
        }
        Self {
            note,
            current,
            field: rizref_format::document::format_background(current),
            focus: presets.iter().position(|preset| *preset == current),
            hovered: None,
            text,
            presets,
            bounds: UiRect::new(x, y, WIDTH, height),
        }
    }

    fn height() -> f32 {
        let rows = PRESETS.len().div_ceil(COLUMNS) as f32;
        PAD + rows * SWATCH + (rows - 1.0) * GAP + PAD + FIELD_HEIGHT + PAD
    }

    /// The panel rectangle.
    #[cfg(test)]
    pub const fn bounds(&self) -> UiRect {
        self.bounds
    }

    fn swatch_rect(&self, index: usize) -> UiRect {
        let column = (index % COLUMNS) as f32;
        let row = (index / COLUMNS) as f32;
        let span = WIDTH - 2.0 * PAD;
        let step = (span - SWATCH) / (COLUMNS as f32 - 1.0);
        UiRect::new(
            self.bounds.x + PAD + column * step,
            self.bounds.y + PAD + row * (SWATCH + GAP),
            SWATCH,
            SWATCH,
        )
    }

    fn field_rect(&self) -> UiRect {
        let y = self.bounds.y + self.bounds.height - PAD - FIELD_HEIGHT;
        UiRect::new(self.bounds.x + PAD, y, 110.0, FIELD_HEIGHT)
    }

    fn pick_rect(&self) -> UiRect {
        let field = self.field_rect();
        let x = field.right() + 8.0;
        UiRect::new(x, field.y, self.bounds.right() - PAD - x, FIELD_HEIGHT)
    }

    /// The parsed field, when it is a valid colour.
    pub fn field_colour(&self) -> Option<[u8; 3]> {
        let text = self.field.trim();
        let text = if text.starts_with('#') {
            text.to_owned()
        } else {
            format!("#{text}")
        };
        rizref_format::document::parse_background(&text)
    }

    /// What is under a logical point.
    pub fn hit(&self, point: DVec2) -> Option<PopoverHit> {
        let (x, y) = (point.x as f32, point.y as f32);
        if !self.bounds.contains(x, y) {
            return None;
        }
        if let Some(index) =
            (0..self.presets.len()).find(|index| self.swatch_rect(*index).contains(x, y))
        {
            return Some(PopoverHit::Swatch(index));
        }
        if self.field_rect().contains(x, y) {
            return Some(PopoverHit::Field);
        }
        if self.pick_rect().contains(x, y) {
            return Some(PopoverHit::Pick);
        }
        Some(PopoverHit::Panel)
    }

    /// Tracks the pointer; returns whether the paint changed.
    pub fn pointer_moved(&mut self, point: DVec2) -> bool {
        let hovered = self.hit(point);
        let changed = hovered != self.hovered;
        self.hovered = hovered;
        changed
    }

    /// A left press.
    pub fn click(&mut self, point: DVec2) -> PopoverResult {
        match self.hit(point) {
            None => PopoverResult::Closed,
            Some(PopoverHit::Swatch(index)) => PopoverResult::Apply(self.presets[index]),
            Some(PopoverHit::Pick) => PopoverResult::Pick,
            Some(PopoverHit::Field | PopoverHit::Panel) => PopoverResult::None,
        }
    }

    /// A navigation or editing key.
    pub fn key(&mut self, key: Key) -> PopoverResult {
        let count = self.presets.len();
        let step = |focus: Option<usize>, delta: isize| {
            let base = focus.unwrap_or(0) as isize;
            Some((base + delta).rem_euclid(count as isize) as usize)
        };
        let moved = match key {
            Key::Escape => return PopoverResult::Closed,
            Key::Enter => {
                return match (self.field_colour(), self.focus) {
                    (Some(colour), _) => PopoverResult::Apply(colour),
                    (None, Some(index)) => PopoverResult::Apply(self.presets[index]),
                    (None, None) => PopoverResult::None,
                };
            }
            Key::Left => step(self.focus, -1),
            Key::Right => step(self.focus, 1),
            Key::Up => step(self.focus, -(COLUMNS as isize)),
            Key::Down => step(self.focus, COLUMNS as isize),
            Key::Backspace => {
                self.field.pop();
                return PopoverResult::Changed;
            }
            _ => return PopoverResult::None,
        };
        self.focus = moved;
        if let Some(index) = moved {
            self.field = rizref_format::document::format_background(self.presets[index]);
        }
        PopoverResult::Changed
    }

    /// Typed text for the hex field: only `#` and hex digits, at most 7.
    pub fn type_text(&mut self, text: &str) -> bool {
        let mut changed = false;
        for c in text.chars() {
            if (c.is_ascii_hexdigit() || (c == '#' && self.field.is_empty()))
                && self.field.len() < 7
            {
                self.field.push(c.to_ascii_lowercase());
                changed = true;
            }
        }
        if changed {
            self.focus = self
                .field_colour()
                .and_then(|colour| self.presets.iter().position(|preset| *preset == colour));
        }
        changed
    }

    /// Paints the popover.
    pub fn paint(&self, painter: &mut Painter) {
        painter.panel(self.bounds, 8.0);
        for (index, preset) in self.presets.iter().enumerate() {
            let rect = self.swatch_rect(index);
            if self.hovered == Some(PopoverHit::Swatch(index)) {
                painter.fill_round_rect(rect.inset(-3.0), 6.0, HOVER_BG);
            }
            painter.fill_round_rect(rect, 5.0, [preset[0], preset[1], preset[2], 255]);
            painter.stroke_round_rect(rect, 5.0, 1.0, PANEL_BORDER);
            if *preset == self.current {
                painter.stroke_round_rect(rect.inset(-2.0), 6.0, 1.5, TEXT);
            }
            if self.focus == Some(index) {
                painter.stroke_round_rect(rect.inset(-4.0), 7.0, 2.0, ACCENT);
            }
        }
        let field = self.field_rect();
        let valid = self.field_colour();
        painter.fill_round_rect(field, 4.0, [14, 14, 18, 255]);
        painter.stroke_round_rect(
            field,
            4.0,
            1.0,
            if valid.is_some() {
                PANEL_BORDER
            } else {
                [220, 80, 70, 255]
            },
        );
        if let Some([r, g, b]) = valid {
            painter.fill_round_rect(
                UiRect::new(field.x + 5.0, field.y + 5.0, 16.0, 16.0),
                3.0,
                [r, g, b, 255],
            );
        }
        painter.text(
            &format!("{}|", self.field),
            field.x + 26.0,
            field.y + 6.0,
            12.0,
            TEXT,
        );
        let pick = self.pick_rect();
        if self.hovered == Some(PopoverHit::Pick) {
            painter.fill_round_rect(pick, 4.0, HOVER_BG);
        }
        painter.stroke_round_rect(pick, 4.0, 1.0, PANEL_BORDER);
        painter.text_centered("Pick from screen", pick, pick.y + 6.0, 11.0, DIM_TEXT);
    }
}

#[cfg(test)]
mod tests {
    use super::{NoteColorPopover, PRESETS, PopoverHit, PopoverResult, WIDTH};
    use crate::shortcuts::Key;
    use glam::DVec2;
    use rizref_core::{DEFAULT_NOTE_BACKGROUND, ItemId};

    fn popover() -> NoteColorPopover {
        NoteColorPopover::open(
            ItemId::from_raw(1),
            DEFAULT_NOTE_BACKGROUND,
            DVec2::new(100.0, 100.0),
            DVec2::new(1280.0, 800.0),
        )
    }

    #[test]
    fn it_opens_on_the_current_colour_and_fits_the_viewport() {
        let open = popover();
        assert_eq!(open.field, "#302c25");
        assert_eq!(open.focus, Some(0));
        assert_eq!(open.bounds().width, WIDTH);
        let clamped = NoteColorPopover::open(
            ItemId::from_raw(1),
            [1, 2, 3],
            DVec2::new(1270.0, 790.0),
            DVec2::new(1280.0, 800.0),
        );
        assert!(clamped.bounds().right() <= 1280.0 && clamped.bounds().bottom() <= 800.0);
        assert_eq!(clamped.focus, None);
    }

    #[test]
    fn a_palette_click_applies_and_outside_closes() {
        let mut open = popover();
        let swatch = open.swatch_rect(4);
        let centre = DVec2::new(
            f64::from(swatch.x + swatch.width * 0.5),
            f64::from(swatch.y + swatch.height * 0.5),
        );
        assert_eq!(open.hit(centre), Some(PopoverHit::Swatch(4)));
        assert_eq!(open.click(centre), PopoverResult::Apply(PRESETS[4]));
        assert_eq!(open.click(DVec2::new(5.0, 5.0)), PopoverResult::Closed);
    }

    #[test]
    fn the_hex_field_validates_and_enter_applies() {
        let mut open = popover();
        for _ in 0..7 {
            open.key(Key::Backspace);
        }
        assert_eq!(open.field_colour(), None);
        assert_eq!(
            open.key(Key::Enter),
            PopoverResult::Apply(DEFAULT_NOTE_BACKGROUND),
            "focus wins"
        );
        open.focus = None;
        assert_eq!(open.key(Key::Enter), PopoverResult::None);
        assert!(open.type_text("#12zz34ab"));
        assert_eq!(open.field, "#1234ab");
        assert_eq!(
            open.key(Key::Enter),
            PopoverResult::Apply([0x12, 0x34, 0xab])
        );
        open.field = "fff".into();
        assert_eq!(
            open.field_colour(),
            Some([255, 255, 255]),
            "short form without #"
        );
    }

    #[test]
    fn arrow_keys_walk_the_palette_and_escape_closes() {
        let mut open = popover();
        assert_eq!(open.key(Key::Right), PopoverResult::Changed);
        assert_eq!(open.focus, Some(1));
        open.key(Key::Down);
        assert_eq!(open.focus, Some(7));
        assert_eq!(
            open.field,
            rizref_format::document::format_background(PRESETS[7])
        );
        open.key(Key::Left);
        open.key(Key::Up);
        assert_eq!(open.focus, Some(0));
        open.key(Key::Left);
        assert_eq!(open.focus, Some(PRESETS.len() - 1), "wraps");
        assert_eq!(open.key(Key::Escape), PopoverResult::Closed);
    }
}
