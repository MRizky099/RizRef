//! The window's icons, loaded from the exe's own resources.
//!
//! `build.rs` embeds the application icon as `RT_GROUP_ICON` 1. Explorer and
//! the taskbar's pinned button read it from the file, but a window has no
//! icon of its own unless one is set: Alt-Tab and the running taskbar button
//! would show the generic window glyph. winit loads a size from a resource
//! group, so each icon is asked for at the size the system draws it, letting
//! Windows pick the hand-tuned rendition instead of scaling a large one.

use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXICON, SM_CXSMICON, SYSTEM_METRICS_INDEX,
};
use winit::dpi::PhysicalSize;
use winit::platform::windows::IconExtWindows;
use winit::window::Icon;

/// The application icon's resource id (`RT_GROUP_ICON` 1).
const APP_ICON: u16 = 1;

/// The small (title bar, Alt-Tab list) and big (taskbar, Alt-Tab preview)
/// icons. `None` when the exe carries no icon resource — test and example
/// binaries do not.
pub fn app_icons() -> (Option<Icon>, Option<Icon>) {
    (load(SM_CXSMICON, 16), load(SM_CXICON, 32))
}

/// Loads the application icon at the system's size for `metric`.
// SAFETY: `GetSystemMetrics` takes a plain index and has no preconditions.
#[allow(unsafe_code)]
fn load(metric: SYSTEM_METRICS_INDEX, fallback: u32) -> Option<Icon> {
    let side = match unsafe { GetSystemMetrics(metric) } {
        side if side > 0 => side as u32,
        _ => fallback,
    };
    Icon::from_resource(APP_ICON, Some(PhysicalSize::new(side, side))).ok()
}
