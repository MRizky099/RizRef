//! Where RizRef keeps its own small state files, per operating system.
//!
//! | | Windows | Linux (XDG) | macOS |
//! |---|---|---|---|
//! | settings, recents | `%APPDATA%\RizRef` | `$XDG_CONFIG_HOME/rizref` | `~/Library/Application Support/RizRef` |
//! | autosave, session marker, crash dumps | `%LOCALAPPDATA%\RizRef` | `$XDG_STATE_HOME/rizref` | `~/Library/Application Support/RizRef` |
//! | logs | `%LOCALAPPDATA%\RizRef\logs` | `$XDG_STATE_HOME/rizref/logs` | `~/Library/Logs/RizRef` |
//!
//! The Windows column is exactly what phases 01–07 used, so existing settings
//! and recents are found where they always were: there is nothing to migrate.
//! Every column falls back to the temp directory when the environment does not
//! provide a location, the rule the logging path always had: a stripped
//! service context must not make the app fail to boot.
//!
//! The table is the pure [`resolve`], tested for all three systems on any host;
//! the public functions apply it to this process's environment.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The operating systems [`resolve`] knows the conventions of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// `%APPDATA%` / `%LOCALAPPDATA%`.
    Windows,
    /// The XDG base directories.
    Linux,
    /// `~/Library`.
    Mac,
}

impl Os {
    /// The system this binary was built for.
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Linux
        }
    }
}

/// RizRef's directories on one system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    /// Settings and the recent-board list (roaming on Windows).
    pub config: PathBuf,
    /// Machine-local state: autosave copies, the session marker, crash dumps.
    pub state: PathBuf,
    /// Rotated log files.
    pub logs: PathBuf,
}

/// The directory table for `os`, reading variables through `env` and falling
/// back under `temp`. Pure: the tests drive it with a fake environment.
pub fn resolve(os: Os, env: impl Fn(&str) -> Option<OsString>, temp: &Path) -> AppDirs {
    // An empty variable counts as unset.
    let var = |name: &str| env(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    match os {
        Os::Windows => {
            let local = var("LOCALAPPDATA");
            let roaming = var("APPDATA")
                .or_else(|| local.clone())
                .unwrap_or_else(|| temp.join("rizref-roaming"))
                .join("RizRef");
            let local = local.unwrap_or_else(|| temp.join("rizref")).join("RizRef");
            AppDirs {
                config: roaming,
                logs: local.join("logs"),
                state: local,
            }
        }
        Os::Linux => {
            // XDG requires its paths to be absolute: a relative value is
            // ignored, never resolved against the launch directory.
            let absolute = |name: &str| var(name).filter(|p| p.has_root());
            let home = absolute("HOME");
            let base = |xdg: &str, under_home: &[&str], temp_name: &str| {
                absolute(xdg)
                    .or_else(|| {
                        home.as_ref()
                            .map(|h| h.join(under_home.iter().collect::<PathBuf>()))
                    })
                    .unwrap_or_else(|| temp.join(temp_name))
                    .join("rizref")
            };
            let state = base("XDG_STATE_HOME", &[".local", "state"], "rizref-state");
            AppDirs {
                config: base("XDG_CONFIG_HOME", &[".config"], "rizref-config"),
                logs: state.join("logs"),
                state,
            }
        }
        Os::Mac => {
            let library = var("HOME")
                .filter(|p| p.has_root())
                .map(|h| h.join("Library"))
                .unwrap_or_else(|| temp.join("rizref-library"));
            let support = library.join("Application Support").join("RizRef");
            AppDirs {
                config: support.clone(),
                state: support,
                logs: library.join("Logs").join("RizRef"),
            }
        }
    }
}

/// This process's directories.
pub fn app_dirs() -> AppDirs {
    resolve(
        Os::current(),
        |name| std::env::var_os(name),
        &std::env::temp_dir(),
    )
}

/// Settings and recents: `%APPDATA%\RizRef`, `$XDG_CONFIG_HOME/rizref` or
/// `~/Library/Application Support/RizRef`.
pub fn roaming_dir() -> PathBuf {
    app_dirs().config
}

/// Machine-local state — crash dumps, autosave copies and the session marker:
/// `%LOCALAPPDATA%\RizRef`, `$XDG_STATE_HOME/rizref` or Application Support.
pub fn local_dir() -> PathBuf {
    app_dirs().state
}

/// The log directory (see the module table).
pub fn logs_dir() -> PathBuf {
    app_dirs().logs
}

/// `settings.json` in [`roaming_dir`].
pub fn settings_file() -> PathBuf {
    roaming_dir().join("settings.json")
}

/// `recents.json` in [`roaming_dir`].
pub fn recents_file() -> PathBuf {
    roaming_dir().join("recents.json")
}

/// A board path in the one spelling RizRef stores and compares: absolute
/// (resolved against the current directory *now*, while the path still means
/// what the user meant), with `.` and `..` folded and `/` turned into `\`
/// (`std::path::absolute`, which on Windows is `GetFullPathNameW`: no disk
/// access, so a board on a sleeping network share costs nothing), and
/// without the `\\?\` verbatim prefix a canonicalised path carries. On Unix,
/// `std::path::absolute` folds `.` but keeps `..` (it cannot know about
/// symlinks without touching the disk).
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
/// regard to case on Windows and macOS (whose default file systems fold
/// case), exactly on Linux.
pub fn same_path(a: &str, b: &str) -> bool {
    path_key(a) == path_key(b)
}

/// The comparison key for [`same_path`]: normalised, and lowercased where the
/// file system folds case.
pub fn path_key(path: &str) -> String {
    let normalized = normalize_str(path);
    if cfg!(target_os = "linux") {
        normalized
    } else {
        normalized.to_lowercase()
    }
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

#[cfg(all(test, windows))]
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

#[cfg(all(test, unix))]
mod unix_normalize_tests {
    use super::{normalize_str, same_path};

    #[test]
    fn dots_fold_and_relative_paths_become_absolute() {
        assert_eq!(normalize_str("/home/a/./b.rizref"), "/home/a/b.rizref");
        let cwd = std::env::current_dir().expect("cwd");
        assert_eq!(
            normalize_str("docs/demo.rizref"),
            cwd.join("docs/demo.rizref").to_string_lossy()
        );
    }

    #[test]
    fn case_matters_only_where_the_file_system_says_so() {
        assert!(same_path("/a/b.rizref", "/a/./b.rizref"));
        assert_eq!(
            same_path("/a/B.rizref", "/a/b.rizref"),
            cfg!(target_os = "macos")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{AppDirs, Os, recents_file, resolve, settings_file};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    fn p(parts: &[&str]) -> PathBuf {
        parts.iter().collect()
    }

    const TEMP: &str = "/tmp";

    #[test]
    fn the_state_files_live_under_the_rizref_directory() {
        for path in [settings_file(), recents_file()] {
            assert!(
                path.components()
                    .any(|c| c.as_os_str().eq_ignore_ascii_case("rizref"))
            );
            assert!(path.extension().is_some_and(|ext| ext == "json"));
        }
    }

    #[test]
    fn windows_keeps_the_phase_07_locations_so_nothing_migrates() {
        let vars = [
            ("APPDATA", r"C:\Users\u\AppData\Roaming"),
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
        ];
        let dirs = resolve(Os::Windows, env(&vars), Path::new(TEMP));
        let roaming = Path::new(r"C:\Users\u\AppData\Roaming").join("RizRef");
        let local = Path::new(r"C:\Users\u\AppData\Local").join("RizRef");
        assert_eq!(
            dirs,
            AppDirs {
                config: roaming,
                logs: local.join("logs"),
                state: local,
            }
        );
    }

    #[test]
    fn windows_without_appdata_uses_local_then_temp() {
        let vars = [("LOCALAPPDATA", r"C:\L")];
        let dirs = resolve(Os::Windows, env(&vars), Path::new(TEMP));
        assert_eq!(dirs.config, Path::new(r"C:\L").join("RizRef"));
        let bare = resolve(Os::Windows, env(&[]), Path::new(TEMP));
        assert_eq!(bare.config, p(&[TEMP, "rizref-roaming", "RizRef"]));
        assert_eq!(bare.state, p(&[TEMP, "rizref", "RizRef"]));
        assert_eq!(bare.logs, p(&[TEMP, "rizref", "RizRef", "logs"]));
    }

    #[test]
    fn linux_follows_the_xdg_variables() {
        let vars = [
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_STATE_HOME", "/st"),
        ];
        let dirs = resolve(Os::Linux, env(&vars), Path::new(TEMP));
        assert_eq!(dirs.config, p(&["/cfg", "rizref"]));
        assert_eq!(dirs.state, p(&["/st", "rizref"]));
        assert_eq!(dirs.logs, p(&["/st", "rizref", "logs"]));
    }

    #[test]
    fn linux_defaults_under_home_and_ignores_relative_or_empty_xdg_paths() {
        let vars = [
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "relative/cfg"),
            ("XDG_STATE_HOME", ""),
        ];
        let dirs = resolve(Os::Linux, env(&vars), Path::new(TEMP));
        assert_eq!(dirs.config, p(&["/home/u", ".config", "rizref"]));
        assert_eq!(dirs.state, p(&["/home/u", ".local", "state", "rizref"]));
    }

    #[test]
    fn linux_without_home_still_boots_from_temp() {
        let dirs = resolve(Os::Linux, env(&[]), Path::new(TEMP));
        assert!(dirs.config.starts_with(TEMP));
        assert!(dirs.state.starts_with(TEMP));
        assert_ne!(dirs.config, dirs.state);
    }

    #[test]
    fn macos_uses_application_support_and_logs() {
        let vars = [("HOME", "/Users/u")];
        let dirs = resolve(Os::Mac, env(&vars), Path::new(TEMP));
        assert_eq!(
            dirs.config,
            p(&["/Users/u", "Library", "Application Support", "RizRef"])
        );
        assert_eq!(dirs.state, dirs.config);
        assert_eq!(dirs.logs, p(&["/Users/u", "Library", "Logs", "RizRef"]));
    }
}
