//! Launching an image's source: the file manager for a file, the browser for
//! a URL.
//!
//! This is a security boundary. Both inputs come from a board file, which is
//! untrusted, so on every OS:
//!
//! * a URL is re-validated with [`crate::provenance::normalize_url`] — only
//!   `http`/`https`, no control characters, nothing overlong — and handed to
//!   the OS's URL opener (`ShellExecuteW`, `xdg-open`, `open`) as one
//!   argument, never through a command shell;
//! * a path becomes one argument of the file manager (or one `file://` URI on
//!   the freedesktop D-Bus interface), refused outright when it holds a
//!   control character.
//!
//! | | select a file | open a folder | open a URL | recent documents |
//! |---|---|---|---|---|
//! | Windows | `explorer /select,` | Explorer | `ShellExecuteW` | `SHAddToRecentDocs` (jump list) |
//! | Linux | `FileManager1.ShowItems`, else the folder | `xdg-open` | `xdg-open` | not yet (`recently-used.xbel`) |
//! | macOS | `open -R` | `open` | `open` | not yet (phase 08b) |

#[cfg(windows)]
pub use super::windows::launch::{
    add_to_recent_documents, open_folder, open_url, select_in_explorer,
};

#[cfg(target_os = "linux")]
pub use super::linux::launch::{
    add_to_recent_documents, open_folder, open_url, select_in_explorer,
};

#[cfg(target_os = "macos")]
pub use super::macos::launch::{
    add_to_recent_documents, open_folder, open_url, select_in_explorer,
};

/// Why a launch did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// The value failed validation and was never passed to the OS.
    Refused,
    /// The OS refused or failed the launch.
    Failed(String),
}

/// Whether `text` is a path RizRef will hand to a launcher: non-empty and
/// free of control characters (none is legal in a real file name a user
/// picked, and a newline is how a crafted value would smuggle a second
/// argument into a line-based tool).
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn launchable_path(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::{LaunchError, launchable_path, open_url};

    #[test]
    fn open_url_refuses_non_web_schemes_without_launching() {
        for hostile in [
            "javascript:alert(1)",
            "file:///C:/Windows/System32/calc.exe",
            "file:///usr/bin/xterm",
            "calc.exe",
            r"C:\Windows\System32\calc.exe",
            "/usr/bin/xterm",
            "ms-settings:",
            "https://e.com/\u{7}",
        ] {
            assert_eq!(open_url(hostile), Err(LaunchError::Refused), "{hostile:?}");
        }
    }

    #[test]
    fn control_characters_never_reach_a_launcher() {
        assert!(launchable_path("/home/u/refs/a b,c&d.png"));
        assert!(!launchable_path(""));
        assert!(!launchable_path("/home/u/a\nb.png"));
        assert!(!launchable_path("/home/u/a\u{0}.png"));
    }
}
