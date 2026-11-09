//! Where "Open location" goes for one image, decided without launching.
//!
//! The rule from the 06b plan: a launchable `source_url` (http/https) wins;
//! otherwise a `source_path`; otherwise there is no location. A relative
//! path resolves against the board file's folder and must then be absolute.
//! The file system is consulted through a probe closure, so the whole
//! decision table is unit-tested; `platform::launch` performs the result.
//!
//! Opening never touches the board: no mutation, no dirty flag, no undo.

use std::path::{Path, PathBuf};

use rizref_format::AssetProvenance;

use crate::provenance;

/// What kind of location an image has, for menu labels and enablement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationKind {
    /// A local file path.
    Local,
    /// An `http`/`https` URL.
    Web,
}

impl LocationKind {
    /// The menu label for opening this kind of location.
    pub fn open_label(self) -> &'static str {
        match self {
            Self::Local => "Open file location",
            Self::Web => "Open image URL",
        }
    }
}

/// The launchable location kind of a provenance record, if any.
///
/// A recorded URL that is not http/https (a hand-edited `javascript:` or
/// `file:` value) does not count; the path, if any, is used instead.
pub fn location_kind(provenance: &AssetProvenance) -> Option<LocationKind> {
    if provenance
        .source_url
        .as_deref()
        .is_some_and(provenance::is_launchable)
    {
        Some(LocationKind::Web)
    } else if provenance
        .source_path
        .as_deref()
        .is_some_and(|path| !path.trim().is_empty())
    {
        Some(LocationKind::Local)
    } else {
        None
    }
}

/// What the file system says about a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// A file (or anything that is not a directory) exists there.
    File,
    /// A directory exists there.
    Directory,
    /// Nothing is there.
    Missing,
}

/// Probes the real file system.
pub fn probe(path: &Path) -> Probe {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => Probe::Directory,
        Ok(_) => Probe::File,
        Err(_) => Probe::Missing,
    }
}

/// The decided action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocationPlan {
    /// Open this validated URL in the default browser.
    OpenUrl(String),
    /// Open Explorer with this file selected.
    SelectFile(PathBuf),
    /// The file has moved; open its still-existing parent folder.
    OpenParent(PathBuf),
    /// Neither the file nor its folder exists any more.
    Gone,
    /// There is no launchable location.
    Nothing,
}

impl LocationPlan {
    /// The toast shown after carrying the plan out successfully, or the
    /// explanation when there is nothing to carry out.
    pub fn message(&self) -> &'static str {
        match self {
            Self::OpenUrl(_) => "Opened the image URL",
            Self::SelectFile(_) => "Opened in Explorer",
            Self::OpenParent(_) => "The image file has moved; opened its folder",
            Self::Gone => "The image file is no longer at that location",
            Self::Nothing => "Pasted or dropped without a source location",
        }
    }
}

/// Resolves a recorded path: absolute as is, relative against the board's
/// folder; `None` when it is still not absolute.
pub fn resolve_path(recorded: &str, board_dir: Option<&Path>) -> Option<PathBuf> {
    let recorded = Path::new(recorded.trim());
    if recorded.as_os_str().is_empty() {
        return None;
    }
    let resolved = if recorded.is_absolute() {
        recorded.to_path_buf()
    } else {
        board_dir?.join(recorded)
    };
    resolved.is_absolute().then_some(resolved)
}

/// Decides what "Open location" does for `provenance`.
pub fn plan(
    provenance: &AssetProvenance,
    board_dir: Option<&Path>,
    probe: impl Fn(&Path) -> Probe,
) -> LocationPlan {
    match location_kind(provenance) {
        Some(LocationKind::Web) => provenance
            .source_url
            .as_deref()
            .and_then(provenance::normalize_url)
            .map_or(LocationPlan::Nothing, LocationPlan::OpenUrl),
        Some(LocationKind::Local) => {
            let Some(path) = provenance
                .source_path
                .as_deref()
                .and_then(|recorded| resolve_path(recorded, board_dir))
            else {
                return LocationPlan::Gone;
            };
            match probe(&path) {
                Probe::File => LocationPlan::SelectFile(path),
                // A directory recorded as an image path: show the folder.
                Probe::Directory => LocationPlan::OpenParent(path),
                Probe::Missing => match path.parent() {
                    Some(parent)
                        if !parent.as_os_str().is_empty() && probe(parent) == Probe::Directory =>
                    {
                        LocationPlan::OpenParent(parent.to_path_buf())
                    }
                    _ => LocationPlan::Gone,
                },
            }
        }
        None => LocationPlan::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rizref_format::AssetOrigin;

    fn record(path: Option<&str>, url: Option<&str>) -> AssetProvenance {
        AssetProvenance {
            origin: AssetOrigin::File,
            source_path: path.map(str::to_owned),
            source_url: url.map(str::to_owned),
        }
    }

    /// A fake file system: `files` exist as files, `dirs` as directories.
    fn fs<'a>(files: &'a [&'a str], dirs: &'a [&'a str]) -> impl Fn(&Path) -> Probe + 'a {
        move |path| {
            let text = path.to_string_lossy();
            if files.iter().any(|file| *file == text) {
                Probe::File
            } else if dirs.iter().any(|dir| *dir == text) {
                Probe::Directory
            } else {
                Probe::Missing
            }
        }
    }

    #[test]
    fn a_web_url_wins_over_a_path() {
        let both = record(Some(r"D:\refs\a.png"), Some("https://e.com/a.png"));
        assert_eq!(location_kind(&both), Some(LocationKind::Web));
        assert_eq!(
            plan(&both, None, fs(&[r"D:\refs\a.png"], &[])),
            LocationPlan::OpenUrl("https://e.com/a.png".to_owned())
        );
        assert_eq!(LocationKind::Web.open_label(), "Open image URL");
        assert_eq!(LocationKind::Local.open_label(), "Open file location");
    }

    // Windows paths; the Unix spelling is `unix_paths_plan_the_same_way`.
    #[cfg(windows)]
    #[test]
    fn a_non_web_url_is_never_a_location() {
        for hostile in [
            "javascript:alert(1)",
            "file:///C:/Windows/System32/calc.exe",
            "data:image/png;base64,AAAA",
        ] {
            let only_url = record(None, Some(hostile));
            assert_eq!(location_kind(&only_url), None, "{hostile}");
            assert_eq!(plan(&only_url, None, fs(&[], &[])), LocationPlan::Nothing);

            let with_path = record(Some(r"D:\refs\a.png"), Some(hostile));
            assert_eq!(location_kind(&with_path), Some(LocationKind::Local));
            assert_eq!(
                plan(&with_path, None, fs(&[r"D:\refs\a.png"], &[])),
                LocationPlan::SelectFile(PathBuf::from(r"D:\refs\a.png"))
            );
        }
    }

    // Windows paths; the Unix spelling is `unix_paths_plan_the_same_way`.
    #[cfg(windows)]
    #[test]
    fn an_existing_file_is_selected() {
        let local = record(Some(r"D:\My Refs\a, b & c.png"), None);
        assert_eq!(
            plan(&local, None, fs(&[r"D:\My Refs\a, b & c.png"], &[])),
            LocationPlan::SelectFile(PathBuf::from(r"D:\My Refs\a, b & c.png"))
        );
    }

    // Windows paths; the Unix spelling is `unix_paths_plan_the_same_way`.
    #[cfg(windows)]
    #[test]
    fn a_moved_file_opens_its_folder_and_a_vanished_one_reports_it() {
        let local = record(Some(r"D:\refs\gone.png"), None);
        assert_eq!(
            plan(&local, None, fs(&[], &[r"D:\refs"])),
            LocationPlan::OpenParent(PathBuf::from(r"D:\refs"))
        );
        assert_eq!(plan(&local, None, fs(&[], &[])), LocationPlan::Gone);
    }

    // Windows paths; the Unix spelling is `unix_paths_plan_the_same_way`.
    #[cfg(windows)]
    #[test]
    fn relative_paths_resolve_against_the_board_folder() {
        let relative = record(Some(r"refs\hero.png"), None);
        assert_eq!(
            plan(
                &relative,
                Some(Path::new(r"D:\boards")),
                fs(&[r"D:\boards\refs\hero.png"], &[])
            ),
            LocationPlan::SelectFile(PathBuf::from(r"D:\boards\refs\hero.png"))
        );
        // Without a saved board the path cannot become absolute.
        assert_eq!(plan(&relative, None, fs(&[], &[])), LocationPlan::Gone);
        assert_eq!(resolve_path(r"refs\a.png", None), None);
        assert_eq!(resolve_path("  ", Some(Path::new(r"D:\b"))), None);
        assert_eq!(
            resolve_path(r"C:\abs.png", Some(Path::new(r"D:\b"))),
            Some(PathBuf::from(r"C:\abs.png"))
        );
    }

    #[test]
    fn no_provenance_means_no_location() {
        let none = AssetProvenance::default();
        assert_eq!(location_kind(&none), None);
        assert_eq!(plan(&none, None, fs(&[], &[])), LocationPlan::Nothing);
        let blank = record(Some("  "), None);
        assert_eq!(location_kind(&blank), None);
        assert_eq!(
            LocationPlan::Nothing.message(),
            "Pasted or dropped without a source location"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_paths_plan_the_same_way() {
        let local = record(Some("/home/u/My Refs/a, b & c.png"), None);
        assert_eq!(
            plan(&local, None, fs(&["/home/u/My Refs/a, b & c.png"], &[])),
            LocationPlan::SelectFile(PathBuf::from("/home/u/My Refs/a, b & c.png"))
        );
        assert_eq!(
            plan(&local, None, fs(&[], &["/home/u/My Refs"])),
            LocationPlan::OpenParent(PathBuf::from("/home/u/My Refs"))
        );
        let relative = record(Some("refs/hero.png"), None);
        assert_eq!(
            plan(
                &relative,
                Some(Path::new("/home/u/boards")),
                fs(&["/home/u/boards/refs/hero.png"], &[])
            ),
            LocationPlan::SelectFile(PathBuf::from("/home/u/boards/refs/hero.png"))
        );
        assert_eq!(plan(&relative, None, fs(&[], &[])), LocationPlan::Gone);
    }

    #[cfg(unix)]
    #[test]
    fn a_windows_path_from_another_machine_is_gone_not_launched() {
        let foreign = record(Some(r"D:\refs\a.png"), None);
        assert_eq!(location_kind(&foreign), Some(LocationKind::Local));
        assert_eq!(plan(&foreign, None, fs(&[], &[])), LocationPlan::Gone);
    }
}
