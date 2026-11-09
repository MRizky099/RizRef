//! Launching an image's source: Explorer for a file, the browser for a URL.
//!
//! This is a security boundary. Both inputs come from a board file, which is
//! untrusted, so:
//!
//! * a URL is re-validated here with [`provenance::normalize_url`] — only
//!   `http`/`https`, no control characters, nothing overlong — and handed to
//!   `ShellExecuteW` with the `open` verb, never through a command shell;
//! * a path becomes **one** `explorer.exe` argument, `/select,"<path>"`,
//!   passed raw so Rust's argument quoting cannot split or re-quote it, and
//!   refused outright if it contains a quote or a control character (neither
//!   is legal in a Windows path, so no real file is lost).
//!
//! The argument building is pure and tested; only the two spawn functions
//! touch the OS.
#![allow(unsafe_code)]

use std::os::windows::process::CommandExt;
use std::path::Path;

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, w};

use crate::provenance;

/// Why a launch did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// The value failed validation and was never passed to the OS.
    Refused,
    /// The OS refused or failed the launch.
    Failed(String),
}

/// The single raw argument that makes Explorer select `path`.
///
/// `None` when the path holds a `"` or a control character, which would let
/// a crafted board break out of the quoted argument.
pub fn select_argument(path: &Path) -> Option<String> {
    let text = quoted_path(path)?;
    Some(format!("/select,{text}"))
}

/// The single raw argument that makes Explorer open the folder `path`.
pub fn folder_argument(path: &Path) -> Option<String> {
    quoted_path(path)
}

/// `"<path>"`, refusing anything that could escape the quotes.
fn quoted_path(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    if text.is_empty() || text.chars().any(|c| c == '"' || c.is_control()) {
        return None;
    }
    Some(format!("\"{text}\""))
}

/// Opens Explorer with `path` selected.
pub fn select_in_explorer(path: &Path) -> Result<(), LaunchError> {
    let argument = select_argument(path).ok_or(LaunchError::Refused)?;
    spawn_explorer(&argument)
}

/// Opens Explorer at the folder `path`.
pub fn open_folder(path: &Path) -> Result<(), LaunchError> {
    let argument = folder_argument(path).ok_or(LaunchError::Refused)?;
    spawn_explorer(&argument)
}

/// Runs `explorer.exe <argument>` with the argument passed verbatim.
fn spawn_explorer(argument: &str) -> Result<(), LaunchError> {
    std::process::Command::new("explorer.exe")
        .raw_arg(argument)
        .spawn()
        .map(|_| ())
        .map_err(|error| LaunchError::Failed(error.to_string()))
}

/// Opens `url` in the default browser after validating it again.
pub fn open_url(url: &str) -> Result<(), LaunchError> {
    let url = provenance::normalize_url(url).ok_or(LaunchError::Refused)?;
    let file = HSTRING::from(url.as_str());
    // SAFETY: every string argument is a live NUL-terminated wide string for
    // the duration of the call; a null window handle is allowed.
    let result = unsafe { ShellExecuteW(None, w!("open"), &file, None, None, SW_SHOWNORMAL) };
    // ShellExecuteW returns a pseudo-HINSTANCE: greater than 32 on success,
    // an error code (0 meaning out of memory or resources) otherwise.
    let code = result.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(LaunchError::Failed(format!(
            "ShellExecuteW returned {code}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{folder_argument, select_argument};
    use std::path::Path;

    #[test]
    fn the_select_argument_is_one_quoted_value() {
        assert_eq!(
            select_argument(Path::new(r"C:\refs\hero.png")).as_deref(),
            Some(r#"/select,"C:\refs\hero.png""#)
        );
        for awkward in [
            r"C:\My Refs\a b.png",
            r"C:\refs\one,two,three.png",
            r"C:\refs\salt & pepper.png",
            r"C:\refs\100%.png",
            r"C:\refs\^caret;semi.png",
            r"C:\Référence\日本語.png",
            r"\\server\share\a.png",
        ] {
            let argument = select_argument(Path::new(awkward)).expect(awkward);
            assert_eq!(argument, format!("/select,\"{awkward}\""));
        }
    }

    #[test]
    fn quotes_and_control_characters_are_refused() {
        for hostile in [
            r#"C:\refs\a.png" /e,C:\Windows"#,
            "C:\\refs\\a\npng",
            "C:\\refs\\a\u{0}.png",
            "",
        ] {
            assert_eq!(select_argument(Path::new(hostile)), None, "{hostile:?}");
            assert_eq!(folder_argument(Path::new(hostile)), None, "{hostile:?}");
        }
    }

    #[test]
    fn the_folder_argument_is_quoted() {
        assert_eq!(
            folder_argument(Path::new(r"C:\My Refs")).as_deref(),
            Some(r#""C:\My Refs""#)
        );
    }

    #[test]
    fn open_url_refuses_non_web_schemes_without_launching() {
        for hostile in [
            "javascript:alert(1)",
            "file:///C:/Windows/System32/calc.exe",
            "calc.exe",
            r"C:\Windows\System32\calc.exe",
            "ms-settings:",
            "https://e.com/\u{7}",
        ] {
            assert_eq!(
                super::open_url(hostile),
                Err(super::LaunchError::Refused),
                "{hostile:?}"
            );
        }
    }
}
