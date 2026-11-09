//! A native error message box, for failures that happen before (or instead
//! of) the window: a panic, a GPU that will not start.
//!
//! Windows uses `MessageBoxW` (`platform/windows/message_box.rs`); Linux and
//! macOS use `rfd`'s message dialog (the xdg-desktop-portal / `zenity` path on
//! Linux, `NSAlert` on macOS). The message is always logged as well, so a
//! headless session without any dialog service still records why RizRef
//! stopped.

#[cfg(windows)]
pub use super::windows::message_box::alert;

#[cfg(unix)]
pub use super::unix::message_box::alert;
