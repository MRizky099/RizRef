//! The parent console, for the GUI-subsystem release build.
//!
//! The release binary is linked for the Windows GUI subsystem, so starting it
//! from Explorer, the Start menu or a jump list never flashes a console
//! window. A GUI-subsystem process has no console of its own, though, so
//! `rizref --help`, `rizref --version` and `rizref inspect` typed into a
//! terminal would print nothing. [`attach_parent`] fixes that: when standard
//! output has no handle (nothing was redirected), it attaches to the console
//! of the process that launched us, if there is one. Redirected output — a
//! pipe from a script, `> file` — already has handles and is left alone.

use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE,
};

/// Attaches to the parent's console when standard output is not connected.
/// Returns whether a console was attached.
// SAFETY: `GetStdHandle` and `AttachConsole` take plain integers and only
// change this process's console association; called once, at startup, before
// any thread writes to standard output.
#[allow(unsafe_code)]
pub fn attach_parent() -> bool {
    unsafe {
        let connected = GetStdHandle(STD_OUTPUT_HANDLE)
            .is_ok_and(|handle| !handle.is_invalid() && !handle.0.is_null());
        if connected {
            return false;
        }
        AttachConsole(ATTACH_PARENT_PROCESS).is_ok()
    }
}
