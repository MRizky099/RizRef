//! Native file dialogs.
//!
//! Board files are opened, saved and exported through the platform's own
//! dialogs rather than a re-implementation: users know where their recent
//! folders are, and the shell already offers network locations, libraries and
//! the file-type filter they expect.
//!
//! The filter list below is the same set `rizref-codecs` can decode, with the
//! formats that are recognised but not decodable (HEIC) deliberately left out so
//! the dialog cannot offer a file the loader will refuse.
//!
//! Phase 03 wires the image dialog to Ctrl+O; board open/save and export
//! dialogs arrive with the UI in phase 05.

use std::path::PathBuf;

/// Extensions the open dialog offers, without dots.
pub const IMAGE_EXTENSIONS: [&str; 10] = [
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tga", "pnm", "qoi", "ico",
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

#[cfg(test)]
mod tests {
    use super::IMAGE_EXTENSIONS;
    use rizref_codecs::ImageFormat;

    #[test]
    fn every_offered_extension_is_decodable() {
        for extension in IMAGE_EXTENSIONS {
            let format = ImageFormat::from_extension(extension)
                .unwrap_or_else(|| panic!("{extension:?} names no known format"));

            assert!(
                format.is_decodable(),
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
}
