//! File-type registration: `rizref register` / `rizref unregister`.
//!
//! * **Windows** writes `.rizref` → `RizRef.Board`, its icon, the open verb and
//!   the Explorer thumbnail provider under `HKCU\Software\Classes` (or `HKLM`
//!   with `--machine`), `platform/windows/registry.rs`.
//! * **Linux** writes the freedesktop equivalents under `$XDG_DATA_HOME` (or
//!   `/usr/local/share` with `--machine`): a desktop entry, the
//!   `application/x-rizref` MIME type, hicolor icons and a thumbnailer entry,
//!   `platform/linux/register.rs`. The `.deb` installs the same files
//!   system-wide.
//! * **macOS** associates documents through the app bundle's `Info.plist`, so
//!   the command only says so (`platform/macos/register.rs`).
//!
//! On every OS `unregister` removes exactly what `register` wrote.

#[cfg(windows)]
pub use super::windows::registry::run;

#[cfg(target_os = "linux")]
pub use super::linux::register::run;

#[cfg(target_os = "macos")]
pub use super::macos::register::run;

/// The `.rizref` file icon's index among the Windows exe's icon groups
/// (`build.rs` embeds the app icon first and the document second).
#[cfg_attr(not(windows), allow(dead_code))]
pub const DOCUMENT_ICON_INDEX: u32 = 1;

/// The MIME type boards are registered as on Linux (and the `Content Type`
/// Windows records).
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub const MIME_TYPE: &str = "application/x-rizref";
