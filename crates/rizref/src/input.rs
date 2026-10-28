//! The declarative input map: what the user does → what the app does.
//!
//! The map is a *table*, not a match statement scattered through the event
//! handler: phase 05's rebinding UI then edits data instead of control flow,
//! and the whole default scheme is reviewable in one glance below. winit types
//! are kept out of the table itself so the mapping stays unit-testable without
//! a window server; the event handler in `app` translates winit events into
//! the small [`Trigger`] vocabulary and looks the command up.

use crate::board::{Modifiers, NUDGE_PIXELS};
use crate::view::PanKind;

/// One user action the canvas can take.
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

/// A user gesture the table recognises, before modifiers are applied.
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
    /// A keyboard key press or release.
    Key {
        /// The key.
        key: Key,
        /// `true` for press, `false` for release.
        pressed: bool,
        /// Modifiers as of the event.
        mods: Modifiers,
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

/// Keyboard keys the canvas binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// `0` — zoom to 100%.
    Digit0,
    /// `1` — zoom to fit.
    Digit1,
    /// `2` — reset rotation.
    Digit2,
    /// `3` — arrange in a grid.
    Digit3,
    /// `4` — hierarchical arrange.
    Digit4,
    /// `5` — spread.
    Digit5,
    /// `6` — stack.
    Digit6,
    /// `F` — frame all content.
    F,
    /// `A`.
    A,
    /// `D`.
    D,
    /// `G`.
    G,
    /// `H`.
    H,
    /// `N`.
    N,
    /// `R`.
    R,
    /// `V`.
    V,
    /// `W`.
    W,
    /// `Y`.
    Y,
    /// `Z`.
    Z,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Home.
    Home,
    /// End.
    End,
    /// Delete.
    Delete,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
    /// Comma.
    Comma,
    /// Period.
    Period,
}

/// One row of the table: this gesture means that command.
struct Row {
    trigger: fn(&Trigger) -> bool,
    command: Command,
}

/// Whether a key trigger is a press of `key` with exactly `mods`.
#[cfg(test)]
const fn key_press(key: Key, mods: Modifiers) -> Trigger {
    Trigger::Key {
        key,
        pressed: true,
        mods,
    }
}

/// Modifier shorthand for the table.
const NONE: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
};
/// Modifiers with shift.
const SHIFT: Modifiers = Modifiers {
    shift: true,
    ctrl: false,
    alt: false,
};
/// Modifiers with control.
const CTRL: Modifiers = Modifiers {
    shift: false,
    ctrl: true,
    alt: false,
};
/// Modifiers with control and shift.
const CTRL_SHIFT: Modifiers = Modifiers {
    shift: true,
    ctrl: true,
    alt: false,
};

/// The default input scheme.
///
/// Rows are matched top to bottom; the first row whose trigger accepts the
/// event wins. Order matters where triggers overlap: modifier combinations come
/// before their unmodified keys, and space+left before plain left.
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
    // --- ctrl+shift combinations, before their shorter forms -----------------
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Z, pressed: true, mods } if *mods == CTRL_SHIFT),
        command: Command::Redo,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::G, pressed: true, mods } if *mods == CTRL_SHIFT),
        command: Command::UngroupSelection,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Left, pressed: true, mods } if *mods == CTRL_SHIFT),
        command: Command::AlignLeft,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Right, pressed: true, mods } if *mods == CTRL_SHIFT),
        command: Command::AlignRight,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Up, pressed: true, mods } if *mods == CTRL_SHIFT),
        command: Command::AlignTop,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Down, pressed: true, mods } if *mods == CTRL_SHIFT),
        command: Command::AlignBottom,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Comma, pressed: true, mods } if *mods == CTRL_SHIFT),
        command: Command::DistributeHorizontal,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Period, pressed: true, mods } if *mods == CTRL_SHIFT),
        command: Command::DistributeVertical,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::W, pressed: true, mods } if *mods == CTRL_SHIFT),
        command: Command::MatchWidth,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::H, pressed: true, mods } if *mods == CTRL_SHIFT),
        command: Command::MatchHeight,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::R, pressed: true, mods } if *mods == CTRL_SHIFT),
        command: Command::ResetTransform,
    },
    // --- ctrl combinations ---------------------------------------------------
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Z, pressed: true, mods } if *mods == CTRL),
        command: Command::Undo,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Y, pressed: true, mods } if *mods == CTRL),
        command: Command::Redo,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::A, pressed: true, mods } if *mods == CTRL),
        command: Command::SelectAll,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::D, pressed: true, mods } if *mods == CTRL),
        command: Command::DuplicateSelection,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::G, pressed: true, mods } if *mods == CTRL),
        command: Command::GroupSelection,
    },
    // --- shift combinations --------------------------------------------------
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::H, pressed: true, mods } if *mods == SHIFT),
        command: Command::FlipHorizontal,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::V, pressed: true, mods } if *mods == SHIFT),
        command: Command::FlipVertical,
    },
    // --- unmodified keys -----------------------------------------------------
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Digit1, pressed: true, mods } if *mods == NONE),
        command: Command::FitContent,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::F, pressed: true, mods } if *mods == NONE),
        command: Command::FitContent,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Digit0, pressed: true, mods } if *mods == NONE),
        command: Command::ZoomHundred,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Digit2, pressed: true, mods } if *mods == NONE),
        command: Command::ResetRotation,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Digit3, pressed: true, mods } if *mods == NONE),
        command: Command::ArrangeGrid,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Digit4, pressed: true, mods } if *mods == NONE),
        command: Command::ArrangeHierarchy,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Digit5, pressed: true, mods } if *mods == NONE),
        command: Command::ArrangeSpread,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Digit6, pressed: true, mods } if *mods == NONE),
        command: Command::ArrangeStack,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Escape, pressed: true, mods } if *mods == NONE),
        command: Command::Deselect,
    },
    Row {
        trigger: |t| {
            matches!(
                t,
                Trigger::Key {
                    key: Key::Delete | Key::Backspace,
                    pressed: true,
                    mods: NONE
                }
            )
        },
        command: Command::DeleteSelection,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Left, pressed: true, mods } if *mods == NONE),
        command: Command::Nudge {
            dx: -1,
            dy: 0,
            pixels: NUDGE_PIXELS,
        },
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Right, pressed: true, mods } if *mods == NONE),
        command: Command::Nudge {
            dx: 1,
            dy: 0,
            pixels: NUDGE_PIXELS,
        },
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Up, pressed: true, mods } if *mods == NONE),
        command: Command::Nudge {
            dx: 0,
            dy: -1,
            pixels: NUDGE_PIXELS,
        },
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Down, pressed: true, mods } if *mods == NONE),
        command: Command::Nudge {
            dx: 0,
            dy: 1,
            pixels: NUDGE_PIXELS,
        },
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Left, pressed: true, mods } if *mods == SHIFT),
        command: Command::Nudge {
            dx: -1,
            dy: 0,
            pixels: NUDGE_PIXELS * 10.0,
        },
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Right, pressed: true, mods } if *mods == SHIFT),
        command: Command::Nudge {
            dx: 1,
            dy: 0,
            pixels: NUDGE_PIXELS * 10.0,
        },
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Up, pressed: true, mods } if *mods == SHIFT),
        command: Command::Nudge {
            dx: 0,
            dy: -1,
            pixels: NUDGE_PIXELS * 10.0,
        },
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Down, pressed: true, mods } if *mods == SHIFT),
        command: Command::Nudge {
            dx: 0,
            dy: 1,
            pixels: NUDGE_PIXELS * 10.0,
        },
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::PageUp, pressed: true, mods } if *mods == NONE),
        command: Command::RaiseSelection,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::PageDown, pressed: true, mods } if *mods == NONE),
        command: Command::LowerSelection,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::Home, pressed: true, mods } if *mods == NONE),
        command: Command::RaiseToTop,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::End, pressed: true, mods } if *mods == NONE),
        command: Command::LowerToBottom,
    },
    Row {
        trigger: |t| matches!(t, Trigger::Key { key: Key::N, pressed: true, mods } if *mods == NONE),
        command: Command::CreateNote,
    },
];

/// Looks up the command for `trigger`, or `None` when the gesture is not
/// bound (the event handler then ignores it).
pub fn resolve(trigger: &Trigger) -> Option<Command> {
    TABLE
        .iter()
        .find(|row| (row.trigger)(trigger))
        .map(|row| row.command)
}

#[cfg(test)]
mod tests {
    use super::{Button, Command, Key, Modifiers, Trigger, key_press, resolve};
    use crate::board::NUDGE_PIXELS;
    use crate::view::PanKind;

    const CTRL: Modifiers = Modifiers {
        shift: false,
        ctrl: true,
        alt: false,
    };
    const CTRL_SHIFT: Modifiers = Modifiers {
        shift: true,
        ctrl: true,
        alt: false,
    };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ctrl: false,
        alt: false,
    };

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
    fn either_pan_button_releases_the_pan() {
        for button in [Button::Left, Button::Middle] {
            assert_eq!(
                resolve(&Trigger::Button {
                    button,
                    pressed: false,
                    space_held: false,
                }),
                Some(Command::EndPan)
            );
        }
    }

    #[test]
    fn the_numbers_and_f_match_the_plan() {
        assert_eq!(
            resolve(&key_press(Key::Digit1, Modifiers::default())),
            Some(Command::FitContent)
        );
        assert_eq!(
            resolve(&key_press(Key::F, Modifiers::default())),
            Some(Command::FitContent)
        );
        assert_eq!(
            resolve(&key_press(Key::Digit0, Modifiers::default())),
            Some(Command::ZoomHundred)
        );
        assert_eq!(
            resolve(&key_press(Key::Digit2, Modifiers::default())),
            Some(Command::ResetRotation)
        );
        for (key, command) in [
            (Key::Digit3, Command::ArrangeGrid),
            (Key::Digit4, Command::ArrangeHierarchy),
            (Key::Digit5, Command::ArrangeSpread),
            (Key::Digit6, Command::ArrangeStack),
        ] {
            assert_eq!(
                resolve(&key_press(key, Modifiers::default())),
                Some(command)
            );
        }
    }

    #[test]
    fn history_and_selection_shortcuts() {
        assert_eq!(resolve(&key_press(Key::Z, CTRL)), Some(Command::Undo));
        assert_eq!(resolve(&key_press(Key::Y, CTRL)), Some(Command::Redo));
        assert_eq!(resolve(&key_press(Key::Z, CTRL_SHIFT)), Some(Command::Redo));
        assert_eq!(resolve(&key_press(Key::A, CTRL)), Some(Command::SelectAll));
        assert_eq!(
            resolve(&key_press(Key::Escape, Modifiers::default())),
            Some(Command::Deselect)
        );
    }

    #[test]
    fn item_shortcuts_do_not_shadow_each_other() {
        assert_eq!(
            resolve(&key_press(Key::H, SHIFT)),
            Some(Command::FlipHorizontal)
        );
        assert_eq!(
            resolve(&key_press(Key::V, SHIFT)),
            Some(Command::FlipVertical)
        );
        assert_eq!(
            resolve(&key_press(Key::H, CTRL_SHIFT)),
            Some(Command::MatchHeight),
            "ctrl+shift+H is match height, not flip"
        );
        assert_eq!(
            resolve(&key_press(Key::W, CTRL_SHIFT)),
            Some(Command::MatchWidth)
        );
        assert_eq!(
            resolve(&key_press(Key::R, CTRL_SHIFT)),
            Some(Command::ResetTransform)
        );
        assert_eq!(
            resolve(&key_press(Key::G, CTRL)),
            Some(Command::GroupSelection)
        );
        assert_eq!(
            resolve(&key_press(Key::G, CTRL_SHIFT)),
            Some(Command::UngroupSelection)
        );
        assert_eq!(
            resolve(&key_press(Key::D, CTRL)),
            Some(Command::DuplicateSelection)
        );
        assert_eq!(
            resolve(&key_press(Key::Delete, Modifiers::default())),
            Some(Command::DeleteSelection)
        );
        assert_eq!(
            resolve(&key_press(Key::Backspace, Modifiers::default())),
            Some(Command::DeleteSelection)
        );
    }

    #[test]
    fn arrows_nudge_and_shift_arrows_nudge_further() {
        assert_eq!(
            resolve(&key_press(Key::Left, Modifiers::default())),
            Some(Command::Nudge {
                dx: -1,
                dy: 0,
                pixels: NUDGE_PIXELS
            })
        );
        assert_eq!(
            resolve(&key_press(Key::Down, SHIFT)),
            Some(Command::Nudge {
                dx: 0,
                dy: 1,
                pixels: NUDGE_PIXELS * 10.0
            })
        );
    }

    #[test]
    fn z_order_shortcuts() {
        assert_eq!(
            resolve(&key_press(Key::PageUp, Modifiers::default())),
            Some(Command::RaiseSelection)
        );
        assert_eq!(
            resolve(&key_press(Key::PageDown, Modifiers::default())),
            Some(Command::LowerSelection)
        );
        assert_eq!(
            resolve(&key_press(Key::Home, Modifiers::default())),
            Some(Command::RaiseToTop)
        );
        assert_eq!(
            resolve(&key_press(Key::End, Modifiers::default())),
            Some(Command::LowerToBottom)
        );
    }

    #[test]
    fn align_and_distribute_shortcuts() {
        for (key, command) in [
            (Key::Left, Command::AlignLeft),
            (Key::Right, Command::AlignRight),
            (Key::Up, Command::AlignTop),
            (Key::Down, Command::AlignBottom),
            (Key::Comma, Command::DistributeHorizontal),
            (Key::Period, Command::DistributeVertical),
        ] {
            assert_eq!(resolve(&key_press(key, CTRL_SHIFT)), Some(command));
        }
    }

    #[test]
    fn key_releases_bind_nothing() {
        for key in [
            Key::Digit0,
            Key::Digit1,
            Key::Digit2,
            Key::F,
            Key::H,
            Key::Z,
        ] {
            assert_eq!(
                resolve(&Trigger::Key {
                    key,
                    pressed: false,
                    mods: Modifiers::default()
                }),
                None,
                "{key:?} release is not a command"
            );
        }
    }
}
