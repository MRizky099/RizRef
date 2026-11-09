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
        /// The image's web address, best effort from the HTML a browser
        /// puts next to the bitmap (`<img src>`, else the page `SourceURL`).
        source_url: Option<String>,
    },
}

/// Why a paste produced nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteError {
    /// The clipboard holds HTML (a link or page selection) but no pixels.
    /// Nothing is downloaded: network access on paste is a privacy decision.
    LinkOnly,
    /// Anything else: no image, a busy clipboard, an encoding failure.
    Other(String),
}

impl std::fmt::Display for PasteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LinkOnly => formatter.write_str("the clipboard holds a link, not an image"),
            Self::Other(reason) => formatter.write_str(reason),
        }
    }
}

/// Reads image files or a bitmap from the clipboard.
///
/// A file list wins over a bitmap when both are present: copying files in
/// Explorer also puts a thumbnail on the clipboard on some systems, and the
/// files are the lossless original the user meant. A bitmap carries the
/// source URL from the HTML clipboard when there is one; HTML without a
/// bitmap is [`PasteError::LinkOnly`].
pub fn read() -> Result<ClipboardContent, PasteError> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| PasteError::Other(format!("cannot open the clipboard: {error}")))?;

    match clipboard.get().file_list() {
        Ok(files) if !files.is_empty() => return Ok(ClipboardContent::Files(files)),
        Ok(_) => {}
        Err(error) => tracing::debug!(%error, "clipboard has no file list"),
    }

    let image = match clipboard.get_image() {
        Ok(image) => image,
        Err(error) => {
            return Err(classify_missing_image(
                crate::platform::clipboard_html::has_html(),
                &error.to_string(),
            ));
        }
    };
    let width = u32::try_from(image.width).map_err(|_| {
        PasteError::Other(format!(
            "clipboard image width {} is out of range",
            image.width
        ))
    })?;
    let height = u32::try_from(image.height).map_err(|_| {
        PasteError::Other(format!(
            "clipboard image height {} is out of range",
            image.height
        ))
    })?;
    let png = png_from_rgba(width, height, image.bytes.into_owned()).map_err(PasteError::Other)?;
    // Release arboard's handle before the raw HTML read opens the clipboard.
    drop(clipboard);
    let source_url = crate::platform::clipboard_html::source_url();

    Ok(ClipboardContent::Image {
        name: "clipboard.png".to_owned(),
        png,
        source_url,
    })
}

/// The error for a clipboard without a bitmap.
fn classify_missing_image(has_html: bool, reason: &str) -> PasteError {
    if has_html {
        PasteError::LinkOnly
    } else {
        PasteError::Other(format!("the clipboard holds no image: {reason}"))
    }
}

/// Reads plain text from the clipboard, for pasting into a note.
pub fn read_text() -> Result<String, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| format!("cannot open the clipboard: {error}"))?;
    clipboard
        .get_text()
        .map_err(|error| format!("the clipboard holds no text: {error}"))
}

/// Writes plain text to the clipboard (the colour picker's hex readout).
pub fn write_text(text: &str) -> Result<(), String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| format!("cannot open the clipboard: {error}"))?;
    clipboard
        .set_text(text.to_owned())
        .map_err(|error| format!("cannot write the clipboard: {error}"))
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
    fn html_without_pixels_is_a_link_not_an_image() {
        use super::{PasteError, classify_missing_image};

        assert_eq!(
            classify_missing_image(true, "no image"),
            PasteError::LinkOnly
        );
        assert_eq!(
            PasteError::LinkOnly.to_string(),
            "the clipboard holds a link, not an image"
        );
        assert!(matches!(
            classify_missing_image(false, "empty"),
            PasteError::Other(reason) if reason.contains("no image")
        ));
    }

    #[test]
    fn mismatched_clipboard_data_is_reported() {
        let error = png_from_rgba(2, 2, vec![0; 4]).expect_err("too short");

        assert!(error.contains("does not match"), "unexpected: {error}");
    }
}
