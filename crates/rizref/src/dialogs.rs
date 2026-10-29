//! Native file dialogs.
//!
//! Board files are opened, saved and exported through the platform's own
//! dialogs rather than a re-implementation: users know where their recent
//! folders are, and the shell already offers network locations, libraries and
//! the file-type filter they expect.
//!
//! The filter list below is the same set `rizref-codecs` can decode, plus the
//! two ISOBMFF families the application adds on top: HEIC/HEIF (decoded by the
//! crate itself) and AVIF (decoded through the OS codec, see
//! `platform::imaging`). Formats that are recognised but never decodable are
//! left out so the dialog cannot offer a file the loader will refuse.
//!
//! Ctrl+O opens the image dialog; the board open, save and PNG-export dialogs
//! were added with the `.rizref` container.

use std::path::PathBuf;

/// Extensions the open dialog offers, without dots.
pub const IMAGE_EXTENSIONS: [&str; 13] = [
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tga", "pnm", "qoi", "ico", "heic", "heif", "avif",
];

/// Opens the native "open images" dialog.
///
/// Returns the chosen paths, or `None` when the user cancels — cancellation is
/// not an error and must not be reported as one.
pub fn pick_images() -> Option<Vec<PathBuf>> {
    rfd::FileDialog::new()
        .set_title("Open images")
        .add_filter("Images", &IMAGE_EXTENSIONS)
        .pick_files()
}

/// Opens the native "open board" dialog.
pub fn pick_board() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Open board")
        .add_filter("RizRef board", &["rizref"])
        .pick_file()
}

/// Opens the native "save board" dialog.
///
/// The returned path always carries the `.rizref` extension, even when the user
/// typed a bare name: a board whose extension is missing would not be offered
/// back by the open dialog or the Explorer thumbnail provider.
pub fn save_board(default_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Save board")
        .add_filter("RizRef board", &["rizref"])
        .set_file_name(default_name)
        .save_file()
        .map(|path| with_extension(path, "rizref"))
}

/// Opens the native "export board as PNG" dialog.
pub fn export_png(default_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Export board as PNG")
        .add_filter("PNG image", &["png"])
        .set_file_name(default_name)
        .save_file()
        .map(|path| with_extension(path, "png"))
}

/// Returns `path` with the requested extension.
///
/// A matching extension is kept as typed; any other name gets the right one, so
/// a board cannot be saved as `board.txt` and then be invisible to the open
/// dialog and the Explorer thumbnail provider.
fn with_extension(path: PathBuf, extension: &str) -> PathBuf {
    if path
        .extension()
        .and_then(|current| current.to_str())
        .is_some_and(|current| current.eq_ignore_ascii_case(extension))
    {
        return path;
    }
    path.with_extension(extension)
}

#[cfg(test)]
mod tests {
    use super::IMAGE_EXTENSIONS;
    use rizref_codecs::ImageFormat;

    #[test]
    fn every_offered_extension_is_decodable() {
        for extension in IMAGE_EXTENSIONS {
            let format = ImageFormat::from_extension(extension)
                .unwrap_or_else(|| panic!("{extension:?} names no known format"));

            // HEIF is decoded by the codec crate; AVIF is decoded by the
            // application through the OS codec, so both are offered here and
            // reported as unsupported only when the codec is missing.
            assert!(
                format.is_decodable() || matches!(format, ImageFormat::Heif | ImageFormat::Avif),
                "{extension:?} is offered by the dialog but cannot be loaded"
            );
        }
    }

    #[test]
    fn no_extension_is_offered_twice() {
        for (index, extension) in IMAGE_EXTENSIONS.iter().enumerate() {
            for other in &IMAGE_EXTENSIONS[index + 1..] {
                assert_ne!(extension, other, "duplicate extension in the filter list");
            }
        }
    }

    #[test]
    fn a_saved_board_always_carries_its_extension() {
        use super::with_extension;
        use std::path::PathBuf;

        assert_eq!(
            with_extension(PathBuf::from("C:/boards/hands"), "rizref"),
            PathBuf::from("C:/boards/hands.rizref")
        );
        assert_eq!(
            with_extension(PathBuf::from("C:/boards/hands.rizref"), "rizref"),
            PathBuf::from("C:/boards/hands.rizref")
        );
        assert_eq!(
            with_extension(PathBuf::from("C:/boards/hands.wrong"), "rizref"),
            PathBuf::from("C:/boards/hands.rizref"),
            "the board extension wins over a foreign one"
        );
        assert_eq!(
            with_extension(PathBuf::from("C:/boards/hands.RIZREF"), "rizref"),
            PathBuf::from("C:/boards/hands.RIZREF"),
            "the extension match is case-insensitive"
        );
    }
}
