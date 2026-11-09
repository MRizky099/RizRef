//! Launching on macOS: `open -R` reveals a file in Finder, `open` opens a
//! folder or a URL. Every value is one `argv` entry, and none can be read as
//! a flag: paths must be absolute (`/…`) and URLs are `http(s)://…`.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::platform::launch::{LaunchError, launchable_path};
use crate::provenance;

/// Reveals `path` in Finder.
pub fn select_in_explorer(path: &Path) -> Result<(), LaunchError> {
    open(&["-R"], checked(path)?)
}

/// Opens the folder `path` in Finder.
pub fn open_folder(path: &Path) -> Result<(), LaunchError> {
    open(&[], checked(path)?)
}

/// Opens `url` in the default browser after validating it again.
pub fn open_url(url: &str) -> Result<(), LaunchError> {
    let url = provenance::normalize_url(url).ok_or(LaunchError::Refused)?;
    open(&[], &url)
}

/// Recent documents (`NSDocumentController`, the Dock menu) arrive with the
/// macOS integration in phase 08b; RizRef's own recents list is unaffected.
pub fn add_to_recent_documents(_path: &Path) {}

fn checked(path: &Path) -> Result<&str, LaunchError> {
    let text = path.to_str().ok_or(LaunchError::Refused)?;
    if launchable_path(text) && path.is_absolute() {
        Ok(text)
    } else {
        Err(LaunchError::Refused)
    }
}

fn open(flags: &[&str], value: &str) -> Result<(), LaunchError> {
    Command::new("/usr/bin/open")
        .args(flags)
        .arg(value)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| LaunchError::Failed(format!("open: {error}")))
}
