//! Window modes, Win32 half: `HWND_TOPMOST`, `WS_EX_LAYERED` /
//! `WS_EX_TRANSPARENT` with the layered alpha, and the global chord polling
//! with `GetAsyncKeyState`. The mode policy is `platform::window_modes`.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::platform::window_modes::WindowModes;

/// Applies the current state to a real window.
///
/// All Win32 failures are logged, not fatal: a window that cannot go
/// always-on-top is still a usable board.
pub fn apply(window: &Window, modes: &WindowModes) {
    set_always_on_top(window, modes.always_on_top);
    apply_layered(window, modes);
}

/// Applies the extended styles implied by `modes` to a raw style value.
///
/// Split out so the style juggling has an exact test without a window.
pub fn styled_ex_style(current: u32, modes: WindowModes) -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{WS_EX_LAYERED, WS_EX_TRANSPARENT};

    let mut style = current;
    style &= !WS_EX_LAYERED.0;
    style &= !WS_EX_TRANSPARENT.0;
    if modes.needs_layered() {
        style |= WS_EX_LAYERED.0;
    }
    if modes.effective_click_through() {
        style |= WS_EX_TRANSPARENT.0;
    }
    style
}

/// The window's `HWND`, or `None` on a non-Windows backend.
pub(crate) fn hwnd(window: &Window) -> Option<windows::Win32::Foundation::HWND> {
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => Some(windows::Win32::Foundation::HWND(
            win32.hwnd.get() as *mut core::ffi::c_void
        )),
        _ => None,
    }
}

/// Sets or clears `HWND_TOPMOST`.
#[allow(unsafe_code)]
fn set_always_on_top(window: &Window, on_top: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };

    let Some(hwnd) = hwnd(window) else {
        return;
    };
    let insert_after = if on_top { HWND_TOPMOST } else { HWND_NOTOPMOST };
    // SAFETY: `hwnd` came from a live winit window; the call only changes the
    // window's z-order and is documented as such.
    if let Err(error) = unsafe {
        SetWindowPos(
            hwnd,
            Some(insert_after),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    } {
        tracing::warn!(%error, on_top, "cannot set always-on-top");
    }
}

/// Applies `WS_EX_LAYERED`/`WS_EX_TRANSPARENT` and the window alpha.
#[allow(unsafe_code)]
fn apply_layered(window: &Window, modes: &WindowModes) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, LWA_ALPHA, SetLayeredWindowAttributes, SetWindowLongPtrW,
    };

    let Some(hwnd) = hwnd(window) else {
        return;
    };
    // SAFETY: `hwnd` is live for the duration of this call; GWL_EXSTYLE reads
    // and writes only the style word. `SetLayeredWindowAttributes` is called
    // only when the layered bit is set, which is its documented precondition.
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let wanted = styled_ex_style(current, *modes);
        if wanted != current {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted as isize);
        }
        if modes.needs_layered() {
            let alpha = (u16::from(modes.effective_opacity_percent()) * 255 / 100) as u8;
            if let Err(error) = SetLayeredWindowAttributes(
                hwnd,
                windows::Win32::Foundation::COLORREF(0),
                alpha,
                LWA_ALPHA,
            ) {
                tracing::warn!(%error, "cannot set layered window opacity");
            }
        }
    }
}

/// Reads one virtual key's current down state.
#[allow(unsafe_code)]
fn key_down(key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    // SAFETY: GetAsyncKeyState reads global key state and has no requirements.
    let state =
        unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(i32::from(key.0)) };
    (state as u16 & 0x8000) != 0
}

/// Whether the global ghost-mode chord (`ctrl+shift+G`) is currently held.
///
/// Polled by the event loop while ghost mode is active, because the window
/// itself is click-through and may not have keyboard focus.
pub fn ghost_chord_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_G, VK_SHIFT};
    key_down(VK_CONTROL) && key_down(VK_SHIFT) && key_down(VK_G)
}

/// Whether the global click-through chord (`ctrl+shift+X`) is currently held.
pub fn click_through_chord_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_SHIFT, VK_X};
    key_down(VK_CONTROL) && key_down(VK_SHIFT) && key_down(VK_X)
}

#[cfg(test)]
mod tests {
    use super::styled_ex_style;
    use crate::platform::window_modes::{
        GHOST_OPACITY_PERCENT, MIN_WINDOW_OPACITY_PERCENT, OPACITY_STEPS, WindowModes,
    };
    use windows::Win32::UI::WindowsAndMessaging::{WS_EX_LAYERED, WS_EX_TRANSPARENT};

    #[test]
    fn a_default_window_is_opaque_and_receives_clicks() {
        let modes = WindowModes::default();
        assert_eq!(modes.effective_opacity_percent(), 100);
        assert!(!modes.effective_click_through());
        assert!(!modes.needs_layered());
        assert_eq!(styled_ex_style(0, modes), 0);
    }

    #[test]
    fn click_through_implies_layered_and_transparent() {
        let mut modes = WindowModes::default();
        modes.toggle_click_through();

        let style = styled_ex_style(0, modes);
        assert_ne!(style & WS_EX_LAYERED.0, 0);
        assert_ne!(style & WS_EX_TRANSPARENT.0, 0);
    }

    #[test]
    fn transparency_implies_layered_but_not_transparent() {
        let mut modes = WindowModes::default();
        modes.set_opacity(40);

        let style = styled_ex_style(0, modes);
        assert_ne!(style & WS_EX_LAYERED.0, 0);
        assert_eq!(style & WS_EX_TRANSPARENT.0, 0);
    }

    #[test]
    fn with_per_pixel_alpha_transparency_is_the_canvas_alone() {
        let mut modes = WindowModes::with_canvas_alpha();
        modes.set_opacity(40);

        assert!((modes.canvas_alpha() - 0.4).abs() < 1.0e-6);
        assert_eq!(
            modes.effective_opacity_percent(),
            100,
            "the window stays opaque"
        );
        assert!(
            !modes.needs_layered(),
            "and is not layered: it takes clicks"
        );
        assert_eq!(styled_ex_style(0, modes), 0);

        // Ghost mode still fades the whole window, canvas transparency kept.
        modes.toggle_ghost();
        assert_eq!(modes.effective_opacity_percent(), GHOST_OPACITY_PERCENT);
        assert!((modes.canvas_alpha() - 0.4).abs() < 1.0e-6);
    }

    #[test]
    fn without_per_pixel_alpha_transparency_falls_back_to_the_window() {
        let mut modes = WindowModes::default();
        modes.set_opacity(60);

        assert_eq!(modes.canvas_alpha(), 1.0);
        assert_eq!(modes.effective_opacity_percent(), 60);
        assert!(modes.needs_layered());
    }

    #[test]
    fn ghost_mode_is_faint_and_click_through() {
        let mut modes = WindowModes::default();
        modes.toggle_ghost();

        assert!(modes.ghost());
        assert_eq!(modes.effective_opacity_percent(), GHOST_OPACITY_PERCENT);
        assert!(modes.effective_click_through());
        let style = styled_ex_style(0, modes);
        assert_ne!(style & WS_EX_LAYERED.0, 0);
        assert_ne!(style & WS_EX_TRANSPARENT.0, 0);
    }

    #[test]
    fn leaving_ghost_restores_the_previous_mode_exactly() {
        let mut modes = WindowModes::default();
        modes.set_opacity(60);
        modes.toggle_click_through();
        let before = modes;

        modes.toggle_ghost();
        assert_eq!(modes.effective_opacity_percent(), GHOST_OPACITY_PERCENT);
        modes.toggle_ghost();

        assert_eq!(modes, before, "ghost mode composes, it does not overwrite");
    }

    #[test]
    fn ghost_mode_remembers_a_transparency_change_made_while_ghosted() {
        let mut modes = WindowModes::default();
        modes.toggle_ghost();
        modes.set_opacity(50);
        modes.toggle_ghost();

        assert_eq!(modes.opacity_percent, 50);
        assert!(!modes.effective_click_through());
    }

    #[test]
    fn opacity_cycles_through_every_step_including_back_to_opaque() {
        let mut modes = WindowModes::default();
        let mut seen = Vec::new();
        for _ in 0..OPACITY_STEPS.len() {
            modes.cycle_opacity();
            seen.push(modes.opacity_percent);
        }
        assert_eq!(seen, vec![80, 60, 40, 20, 0, 100]);
    }

    #[test]
    fn opacity_is_clamped_to_the_supported_band() {
        let mut modes = WindowModes::default();
        modes.set_opacity(0);
        assert_eq!(
            modes.opacity_percent, 0,
            "a fully invisible canvas is allowed"
        );
        modes.set_opacity(200);
        assert_eq!(modes.opacity_percent, 100);
    }

    #[test]
    fn zero_percent_hides_the_canvas_but_never_the_window() {
        let mut modes = WindowModes::with_canvas_alpha();
        modes.set_opacity(0);
        assert_eq!(modes.canvas_alpha(), 0.0);
        assert_eq!(modes.effective_opacity_percent(), 100);
        assert!(
            !modes.needs_layered(),
            "an opaque, unlayered window takes clicks"
        );

        // The whole-window fallback stops at the lowest safe step.
        modes.canvas_alpha_supported = false;
        assert_eq!(modes.canvas_alpha(), 1.0);
        assert_eq!(
            modes.effective_opacity_percent(),
            MIN_WINDOW_OPACITY_PERCENT
        );
    }

    #[test]
    fn leaving_click_through_removes_both_bits() {
        let mut modes = WindowModes::default();
        modes.toggle_click_through();
        let layered = styled_ex_style(0, modes);
        modes.toggle_click_through();

        let cleared = styled_ex_style(layered, modes);
        assert_eq!(cleared & WS_EX_LAYERED.0, 0);
        assert_eq!(cleared & WS_EX_TRANSPARENT.0, 0);
    }
}
