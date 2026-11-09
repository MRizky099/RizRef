//! Where RizRef keeps its own small state files.
//!
//! Settings and the recent-board list are *roaming* app data (the plan says
//! `%APPDATA%\RizRef`), while logs and the dev camera checkpoint stay in local
//! app data where they belong. Both fall back to the temp directory when the
//! environment does not provide an app-data location, the same rule the logging
//! path uses: a stripped service context must not make the app fail to boot.

use std::path::{Path, PathBuf};

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

/// A board path in the one spelling RizRef stores and compares: absolute
/// (resolved against the current directory *now*, while the path still means
/// what the user meant), with `.` and `..` folded and `/` turned into `\`
/// (`std::path::absolute`, which on Windows is `GetFullPathNameW`: no disk
/// access, so a board on a sleeping network share costs nothing), and
/// without the `\\?\` verbatim prefix a canonicalised path carries.
///
/// The same board reached as `F:/a/b.rizref` from a command line and as
/// `F:\a\b.rizref` from Save was two recent entries; compare normalised
/// paths with [`same_path`].
pub fn normalize(path: &Path) -> PathBuf {
    let stripped = strip_verbatim(path);
    std::path::absolute(&stripped).unwrap_or(stripped)
}

/// [`normalize`] for a path held as a string.
pub fn normalize_str(path: &str) -> String {
    normalize(Path::new(path)).to_string_lossy().into_owned()
}

/// Whether two paths name the same file: normalised, then compared without
/// regard to case, as Windows file systems do.
pub fn same_path(a: &str, b: &str) -> bool {
    path_key(a) == path_key(b)
}

/// The comparison key for [`same_path`]: normalised and lowercased.
pub fn path_key(path: &str) -> String {
    normalize_str(path).to_lowercase()
}

/// `\\?\C:\x` → `C:\x` and `\\?\UNC\host\share` → `\\host\share`; any other
/// path unchanged.
fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod normalize_tests {
    use super::{normalize, normalize_str, same_path};
    use std::path::Path;

    #[test]
    fn slashes_dots_and_verbatim_prefixes_fold_to_one_spelling() {
        assert_eq!(
            normalize_str("F:/VibeCode/RizRef/.local/manual-test/board3.rizref"),
            r"F:\VibeCode\RizRef\.local\manual-test\board3.rizref"
        );
        assert_eq!(normalize_str(r"F:\a\.\b\..\c.rizref"), r"F:\a\c.rizref");
        assert_eq!(normalize_str(r"\\?\F:\a\c.rizref"), r"F:\a\c.rizref");
        assert_eq!(
            normalize_str(r"\\?\UNC\host\share\c.rizref"),
            r"\\host\share\c.rizref"
        );
    }

    #[test]
    fn a_relative_path_resolves_against_the_current_directory() {
        let cwd = std::env::current_dir().expect("cwd");
        assert_eq!(
            normalize(Path::new(r"docs\demo.rizref")),
            cwd.join("docs").join("demo.rizref")
        );
    }

    #[test]
    fn the_same_board_in_two_spellings_is_one_path() {
        assert!(same_path(
            "F:/VibeCode/RizRef/.local/manual-test/board3.rizref",
            r"f:\vibecode\RizRef\.local\MANUAL-TEST\board3.rizref"
        ));
        assert!(!same_path(r"F:\a\b.rizref", r"F:\a\c.rizref"));
    }
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
