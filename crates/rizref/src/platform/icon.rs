//! The window and taskbar icons.
//!
//! Windows loads them from the exe's own resources at the sizes the shell
//! asks for (`platform/windows/icon.rs`). Elsewhere the icon is decoded from
//! the same `.ico` the Windows build embeds, compiled into the binary, and
//! handed to winit (X11 `_NET_WM_ICON`; Wayland and macOS take the icon from
//! the desktop entry or the app bundle instead, so winit ignores it there).

#[cfg(windows)]
pub use super::windows::icon::app_icons;

#[cfg(unix)]
pub use super::unix::icon::app_icons;
