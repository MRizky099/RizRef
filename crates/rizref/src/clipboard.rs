//! The system clipboard, for pasting reference images.
//!
//! Two shapes of image data reach a board from the clipboard: a list of files
//! (copying a file in Explorer, or several at once) and a raw bitmap (copying
//! an image out of a browser or an image editor). Files are handed to the
//! importer as paths; a raw bitmap is encoded to PNG first, because the rest of
//! the pipeline — and every asset's provenance — speaks encoded image files,
//! never bare RGBA.
//!
//! Everything here runs on the UI thread: `arboard` warns that concurrent
//! clipboard opens on Windows can fail or deadlock, so the worker pool is
//! never given a clipboard handle.

use std::path::PathBuf;

use rizref_codecs::{ImageBuffer, encode_png};

/// What one paste produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardContent {
    /// Image files copied as shell objects.
    Files(Vec<PathBuf>),
    /// A bitmap copied from an application, already encoded as PNG.
    Image {
        /// A stable pseudo-filename, used in logs and error messages.
        name: String,
        /// The PNG bytes.
        png: Vec<u8>,
    },
}

/// Reads image files or a bitmap from the clipboard.
///
/// A file list wins over a bitmap when both are present: copying files in
/// Explorer also puts a thumbnail on the clipboard on some systems, and the
/// files are the lossless original the user meant.
pub fn read() -> Result<ClipboardContent, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| format!("cannot open the clipboard: {error}"))?;

    match clipboard.get().file_list() {
        Ok(files) if !files.is_empty() => return Ok(ClipboardContent::Files(files)),
        Ok(_) => {}
        Err(error) => tracing::debug!(%error, "clipboard has no file list"),
    }

    let image = clipboard
        .get_image()
        .map_err(|error| format!("the clipboard holds no image: {error}"))?;
    let width = u32::try_from(image.width)
        .map_err(|_| format!("clipboard image width {} is out of range", image.width))?;
    let height = u32::try_from(image.height)
        .map_err(|_| format!("clipboard image height {} is out of range", image.height))?;
    let png = png_from_rgba(width, height, image.bytes.into_owned())?;

    Ok(ClipboardContent::Image {
        name: "clipboard.png".to_owned(),
        png,
    })
}

/// Encodes raw clipboard RGBA as PNG.
fn png_from_rgba(width: u32, height: u32, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    let image = ImageBuffer::from_rgba(width, height, bytes)
        .ok_or_else(|| format!("clipboard image data does not match {width} x {height} RGBA"))?;
    encode_png(&image).map_err(|error| format!("cannot encode the clipboard image: {error}"))
}

/// Opens the system clipboard and immediately releases it.
///
/// Kept for the startup summary: a session without a desktop can fail to open
/// the clipboard, and saying so at launch is friendlier than a silent Ctrl+V.
pub fn availability() -> Result<(), String> {
    match arboard::Clipboard::new() {
        Ok(_) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::png_from_rgba;
    use rizref_codecs::decode;

    #[test]
    fn clipboard_rgba_becomes_a_decodable_png() {
        let rgba = vec![
            255, 0, 0, 255, //
            0, 255, 0, 128,
        ];

        let png = png_from_rgba(2, 1, rgba).expect("encode");
        let decoded = decode(&png).expect("decode");

        assert_eq!((decoded.width(), decoded.height()), (2, 1));
        assert_eq!(decoded.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(decoded.pixel(1, 0), Some([0, 255, 0, 128]));
    }

    #[test]
    fn mismatched_clipboard_data_is_reported() {
        let error = png_from_rgba(2, 2, vec![0; 4]).expect_err("too short");

        assert!(error.contains("does not match"), "unexpected: {error}");
    }
}
