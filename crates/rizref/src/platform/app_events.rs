//! Requests the operating system makes of the application as a whole.
//!
//! On macOS, Finder opens documents and the Dock quits the app through the
//! `NSApplication` delegate rather than through `argv` or a window
//! (`platform/macos/app_delegate.rs`). Windows and Linux hand documents over
//! on the command line (and through the single-instance channel), so there
//! [`install`] does nothing.

/// Installs the handlers: `on_open` receives document paths Finder asked
/// RizRef to open, `on_quit` a system quit request. Call once, after the
/// winit event loop is built and before it runs.
pub fn install(
    on_open: impl Fn(Vec<String>) + Send + Sync + 'static,
    on_quit: impl Fn() + Send + Sync + 'static,
) {
    #[cfg(target_os = "macos")]
    super::macos::app_delegate::install(on_open, on_quit);
    #[cfg(not(target_os = "macos"))]
    let _ = (on_open, on_quit);
}
