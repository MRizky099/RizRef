//! What differs on macOS from Linux: launching through `open`, the
//! `register` command (associations come from the app bundle), Finder's
//! open-document requests on the application delegate, and the window modes
//! AppKit offers beyond winit (whole-window alpha, the global chords).

pub mod app_delegate;
pub mod launch;
pub mod register;
pub mod window_modes;
