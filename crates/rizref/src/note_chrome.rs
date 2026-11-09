//! Layout and hit-testing for the note chrome: the rendered-mode hover
//! toolbar (background · edit · delete) and the edit-mode ✗/✓ pill.
//!
//! Everything is in logical viewport pixels and screen-constant: the chrome
//! never scales with the note or the zoom. The geometry is pure so the app can
//! hit-test it before board input (pressing a button never starts a drag or
//! changes the selection) and the overlay can draw it from the same numbers.

use glam::Vec2;

/// Inset of the chrome from the note's corner, in logical pixels.
pub const CHROME_INSET: f32 = 8.0;

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
    /// The buttons, left to right.
    pub buttons: [(ToolbarButton, ScreenRect); 3],
}

impl ToolbarLayout {
    /// Places the toolbar at the note's top-left, inset; a note too small to
    /// hold it gets the toolbar just outside its top-left corner instead.
    pub fn new(note: ScreenRect) -> Self {
        let width = 3.0 * TOOLBAR_BUTTON + 2.0 * TOOLBAR_GAP + 2.0 * PILL_PAD;
        let height = TOOLBAR_BUTTON + 2.0 * PILL_PAD;
        let size = note.size();
        let fits = size.x >= width + 2.0 * CHROME_INSET && size.y >= height + 2.0 * CHROME_INSET;
        let origin = if fits {
            note.min + Vec2::splat(CHROME_INSET)
        } else {
            Vec2::new(note.min.x, note.min.y - height - TOOLBAR_GAP)
        };
        let pill = ScreenRect::new(origin, Vec2::new(width, height));
        let buttons = ToolbarButton::ORDER.map(|button| {
            let index = ToolbarButton::ORDER
                .iter()
                .position(|b| *b == button)
                .unwrap_or(0) as f32;
            let min =
                origin + Vec2::new(PILL_PAD + index * (TOOLBAR_BUTTON + TOOLBAR_GAP), PILL_PAD);
            (button, ScreenRect::new(min, Vec2::splat(TOOLBAR_BUTTON)))
        });
        Self { pill, buttons }
    }

    /// The button under `point`.
    pub fn hit(&self, point: Vec2) -> Option<ToolbarButton> {
        self.buttons
            .iter()
            .find(|(_, rect)| rect.contains(point))
            .map(|(button, _)| *button)
    }

    /// Whether `point` is anywhere on the toolbar.
    pub fn contains(&self, point: Vec2) -> bool {
        self.pill.contains(point)
    }

    /// The rectangle of one button.
    pub fn button(&self, button: ToolbarButton) -> ScreenRect {
        self.buttons
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
    /// Places the pill at the note's top-right, inset; it moves outside the
    /// note's top-right corner when the note is too small to hold it.
    pub fn new(note: ScreenRect) -> Self {
        let width = 2.0 * PILL_BUTTON + TOOLBAR_GAP + 2.0 * PILL_PAD;
        let height = PILL_BUTTON + 2.0 * PILL_PAD;
        let size = note.size();
        let fits = size.x >= width + 2.0 * CHROME_INSET && size.y >= height + 2.0 * CHROME_INSET;
        let origin = if fits {
            Vec2::new(note.max.x - CHROME_INSET - width, note.min.y + CHROME_INSET)
        } else {
            Vec2::new(note.max.x - width, note.min.y - height - TOOLBAR_GAP)
        };
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
    fn the_toolbar_is_swatch_pencil_trash_from_the_top_left() {
        let layout = ToolbarLayout::new(note());
        let order: Vec<ToolbarButton> = layout.buttons.iter().map(|(b, _)| *b).collect();
        assert_eq!(
            order,
            [
                ToolbarButton::Background,
                ToolbarButton::Edit,
                ToolbarButton::Delete
            ]
        );
        assert_eq!(layout.pill.min, Vec2::splat(100.0 + CHROME_INSET));
        let xs: Vec<f32> = layout.buttons.iter().map(|(_, r)| r.min.x).collect();
        assert!(xs[0] < xs[1] && xs[1] < xs[2]);
        for (button, rect) in layout.buttons {
            assert_eq!(rect.size(), Vec2::splat(TOOLBAR_BUTTON));
            let centre = (rect.min + rect.max) * 0.5;
            assert_eq!(layout.hit(centre), Some(button));
        }
        assert_eq!(layout.hit(Vec2::new(300.0, 300.0)), None);
    }

    #[test]
    fn a_small_note_gets_the_toolbar_outside_its_corner() {
        let small = ScreenRect::new(Vec2::new(50.0, 80.0), Vec2::new(40.0, 30.0));
        let layout = ToolbarLayout::new(small);
        assert!(layout.pill.max.y < small.min.y, "above the note");
        assert_eq!(layout.pill.min.x, small.min.x);
    }

    #[test]
    fn the_pill_puts_confirm_rightmost_at_the_top_right() {
        let layout = PillLayout::new(note());
        assert!(layout.confirm.min.x > layout.cancel.min.x);
        assert!((layout.pill.max.x - (460.0 - CHROME_INSET)).abs() < 0.01);
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
        let near = ToolbarLayout::new(ScreenRect::new(Vec2::ZERO, Vec2::new(2000.0, 1500.0)));
        let far = ToolbarLayout::new(ScreenRect::new(Vec2::ZERO, Vec2::new(200.0, 150.0)));
        assert_eq!(near.buttons[0].1.size(), far.buttons[0].1.size());
    }
}
