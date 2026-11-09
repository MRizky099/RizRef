//! What differs on Linux from macOS: launching through `xdg-open` and the
//! file manager's D-Bus interface, and the freedesktop file-type
//! registration.

pub mod launch;
pub mod register;
