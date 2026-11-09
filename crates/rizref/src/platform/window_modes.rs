//! Window modes: always-on-top, transparency, click-through and ghost mode.
//!
//! Transparency (phase 07) is the canvas background only: images, notes,
//! selection chrome, the HUD and the title bar stay opaque. It needs a GPU
//! surface that composites per-pixel alpha (DX12 through DirectComposition on
//! Windows); the renderer reports whether it got one, and where it did not
//! (Vulkan's Win32 surfaces are opaque) the setting falls back to the
//! whole-window opacity of earlier builds, through `WS_EX_LAYERED`. Ghost mode
//! keeps its whole-window 15 %: it is for tracing over another application,
//! where the images themselves must fade too.
//!
//! These are one module because the Win32 extended styles interact: layered is
//! required by click-through, click-through is required by ghost mode, and
//! ghost mode temporarily overrides opacity. The *policy* is pure data and
//! unit-tested; only [`WindowModes::apply`] calls user32.
//!
//! Ghost mode has a chicken-and-egg problem: once `WS_EX_TRANSPARENT` is set the
//! window receives no mouse or keyboard input, so a normal in-app shortcut can
//! no longer turn it off. The escape hatch is therefore global — the app polls
//! the ghost chord with `GetAsyncKeyState` on a short timer while the mode is
//! active, exactly as long as it is needed.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Fullscreen, Window};

/// Default window opacity in ghost mode, in percent.
///
/// Low enough to trace over, high enough that the board's silhouette is still
/// recognisable at a glance.
pub const GHOST_OPACITY_PERCENT: u8 = 15;

/// The smallest transparency setting: a fully invisible canvas background.
pub const MIN_OPACITY_PERCENT: u8 = 0;

/// The lowest opacity ever applied to the whole window (the fallback where
/// per-pixel alpha is unavailable): 0 % there would hide RizRef entirely,
/// with nothing left to click to bring it back.
pub const MIN_WINDOW_OPACITY_PERCENT: u8 = 20;

/// The order the transparency menu cycles through.
pub const OPACITY_STEPS: [u8; 6] = [100, 80, 60, 40, 20, 0];

/// The mode state, independent of any window handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowModes {
    /// Whether the window floats above other applications.
    pub always_on_top: bool,
    /// Transparency setting, 20–100 %: the canvas background's opacity, or
    /// the whole window's where per-pixel alpha is unavailable.
    pub opacity_percent: u8,
    /// Whether the GPU surface composites per-pixel alpha, so transparency
    /// can be the canvas alone. Set once the GPU is attached.
    pub canvas_alpha_supported: bool,
    /// Whether ordinary click-through is armed.
    pub click_through: bool,
    /// Whether ghost mode is active, with the state to restore when it ends.
    ghost: Option<GhostRestore>,
    /// Opacity used while ghost mode is active.
    pub ghost_opacity_percent: u8,
    /// Whether borderless fullscreen is active.
    pub borderless: bool,
}

/// What ghost mode overrides, restored when it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GhostRestore {
    opacity_percent: u8,
    click_through: bool,
}

impl Default for WindowModes {
    fn default() -> Self {
        Self {
            always_on_top: false,
            opacity_percent: 100,
            canvas_alpha_supported: false,
            click_through: false,
            ghost: None,
            ghost_opacity_percent: GHOST_OPACITY_PERCENT,
            borderless: false,
        }
    }
}

impl WindowModes {
    /// The modes from persisted settings.
    ///
    /// Click-through and ghost mode are deliberately not persisted: an
    /// invisible, input-ignoring window at launch reads as a failed launch.
    pub fn from_settings(settings: &crate::settings::Settings) -> Self {
        Self {
            always_on_top: settings.always_on_top,
            opacity_percent: settings.opacity_percent.clamp(MIN_OPACITY_PERCENT, 100),
            ghost_opacity_percent: settings.ghost_opacity_percent.clamp(10, 40),
            ..Self::default()
        }
    }

    /// Whether ghost mode is active.
    pub fn ghost(self) -> bool {
        self.ghost.is_some()
    }

    /// The alpha applied to the whole window, in percent: ghost mode's, or —
    /// only without per-pixel alpha — the transparency setting.
    pub fn effective_opacity_percent(self) -> u8 {
        match self.ghost {
            Some(_) => self.ghost_opacity_percent,
            None if self.canvas_alpha_supported => 100,
            None => self.opacity_percent.max(MIN_WINDOW_OPACITY_PERCENT),
        }
    }

    /// The canvas background's opacity, 0–1: the transparency setting when
    /// the surface composites per-pixel alpha, otherwise opaque (the window
    /// carries the transparency instead).
    pub fn canvas_alpha(self) -> f32 {
        if self.canvas_alpha_supported {
            f32::from(self.opacity_percent) / 100.0
        } else {
            1.0
        }
    }

    /// Whether clicks should pass through the window.
    pub fn effective_click_through(self) -> bool {
        self.ghost.is_some() || self.click_through
    }

    /// Whether the window needs `WS_EX_LAYERED` for its current state.
    pub fn needs_layered(self) -> bool {
        self.effective_click_through() || self.effective_opacity_percent() < 100
    }

    /// Toggles ghost mode, saving and restoring the state it overrides.
    pub fn toggle_ghost(&mut self) {
        match self.ghost.take() {
            Some(restore) => {
                self.opacity_percent = restore.opacity_percent;
                self.click_through = restore.click_through;
            }
            None => {
                self.ghost = Some(GhostRestore {
                    opacity_percent: self.opacity_percent,
                    click_through: self.click_through,
                });
            }
        }
    }

    /// Cycles ordinary opacity through [`OPACITY_STEPS`].
    pub fn cycle_opacity(&mut self) {
        let current = self.opacity_percent;
        let next = OPACITY_STEPS
            .iter()
            .copied()
            .find(|step| *step < current)
            .unwrap_or(OPACITY_STEPS[0]);
        self.opacity_percent = next;
        if let Some(ghost) = self.ghost.as_mut() {
            // Changing transparency while ghosted updates what will be
            // restored, so leaving ghost mode lands on the user's new choice.
            ghost.opacity_percent = next;
        }
    }

    /// Sets ordinary opacity, clamped to the supported band.
    pub fn set_opacity(&mut self, percent: u8) {
        self.opacity_percent = percent.clamp(MIN_OPACITY_PERCENT, 100);
        if let Some(ghost) = self.ghost.as_mut() {
            ghost.opacity_percent = self.opacity_percent;
        }
    }

    /// Toggles ordinary click-through.
    pub fn toggle_click_through(&mut self) {
        self.click_through = !self.click_through;
        if let Some(ghost) = self.ghost.as_mut() {
            ghost.click_through = self.click_through;
        }
    }

    /// Applies the current state to a real window.
    ///
    /// All Win32 failures are logged, not fatal: a window that cannot go
    /// always-on-top is still a usable board.
    pub fn apply(&self, window: &Window) {
        set_always_on_top(window, self.always_on_top);
        apply_layered(window, self);
        set_borderless(window, self.borderless);
    }
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

/// Toggles borderless fullscreen through winit (which owns the taskbar and
/// monitor bookkeeping).
pub fn set_borderless(window: &Window, borderless: bool) {
    if borderless {
        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
    } else {
        window.set_fullscreen(None);
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
    use super::{
        GHOST_OPACITY_PERCENT, MIN_WINDOW_OPACITY_PERCENT, OPACITY_STEPS, WindowModes,
        styled_ex_style,
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
        let mut modes = WindowModes {
            canvas_alpha_supported: true,
            ..WindowModes::default()
        };
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
        let mut modes = WindowModes {
            canvas_alpha_supported: true,
            ..WindowModes::default()
        };
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
