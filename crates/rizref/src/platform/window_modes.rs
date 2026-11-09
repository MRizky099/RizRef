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
//! unit-tested here on every OS; only [`WindowModes::apply`] reaches the OS
//! half (`platform/windows/` calls user32; `platform/unix/` goes through
//! winit's window level and cursor hit-test).
//!
//! Ghost mode has a chicken-and-egg problem: once `WS_EX_TRANSPARENT` is set the
//! window receives no mouse or keyboard input, so a normal in-app shortcut can
//! no longer turn it off. The escape hatch is therefore global — the app polls
//! the ghost chord with `GetAsyncKeyState` on a short timer while the mode is
//! active, exactly as long as it is needed.

use winit::window::{Fullscreen, Window};

#[cfg(windows)]
use super::windows::window_modes as os;

#[cfg(unix)]
use super::unix::window_modes as os;

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

    /// The default modes on a surface that composites per-pixel alpha.
    #[cfg(test)]
    pub(crate) fn with_canvas_alpha() -> Self {
        Self {
            canvas_alpha_supported: true,
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
    #[cfg_attr(not(windows), allow(dead_code))]
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
    /// All OS failures are logged, not fatal: a window that cannot go
    /// always-on-top is still a usable board. A mode the window system does
    /// not offer (`platform::capabilities`) is skipped by the OS half.
    pub fn apply(&self, window: &Window) {
        os::apply(window, self);
        set_borderless(window, self.borderless);
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

/// Whether the global ghost-mode chord (`ctrl+shift+G`) is currently held.
///
/// Polled by the event loop while ghost mode is active, because the window
/// itself is click-through and may not have keyboard focus. Always `false`
/// where the OS offers no global key state (`Capabilities::global_chords`);
/// there the ordinary shortcut leaves the mode once RizRef has focus.
pub fn ghost_chord_down() -> bool {
    os::ghost_chord_down()
}

/// Whether the global click-through chord (`ctrl+shift+X`) is currently held.
pub fn click_through_chord_down() -> bool {
    os::click_through_chord_down()
}

#[cfg(test)]
mod tests {
    use super::{MIN_WINDOW_OPACITY_PERCENT, WindowModes};

    #[test]
    fn ghost_mode_is_click_through_and_restores_on_exit() {
        let mut modes = WindowModes::default();
        modes.toggle_ghost();
        assert!(modes.ghost() && modes.effective_click_through());
        modes.toggle_ghost();
        assert_eq!(modes, WindowModes::default());
    }

    #[test]
    fn per_pixel_alpha_keeps_the_window_opaque() {
        let mut modes = WindowModes::with_canvas_alpha();
        modes.set_opacity(0);
        assert_eq!(modes.effective_opacity_percent(), 100);
        assert_eq!(modes.canvas_alpha(), 0.0);
        modes.canvas_alpha_supported = false;
        assert_eq!(
            modes.effective_opacity_percent(),
            MIN_WINDOW_OPACITY_PERCENT
        );
        assert_eq!(modes.canvas_alpha(), 1.0);
    }
}
