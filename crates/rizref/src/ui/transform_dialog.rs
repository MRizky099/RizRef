//! The size and rotation dialog: type exact numbers for the selection.
//!
//! `Set size…` shows the width and height in board units (an image at 100%
//! is one unit per pixel) with a "Keep aspect ratio" box; `Set rotation…`
//! shows the angle in degrees, clockwise. It is a modal shell overlay: while
//! open it owns input, `Tab` moves between the fields, the arrow keys step
//! the focused number (`Shift` for ten), `Enter` applies as one undo step and
//! `Esc` or a click outside closes without applying.

use glam::{DVec2, Vec2};

use crate::board::{SelectionGeometry, angle_degrees};
use crate::shortcuts::{Key, KeyChord};
use crate::ui::paint::{ACCENT, DIM_TEXT, HOVER_BG, PANEL_BORDER, Painter, TEXT, UiRect};

/// Dialog width in logical pixels.
const WIDTH: f32 = 300.0;

/// Inner padding.
const PAD: f32 = 18.0;

/// Height of a number field.
const FIELD_HEIGHT: f32 = 28.0;

/// Height of a button.
const BUTTON_HEIGHT: f32 = 30.0;

/// Width of a button.
const BUTTON_WIDTH: f32 = 84.0;

/// Where the fields start, left of them sits the label.
const FIELD_X: f32 = 110.0;

/// Longest number the fields accept.
const MAX_CHARS: usize = 10;

/// Which numbers the dialog edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    /// Width and height.
    Size,
    /// The angle.
    Rotation,
}

/// Something in the dialog that takes focus or a click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// The width field.
    Width,
    /// The height field.
    Height,
    /// The angle field.
    Angle,
    /// The "Keep aspect ratio" box.
    KeepAspect,
    /// The cancel button.
    Cancel,
    /// The apply button.
    Apply,
}

/// What a key or click on the dialog asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DialogResult {
    /// Nothing to do.
    None,
    /// The dialog repainted.
    Changed,
    /// Close without applying.
    Closed,
    /// Resize the selection to this many board units.
    ApplySize(Vec2),
    /// Turn the selection to this many radians.
    ApplyRotation(f32),
}

/// The open dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct TransformDialog {
    /// Which numbers it edits.
    pub kind: DialogKind,
    /// The width field's text.
    pub width: String,
    /// The height field's text.
    pub height: String,
    /// The angle field's text.
    pub angle: String,
    /// Whether changing one side changes the other.
    pub keep_aspect: bool,
    /// Whether the aspect is forced (a text item only ever scales).
    pub aspect_locked: bool,
    /// The focused control.
    pub focus: Control,
    /// What the pointer is over.
    pub hovered: Option<Control>,
    /// Whether the next typed character replaces the focused field (it was
    /// just focused, as if its text were selected).
    fresh: bool,
    /// Width over height when the dialog opened.
    aspect: f32,
    bounds: UiRect,
}

/// A number as a field shows it: up to two decimals, no trailing zeros.
pub fn format_number(value: f32) -> String {
    let text = format!("{:.2}", (value * 100.0).round() / 100.0);
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// A field's number, when it holds one.
pub fn parse_number(text: &str) -> Option<f32> {
    let value: f32 = text.trim().replace(',', ".").parse().ok()?;
    value.is_finite().then_some(value)
}

impl TransformDialog {
    /// Opens centred in the viewport on the selection's current numbers.
    pub fn open(kind: DialogKind, geometry: &SelectionGeometry, viewport: DVec2) -> Self {
        let height = match kind {
            DialogKind::Size => 214.0,
            DialogKind::Rotation => 172.0,
        };
        let x = ((viewport.x as f32 - WIDTH) * 0.5).max(4.0);
        let y = ((viewport.y as f32 - height) * 0.5).max(4.0);
        let size = geometry.size;
        Self {
            kind,
            width: format_number(size.x),
            height: format_number(size.y),
            angle: geometry
                .rotation
                .map_or_else(String::new, |angle| format_number(angle_degrees(angle))),
            keep_aspect: true,
            aspect_locked: geometry.text,
            focus: match kind {
                DialogKind::Size => Control::Width,
                DialogKind::Rotation => Control::Angle,
            },
            hovered: None,
            fresh: true,
            aspect: if size.y > 0.0 { size.x / size.y } else { 1.0 },
            bounds: UiRect::new(x, y, WIDTH, height),
        }
    }

    /// The panel rectangle.
    #[cfg(test)]
    pub const fn bounds(&self) -> UiRect {
        self.bounds
    }

    fn controls(&self) -> &'static [Control] {
        match self.kind {
            DialogKind::Size => &[
                Control::Width,
                Control::Height,
                Control::KeepAspect,
                Control::Cancel,
                Control::Apply,
            ],
            DialogKind::Rotation => &[Control::Angle, Control::Cancel, Control::Apply],
        }
    }

    fn rect(&self, control: Control) -> UiRect {
        let b = self.bounds;
        let field = |row: f32| {
            UiRect::new(
                b.x + FIELD_X,
                b.y + 52.0 + row * 40.0,
                WIDTH - FIELD_X - PAD,
                FIELD_HEIGHT,
            )
        };
        let button_y = b.bottom() - PAD - BUTTON_HEIGHT;
        match control {
            Control::Width => field(0.0),
            Control::Height => field(1.0),
            Control::Angle => field(0.0),
            Control::KeepAspect => UiRect::new(b.x + PAD, b.y + 136.0, WIDTH - 2.0 * PAD, 22.0),
            Control::Apply => UiRect::new(
                b.right() - PAD - BUTTON_WIDTH,
                button_y,
                BUTTON_WIDTH,
                BUTTON_HEIGHT,
            ),
            Control::Cancel => UiRect::new(
                b.right() - PAD - 2.0 * BUTTON_WIDTH - 10.0,
                button_y,
                BUTTON_WIDTH,
                BUTTON_HEIGHT,
            ),
        }
    }

    /// What is under a logical point: `None` outside the panel.
    pub fn hit(&self, point: DVec2) -> Option<Option<Control>> {
        let (x, y) = (point.x as f32, point.y as f32);
        if !self.bounds.contains(x, y) {
            return None;
        }
        Some(
            self.controls()
                .iter()
                .copied()
                .find(|control| self.rect(*control).contains(x, y)),
        )
    }

    /// Tracks the pointer; returns whether the paint changed.
    pub fn pointer_moved(&mut self, point: DVec2) -> bool {
        let hovered = self.hit(point).flatten();
        let changed = hovered != self.hovered;
        self.hovered = hovered;
        changed
    }

    /// A left press.
    pub fn click(&mut self, point: DVec2) -> DialogResult {
        match self.hit(point) {
            None => DialogResult::Closed,
            Some(None) => DialogResult::None,
            Some(Some(Control::Cancel)) => DialogResult::Closed,
            Some(Some(Control::Apply)) => self.apply(),
            Some(Some(Control::KeepAspect)) => {
                self.toggle_aspect();
                DialogResult::Changed
            }
            Some(Some(control)) => {
                self.focus = control;
                self.fresh = false;
                DialogResult::Changed
            }
        }
    }

    fn toggle_aspect(&mut self) {
        if !self.aspect_locked {
            self.keep_aspect = !self.keep_aspect;
            if self.keep_aspect {
                self.aspect = self.current_aspect().unwrap_or(self.aspect);
            }
        }
    }

    fn current_aspect(&self) -> Option<f32> {
        let (width, height) = (parse_number(&self.width)?, parse_number(&self.height)?);
        (width > 0.0 && height > 0.0).then(|| width / height)
    }

    fn keeps_aspect(&self) -> bool {
        self.keep_aspect || self.aspect_locked
    }

    /// The numbers to apply, when they are valid.
    pub fn result(&self) -> Option<DialogResult> {
        match self.kind {
            DialogKind::Size => {
                let width = parse_number(&self.width)?;
                let height = parse_number(&self.height)?;
                (width > 0.0 && height > 0.0)
                    .then_some(DialogResult::ApplySize(Vec2::new(width, height)))
            }
            DialogKind::Rotation => {
                let degrees = parse_number(&self.angle)?;
                Some(DialogResult::ApplyRotation(degrees.to_radians()))
            }
        }
    }

    fn apply(&self) -> DialogResult {
        self.result().unwrap_or(DialogResult::None)
    }

    fn field_mut(&mut self, control: Control) -> Option<&mut String> {
        match control {
            Control::Width => Some(&mut self.width),
            Control::Height => Some(&mut self.height),
            Control::Angle => Some(&mut self.angle),
            _ => None,
        }
    }

    /// Keeps the other side in step after `edited` changed.
    fn follow_aspect(&mut self, edited: Control) {
        if self.kind != DialogKind::Size || !self.keeps_aspect() || self.aspect <= 0.0 {
            return;
        }
        match edited {
            Control::Width => {
                if let Some(width) = parse_number(&self.width).filter(|w| *w > 0.0) {
                    self.height = format_number(width / self.aspect);
                }
            }
            Control::Height => {
                if let Some(height) = parse_number(&self.height).filter(|h| *h > 0.0) {
                    self.width = format_number(height * self.aspect);
                }
            }
            _ => {}
        }
    }

    /// A navigation or editing key.
    pub fn key(&mut self, chord: KeyChord) -> DialogResult {
        let controls = self.controls();
        let index = controls
            .iter()
            .position(|control| *control == self.focus)
            .unwrap_or(0);
        match chord.key {
            Key::Escape => DialogResult::Closed,
            Key::Enter => match self.focus {
                Control::Cancel => DialogResult::Closed,
                Control::KeepAspect => {
                    self.toggle_aspect();
                    DialogResult::Changed
                }
                _ => self.apply(),
            },
            Key::Space if self.focus == Control::KeepAspect => {
                self.toggle_aspect();
                DialogResult::Changed
            }
            Key::Tab => {
                let count = controls.len() as isize;
                let step = if chord.shift { -1 } else { 1 };
                self.focus = controls[(index as isize + step).rem_euclid(count) as usize];
                self.fresh = true;
                DialogResult::Changed
            }
            Key::Up | Key::Down => {
                let focus = self.focus;
                let Some(field) = self.field_mut(focus) else {
                    return DialogResult::None;
                };
                let amount = if chord.shift { 10.0 } else { 1.0 };
                let delta = if chord.key == Key::Up {
                    amount
                } else {
                    -amount
                };
                let value = parse_number(field).unwrap_or(0.0) + delta;
                let value = if focus == Control::Angle {
                    value
                } else {
                    value.max(1.0)
                };
                *field = format_number(value);
                self.fresh = false;
                self.follow_aspect(focus);
                DialogResult::Changed
            }
            Key::Backspace => {
                let focus = self.focus;
                let fresh = self.fresh;
                let Some(field) = self.field_mut(focus) else {
                    return DialogResult::None;
                };
                if fresh {
                    field.clear();
                } else {
                    field.pop();
                }
                self.fresh = false;
                self.follow_aspect(focus);
                DialogResult::Changed
            }
            _ => DialogResult::None,
        }
    }

    /// Typed text for the focused field: digits, one decimal point and, for
    /// the angle, a leading minus.
    pub fn type_text(&mut self, text: &str) -> bool {
        let focus = self.focus;
        let fresh = self.fresh;
        let Some(field) = self.field_mut(focus) else {
            return false;
        };
        let mut edited = if fresh { String::new() } else { field.clone() };
        let mut changed = false;
        for c in text.chars() {
            let c = if c == ',' { '.' } else { c };
            let accept = c.is_ascii_digit()
                || (c == '.' && !edited.contains('.'))
                || (c == '-' && focus == Control::Angle && edited.is_empty());
            if accept && edited.len() < MAX_CHARS {
                edited.push(c);
                changed = true;
            }
        }
        if changed {
            *field = edited;
            self.fresh = false;
            self.follow_aspect(focus);
        }
        changed
    }

    /// Paints the dialog over a dimmed board.
    pub fn paint(&self, painter: &mut Painter, viewport: DVec2) {
        painter.fill_rect(
            UiRect::new(0.0, 0.0, viewport.x as f32, viewport.y as f32),
            [0, 0, 0, 110],
        );
        let b = self.bounds;
        painter.panel(b, 10.0);
        let title = match self.kind {
            DialogKind::Size => "Set size",
            DialogKind::Rotation => "Set rotation",
        };
        painter.text(title, b.x + PAD, b.y + 16.0, 16.0, TEXT);

        match self.kind {
            DialogKind::Size => {
                self.paint_field(painter, "Width", Control::Width, &self.width);
                self.paint_field(painter, "Height", Control::Height, &self.height);
                let row = self.rect(Control::KeepAspect);
                let focused = self.focus == Control::KeepAspect;
                if self.hovered == Some(Control::KeepAspect) && !self.aspect_locked {
                    painter.fill_round_rect(row.inset(-3.0), 4.0, HOVER_BG);
                }
                let tick = UiRect::new(row.x, row.y + 3.0, 16.0, 16.0);
                painter.stroke_round_rect(
                    tick,
                    3.0,
                    if focused { 2.0 } else { 1.0 },
                    if focused { ACCENT } else { PANEL_BORDER },
                );
                if self.keeps_aspect() {
                    painter.fill_round_rect(tick.inset(3.0), 2.0, ACCENT);
                }
                let label = if self.aspect_locked {
                    "Keep aspect ratio (text scales evenly)"
                } else {
                    "Keep aspect ratio"
                };
                painter.text(
                    label,
                    row.x + 24.0,
                    row.y + 4.0,
                    12.0,
                    if self.aspect_locked { DIM_TEXT } else { TEXT },
                );
            }
            DialogKind::Rotation => {
                self.paint_field(painter, "Angle (°)", Control::Angle, &self.angle);
                painter.text(
                    "Clockwise. Arrow keys step 1° (Shift 10°).",
                    b.x + PAD,
                    b.y + 92.0,
                    11.0,
                    DIM_TEXT,
                );
            }
        }

        let valid = self.result().is_some();
        for control in [Control::Cancel, Control::Apply] {
            let rect = self.rect(control);
            let focused = self.focus == control;
            let hovered = self.hovered == Some(control);
            if control == Control::Apply {
                let fill = match (valid, hovered || focused) {
                    (false, _) => [60, 64, 76, 255],
                    (true, true) => [96, 172, 255, 255],
                    (true, false) => ACCENT,
                };
                painter.fill_round_rect(rect, 6.0, fill);
                if focused {
                    painter.stroke_round_rect(rect.inset(-2.0), 7.0, 1.5, TEXT);
                }
                painter.text_centered(
                    "Apply",
                    rect,
                    rect.y + 8.0,
                    12.0,
                    if valid {
                        [255, 255, 255, 255]
                    } else {
                        DIM_TEXT
                    },
                );
            } else {
                if hovered || focused {
                    painter.fill_round_rect(rect, 6.0, HOVER_BG);
                }
                painter.stroke_round_rect(
                    rect,
                    6.0,
                    1.0,
                    if focused { ACCENT } else { PANEL_BORDER },
                );
                painter.text_centered("Cancel", rect, rect.y + 8.0, 12.0, TEXT);
            }
        }
    }

    fn paint_field(&self, painter: &mut Painter, label: &str, control: Control, text: &str) {
        let rect = self.rect(control);
        painter.text(label, self.bounds.x + PAD, rect.y + 7.0, 12.5, DIM_TEXT);
        let focused = self.focus == control;
        let valid =
            parse_number(text).is_some_and(|value| control == Control::Angle || value > 0.0);
        painter.fill_round_rect(rect, 4.0, [14, 14, 18, 255]);
        if focused && self.fresh && !text.is_empty() {
            // The whole number is "selected": typing replaces it.
            let width = painter.measure(text, 13.0);
            painter.fill_round_rect(
                UiRect::new(rect.x + 7.0, rect.y + 5.0, width + 2.0, FIELD_HEIGHT - 10.0),
                2.0,
                [77, 158, 255, 90],
            );
        }
        painter.stroke_round_rect(
            rect,
            4.0,
            if focused { 1.5 } else { 1.0 },
            match (valid, focused) {
                (false, _) => [220, 80, 70, 255],
                (true, true) => ACCENT,
                (true, false) => PANEL_BORDER,
            },
        );
        let shown = if focused && !self.fresh {
            format!("{text}|")
        } else {
            text.to_owned()
        };
        painter.text(&shown, rect.x + 8.0, rect.y + 7.0, 13.0, TEXT);
    }
}

#[cfg(test)]
mod tests {
    use super::{Control, DialogKind, DialogResult, TransformDialog, format_number, parse_number};
    use crate::board::SelectionGeometry;
    use crate::shortcuts::{Key, KeyChord};
    use glam::{DVec2, Vec2};

    fn geometry(size: Vec2, rotation: Option<f32>, text: bool) -> SelectionGeometry {
        SelectionGeometry {
            size,
            rotation,
            single: true,
            text,
            rotatable: true,
        }
    }

    fn open(kind: DialogKind, size: Vec2, rotation: Option<f32>) -> TransformDialog {
        TransformDialog::open(
            kind,
            &geometry(size, rotation, false),
            DVec2::new(1280.0, 800.0),
        )
    }

    fn centre(dialog: &TransformDialog, control: Control) -> DVec2 {
        let rect = dialog.rect(control);
        DVec2::new(
            f64::from(rect.x + rect.width * 0.5),
            f64::from(rect.y + rect.height * 0.5),
        )
    }

    #[test]
    fn numbers_format_and_parse_like_people_type_them() {
        assert_eq!(format_number(1322.0), "1322");
        assert_eq!(format_number(12.345), "12.35");
        assert_eq!(format_number(0.5), "0.5");
        assert_eq!(format_number(-0.001), "0");
        assert_eq!(parse_number(" 12,5 "), Some(12.5));
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number(""), None);
    }

    #[test]
    fn typing_replaces_the_number_and_keeps_the_aspect() {
        let mut dialog = open(DialogKind::Size, Vec2::new(400.0, 200.0), None);
        assert_eq!(dialog.focus, Control::Width);
        assert!(dialog.type_text("800"));
        assert_eq!(dialog.width, "800", "the first key replaces the old width");
        assert_eq!(dialog.height, "400", "the height follows");
        assert_eq!(
            dialog.key(KeyChord::plain(Key::Enter)),
            DialogResult::ApplySize(Vec2::new(800.0, 400.0))
        );
    }

    #[test]
    fn without_the_aspect_box_each_side_is_free() {
        let mut dialog = open(DialogKind::Size, Vec2::new(400.0, 200.0), None);
        assert_eq!(
            dialog.click(centre(&dialog, Control::KeepAspect)),
            DialogResult::Changed
        );
        assert!(!dialog.keep_aspect);
        dialog.key(KeyChord::plain(Key::Tab));
        assert_eq!(dialog.focus, Control::Height);
        dialog.type_text("50");
        assert_eq!(
            (dialog.width.as_str(), dialog.height.as_str()),
            ("400", "50")
        );
        assert_eq!(
            dialog.click(centre(&dialog, Control::Apply)),
            DialogResult::ApplySize(Vec2::new(400.0, 50.0))
        );
    }

    #[test]
    fn a_text_item_always_keeps_its_aspect() {
        let mut dialog = TransformDialog::open(
            DialogKind::Size,
            &geometry(Vec2::new(300.0, 100.0), None, true),
            DVec2::new(1280.0, 800.0),
        );
        dialog.click(centre(&dialog, Control::KeepAspect));
        dialog.type_text("600");
        assert_eq!(dialog.height, "200");
    }

    #[test]
    fn an_empty_or_zero_size_cannot_be_applied() {
        let mut dialog = open(DialogKind::Size, Vec2::new(400.0, 200.0), None);
        dialog.key(KeyChord::plain(Key::Backspace));
        assert_eq!(dialog.width, "");
        assert_eq!(dialog.key(KeyChord::plain(Key::Enter)), DialogResult::None);
        dialog.type_text("0");
        assert_eq!(dialog.key(KeyChord::plain(Key::Enter)), DialogResult::None);
    }

    #[test]
    fn the_angle_opens_on_the_item_and_steps_with_the_arrows() {
        let mut dialog = open(
            DialogKind::Rotation,
            Vec2::splat(10.0),
            Some(30.0_f32.to_radians()),
        );
        assert_eq!(dialog.angle, "30");
        let mut shift_up = KeyChord::plain(Key::Up);
        shift_up.shift = true;
        dialog.key(shift_up);
        assert_eq!(dialog.angle, "40");
        dialog.key(KeyChord::plain(Key::Down));
        assert_eq!(dialog.angle, "39");
        assert!(dialog.type_text("5"), "after a step, typing appends");
        assert_eq!(dialog.angle, "395");

        dialog.key(KeyChord::plain(Key::Tab));
        dialog.key(KeyChord::plain(Key::Tab));
        dialog.key(KeyChord::plain(Key::Tab));
        assert_eq!(dialog.focus, Control::Angle, "tab wraps");
        assert!(dialog.type_text("-45"));
        let DialogResult::ApplyRotation(radians) = dialog.key(KeyChord::plain(Key::Enter)) else {
            panic!("applies");
        };
        assert!((radians.to_degrees() + 45.0).abs() < 1.0e-4);
    }

    #[test]
    fn escape_cancel_and_outside_close_without_applying() {
        let mut dialog = open(DialogKind::Rotation, Vec2::splat(10.0), None);
        assert_eq!(dialog.angle, "", "mixed angles open empty");
        assert_eq!(
            dialog.key(KeyChord::plain(Key::Escape)),
            DialogResult::Closed
        );
        assert_eq!(
            dialog.click(centre(&dialog, Control::Cancel)),
            DialogResult::Closed
        );
        assert_eq!(dialog.click(DVec2::new(2.0, 2.0)), DialogResult::Closed);
        let inside = DVec2::new(
            f64::from(dialog.bounds().x + 5.0),
            f64::from(dialog.bounds().y + 5.0),
        );
        assert_eq!(dialog.click(inside), DialogResult::None);
    }
}
