//! Terminal output for the GUI-subsystem release build.
//!
//! On Windows a release `rizref.exe` has no console, so `--help`, `--version`
//! and `inspect` attach to the parent's (`platform/windows/console.rs`). Unix
//! processes always inherit their parent's standard streams: nothing to do.

#[cfg(windows)]
pub use super::windows::console::attach_parent;

/// Unix: the standard streams are already the terminal's (or `/dev/null`);
/// reports that nothing was attached.
#[cfg(unix)]
pub fn attach_parent() -> bool {
    false
}
