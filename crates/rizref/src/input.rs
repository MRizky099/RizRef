//! The pointer input map: what the user does with the mouse → what the app does.
//!
//! Keyboard bindings are *not* here. They live in [`crate::shortcuts`], which
//! owns the rebindable table and the conflict rules; this module only keeps the
//! gestures that are structural rather than user-bindable (a wheel notch is a
//! wheel notch, a middle-button drag is a pan). Keeping the pointer table
//! separate means the shortcut editor can never accidentally rebind panning to
//! a key that is already taken by text input.
//!
//! winit types are kept out of the tables themselves so both mappings stay
//! unit-testable without a window server.

use crate::view::PanKind;

/// One action the canvas can take.
///
/// View commands drive the camera; item commands go through the board's command
/// stack, so every one of them is undoable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// Start a pan drag with the given gesture.
    BeginPan(PanKind),
    /// End the active pan drag.
    EndPan,
    /// Zoom by `notches` wheel notches, anchored at the cursor.
    ZoomNotch(i32),
    /// Turn the view by `notches` wheel notches (15 degrees per notch).
    RotateNotch(i32),
    /// Frame all content with padding (`F`), or zoom to fit (`1`).
    FitContent,
    /// Zoom to 100% (`0`).
    ZoomHundred,
    /// Reset the view rotation to zero (`2`).
    ResetRotation,
    /// Reset zoom to 100%, rotation to zero and centre on the content
    /// (`ctrl+0`).
    ResetView,
    /// `ctrl+A`.
    SelectAll,
    /// `esc`.
    Deselect,
    /// `ctrl+Z`.
    Undo,
    /// `ctrl+Y` or `ctrl+shift+Z`.
    Redo,
    /// `ctrl+D`.
    DuplicateSelection,
    /// `del` or `backspace`.
    DeleteSelection,
    /// Arrow-key nudge by `pixels` screen pixels in the given direction.
    Nudge {
        /// Horizontal step, -1, 0 or 1.
        dx: i32,
        /// Vertical step, -1, 0 or 1.
        dy: i32,
        /// Screen pixels per step.
        pixels: f32,
    },
    /// `shift+H`.
    FlipHorizontal,
    /// `shift+V`.
    FlipVertical,
    /// `ctrl+shift+R`.
    ResetTransform,
    /// `ctrl+shift+W`.
    MatchWidth,
    /// `ctrl+shift+H`.
    MatchHeight,
    /// `ctrl+G`.
    GroupSelection,
    /// `ctrl+shift+G`.
    UngroupSelection,
    /// `pgup`.
    RaiseSelection,
    /// `pgdn`.
    LowerSelection,
    /// `home`.
    RaiseToTop,
    /// `end`.
    LowerToBottom,
    /// `3` — grid.
    ArrangeGrid,
    /// `4` — largest centred.
    ArrangeHierarchy,
    /// `5` — evenly spaced row.
    ArrangeSpread,
    /// `6` — pile on one point.
    ArrangeStack,
    /// `N` — create a note and start editing it.
    CreateNote,
    /// `T` — add a text item and start typing into it.
    CreateText,
    /// `ctrl+shift+left` — flush left.
    AlignLeft,
    /// `ctrl+shift+right` — flush right.
    AlignRight,
    /// `ctrl+shift+up` — flush top.
    AlignTop,
    /// `ctrl+shift+down` — flush bottom.
    AlignBottom,
    /// `ctrl+shift+,` — distribute horizontally.
    DistributeHorizontal,
    /// `ctrl+shift+.` — distribute vertically.
    DistributeVertical,
}

/// A pointer gesture the table recognises, before modifiers are applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A mouse-wheel notch, with or without alt.
    Wheel {
        /// Alt was held.
        alt: bool,
    },
    /// A mouse button press or release.
    Button {
        /// The button.
        button: Button,
        /// `true` for press, `false` for release.
        pressed: bool,
        /// Space was held (the left-drag pan modifier).
        space_held: bool,
    },
}

/// Mouse buttons the canvas listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// The left button.
    Left,
    /// The middle button.
    Middle,
}

/// One row of the pointer table.
struct Row {
    trigger: fn(&Trigger) -> bool,
    command: Command,
}

/// The structural pointer scheme.
///
/// Rows are matched top to bottom; order matters where triggers overlap:
/// space+left comes before plain left.
const TABLE: &[Row] = &[
    Row {
        trigger: |t| matches!(t, Trigger::Wheel { alt: true }),
        command: Command::RotateNotch(1),
    },
    Row {
        trigger: |t| matches!(t, Trigger::Wheel { alt: false }),
        command: Command::ZoomNotch(1),
    },
    Row {
        trigger: |t| {
            matches!(
                t,
                Trigger::Button {
                    button: Button::Middle,
                    pressed: true,
                    ..
                }
            )
        },
        command: Command::BeginPan(PanKind::MiddleDrag),
    },
    Row {
        trigger: |t| {
            matches!(
                t,
                Trigger::Button {
                    button: Button::Left,
                    pressed: true,
                    space_held: true
                }
            )
        },
        command: Command::BeginPan(PanKind::SpaceDrag),
    },
    Row {
        trigger: |t| {
            matches!(
                t,
                Trigger::Button {
                    button: Button::Middle | Button::Left,
                    pressed: false,
                    ..
                }
            )
        },
        command: Command::EndPan,
    },
];

/// Looks up the command for a pointer `trigger`, or `None` when the gesture is
/// not bound (the event handler then handles it as a selection/gesture).
pub fn resolve(trigger: &Trigger) -> Option<Command> {
    TABLE
        .iter()
        .find(|row| (row.trigger)(trigger))
        .map(|row| row.command)
}

/// Whether the board must also see `trigger` after its table command ran.
///
/// A left release resolves to [`Command::EndPan`] so a space-drag pan always
/// ends, but the same release also ends whatever board gesture the press
/// began (move, scale, rotate, marquee). Swallowing it there left an item
/// stuck to the cursor after a plain click.
pub fn board_also_handles(trigger: &Trigger) -> bool {
    matches!(
        trigger,
        Trigger::Button {
            button: Button::Left,
            pressed: false,
            ..
        }
    )
}

#[cfg(test)]
mod tests {
    use super::{Button, Command, Trigger, board_also_handles, resolve};

    #[test]
    fn a_left_release_ends_the_pan_and_the_board_gesture() {
        for space_held in [false, true] {
            let release = Trigger::Button {
                button: Button::Left,
                pressed: false,
                space_held,
            };
            assert_eq!(resolve(&release), Some(Command::EndPan));
            assert!(board_also_handles(&release), "space {space_held}");
        }
        for (button, pressed) in [
            (Button::Left, true),
            (Button::Middle, true),
            (Button::Middle, false),
        ] {
            assert!(!board_also_handles(&Trigger::Button {
                button,
                pressed,
                space_held: false,
            }));
        }
        assert!(!board_also_handles(&Trigger::Wheel { alt: false }));
    }
    use crate::view::PanKind;

    #[test]
    fn the_wheel_zooms_and_alt_wheel_rotates() {
        assert_eq!(
            resolve(&Trigger::Wheel { alt: false }),
            Some(Command::ZoomNotch(1))
        );
        assert_eq!(
            resolve(&Trigger::Wheel { alt: true }),
            Some(Command::RotateNotch(1))
        );
    }

    #[test]
    fn the_middle_button_pans_anywhere() {
        for space in [false, true] {
            assert_eq!(
                resolve(&Trigger::Button {
                    button: Button::Middle,
                    pressed: true,
                    space_held: space,
                }),
                Some(Command::BeginPan(PanKind::MiddleDrag))
            );
        }
    }

    #[test]
    fn space_plus_left_pans_but_left_alone_does_not() {
        assert_eq!(
            resolve(&Trigger::Button {
                button: Button::Left,
                pressed: true,
                space_held: true,
            }),
            Some(Command::BeginPan(PanKind::SpaceDrag))
        );
        assert_eq!(
            resolve(&Trigger::Button {
                button: Button::Left,
                pressed: true,
                space_held: false,
            }),
            None,
            "plain left is selection, handled by the board"
        );
    }

    #[test]
    fn releasing_a_button_never_zooms_or_rotates() {
        for button in [Button::Left, Button::Middle] {
            assert_eq!(
                resolve(&Trigger::Button {
                    button,
                    pressed: false,
                    space_held: true,
                }),
                Some(Command::EndPan),
                "releasing a button always ends a pan"
            );
        }
    }
}
