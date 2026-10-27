//! The declarative input map: what the user does → what the app does.
//!
//! The map is a *table*, not a match statement scattered through the event
//! handler: phase 05's rebinding UI then edits data instead of control flow,
//! and the whole default scheme is reviewable in one glance below. winit types
//! are kept out of the table itself so the mapping stays unit-testable without
//! a window server; the event handler in `app` translates winit events into
//! the small [`Trigger`] vocabulary and looks the command up.

use crate::view::PanKind;

/// One user action the canvas can take.
///
/// View-only in phase 02; item commands (cut, paste, arrange) join the table
/// in later phases without changing its shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Start a pan drag with the given gesture.
    BeginPan(PanKind),
    /// End the active pan drag.
    EndPan,
    /// Zoom by `notches` wheel notches, anchored at the cursor.
    ///
    /// The table binds one notch; the event handler scales the count by the
    /// wheel delta's sign and magnitude before applying it.
    ZoomNotch(i32),
    /// Turn the view by `notches` wheel notches (15 degrees per notch).
    RotateNotch(i32),
    /// Frame all content with padding (`F`), or zoom to fit (`1`).
    FitContent,
    /// Zoom to 100% (`0`).
    ZoomHundred,
    /// Reset the view rotation to zero (`2`).
    ResetRotation,
}

/// A user gesture the table recognises, before modifiers are applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A mouse-wheel notch, with or without alt.
    Wheel { alt: bool },
    /// A mouse button press or release.
    Button {
        button: Button,
        pressed: bool,
        space_held: bool,
    },
    /// A keyboard key press or release.
    Key { key: Key, pressed: bool },
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
    /// `F` — frame all content.
    F,
}

/// One row of the table: this gesture, while space is in this state, means
/// that command.
struct Row {
    trigger: fn(&Trigger) -> bool,
    command: Command,
}

/// The default input scheme, exactly as the phase-02 plan writes it down.
///
/// Rows are matched top to bottom; the first row whose trigger accepts the
/// event wins. Order matters only where triggers overlap (space+left before
/// plain left), and the table reads in that order.
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
    Row {
        trigger: |t| {
            matches!(
                t,
                Trigger::Key {
                    key: Key::Digit1,
                    pressed: true
                }
            )
        },
        command: Command::FitContent,
    },
    Row {
        trigger: |t| {
            matches!(
                t,
                Trigger::Key {
                    key: Key::F,
                    pressed: true
                }
            )
        },
        command: Command::FitContent,
    },
    Row {
        trigger: |t| {
            matches!(
                t,
                Trigger::Key {
                    key: Key::Digit0,
                    pressed: true
                }
            )
        },
        command: Command::ZoomHundred,
    },
    Row {
        trigger: |t| {
            matches!(
                t,
                Trigger::Key {
                    key: Key::Digit2,
                    pressed: true
                }
            )
        },
        command: Command::ResetRotation,
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
    use super::{Button, Command, Key, Trigger, resolve};
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
            "plain left is item selection from phase 04, not pan"
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
    fn the_number_keys_and_f_match_the_plan() {
        assert_eq!(
            resolve(&Trigger::Key {
                key: Key::Digit1,
                pressed: true
            }),
            Some(Command::FitContent)
        );
        assert_eq!(
            resolve(&Trigger::Key {
                key: Key::F,
                pressed: true
            }),
            Some(Command::FitContent)
        );
        assert_eq!(
            resolve(&Trigger::Key {
                key: Key::Digit0,
                pressed: true
            }),
            Some(Command::ZoomHundred)
        );
        assert_eq!(
            resolve(&Trigger::Key {
                key: Key::Digit2,
                pressed: true
            }),
            Some(Command::ResetRotation)
        );
    }

    #[test]
    fn key_releases_bind_nothing() {
        for key in [Key::Digit0, Key::Digit1, Key::Digit2, Key::F] {
            assert_eq!(
                resolve(&Trigger::Key {
                    key,
                    pressed: false
                }),
                None,
                "{key:?} release is not a command"
            );
        }
    }
}
