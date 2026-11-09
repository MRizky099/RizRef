//! Where RizRef keeps its own small state files.
//!
//! Settings and the recent-board list are *roaming* app data (the plan says
//! `%APPDATA%\RizRef`), while logs and the dev camera checkpoint stay in local
//! app data where they belong. Both fall back to the temp directory when the
//! environment does not provide an app-data location, the same rule the logging
//! path uses: a stripped service context must not make the app fail to boot.

use std::path::PathBuf;

/// The roaming application directory, `%APPDATA%\RizRef`.
pub fn roaming_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rizref-roaming"))
        .join("RizRef")
}

/// The local (non-roaming) application directory, `%LOCALAPPDATA%\RizRef`:
/// logs, crash dumps, autosave copies and the session marker.
pub fn local_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rizref"))
        .join("RizRef")
}

/// `%APPDATA%\RizRef\settings.json`.
pub fn settings_file() -> PathBuf {
    roaming_dir().join("settings.json")
}

/// `%APPDATA%\RizRef\recents.json`.
pub fn recents_file() -> PathBuf {
    roaming_dir().join("recents.json")
}

#[cfg(test)]
mod tests {
    use super::{recents_file, settings_file};

    #[test]
    fn the_state_files_live_under_the_rizref_directory() {
        for path in [settings_file(), recents_file()] {
            assert!(path.components().any(|c| c.as_os_str() == "RizRef"));
            assert!(path.extension().is_some_and(|ext| ext == "json"));
        }
    }
}
