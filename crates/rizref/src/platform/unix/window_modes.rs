//! Window modes on Linux and macOS, through winit (see
//! `platform::window_modes` for the policy and `platform::capabilities` for
//! what each backend offers).
//!
//! * always on top — `WindowLevel::AlwaysOnTop` (`_NET_WM_STATE_ABOVE` on X11,
//!   the floating window level on macOS); skipped on Wayland, which has no
//!   such request;
//! * click-through — winit's cursor hit-test (an empty input shape on X11, an
//!   empty input region on Wayland, `ignoresMouseEvents` on macOS);
//! * whole-window opacity — `NSWindow` alpha on macOS
//!   (`platform/macos/window_modes.rs`); not available through winit on
//!   Linux, where transparency is the canvas alone if the GPU surface
//!   composites alpha, otherwise opaque;
//! * the global chords — `CGEventSourceKeyState` on macOS (⌘⇧G / ⌘⇧X); on
//!   Linux no global key state without an OS hook, so they are never "down"
//!   and the in-app shortcut leaves ghost mode once RizRef has focus again.

use winit::window::{Window, WindowLevel};

use crate::platform::capabilities;
use crate::platform::window_modes::WindowModes;

/// Applies the current state to a real window.
pub fn apply(window: &Window, modes: &WindowModes) {
    let caps = capabilities::for_window(window);
    if caps.always_on_top {
        window.set_window_level(if modes.always_on_top {
            WindowLevel::AlwaysOnTop
        } else {
            WindowLevel::Normal
        });
    }
    if caps.click_through
        && let Err(error) = window.set_cursor_hittest(!modes.effective_click_through())
    {
        tracing::warn!(%error, "cannot change click-through");
    }
    #[cfg(target_os = "macos")]
    if caps.window_opacity {
        crate::platform::macos::window_modes::set_window_alpha(
            window,
            f64::from(modes.effective_opacity_percent()) / 100.0,
        );
    }
}

/// Whether the ghost chord is held anywhere (macOS), never on Linux.
pub fn ghost_chord_down() -> bool {
    #[cfg(target_os = "macos")]
    return crate::platform::macos::window_modes::ghost_chord_down();
    #[cfg(not(target_os = "macos"))]
    false
}

/// Whether the click-through chord is held anywhere (macOS), never on Linux.
pub fn click_through_chord_down() -> bool {
    #[cfg(target_os = "macos")]
    return crate::platform::macos::window_modes::click_through_chord_down();
    #[cfg(not(target_os = "macos"))]
    false
}
