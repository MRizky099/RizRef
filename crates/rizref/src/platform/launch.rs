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

use std::os::windows::ffi::OsStringExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use windows::Win32::Storage::FileSystem::GetLongPathNameW;

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
    let argument = select_argument(&long_path(path)).ok_or(LaunchError::Refused)?;
    spawn_explorer(&argument)
}

/// Opens Explorer at the folder `path`.
pub fn open_folder(path: &Path) -> Result<(), LaunchError> {
    let argument = folder_argument(&long_path(path)).ok_or(LaunchError::Refused)?;
    spawn_explorer(&argument)
}

/// Expands 8.3 short names (`USERNA~1`) to the long path.
///
/// Explorer opens the folder of a short `/select` path but does not select
/// the file, and short names reach a board through command-line arguments.
/// Falls back to the path as given when it does not exist or cannot expand.
fn long_path(path: &Path) -> PathBuf {
    let wide = HSTRING::from(path.as_os_str());
    // SAFETY: a size query with no buffer; the string is NUL-terminated.
    let needed = unsafe { GetLongPathNameW(&wide, None) } as usize;
    if needed == 0 {
        return path.to_path_buf();
    }
    let mut buffer = vec![0u16; needed];
    // SAFETY: the buffer holds `needed` units, which the call asked for.
    let written = unsafe { GetLongPathNameW(&wide, Some(&mut buffer)) } as usize;
    if written == 0 || written >= buffer.len() {
        return path.to_path_buf();
    }
    buffer.truncate(written);
    PathBuf::from(std::ffi::OsString::from_wide(&buffer))
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

/// Adds `path` to the shell's recent documents.
///
/// That one call feeds the taskbar jump list's *Recent* category (once
/// `.rizref` is registered to RizRef — `rizref register`), Explorer's Recent
/// folder and the Start menu: the jump list the plan asks for, with Windows
/// keeping the list, its pinning and its "remove from this list" behaviour.
/// A custom `ICustomDestinationList` would only duplicate that.
// SAFETY: `SHAddToRecentDocs(SHARD_PATHW, …)` reads one NUL-terminated wide
// string that outlives the call.
#[allow(unsafe_code)]
pub fn add_to_recent_documents(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{SHARD_PATHW, SHAddToRecentDocs};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe { SHAddToRecentDocs(SHARD_PATHW.0 as u32, Some(wide.as_ptr().cast())) };
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
    fn short_names_expand_and_missing_paths_pass_through() {
        let dir = std::env::temp_dir().join(format!("rizref long name {}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("a long file name.png");
        std::fs::write(&file, b"x").expect("file");
        let long = super::long_path(&file);
        assert!(
            long.to_string_lossy().ends_with("a long file name.png"),
            "{}",
            long.display()
        );
        assert!(!long.to_string_lossy().contains('~'), "{}", long.display());
        let missing = Path::new(r"C:\definitely\absent\here.png");
        assert_eq!(super::long_path(missing), missing);
        std::fs::remove_dir_all(&dir).ok();
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
