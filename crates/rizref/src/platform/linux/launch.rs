//! Launching on Linux: `xdg-open` for URLs and folders, the freedesktop
//! `org.freedesktop.FileManager1.ShowItems` D-Bus call (Nautilus, Dolphin,
//! Nemo, Caja, Thunar…) to select a file, falling back to opening its folder.
//!
//! Every value is passed as one `argv` entry to a program found on `PATH`,
//! never through a shell. The D-Bus call goes through `dbus-send` with the
//! path as a percent-encoded `file://` URI, so neither a space nor a comma
//! (the `array:string:` separator) can split it.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::platform::launch::{LaunchError, launchable_path};
use crate::provenance;

/// Selects `path` in the file manager, or opens its folder when no file
/// manager answers the D-Bus call.
pub fn select_in_explorer(path: &Path) -> Result<(), LaunchError> {
    let uri = file_uri(path).ok_or(LaunchError::Refused)?;
    let shown = Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{uri}"))
        .arg("string:")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if shown {
        return Ok(());
    }
    tracing::debug!("no FileManager1 on the session bus; opening the folder instead");
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => open_folder(parent),
        _ => Err(LaunchError::Refused),
    }
}

/// Opens the folder `path` in the user's file manager.
pub fn open_folder(path: &Path) -> Result<(), LaunchError> {
    let text = path.to_str().ok_or(LaunchError::Refused)?;
    if !launchable_path(text) || !path.is_absolute() {
        return Err(LaunchError::Refused);
    }
    xdg_open(text)
}

/// Opens `url` in the default browser after validating it again.
pub fn open_url(url: &str) -> Result<(), LaunchError> {
    let url = provenance::normalize_url(url).ok_or(LaunchError::Refused)?;
    xdg_open(&url)
}

/// Recent documents are not recorded on Linux yet: the shared
/// `recently-used.xbel` is another application's file to merge into, which
/// the phase-08 plan lists as its own item. RizRef's own recents list is
/// unaffected.
pub fn add_to_recent_documents(_path: &Path) {}

/// Runs `xdg-open <value>` without waiting for it.
fn xdg_open(value: &str) -> Result<(), LaunchError> {
    Command::new("xdg-open")
        .arg(value)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| LaunchError::Failed(format!("xdg-open: {error}")))
}

/// `file://` plus the absolute path, percent-encoded except for the RFC 3986
/// unreserved characters and `/`. `None` for a relative path, a non-UTF-8
/// path or one with a control character.
pub(crate) fn file_uri(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    if !launchable_path(text) || !path.is_absolute() {
        return None;
    }
    let mut uri = String::with_capacity(text.len() + 7);
    uri.push_str("file://");
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Some(uri)
}

#[cfg(test)]
mod tests {
    use super::{file_uri, open_folder};
    use crate::platform::launch::LaunchError;
    use std::path::Path;

    #[test]
    fn file_uris_escape_everything_that_could_split_an_argument() {
        assert_eq!(
            file_uri(Path::new("/home/u/My Refs/a,b&c.png")).as_deref(),
            Some("file:///home/u/My%20Refs/a%2Cb%26c.png")
        );
        assert_eq!(
            file_uri(Path::new("/home/u/Référence.png")).as_deref(),
            Some("file:///home/u/R%C3%A9f%C3%A9rence.png")
        );
    }

    #[test]
    fn relative_and_control_character_paths_are_refused() {
        assert_eq!(file_uri(Path::new("refs/a.png")), None);
        assert_eq!(file_uri(Path::new("/home/u/a\nb.png")), None);
        assert_eq!(
            open_folder(Path::new("relative/dir")),
            Err(LaunchError::Refused)
        );
    }
}
