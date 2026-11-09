//! File-type registration on Linux, the freedesktop way.
//!
//! `rizref register` writes, under the XDG data directory
//! (`$XDG_DATA_HOME`, default `~/.local/share`; `/usr/local/share` with
//! `--machine`):
//!
//! | file | what it does |
//! |---|---|
//! | `applications/rizref.desktop` | the launcher entry, opening `application/x-rizref` |
//! | `mime/packages/rizref.xml` | the MIME type: `*.rizref` and the container magic |
//! | `icons/hicolor/<n>x<n>/apps/rizref.png` | the app icon, every committed size |
//! | `icons/hicolor/<n>x<n>/mimetypes/application-x-rizref.png` | the document icon |
//! | `thumbnailers/rizref.thumbnailer` | file-manager previews via `rizref thumbnail` |
//!
//! then refreshes the caches (`update-mime-database`, `update-desktop-database`,
//! `gtk-update-icon-cache`) when those tools are installed; a missing tool is
//! reported, never fatal. `unregister` deletes exactly those files and
//! refreshes again. The file set is the pure [`plan`], pinned by the tests.
//!
//! The icons are the PNG images inside the same `.ico` assets the Windows
//! build embeds, so the platforms cannot drift apart.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use crate::platform::register::MIME_TYPE;

/// The application icon, the same asset `build.rs` embeds on Windows.
const APP_ICO: &[u8] = include_bytes!("../../../assets/rizref.ico");
/// The `.rizref` document icon.
const DOCUMENT_ICO: &[u8] = include_bytes!("../../../assets/rizref_document.ico");

/// The hicolor name of the document icon: the MIME type with `/` → `-`.
const MIME_ICON: &str = "application-x-rizref";

/// Where the files go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `$XDG_DATA_HOME`: this user, no root.
    User,
    /// `/usr/local/share`: every user; needs root.
    Machine,
}

/// One file `register` writes, relative to the data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallFile {
    /// Path below the data directory.
    pub path: PathBuf,
    /// The bytes written there.
    pub contents: Vec<u8>,
}

/// Every file `register` writes for an executable at `exe`.
///
/// `None` when the executable path cannot be written into a desktop entry
/// (not UTF-8, or a control character).
pub fn plan(exe: &Path) -> Option<Vec<InstallFile>> {
    let exe_text = exe.to_str()?;
    if exe_text.chars().any(char::is_control) {
        return None;
    }
    let exec = exec_quote(exe_text);
    let mut files = vec![
        InstallFile {
            path: PathBuf::from("applications/rizref.desktop"),
            contents: desktop_entry(&exec).into_bytes(),
        },
        InstallFile {
            path: PathBuf::from("mime/packages/rizref.xml"),
            contents: mime_package().into_bytes(),
        },
        InstallFile {
            path: PathBuf::from("thumbnailers/rizref.thumbnailer"),
            contents: thumbnailer(exe_text, &exec).into_bytes(),
        },
    ];
    for (ico, kind, name) in [
        (APP_ICO, "apps", "rizref"),
        (DOCUMENT_ICO, "mimetypes", MIME_ICON),
    ] {
        for (side, png) in rizref_thumb::ico_pngs(ico) {
            files.push(InstallFile {
                path: PathBuf::from(format!("icons/hicolor/{side}x{side}/{kind}/{name}.png")),
                contents: png.to_vec(),
            });
        }
    }
    Some(files)
}

/// The launcher entry. `%F`: RizRef opens any number of boards and images.
fn desktop_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.5\n\
         Name=RizRef\n\
         GenericName=Reference board\n\
         Comment=Collect and arrange reference images on an infinite board\n\
         Exec={exec} %F\n\
         Icon=rizref\n\
         Terminal=false\n\
         Categories=Graphics;Viewer;2DGraphics;RasterGraphics;\n\
         MimeType={MIME_TYPE};\n\
         StartupWMClass=rizref\n\
         Keywords=reference;moodboard;images;board;\n"
    )
}

/// The shared-mime-info package: the glob plus the container's magic bytes
/// (`rizref_format::header::MAGIC`), so a renamed board is still a board.
fn mime_package() -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\">\n\
         \x20 <mime-type type=\"{MIME_TYPE}\">\n\
         \x20   <comment>RizRef board</comment>\n\
         \x20   <icon name=\"{MIME_ICON}\"/>\n\
         \x20   <glob pattern=\"*.rizref\"/>\n\
         \x20   <magic priority=\"60\">\n\
         \x20     <match type=\"string\" offset=\"0\" value=\"RIZREF\\x01\"/>\n\
         \x20   </magic>\n\
         \x20 </mime-type>\n\
         </mime-info>\n"
    )
}

/// The thumbnailer entry file managers (GNOME, and others reading the same
/// directory) run with the requested size, input path and output PNG path.
fn thumbnailer(exe: &str, exec: &str) -> String {
    format!(
        "[Thumbnailer Entry]\n\
         TryExec={exe}\n\
         Exec={exec} thumbnail -s %s %i %o\n\
         MimeType={MIME_TYPE};\n"
    )
}

/// `exe` as one argument of a desktop entry's `Exec` key.
///
/// The Desktop Entry spec quotes with `"…"`, inside which `"`, `` ` ``, `$`
/// and `\` take a backslash; the string-value escape then doubles every
/// backslash, and a literal `%` is `%%` (field codes are `%x`). Always
/// quoting is valid and keeps one rule for every path.
fn exec_quote(exe: &str) -> String {
    let mut quoted = String::with_capacity(exe.len() + 2);
    quoted.push('"');
    for c in exe.chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// The data directory for `scope`.
fn data_dir(scope: Scope) -> Option<PathBuf> {
    match scope {
        Scope::Machine => Some(PathBuf::from("/usr/local/share")),
        Scope::User => std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .map(|home| home.join(".local").join("share"))
            }),
    }
}

/// Writes every planned file under `root`.
pub(crate) fn apply(root: &Path, files: &[InstallFile]) -> Result<(), String> {
    for file in files {
        let target = root.join(&file.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        // Write beside, then rename: a crash mid-write never leaves a
        // truncated desktop entry for the menu to choke on.
        let partial = target.with_extension("rizref-partial");
        std::fs::File::create(&partial)
            .and_then(|mut out| out.write_all(&file.contents))
            .and_then(|()| std::fs::rename(&partial, &target))
            .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    }
    Ok(())
}

/// Deletes every planned file under `root`; a file already gone is fine.
pub(crate) fn remove(root: &Path, files: &[InstallFile]) -> Result<(), String> {
    for file in files {
        let target = root.join(&file.path);
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot remove {}: {e}", target.display())),
        }
    }
    Ok(())
}

/// Refreshes the desktop caches that index the files just changed.
fn refresh_caches(root: &Path) {
    let runs: [(&str, Vec<PathBuf>); 3] = [
        ("update-mime-database", vec![root.join("mime")]),
        ("update-desktop-database", vec![root.join("applications")]),
        (
            "gtk-update-icon-cache",
            vec![
                PathBuf::from("-f"),
                PathBuf::from("-t"),
                root.join("icons/hicolor"),
            ],
        ),
    ];
    for (tool, args) in runs {
        let status = Command::new(tool)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => println!("note: {tool} exited with {status}"),
            Err(_) => {
                println!("note: {tool} is not installed; the desktop may pick changes up later")
            }
        }
    }
}

/// `rizref register [--machine]` / `rizref unregister [--machine]`.
/// Exit code 0 on success, 1 on a file-system failure, 2 on a bad argument.
pub fn run(register: bool, args: &[String]) -> ExitCode {
    let scope = match args {
        [] => Scope::User,
        [flag] if flag == "--machine" => Scope::Machine,
        _ => {
            eprintln!(
                "usage: rizref {} [--machine]",
                if register { "register" } else { "unregister" }
            );
            return ExitCode::from(2);
        }
    };
    let Some(root) = data_dir(scope) else {
        eprintln!("rizref: neither $XDG_DATA_HOME nor $HOME is set to an absolute path");
        return ExitCode::FAILURE;
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("rizref: cannot locate the rizref executable: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(files) = plan(&exe) else {
        eprintln!(
            "rizref: {} cannot be written into a desktop entry",
            exe.display()
        );
        return ExitCode::FAILURE;
    };
    let result = if register {
        apply(&root, &files)
    } else {
        remove(&root, &files)
    };
    match result {
        Ok(()) => {
            refresh_caches(&root);
            if register {
                println!(
                    "registered .rizref ({MIME_TYPE}) for {} in {} ({scope:?})",
                    exe.display(),
                    root.display()
                );
            } else {
                println!("unregistered .rizref from {} ({scope:?})", root.display());
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("rizref: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{apply, exec_quote, plan, remove};
    use std::path::Path;

    fn text(files: &[super::InstallFile], path: &str) -> String {
        let file = files
            .iter()
            .find(|f| f.path == Path::new(path))
            .unwrap_or_else(|| panic!("{path} is planned"));
        String::from_utf8(file.contents.clone()).expect("utf-8")
    }

    #[test]
    fn the_desktop_entry_opens_boards_with_this_executable() {
        let files = plan(Path::new("/opt/rizref/rizref")).expect("plan");
        let desktop = text(&files, "applications/rizref.desktop");
        assert!(desktop.starts_with("[Desktop Entry]\n"));
        assert!(desktop.contains("\nExec=\"/opt/rizref/rizref\" %F\n"));
        assert!(desktop.contains("\nMimeType=application/x-rizref;\n"));
        assert!(desktop.contains("\nIcon=rizref\n"));
    }

    #[test]
    fn the_mime_type_matches_the_extension_and_the_container_magic() {
        let files = plan(Path::new("/opt/rizref/rizref")).expect("plan");
        let mime = text(&files, "mime/packages/rizref.xml");
        assert!(mime.contains("<mime-type type=\"application/x-rizref\">"));
        assert!(mime.contains("<glob pattern=\"*.rizref\"/>"));
        assert!(mime.contains("value=\"RIZREF\\x01\""));
        assert_eq!(&rizref_format::header::MAGIC[..7], b"RIZREF\x01");
    }

    #[test]
    fn the_thumbnailer_calls_the_thumbnail_subcommand() {
        let files = plan(Path::new("/opt/rizref/rizref")).expect("plan");
        let entry = text(&files, "thumbnailers/rizref.thumbnailer");
        assert!(entry.contains("\nTryExec=/opt/rizref/rizref\n"));
        assert!(entry.contains("\nExec=\"/opt/rizref/rizref\" thumbnail -s %s %i %o\n"));
    }

    #[test]
    fn every_committed_icon_size_is_installed_for_app_and_document() {
        let files = plan(Path::new("/opt/rizref/rizref")).expect("plan");
        for size in [16, 32, 48, 256] {
            for name in [
                format!("icons/hicolor/{size}x{size}/apps/rizref.png"),
                format!("icons/hicolor/{size}x{size}/mimetypes/application-x-rizref.png"),
            ] {
                let png = files
                    .iter()
                    .find(|f| f.path == Path::new(&name))
                    .unwrap_or_else(|| panic!("{name}"));
                assert_eq!(&png.contents[..8], b"\x89PNG\r\n\x1a\n", "{name}");
            }
        }
    }

    #[test]
    fn exec_paths_are_quoted_and_escaped_per_the_spec() {
        assert_eq!(exec_quote("/opt/My Apps/rizref"), "\"/opt/My Apps/rizref\"");
        assert_eq!(exec_quote("/a/100%/b"), "\"/a/100%%/b\"");
        assert_eq!(exec_quote("/a/$x\"`"), "\"/a/\\\\$x\\\\\"\\\\`\"");
        assert_eq!(exec_quote("/a\\b"), "\"/a\\\\\\\\b\"");
        assert!(plan(Path::new("/a\nb/rizref")).is_none());
    }

    #[test]
    fn unregister_removes_exactly_what_register_wrote() {
        let root = std::env::temp_dir().join(format!("rizref-xdg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("applications")).expect("dir");
        let bystander = root.join("applications/other.desktop");
        std::fs::write(&bystander, b"[Desktop Entry]\n").expect("bystander");

        let files = plan(Path::new("/opt/rizref/rizref")).expect("plan");
        apply(&root, &files).expect("apply");
        for file in &files {
            assert!(root.join(&file.path).is_file(), "{}", file.path.display());
        }
        remove(&root, &files).expect("remove");
        for file in &files {
            assert!(!root.join(&file.path).exists(), "{}", file.path.display());
        }
        assert!(bystander.is_file(), "other applications' files stay");
        remove(&root, &files).expect("removing twice is fine");
        std::fs::remove_dir_all(&root).ok();
    }
}
