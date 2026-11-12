//! The rebindable keyboard map: one declarative table for every action.
//!
//! This is the single source of truth the phase-05 shortcut overlay, the menu
//! accelerators and the event loop all read. Bindings are data ([`KeyChord`]),
//! not closures, so the overlay can edit them, the settings file can persist
//! them, and a conflict is a value the user can see rather than a key that
//! silently does nothing.
//!
//! Pointer gestures stay in [`crate::input`]: a wheel notch and a pan drag are
//! structural, not user preferences.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::input::Command;

/// A keyboard key the shortcut table can bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Key {
    /// `0`.
    Digit0,
    /// `1`.
    Digit1,
    /// `2`.
    Digit2,
    /// `3`.
    Digit3,
    /// `4`.
    Digit4,
    /// `5`.
    Digit5,
    /// `6`.
    Digit6,
    /// `7`.
    Digit7,
    /// `8`.
    Digit8,
    /// `9`.
    Digit9,
    /// `A`.
    A,
    /// `B`.
    B,
    /// `C`.
    C,
    /// `D`.
    D,
    /// `E`.
    E,
    /// `F`.
    F,
    /// `G`.
    G,
    /// `H`.
    H,
    /// `I`.
    I,
    /// `J`.
    J,
    /// `K`.
    K,
    /// `L`.
    L,
    /// `M`.
    M,
    /// `N`.
    N,
    /// `O`.
    O,
    /// `P`.
    P,
    /// `Q`.
    Q,
    /// `R`.
    R,
    /// `S`.
    S,
    /// `T`.
    T,
    /// `U`.
    U,
    /// `V`.
    V,
    /// `W`.
    W,
    /// `X`.
    X,
    /// `Y`.
    Y,
    /// `Z`.
    Z,
    /// `F1`.
    F1,
    /// `F2`.
    F2,
    /// `F11`.
    F11,
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
    /// Minus.
    Minus,
    /// Equal.
    Equal,
    /// Tab.
    Tab,
    /// Space.
    Space,
    /// Enter.
    Enter,
}

impl Key {
    /// The name used in settings files and shown in the shortcut list.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Digit0 => "0",
            Self::Digit1 => "1",
            Self::Digit2 => "2",
            Self::Digit3 => "3",
            Self::Digit4 => "4",
            Self::Digit5 => "5",
            Self::Digit6 => "6",
            Self::Digit7 => "7",
            Self::Digit8 => "8",
            Self::Digit9 => "9",
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
            Self::H => "H",
            Self::I => "I",
            Self::J => "J",
            Self::K => "K",
            Self::L => "L",
            Self::M => "M",
            Self::N => "N",
            Self::O => "O",
            Self::P => "P",
            Self::Q => "Q",
            Self::R => "R",
            Self::S => "S",
            Self::T => "T",
            Self::U => "U",
            Self::V => "V",
            Self::W => "W",
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F11 => "F11",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::PageUp => "PgUp",
            Self::PageDown => "PgDn",
            Self::Home => "Home",
            Self::End => "End",
            Self::Delete => "Del",
            Self::Backspace => "Backspace",
            Self::Escape => "Esc",
            Self::Comma => ",",
            Self::Period => ".",
            Self::Minus => "-",
            Self::Equal => "=",
            Self::Tab => "Tab",
            Self::Space => "Space",
            Self::Enter => "Enter",
        }
    }

    /// The key name `text` names, if any (case-insensitive).
    pub fn from_name(text: &str) -> Option<Self> {
        let wanted = text.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|key| {
            let name = key.name().to_ascii_lowercase();
            name == wanted
                || matches!(
                    (key, wanted.as_str()),
                    (Self::Delete, "delete")
                        | (Self::Escape, "escape")
                        | (Self::PageUp, "pageup")
                        | (Self::PageDown, "pagedown")
                )
        })
    }

    /// Every key this build can name.
    pub const ALL: [Self; 57] = [
        Self::Digit0,
        Self::Digit1,
        Self::Digit2,
        Self::Digit3,
        Self::Digit4,
        Self::Digit5,
        Self::Digit6,
        Self::Digit7,
        Self::Digit8,
        Self::Digit9,
        Self::A,
        Self::B,
        Self::C,
        Self::D,
        Self::E,
        Self::F,
        Self::G,
        Self::H,
        Self::I,
        Self::J,
        Self::K,
        Self::L,
        Self::M,
        Self::N,
        Self::O,
        Self::P,
        Self::Q,
        Self::R,
        Self::S,
        Self::T,
        Self::U,
        Self::V,
        Self::W,
        Self::X,
        Self::Y,
        Self::Z,
        Self::F1,
        Self::F2,
        Self::F11,
        Self::Left,
        Self::Right,
        Self::Up,
        Self::Down,
        Self::PageUp,
        Self::PageDown,
        Self::Home,
        Self::End,
        Self::Delete,
        Self::Backspace,
        Self::Escape,
        Self::Comma,
        Self::Period,
        Self::Minus,
        Self::Equal,
        Self::Tab,
        Self::Space,
        Self::Enter,
    ];
}

/// One key plus the exact modifier set that must be held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyChord {
    /// The non-modifier key.
    pub key: Key,
    /// Control held.
    pub ctrl: bool,
    /// Shift held.
    pub shift: bool,
    /// Alt held.
    pub alt: bool,
}

impl KeyChord {
    /// A chord with no modifiers.
    pub const fn plain(key: Key) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    /// A chord with control.
    pub const fn ctrl(key: Key) -> Self {
        Self {
            key,
            ctrl: true,
            shift: false,
            alt: false,
        }
    }

    /// A chord with control and shift.
    pub const fn ctrl_shift(key: Key) -> Self {
        Self {
            key,
            ctrl: true,
            shift: true,
            alt: false,
        }
    }

    /// A chord with shift.
    pub const fn shift(key: Key) -> Self {
        Self {
            key,
            ctrl: false,
            shift: true,
            alt: false,
        }
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            formatter.write_str("Ctrl+")?;
        }
        if self.shift {
            formatter.write_str("Shift+")?;
        }
        if self.alt {
            formatter.write_str("Alt+")?;
        }
        formatter.write_str(self.key.name())
    }
}

/// Why a chord could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChordParseError {
    /// The offending text.
    pub text: String,
}

impl fmt::Display for ChordParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "not a key chord: {:?}", self.text)
    }
}

impl std::error::Error for ChordParseError {}

impl FromStr for KeyChord {
    type Err = ChordParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut chord = KeyChord::plain(Key::A);
        let mut key = None;
        for part in text.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" => chord.alt = true,
                _ => {
                    if key.is_some() {
                        return Err(ChordParseError {
                            text: text.to_owned(),
                        });
                    }
                    key = Key::from_name(part);
                }
            }
        }
        match key {
            Some(key) => {
                chord.key = key;
                Ok(chord)
            }
            None => Err(ChordParseError {
                text: text.to_owned(),
            }),
        }
    }
}

impl Serialize for KeyChord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for KeyChord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// One user-facing action that can be rebound.
///
/// The identifiers are stable: they are the keys in `settings.json`, so
/// renaming one would silently drop a user's binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShortcutId {
    /// Undo.
    Undo,
    /// Redo.
    Redo,
    /// Select everything.
    SelectAll,
    /// Clear the selection.
    Deselect,
    /// Duplicate the selection.
    DuplicateSelection,
    /// Delete the selection.
    DeleteSelection,
    /// Nudge left.
    NudgeLeft,
    /// Nudge right.
    NudgeRight,
    /// Nudge up.
    NudgeUp,
    /// Nudge down.
    NudgeDown,
    /// Nudge left by ten steps.
    NudgeLeftFar,
    /// Nudge right by ten steps.
    NudgeRightFar,
    /// Nudge up by ten steps.
    NudgeUpFar,
    /// Nudge down by ten steps.
    NudgeDownFar,
    /// Flip horizontally.
    FlipHorizontal,
    /// Flip vertically.
    FlipVertical,
    /// Reset the item transform.
    ResetTransform,
    /// Match the selection's width.
    MatchWidth,
    /// Match the selection's height.
    MatchHeight,
    /// Group the selection.
    GroupSelection,
    /// Dissolve a group.
    UngroupSelection,
    /// Raise one z step.
    RaiseSelection,
    /// Lower one z step.
    LowerSelection,
    /// Send to front.
    RaiseToTop,
    /// Send to back.
    LowerToBottom,
    /// Arrange in a grid.
    ArrangeGrid,
    /// Arrange hierarchically.
    ArrangeHierarchy,
    /// Arrange in an evenly spaced row.
    ArrangeSpread,
    /// Stack on one point.
    ArrangeStack,
    /// Create a note.
    CreateNote,
    /// Add a free-standing text item.
    CreateText,
    /// Align left.
    AlignLeft,
    /// Align right.
    AlignRight,
    /// Align top.
    AlignTop,
    /// Align bottom.
    AlignBottom,
    /// Distribute horizontally.
    DistributeHorizontal,
    /// Distribute vertically.
    DistributeVertical,
    /// Frame all content.
    FitContent,
    /// Zoom to 100%.
    ZoomHundred,
    /// Reset the view rotation.
    ResetRotation,
    /// Reset the whole view.
    ResetView,
    /// Zoom in at the viewport centre.
    ZoomIn,
    /// Zoom out at the viewport centre.
    ZoomOut,
    /// Toggle the grid.
    ToggleGrid,
    /// Toggle snapping.
    ToggleSnap,
    /// Toggle the HUD readout.
    ToggleHud,
    /// Toggle the scene-rect border.
    ToggleBorder,
    /// Toggle the checkerboard backdrop.
    ToggleCheckerboard,
    /// Toggle grayscale display.
    ToggleGrayscale,
    /// Lock item editing.
    LockCanvas,
    /// Start a new board.
    NewBoard,
    /// Open image files.
    OpenImages,
    /// Paste from the clipboard.
    Paste,
    /// Save the board.
    Save,
    /// Save the board under a new name.
    SaveAs,
    /// Export the board.
    Export,
    /// Return to the start view.
    ShowStart,
    /// Open the shortcut overlay.
    ShowShortcuts,
    /// Open the settings dialog.
    ShowSettings,
    /// Toggle always-on-top.
    ToggleAlwaysOnTop,
    /// Cycle the window opacity.
    CycleOpacity,
    /// Toggle click-through.
    ToggleClickThrough,
    /// Toggle ghost mode.
    ToggleGhostMode,
    /// Toggle borderless fullscreen.
    Borderless,
    /// Pick a colour from the canvas.
    ColorPicker,
    /// Quit.
    Quit,
}

impl ShortcutId {
    /// Every shortcut, in the order the F1 overlay lists them.
    pub const ALL: [Self; 66] = [
        Self::Undo,
        Self::Redo,
        Self::SelectAll,
        Self::Deselect,
        Self::DuplicateSelection,
        Self::DeleteSelection,
        Self::NudgeLeft,
        Self::NudgeRight,
        Self::NudgeUp,
        Self::NudgeDown,
        Self::NudgeLeftFar,
        Self::NudgeRightFar,
        Self::NudgeUpFar,
        Self::NudgeDownFar,
        Self::FlipHorizontal,
        Self::FlipVertical,
        Self::ResetTransform,
        Self::MatchWidth,
        Self::MatchHeight,
        Self::GroupSelection,
        Self::UngroupSelection,
        Self::RaiseSelection,
        Self::LowerSelection,
        Self::RaiseToTop,
        Self::LowerToBottom,
        Self::ArrangeGrid,
        Self::ArrangeHierarchy,
        Self::ArrangeSpread,
        Self::ArrangeStack,
        Self::CreateNote,
        Self::CreateText,
        Self::AlignLeft,
        Self::AlignRight,
        Self::AlignTop,
        Self::AlignBottom,
        Self::DistributeHorizontal,
        Self::DistributeVertical,
        Self::FitContent,
        Self::ZoomHundred,
        Self::ResetRotation,
        Self::ResetView,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ToggleGrid,
        Self::ToggleSnap,
        Self::ToggleHud,
        Self::ToggleBorder,
        Self::ToggleCheckerboard,
        Self::ToggleGrayscale,
        Self::LockCanvas,
        Self::NewBoard,
        Self::OpenImages,
        Self::Paste,
        Self::Save,
        Self::SaveAs,
        Self::Export,
        Self::ShowStart,
        Self::ShowShortcuts,
        Self::ShowSettings,
        Self::ToggleAlwaysOnTop,
        Self::CycleOpacity,
        Self::ToggleClickThrough,
        Self::ToggleGhostMode,
        Self::Borderless,
        Self::ColorPicker,
        Self::Quit,
    ];

    /// The user-facing name shown in menus and the shortcut list.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::SelectAll => "Select all",
            Self::Deselect => "Deselect",
            Self::DuplicateSelection => "Duplicate",
            Self::DeleteSelection => "Delete",
            Self::NudgeLeft => "Nudge left",
            Self::NudgeRight => "Nudge right",
            Self::NudgeUp => "Nudge up",
            Self::NudgeDown => "Nudge down",
            Self::NudgeLeftFar => "Nudge left (10 px)",
            Self::NudgeRightFar => "Nudge right (10 px)",
            Self::NudgeUpFar => "Nudge up (10 px)",
            Self::NudgeDownFar => "Nudge down (10 px)",
            Self::FlipHorizontal => "Flip horizontal",
            Self::FlipVertical => "Flip vertical",
            Self::ResetTransform => "Reset transform",
            Self::MatchWidth => "Match width",
            Self::MatchHeight => "Match height",
            Self::GroupSelection => "Group",
            Self::UngroupSelection => "Ungroup",
            Self::RaiseSelection => "Raise",
            Self::LowerSelection => "Lower",
            Self::RaiseToTop => "Bring to front",
            Self::LowerToBottom => "Send to back",
            Self::ArrangeGrid => "Arrange grid",
            Self::ArrangeHierarchy => "Arrange hierarchy",
            Self::ArrangeSpread => "Spread evenly",
            Self::ArrangeStack => "Stack",
            Self::CreateNote => "New note",
            Self::CreateText => "Add text",
            Self::AlignLeft => "Align left",
            Self::AlignRight => "Align right",
            Self::AlignTop => "Align top",
            Self::AlignBottom => "Align bottom",
            Self::DistributeHorizontal => "Distribute horizontally",
            Self::DistributeVertical => "Distribute vertically",
            Self::FitContent => "Frame everything",
            Self::ZoomHundred => "Zoom 100%",
            Self::ResetRotation => "Reset rotation",
            Self::ResetView => "Reset view",
            Self::ZoomIn => "Zoom in",
            Self::ZoomOut => "Zoom out",
            Self::ToggleGrid => "Toggle grid",
            Self::ToggleSnap => "Toggle snapping",
            Self::ToggleHud => "Toggle HUD",
            Self::ToggleBorder => "Toggle scene boundary",
            Self::ToggleCheckerboard => "Toggle checkerboard",
            Self::ToggleGrayscale => "Toggle grayscale",
            Self::LockCanvas => "Lock canvas",
            Self::NewBoard => "New board",
            Self::OpenImages => "Open images…",
            Self::Paste => "Paste",
            Self::Save => "Save",
            Self::SaveAs => "Save as…",
            Self::Export => "Export…",
            Self::ShowStart => "Start view",
            Self::ShowShortcuts => "Keyboard shortcuts",
            Self::ShowSettings => "Settings",
            Self::ToggleAlwaysOnTop => "Always on top",
            Self::CycleOpacity => "Cycle canvas transparency",
            Self::ToggleClickThrough => "Click-through",
            Self::ToggleGhostMode => "Ghost mode",
            Self::Borderless => "Borderless fullscreen",
            Self::ColorPicker => "Colour picker",
            Self::Quit => "Quit",
        }
    }

    /// The board command this shortcut runs, when it is a board command.
    ///
    /// View and shell actions are dispatched by the application; keeping them
    /// out of this match means the shortcut table has no dependency on the
    /// windowing layer.
    pub const fn command(self) -> Option<Command> {
        use crate::board::NUDGE_PIXELS;
        Some(match self {
            Self::Undo => Command::Undo,
            Self::Redo => Command::Redo,
            Self::SelectAll => Command::SelectAll,
            Self::Deselect => Command::Deselect,
            Self::DuplicateSelection => Command::DuplicateSelection,
            Self::DeleteSelection => Command::DeleteSelection,
            Self::NudgeLeft => Command::Nudge {
                dx: -1,
                dy: 0,
                pixels: NUDGE_PIXELS,
            },
            Self::NudgeRight => Command::Nudge {
                dx: 1,
                dy: 0,
                pixels: NUDGE_PIXELS,
            },
            Self::NudgeUp => Command::Nudge {
                dx: 0,
                dy: -1,
                pixels: NUDGE_PIXELS,
            },
            Self::NudgeDown => Command::Nudge {
                dx: 0,
                dy: 1,
                pixels: NUDGE_PIXELS,
            },
            Self::NudgeLeftFar => Command::Nudge {
                dx: -1,
                dy: 0,
                pixels: NUDGE_PIXELS * 10.0,
            },
            Self::NudgeRightFar => Command::Nudge {
                dx: 1,
                dy: 0,
                pixels: NUDGE_PIXELS * 10.0,
            },
            Self::NudgeUpFar => Command::Nudge {
                dx: 0,
                dy: -1,
                pixels: NUDGE_PIXELS * 10.0,
            },
            Self::NudgeDownFar => Command::Nudge {
                dx: 0,
                dy: 1,
                pixels: NUDGE_PIXELS * 10.0,
            },
            Self::FlipHorizontal => Command::FlipHorizontal,
            Self::FlipVertical => Command::FlipVertical,
            Self::ResetTransform => Command::ResetTransform,
            Self::MatchWidth => Command::MatchWidth,
            Self::MatchHeight => Command::MatchHeight,
            Self::GroupSelection => Command::GroupSelection,
            Self::UngroupSelection => Command::UngroupSelection,
            Self::RaiseSelection => Command::RaiseSelection,
            Self::LowerSelection => Command::LowerSelection,
            Self::RaiseToTop => Command::RaiseToTop,
            Self::LowerToBottom => Command::LowerToBottom,
            Self::ArrangeGrid => Command::ArrangeGrid,
            Self::ArrangeHierarchy => Command::ArrangeHierarchy,
            Self::ArrangeSpread => Command::ArrangeSpread,
            Self::ArrangeStack => Command::ArrangeStack,
            Self::CreateNote => Command::CreateNote,
            Self::CreateText => Command::CreateText,
            Self::AlignLeft => Command::AlignLeft,
            Self::AlignRight => Command::AlignRight,
            Self::AlignTop => Command::AlignTop,
            Self::AlignBottom => Command::AlignBottom,
            Self::DistributeHorizontal => Command::DistributeHorizontal,
            Self::DistributeVertical => Command::DistributeVertical,
            Self::FitContent => Command::FitContent,
            Self::ZoomHundred => Command::ZoomHundred,
            Self::ResetRotation => Command::ResetRotation,
            Self::ResetView => Command::ResetView,
            Self::ZoomIn => Command::ZoomNotch(1),
            Self::ZoomOut => Command::ZoomNotch(-1),
            _ => return None,
        })
    }

    /// The default chords for this action, primary first.
    ///
    /// More than one entry is an alias: the F1 overlay shows the first, while
    /// both resolve. A rebind replaces the whole list with one chord.
    pub fn default_chords(self) -> Vec<KeyChord> {
        use Key::*;
        match self {
            Self::Undo => vec![KeyChord::ctrl(Z)],
            Self::Redo => vec![KeyChord::ctrl(Y), KeyChord::ctrl_shift(Z)],
            Self::SelectAll => vec![KeyChord::ctrl(A)],
            Self::Deselect => vec![KeyChord::plain(Escape)],
            Self::DuplicateSelection => vec![KeyChord::ctrl(D)],
            Self::DeleteSelection => vec![KeyChord::plain(Delete), KeyChord::plain(Backspace)],
            Self::NudgeLeft => vec![KeyChord::plain(Left)],
            Self::NudgeRight => vec![KeyChord::plain(Right)],
            Self::NudgeUp => vec![KeyChord::plain(Up)],
            Self::NudgeDown => vec![KeyChord::plain(Down)],
            Self::NudgeLeftFar => vec![KeyChord::shift(Left)],
            Self::NudgeRightFar => vec![KeyChord::shift(Right)],
            Self::NudgeUpFar => vec![KeyChord::shift(Up)],
            Self::NudgeDownFar => vec![KeyChord::shift(Down)],
            Self::FlipHorizontal => vec![KeyChord::shift(H)],
            Self::FlipVertical => vec![KeyChord::shift(V)],
            Self::ResetTransform => vec![KeyChord::ctrl_shift(R)],
            Self::MatchWidth => vec![KeyChord::ctrl_shift(W)],
            Self::MatchHeight => vec![KeyChord::ctrl_shift(H)],
            Self::GroupSelection => vec![KeyChord::ctrl(G)],
            Self::UngroupSelection => vec![KeyChord::ctrl_shift(U)],
            Self::RaiseSelection => vec![KeyChord::plain(PageUp)],
            Self::LowerSelection => vec![KeyChord::plain(PageDown)],
            Self::RaiseToTop => vec![KeyChord::plain(Home)],
            Self::LowerToBottom => vec![KeyChord::plain(End)],
            Self::ArrangeGrid => vec![KeyChord::plain(Digit3)],
            Self::ArrangeHierarchy => vec![KeyChord::plain(Digit4)],
            Self::ArrangeSpread => vec![KeyChord::plain(Digit5)],
            Self::ArrangeStack => vec![KeyChord::plain(Digit6)],
            Self::CreateNote => vec![KeyChord::plain(N)],
            Self::CreateText => vec![KeyChord::plain(T)],
            Self::AlignLeft => vec![KeyChord::ctrl_shift(Left)],
            Self::AlignRight => vec![KeyChord::ctrl_shift(Right)],
            Self::AlignTop => vec![KeyChord::ctrl_shift(Up)],
            Self::AlignBottom => vec![KeyChord::ctrl_shift(Down)],
            Self::DistributeHorizontal => vec![KeyChord::ctrl_shift(Comma)],
            Self::DistributeVertical => vec![KeyChord::ctrl_shift(Period)],
            Self::FitContent => vec![KeyChord::plain(F), KeyChord::plain(Digit1)],
            Self::ZoomHundred => vec![KeyChord::plain(Digit0)],
            Self::ResetRotation => vec![KeyChord::plain(Digit2)],
            Self::ResetView => vec![KeyChord::ctrl(Digit0)],
            Self::ZoomIn => vec![KeyChord::ctrl(Equal)],
            Self::ZoomOut => vec![KeyChord::ctrl(Minus)],
            Self::ToggleGrid => vec![KeyChord::plain(G)],
            Self::ToggleSnap => vec![KeyChord::shift(S)],
            Self::ToggleHud => vec![KeyChord::ctrl(H)],
            Self::ToggleBorder => vec![KeyChord::ctrl(B)],
            Self::ToggleCheckerboard => vec![KeyChord::ctrl_shift(B)],
            Self::ToggleGrayscale => vec![KeyChord::plain(U)],
            Self::LockCanvas => vec![KeyChord::plain(L)],
            Self::NewBoard => vec![KeyChord::ctrl(N)],
            Self::OpenImages => vec![KeyChord::ctrl(O)],
            Self::Paste => vec![KeyChord::ctrl(V)],
            Self::Save => vec![KeyChord::ctrl(S)],
            Self::SaveAs => vec![KeyChord::ctrl_shift(S)],
            Self::Export => vec![KeyChord::ctrl(E)],
            Self::ShowStart => vec![],
            Self::ShowShortcuts => vec![KeyChord::plain(F1)],
            Self::ShowSettings => vec![KeyChord::ctrl(Comma)],
            Self::ToggleAlwaysOnTop => vec![KeyChord::ctrl_shift(T)],
            Self::CycleOpacity => vec![KeyChord::ctrl_shift(O)],
            Self::ToggleClickThrough => vec![KeyChord::ctrl_shift(X)],
            Self::ToggleGhostMode => vec![KeyChord::ctrl_shift(G)],
            Self::Borderless => vec![KeyChord::plain(F11)],
            Self::ColorPicker => vec![KeyChord::plain(I)],
            Self::Quit => vec![KeyChord::ctrl(Q)],
        }
    }
}

/// The action already using a chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conflict {
    /// The action that owns the chord.
    pub action: ShortcutId,
}

/// The live binding table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcuts {
    entries: Vec<(ShortcutId, KeyChord)>,
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self::new()
    }
}

impl Shortcuts {
    /// The default table.
    pub fn new() -> Self {
        let mut table = Self {
            entries: Vec::new(),
        };
        for action in ShortcutId::ALL {
            for chord in action.default_chords() {
                table.entries.push((action, chord));
            }
        }
        table
    }

    /// The chord currently bound to `action`, primary first.
    pub fn chord_for(&self, action: ShortcutId) -> Option<KeyChord> {
        self.entries
            .iter()
            .find(|(id, _)| *id == action)
            .map(|(_, chord)| *chord)
    }

    /// The action bound to `chord`, if any.
    pub fn resolve(&self, chord: KeyChord) -> Option<ShortcutId> {
        self.entries
            .iter()
            .find(|(_, bound)| *bound == chord)
            .map(|(action, _)| *action)
    }

    /// The action already using `chord`, ignoring `except`.
    pub fn conflict(&self, chord: KeyChord, except: Option<ShortcutId>) -> Option<ShortcutId> {
        self.entries
            .iter()
            .find(|(action, bound)| *bound == chord && Some(*action) != except)
            .map(|(action, _)| *action)
    }

    /// Binds `action` to `chord`, replacing its whole chord list.
    ///
    /// Fails with the conflicting action, leaving the table untouched: a
    /// rebinding that silently steals a key is worse than a rejected one.
    pub fn bind(&mut self, action: ShortcutId, chord: KeyChord) -> Result<(), Conflict> {
        if let Some(other) = self.conflict(chord, Some(action)) {
            return Err(Conflict { action: other });
        }
        self.entries.retain(|(id, _)| *id != action);
        self.entries.push((action, chord));
        Ok(())
    }

    /// Restores the default binding(s) for `action`.
    pub fn reset(&mut self, action: ShortcutId) {
        self.entries.retain(|(id, _)| *id != action);
        for chord in action.default_chords() {
            self.entries.push((action, chord));
        }
    }

    /// Applies persisted overrides, skipping any that are malformed or would
    /// conflict with an earlier override.
    ///
    /// A settings file is user-editable data, not a trusted program: a bad
    /// entry is logged and ignored, never allowed to make the app unbootable.
    pub fn apply_overrides(&mut self, overrides: &BTreeMap<ShortcutId, KeyChord>) {
        for (action, chord) in overrides {
            if let Err(conflict) = self.bind(*action, *chord) {
                tracing::warn!(
                    action = ?action,
                    chord = %chord,
                    conflicts_with = ?conflict.action,
                    "ignoring a conflicting saved shortcut"
                );
            }
        }
    }

    /// The overrides to persist: only actions whose binding differs from the
    /// default table. Aliases are collapsed to their primary chord.
    pub fn overrides(&self) -> BTreeMap<ShortcutId, KeyChord> {
        let mut overrides = BTreeMap::new();
        for action in ShortcutId::ALL {
            let current = self.chord_for(action);
            let default = action.default_chords().first().copied();
            if let Some(chord) = current
                && Some(chord) != default
            {
                overrides.insert(action, chord);
            }
        }
        overrides
    }

    /// Number of bound actions, for the F1 overlay header.
    pub fn bound_count(&self) -> usize {
        self.entries
            .iter()
            .map(|(action, _)| *action)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::{Key, KeyChord, ShortcutId, Shortcuts};
    use std::collections::BTreeMap;

    #[test]
    fn every_action_has_a_unique_chord_set() {
        let table = Shortcuts::new();
        for (index, (action, chord)) in table.entries.iter().enumerate() {
            for (other_action, other_chord) in &table.entries[index + 1..] {
                assert!(
                    *chord != *other_chord || action == other_action,
                    "{action:?} and {other_action:?} both use {chord}"
                );
            }
        }
    }

    #[test]
    fn the_default_table_has_at_least_forty_actions() {
        assert!(
            ShortcutId::ALL.len() >= 40,
            "the shortcut overlay must list at least forty actions"
        );
        assert_ne!(Shortcuts::new().bound_count(), 0);
    }

    #[test]
    fn chords_parse_and_format_in_both_directions() {
        for text in ["Ctrl+Shift+Z", "F11", "Shift+H", "Ctrl+Shift+G", ",", "Del"] {
            let chord: KeyChord = text.parse().expect("a valid chord");
            assert_eq!(chord.to_string(), text, "round trip of {text}");
        }
        assert!("Ctrl+".parse::<KeyChord>().is_err());
        assert!("Ctrl+A+B".parse::<KeyChord>().is_err());
        assert!("Nonsense".parse::<KeyChord>().is_err());
    }

    #[test]
    fn key_names_cover_every_key_without_duplicates() {
        let names: std::collections::BTreeSet<_> = Key::ALL.iter().map(|key| key.name()).collect();
        assert_eq!(names.len(), Key::ALL.len());
        for key in Key::ALL {
            assert_eq!(Key::from_name(key.name()), Some(key));
        }
    }

    #[test]
    fn resolving_uses_the_exact_modifier_set() {
        let table = Shortcuts::new();
        assert_eq!(
            table.resolve(KeyChord::ctrl(Key::Z)),
            Some(ShortcutId::Undo)
        );
        assert_eq!(
            table.resolve(KeyChord::ctrl_shift(Key::Z)),
            Some(ShortcutId::Redo)
        );
        assert_eq!(table.resolve(KeyChord::plain(Key::Z)), None);
    }

    #[test]
    fn a_rebind_removes_aliases_and_rejects_conflicts() {
        let mut table = Shortcuts::new();
        table
            .bind(ShortcutId::Undo, KeyChord::ctrl(Key::U))
            .expect("free chord");
        assert_eq!(
            table.chord_for(ShortcutId::Undo),
            Some(KeyChord::ctrl(Key::U))
        );
        assert_eq!(
            table.resolve(KeyChord::ctrl(Key::U)),
            Some(ShortcutId::Undo)
        );
        assert_eq!(table.resolve(KeyChord::ctrl(Key::Z)), None);

        let conflict = table
            .bind(ShortcutId::Redo, KeyChord::ctrl(Key::U))
            .expect_err("already taken");
        assert_eq!(conflict.action, ShortcutId::Undo);
        assert_eq!(
            table.chord_for(ShortcutId::Redo),
            Some(KeyChord::ctrl(Key::Y)),
            "a rejected bind changes nothing"
        );
    }

    #[test]
    fn reset_restores_every_default_chord() {
        let mut table = Shortcuts::new();
        table
            .bind(ShortcutId::FitContent, KeyChord::plain(Key::F2))
            .expect("free chord");
        table.reset(ShortcutId::FitContent);

        assert_eq!(
            table.chord_for(ShortcutId::FitContent),
            Some(KeyChord::plain(Key::F))
        );
        assert_eq!(
            table.resolve(KeyChord::plain(Key::Digit1)),
            Some(ShortcutId::FitContent)
        );
    }

    #[test]
    fn persisted_overrides_round_trip_and_bad_ones_are_skipped() {
        let mut table = Shortcuts::new();
        table
            .bind(ShortcutId::Undo, KeyChord::ctrl(Key::U))
            .expect("free chord");
        let overrides = table.overrides();
        assert_eq!(
            overrides.get(&ShortcutId::Undo),
            Some(&KeyChord::ctrl(Key::U))
        );

        let mut restored = Shortcuts::new();
        restored.apply_overrides(&overrides);
        assert_eq!(
            restored.chord_for(ShortcutId::Undo),
            Some(KeyChord::ctrl(Key::U))
        );

        // Two actions claiming one chord: the second is ignored, the first
        // survives, and nothing panics.
        let mut conflicting = BTreeMap::new();
        conflicting.insert(ShortcutId::Undo, KeyChord::ctrl(Key::J));
        conflicting.insert(ShortcutId::Redo, KeyChord::ctrl(Key::J));
        let mut table = Shortcuts::new();
        table.apply_overrides(&conflicting);
        assert_eq!(
            table.chord_for(ShortcutId::Undo),
            Some(KeyChord::ctrl(Key::J))
        );
        assert_eq!(
            table.chord_for(ShortcutId::Redo),
            Some(KeyChord::ctrl(Key::Y))
        );
    }
}
