//! The macOS-only window modes: whole-window opacity through
//! `-[NSWindow setAlphaValue:]`, and the global ghost / click-through chords
//! through `CGEventSourceKeyState`, the CoreGraphics counterpart of Win32's
//! `GetAsyncKeyState` (it reads the hardware key state, so the chord is seen
//! while another application has the keyboard; no Accessibility or Input
//! Monitoring permission is involved in a key-state query).

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// `kCGEventSourceStateHIDSystemState`: the physical keyboard.
const HID_SYSTEM_STATE: i32 = 1;

/// Virtual key codes (`Carbon/HIToolbox/Events.h`).
const KEY_G: u16 = 0x05;
const KEY_X: u16 = 0x07;
const KEY_COMMAND: u16 = 0x37;
const KEY_RIGHT_COMMAND: u16 = 0x36;
const KEY_SHIFT: u16 = 0x38;
const KEY_RIGHT_SHIFT: u16 = 0x3C;

#[allow(unsafe_code)]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
}

/// Whether `key` is held on the physical keyboard.
#[allow(unsafe_code)]
fn key_down(key: u16) -> bool {
    // SAFETY: a pure query of global key state; any key code is valid.
    unsafe { CGEventSourceKeyState(HID_SYSTEM_STATE, key) }
}

/// ⌘⇧ plus `key`, the macOS spelling of the Windows `Ctrl+Shift` chords.
fn command_shift(key: u16) -> bool {
    (key_down(KEY_COMMAND) || key_down(KEY_RIGHT_COMMAND))
        && (key_down(KEY_SHIFT) || key_down(KEY_RIGHT_SHIFT))
        && key_down(key)
}

/// ⌘⇧G.
pub fn ghost_chord_down() -> bool {
    command_shift(KEY_G)
}

/// ⌘⇧X.
pub fn click_through_chord_down() -> bool {
    command_shift(KEY_X)
}

/// Sets the whole window's alpha, 0–1.
#[allow(unsafe_code)]
pub fn set_window_alpha(window: &Window, alpha: f64) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // SAFETY: winit's AppKit handle points at the window's live `NSView`,
    // and this runs on the main thread, where winit hands out the window.
    let view: &objc2_app_kit::NSView = unsafe { appkit.ns_view.cast().as_ref() };
    if let Some(ns_window) = view.window() {
        ns_window.setAlphaValue(alpha.clamp(0.0, 1.0));
    }
}
