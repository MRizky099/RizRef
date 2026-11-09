//! What the window system under this process can do.
//!
//! Some window modes are not offered everywhere: core Wayland has no way for a
//! client to keep itself above other windows, and the global key polling that
//! lets ghost mode be left from another application is a Win32 call. Rather
//! than offering a command that silently does nothing, the menu greys it out
//! and the command explains itself in a toast. The table is pure and tested;
//! [`for_window`] reads which windowing backend a live window is on.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// The windowing backend a window runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// A Win32 `HWND`.
    Win32,
    /// An X11 window (Xlib or XCB), including XWayland.
    X11,
    /// A native Wayland surface.
    Wayland,
    /// A macOS `NSView`.
    AppKit,
    /// Anything else (no window yet, or an unknown handle).
    Unknown,
}

/// The window-mode features a backend supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// The window can float above other applications.
    pub always_on_top: bool,
    /// Mouse input can pass through the window.
    pub click_through: bool,
    /// The whole window can be faded (the transparency fallback where the GPU
    /// surface has no per-pixel alpha, and ghost mode's fade).
    pub window_opacity: bool,
    /// The ghost and click-through chords are seen while another application
    /// has the keyboard. Without it, the chords work once RizRef is focused
    /// again (Alt+Tab, the taskbar or the dock).
    pub global_chords: bool,
    /// Drops can carry virtual files (Outlook attachments, files inside a
    /// compressed folder) — a Windows shell concept.
    pub virtual_file_drops: bool,
}

impl Capabilities {
    /// Everything the Win32 glue of phases 01–07 offers.
    pub const WIN32: Self = Self {
        always_on_top: true,
        click_through: true,
        window_opacity: true,
        global_chords: true,
        virtual_file_drops: true,
    };
}

/// The capability table, by backend.
pub fn for_backend(backend: Backend) -> Capabilities {
    let unix = Capabilities {
        // winit maps the window level to `_NET_WM_STATE_ABOVE` on X11 and to
        // the `NSWindow` level on macOS.
        always_on_top: true,
        // winit's cursor hit-test: an empty input shape on X11, a null input
        // region on Wayland, `ignoresMouseEvents` on macOS.
        click_through: true,
        window_opacity: false,
        global_chords: false,
        virtual_file_drops: false,
    };
    match backend {
        Backend::Win32 => Capabilities::WIN32,
        Backend::X11 | Backend::AppKit => unix,
        Backend::Wayland => Capabilities {
            // Not in the core protocol: the compositor alone decides stacking.
            always_on_top: false,
            ..unix
        },
        Backend::Unknown => {
            if cfg!(windows) {
                Capabilities::WIN32
            } else {
                unix
            }
        }
    }
}

/// The backend of a live window.
pub fn backend_of(window: &Window) -> Backend {
    let Ok(handle) = window.window_handle() else {
        return Backend::Unknown;
    };
    match handle.as_raw() {
        RawWindowHandle::Win32(_) => Backend::Win32,
        RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_) => Backend::X11,
        RawWindowHandle::Wayland(_) => Backend::Wayland,
        RawWindowHandle::AppKit(_) => Backend::AppKit,
        _ => Backend::Unknown,
    }
}

/// The capabilities of a live window's backend.
pub fn for_window(window: &Window) -> Capabilities {
    for_backend(backend_of(window))
}

#[cfg(test)]
mod tests {
    use super::{Backend, Capabilities, for_backend};

    #[test]
    fn windows_keeps_every_phase_07_mode() {
        assert_eq!(for_backend(Backend::Win32), Capabilities::WIN32);
        if cfg!(windows) {
            assert_eq!(for_backend(Backend::Unknown), Capabilities::WIN32);
        }
    }

    #[test]
    fn wayland_cannot_stay_on_top_but_can_pass_clicks_through() {
        let wayland = for_backend(Backend::Wayland);
        assert!(!wayland.always_on_top);
        assert!(wayland.click_through);
        assert!(for_backend(Backend::X11).always_on_top);
    }

    #[test]
    fn only_windows_polls_chords_globally_or_drops_virtual_files() {
        for backend in [Backend::X11, Backend::Wayland, Backend::AppKit] {
            let caps = for_backend(backend);
            assert!(!caps.global_chords, "{backend:?}");
            assert!(!caps.virtual_file_drops, "{backend:?}");
        }
    }
}
