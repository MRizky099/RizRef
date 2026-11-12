//! Layout and hit-testing for the note chrome: the rendered-mode hover
//! toolbar (background · edit · delete), the edit-mode ✗/✓ pill, and the
//! text item's format bar (bold · italic · underline · colour · ✗ · ✓).
//!
//! Everything is in logical viewport pixels and screen-constant: the chrome
//! never scales with the note or the zoom. The geometry is pure so the app can
//! hit-test it before board input (pressing a button never starts a drag or
//! changes the selection) and the overlay can draw it from the same numbers.

use glam::Vec2;

/// Gap between the chrome and the note's edge, in logical pixels. It clears
/// the selection frame's corner handles (7 px), so a handle is never under a
/// toolbar button.
pub const CHROME_INSET: f32 = 8.0;

/// Half-width of the strip above the note's top centre kept clear for the
/// selection frame's rotate handle (7 px radius plus a margin).
const ROTATE_CLEARANCE: f32 = 12.0;

/// Side of a toolbar button.
pub const TOOLBAR_BUTTON: f32 = 26.0;

/// Gap between toolbar buttons.
pub const TOOLBAR_GAP: f32 = 4.0;

/// Side of an edit-pill button.
pub const PILL_BUTTON: f32 = 28.0;

/// Padding inside a pill around its buttons.
const PILL_PAD: f32 = 3.0;

/// An axis-aligned screen rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRect {
    /// Top-left corner.
    pub min: Vec2,
    /// Bottom-right corner.
    pub max: Vec2,
}

impl ScreenRect {
    /// A rectangle from its top-left corner and size.
    pub fn new(min: Vec2, size: Vec2) -> Self {
        Self {
            min,
            max: min + size,
        }
    }

    /// Whether `point` is inside (edges included).
    pub fn contains(&self, point: Vec2) -> bool {
        point.x >= self.min.x
            && point.y >= self.min.y
            && point.x <= self.max.x
            && point.y <= self.max.y
    }

    /// Width and height.
    #[cfg(test)]
    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    /// The bounding box of some points.
    pub fn bounding(points: &[Vec2]) -> Self {
        let min = points
            .iter()
            .copied()
            .reduce(Vec2::min)
            .unwrap_or(Vec2::ZERO);
        let max = points
            .iter()
            .copied()
            .reduce(Vec2::max)
            .unwrap_or(Vec2::ZERO);
        Self { min, max }
    }
}

/// Where chrome of `size` goes for `note`: outside the card so it never
/// covers the note's text — above it at the `left` or right corner, or below
/// it when above would leave the visible area (`top` is the topmost visible
/// logical y) or cover the rotate handle at the top centre. A note filling
/// the whole window has no outside left on screen (`bottom` is the lowest
/// visible y): the chrome is then pinned to the window's top edge.
fn outside(note: ScreenRect, size: Vec2, left: bool, top: f32, bottom: f32) -> Vec2 {
    let x = if left {
        note.min.x
    } else {
        note.max.x - size.x
    };
    let above = Vec2::new(x, note.min.y - CHROME_INSET - size.y);
    let centre = (note.min.x + note.max.x) * 0.5;
    let clear_of_rotate = x + size.x <= centre - ROTATE_CLEARANCE || x >= centre + ROTATE_CLEARANCE;
    let below = Vec2::new(x, note.max.y + CHROME_INSET);
    if above.y >= top && clear_of_rotate {
        above
    } else if below.y + size.y <= bottom || above.y < top && note.max.y < bottom {
        below
    } else if above.y >= top {
        above
    } else {
        Vec2::new(x, top + CHROME_INSET)
    }
}

/// One hover-toolbar button, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarButton {
    /// The background swatch; opens the colour popover.
    Background,
    /// The pencil; enters edit mode.
    Edit,
    /// The trash can; deletes the note.
    Delete,
}

impl ToolbarButton {
    /// Left-to-right order.
    pub const ORDER: [Self; 3] = [Self::Background, Self::Edit, Self::Delete];

    /// Tooltip and accessible name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Background => "Note background",
            Self::Edit => "Edit note",
            Self::Delete => "Delete note",
        }
    }
}

/// One edit-pill button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillButton {
    /// ✗: cancel the edit.
    Cancel,
    /// ✓: commit the edit.
    Confirm,
}

impl PillButton {
    /// Accessible name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cancel => "Cancel note edit",
            Self::Confirm => "Confirm note edit",
        }
    }
}

/// The hover toolbar placed for one note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolbarLayout {
    /// The translucent pill behind the buttons.
    pub pill: ScreenRect,
    /// The buttons, left to right; only the first `count` are shown.
    buttons: [(ToolbarButton, ScreenRect); 3],
    count: usize,
}

impl ToolbarLayout {
    /// Places the toolbar outside the note, above its top-left corner (below
    /// its bottom-left when there is no room above): drawn over the card it
    /// covered the note's first line (phase 07 manual test).
    pub fn new(note: ScreenRect, top: f32, bottom: f32) -> Self {
        Self::with_buttons(note, top, bottom, &ToolbarButton::ORDER)
    }

    /// The toolbar of a text item: it has no card, so no background swatch.
    pub fn for_text(note: ScreenRect, top: f32, bottom: f32) -> Self {
        Self::with_buttons(
            note,
            top,
            bottom,
            &[ToolbarButton::Edit, ToolbarButton::Delete],
        )
    }

    fn with_buttons(note: ScreenRect, top: f32, bottom: f32, order: &[ToolbarButton]) -> Self {
        let count = order.len().min(3);
        let n = count as f32;
        let width = n * TOOLBAR_BUTTON + (n - 1.0) * TOOLBAR_GAP + 2.0 * PILL_PAD;
        let height = TOOLBAR_BUTTON + 2.0 * PILL_PAD;
        let origin = outside(note, Vec2::new(width, height), true, top, bottom);
        let pill = ScreenRect::new(origin, Vec2::new(width, height));
        let mut buttons = [(ToolbarButton::Delete, ScreenRect::new(origin, Vec2::ZERO)); 3];
        for (index, button) in order.iter().take(3).enumerate() {
            let min = origin
                + Vec2::new(
                    PILL_PAD + index as f32 * (TOOLBAR_BUTTON + TOOLBAR_GAP),
                    PILL_PAD,
                );
            buttons[index] = (*button, ScreenRect::new(min, Vec2::splat(TOOLBAR_BUTTON)));
        }
        Self {
            pill,
            buttons,
            count,
        }
    }

    /// The shown buttons, left to right.
    pub fn buttons(&self) -> &[(ToolbarButton, ScreenRect)] {
        &self.buttons[..self.count]
    }

    /// The button under `point`.
    pub fn hit(&self, point: Vec2) -> Option<ToolbarButton> {
        self.buttons()
            .iter()
            .find(|(_, rect)| rect.contains(point))
            .map(|(button, _)| *button)
    }

    /// Whether `point` is on the toolbar or the gap between it and the note,
    /// so the pointer can travel from the note to a button without the
    /// toolbar disappearing on the way.
    pub fn contains(&self, point: Vec2) -> bool {
        let bridge = Vec2::new(0.0, CHROME_INSET);
        ScreenRect {
            min: self.pill.min - bridge,
            max: self.pill.max + bridge,
        }
        .contains(point)
    }

    /// The rectangle of one button.
    pub fn button(&self, button: ToolbarButton) -> ScreenRect {
        self.buttons()
            .iter()
            .find(|(b, _)| *b == button)
            .map_or(self.pill, |(_, rect)| *rect)
    }
}

/// The edit-mode pill placed for one note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PillLayout {
    /// The pill behind the buttons.
    pub pill: ScreenRect,
    /// ✗, left.
    pub cancel: ScreenRect,
    /// ✓, rightmost.
    pub confirm: ScreenRect,
}

impl PillLayout {
    /// Places the pill outside the note, above its top-right corner (below
    /// its bottom-right when there is no room above), clear of the text.
    pub fn new(note: ScreenRect, top: f32, bottom: f32) -> Self {
        let width = 2.0 * PILL_BUTTON + TOOLBAR_GAP + 2.0 * PILL_PAD;
        let height = PILL_BUTTON + 2.0 * PILL_PAD;
        let origin = outside(note, Vec2::new(width, height), false, top, bottom);
        let pill = ScreenRect::new(origin, Vec2::new(width, height));
        let cancel = ScreenRect::new(origin + Vec2::splat(PILL_PAD), Vec2::splat(PILL_BUTTON));
        let confirm = ScreenRect::new(
            origin + Vec2::new(PILL_PAD + PILL_BUTTON + TOOLBAR_GAP, PILL_PAD),
            Vec2::splat(PILL_BUTTON),
        );
        Self {
            pill,
            cancel,
            confirm,
        }
    }

    /// The button under `point`.
    pub fn hit(&self, point: Vec2) -> Option<PillButton> {
        if self.confirm.contains(point) {
            Some(PillButton::Confirm)
        } else if self.cancel.contains(point) {
            Some(PillButton::Cancel)
        } else {
            None
        }
    }
}

/// One button of a text item's format bar, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextBarButton {
    /// **B**: toggle bold.
    Bold,
    /// *I*: toggle italic.
    Italic,
    /// U̲: toggle underline.
    Underline,
    /// A−: step the font size down.
    Smaller,
    /// A+: step the font size up.
    Larger,
    /// The colour swatch; opens the text colour popover.
    Colour,
    /// ✗: cancel the edit.
    Cancel,
    /// ✓: commit the edit.
    Confirm,
}

impl TextBarButton {
    /// Left-to-right order.
    pub const ORDER: [Self; 8] = [
        Self::Bold,
        Self::Italic,
        Self::Underline,
        Self::Smaller,
        Self::Larger,
        Self::Colour,
        Self::Cancel,
        Self::Confirm,
    ];

    /// Tooltip and accessible name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bold => "Bold (Ctrl+B)",
            Self::Italic => "Italic (Ctrl+I)",
            Self::Underline => "Underline (Ctrl+U)",
            Self::Smaller => "Smaller text (Ctrl+[)",
            Self::Larger => "Larger text (Ctrl+])",
            Self::Colour => "Text colour",
            Self::Cancel => "Cancel text edit (Esc)",
            Self::Confirm => "Confirm text edit (Ctrl+Enter)",
        }
    }
}

/// Extra space between the format buttons and the ✗/✓ pair.
const TEXT_BAR_DIVIDER: f32 = 10.0;

/// The format bar placed for one text item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextBarLayout {
    /// The pill behind the buttons.
    pub pill: ScreenRect,
    /// The buttons, left to right.
    pub buttons: [(TextBarButton, ScreenRect); 8],
    /// Where the divider between the format and edit buttons is drawn.
    pub divider_x: f32,
}

impl TextBarLayout {
    /// Places the bar outside the item, above its top-left corner (below it
    /// when there is no room above), clear of the text being typed.
    pub fn new(note: ScreenRect, top: f32, bottom: f32) -> Self {
        let width = 8.0 * PILL_BUTTON + 7.0 * TOOLBAR_GAP + TEXT_BAR_DIVIDER + 2.0 * PILL_PAD;
        let height = PILL_BUTTON + 2.0 * PILL_PAD;
        let origin = outside(note, Vec2::new(width, height), true, top, bottom);
        let pill = ScreenRect::new(origin, Vec2::new(width, height));
        let mut x = origin.x + PILL_PAD;
        let mut divider_x = x;
        let buttons = TextBarButton::ORDER.map(|button| {
            if button == TextBarButton::Cancel {
                divider_x = x + TEXT_BAR_DIVIDER * 0.5 - TOOLBAR_GAP * 0.5;
                x += TEXT_BAR_DIVIDER;
            }
            let rect = ScreenRect::new(Vec2::new(x, origin.y + PILL_PAD), Vec2::splat(PILL_BUTTON));
            x += PILL_BUTTON + TOOLBAR_GAP;
            (button, rect)
        });
        Self {
            pill,
            buttons,
            divider_x,
        }
    }

    /// The button under `point`.
    pub fn hit(&self, point: Vec2) -> Option<TextBarButton> {
        self.buttons
            .iter()
            .find(|(_, rect)| rect.contains(point))
            .map(|(button, _)| *button)
    }

    /// The rectangle of one button.
    pub fn button(&self, button: TextBarButton) -> ScreenRect {
        self.buttons
            .iter()
            .find(|(b, _)| *b == button)
            .map_or(self.pill, |(_, rect)| *rect)
    }
}

/// What the overlay should draw for notes this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoteChrome {
    /// The rendered-mode toolbar.
    Toolbar {
        /// Placement.
        layout: ToolbarLayout,
        /// The note's background, for the swatch.
        background: [u8; 3],
        /// The button under the pointer.
        hovered: Option<ToolbarButton>,
    },
    /// The edit-mode pill.
    Pill {
        /// Placement.
        layout: PillLayout,
        /// The button under the pointer.
        hovered: Option<PillButton>,
    },
    /// A text item's format bar while it is edited.
    TextBar {
        /// Placement.
        layout: TextBarLayout,
        /// What the toggles show as on, and the colour swatch.
        bold: bool,
        /// Italic is on.
        italic: bool,
        /// Underline is on.
        underline: bool,
        /// The colour swatch.
        colour: [u8; 3],
        /// The button under the pointer.
        hovered: Option<TextBarButton>,
    },
}

#[cfg(test)]
mod tests {
    use super::{
        CHROME_INSET, PillButton, PillLayout, ScreenRect, TOOLBAR_BUTTON, ToolbarButton,
        ToolbarLayout,
    };
    use glam::Vec2;

    fn note() -> ScreenRect {
        ScreenRect::new(Vec2::new(100.0, 100.0), Vec2::new(360.0, 220.0))
    }

    #[test]
    fn the_toolbar_is_swatch_pencil_trash_above_the_top_left() {
        let layout = ToolbarLayout::new(note(), 0.0, 10_000.0);
        let order: Vec<ToolbarButton> = layout.buttons().iter().map(|(b, _)| *b).collect();
        assert_eq!(
            order,
            [
                ToolbarButton::Background,
                ToolbarButton::Edit,
                ToolbarButton::Delete
            ]
        );
        assert_eq!(layout.pill.min.x, 100.0);
        assert_eq!(layout.pill.max.y, 100.0 - CHROME_INSET, "above the card");
        let xs: Vec<f32> = layout.buttons().iter().map(|(_, r)| r.min.x).collect();
        assert!(xs[0] < xs[1] && xs[1] < xs[2]);
        for &(button, rect) in layout.buttons() {
            assert_eq!(rect.size(), Vec2::splat(TOOLBAR_BUTTON));
            let centre = (rect.min + rect.max) * 0.5;
            assert_eq!(layout.hit(centre), Some(button));
        }
        assert_eq!(layout.hit(Vec2::new(300.0, 300.0)), None);
    }

    fn overlaps(a: ScreenRect, b: ScreenRect) -> bool {
        a.min.x < b.max.x && b.min.x < a.max.x && a.min.y < b.max.y && b.min.y < a.max.y
    }

    #[test]
    fn the_chrome_never_covers_the_note_at_any_zoom() {
        // From a 20 px speck to a note far bigger than the window, near the
        // top edge and away from it: neither the toolbar nor the pill
        // overlaps the card.
        for size in [20.0, 60.0, 150.0, 360.0, 4000.0] {
            for top_y in [-500.0, 0.0, 10.0, 60.0, 300.0] {
                let note = ScreenRect::new(Vec2::new(50.0, top_y), Vec2::new(size, size * 0.6));
                let toolbar = ToolbarLayout::new(note, 0.0, 10_000.0);
                let pill = PillLayout::new(note, 0.0, 10_000.0);
                assert!(!overlaps(toolbar.pill, note), "toolbar {size} at {top_y}");
                assert!(!overlaps(pill.pill, note), "pill {size} at {top_y}");
            }
        }
    }

    #[test]
    fn without_room_above_the_chrome_flips_below() {
        let note = ScreenRect::new(Vec2::new(50.0, 10.0), Vec2::new(360.0, 200.0));
        let toolbar = ToolbarLayout::new(note, 0.0, 10_000.0);
        assert_eq!(toolbar.pill.min.y, note.max.y + CHROME_INSET);
        let pill = PillLayout::new(note, 0.0, 10_000.0);
        assert_eq!(pill.pill.min.y, note.max.y + CHROME_INSET);
    }

    #[test]
    fn a_note_filling_the_window_pins_the_chrome_to_its_top() {
        let huge = ScreenRect::new(Vec2::new(-100.0, -300.0), Vec2::new(2000.0, 2000.0));
        let toolbar = ToolbarLayout::new(huge, 0.0, 400.0);
        assert_eq!(toolbar.pill.min.y, CHROME_INSET, "visible, at the top");
    }

    #[test]
    fn a_narrow_note_keeps_its_rotate_handle_clear() {
        // The toolbar would reach the top-centre rotate handle: it goes below.
        let narrow = ScreenRect::new(Vec2::new(50.0, 200.0), Vec2::new(80.0, 60.0));
        let layout = ToolbarLayout::new(narrow, 0.0, 10_000.0);
        assert!(layout.pill.min.y > narrow.max.y, "below the note");
        assert_eq!(layout.pill.min.x, narrow.min.x);
    }

    #[test]
    fn the_gap_between_note_and_toolbar_keeps_the_toolbar() {
        let note = note();
        let layout = ToolbarLayout::new(note, 0.0, 10_000.0);
        // A point in the gap, straight below the first button.
        let gap = Vec2::new(layout.buttons()[0].1.min.x + 2.0, note.min.y - 2.0);
        assert!(layout.contains(gap));
    }

    #[test]
    fn the_pill_puts_confirm_rightmost_above_the_top_right() {
        let layout = PillLayout::new(note(), 0.0, 10_000.0);
        assert!(layout.confirm.min.x > layout.cancel.min.x);
        assert!((layout.pill.max.x - 460.0).abs() < 0.01);
        assert_eq!(layout.pill.max.y, 100.0 - CHROME_INSET);
        assert_eq!(layout.cancel.size(), Vec2::splat(28.0));
        let centre = |rect: ScreenRect| (rect.min + rect.max) * 0.5;
        assert_eq!(
            layout.hit(centre(layout.confirm)),
            Some(PillButton::Confirm)
        );
        assert_eq!(layout.hit(centre(layout.cancel)), Some(PillButton::Cancel));
        assert_eq!(PillButton::Confirm.label(), "Confirm note edit");
    }

    #[test]
    fn the_layout_is_screen_constant() {
        // The same note at two zoom levels: the buttons keep their size.
        let near = ToolbarLayout::new(
            ScreenRect::new(Vec2::ZERO, Vec2::new(2000.0, 1500.0)),
            0.0,
            10_000.0,
        );
        let far = ToolbarLayout::new(
            ScreenRect::new(Vec2::ZERO, Vec2::new(200.0, 150.0)),
            0.0,
            10_000.0,
        );
        assert_eq!(near.buttons()[0].1.size(), far.buttons()[0].1.size());
    }

    #[test]
    fn a_text_toolbar_has_no_background_swatch() {
        let layout = ToolbarLayout::for_text(note(), 0.0, 10_000.0);
        let order: Vec<ToolbarButton> = layout.buttons().iter().map(|(b, _)| *b).collect();
        assert_eq!(order, [ToolbarButton::Edit, ToolbarButton::Delete]);
        assert!(layout.pill.size().x < ToolbarLayout::new(note(), 0.0, 10_000.0).pill.size().x);
    }

    #[test]
    fn the_text_bar_is_format_buttons_then_cancel_and_confirm_outside_the_item() {
        use super::{TextBarButton, TextBarLayout};
        let layout = TextBarLayout::new(note(), 0.0, 10_000.0);
        assert!(!overlaps(layout.pill, note()));
        let xs: Vec<f32> = layout.buttons.iter().map(|(_, r)| r.min.x).collect();
        assert!(xs.windows(2).all(|pair| pair[0] < pair[1]));
        for (button, rect) in layout.buttons {
            assert_eq!(layout.hit((rect.min + rect.max) * 0.5), Some(button));
        }
        let cancel = layout.button(TextBarButton::Cancel);
        let colour = layout.button(TextBarButton::Colour);
        assert!(layout.divider_x > colour.max.x && layout.divider_x < cancel.min.x);
    }
}
