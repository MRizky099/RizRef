//! HEIF/HEIC, decoded in Rust by [`heic_rs`].
//!
//! Apple's HEIC photographs are ISOBMFF containers holding HEVC still pictures,
//! usually as a grid of 512 × 512 tiles above half a kilopixel on a side. Until
//! recently the only ways to read them from Rust were a binding to the C
//! `libheif` (a C toolchain, LGPL library, GPL codec plugins in practice) or an
//! AGPL decoder. `heic-rs` is neither: it implements the container and the HEVC
//! still-picture decoder itself, under MIT OR Apache-2.0, with
//! `#![forbid(unsafe_code)]` and no build script.
//!
//! That is why this module can live in `rizref-codecs` at all — the crate's rule
//! is that nothing here links C — and why HEIC works on every platform the
//! application builds for rather than only where an OS codec happens to be
//! installed.
//!
//! # What is asked of the decoder
//!
//! * RGBA8 output, the crate's canonical layout.
//! * `irot`/`imir`/`clap` transforms applied, so a portrait photograph arrives
//!   upright instead of needing the caller to read the container.
//! * The caller's [`DecodeLimits`] honoured **before** allocation: `heic-rs`
//!   refuses a file whose declared size exceeds `max_pixels` rather than
//!   decoding and then complaining.
//! * One thread. The import pool already decodes one file per worker; letting
//!   the decoder spawn its own pool inside that would oversubscribe the machine,
//!   which is why the `parallel` feature is off in the workspace manifest.
//!
//! AVIF shares the container family but not the codec, and is handled elsewhere;
//! see [`ImageFormat::Avif`](crate::ImageFormat::Avif).

use heic_rs::{DecodeOptions, PixelLayout};

use crate::buffer::ImageBuffer;
use crate::decode::DecodeLimits;
use crate::error::CodecError;
use crate::sniff::ImageFormat;

/// Decodes a HEIF/HEIC file into the canonical RGBA buffer.
pub fn decode(bytes: &[u8], limits: DecodeLimits) -> Result<ImageBuffer, CodecError> {
    let options = DecodeOptions {
        layout: PixelLayout::Rgba8,
        max_pixels: Some(limits.max_pixels),
        apply_transforms: true,
        decode_alpha: true,
        // A file carrying a property this build does not implement is decoded
        // with that property ignored rather than refused: a reference board
        // would rather show the picture than reject it over a metadata box.
        strict: false,
        threads: Some(1),
    };

    let image = heic_rs::decode(bytes, &options).map_err(map_error)?;
    let width = image.width;
    let height = image.height;
    ImageBuffer::from_rgba(width, height, image.data).ok_or_else(|| CodecError::Decode {
        format: ImageFormat::Heif,
        reason: format!("decoder returned a buffer that does not match {width} x {height} RGBA"),
    })
}

/// Maps a `heic-rs` failure onto the error the rest of the crate speaks.
fn map_error(error: heic_rs::Error) -> CodecError {
    use heic_rs::Error;

    match error {
        Error::Unsupported(detail) => CodecError::Unsupported {
            format: ImageFormat::Heif,
            detail: detail.to_owned(),
        },
        // The decoder reports the pixel count rather than the two dimensions the
        // `TooLarge` variant carries, so its own message is forwarded: it names
        // both the requested and the allowed count, which is what a log wants.
        other => CodecError::Decode {
            format: ImageFormat::Heif,
            reason: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, map_error};
    use crate::decode::DecodeLimits;
    use crate::error::CodecError;
    use crate::sniff::ImageFormat;

    #[test]
    fn garbage_is_a_decode_error_not_a_panic() {
        let outcome =
            std::panic::catch_unwind(|| decode(b"not a heif file", DecodeLimits::default()));

        let error = outcome
            .expect("decode must not panic")
            .expect_err("garbage");
        assert!(
            matches!(error, CodecError::Decode { .. }),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn an_unsupported_coding_tool_keeps_its_explanation() {
        let error = map_error(heic_rs::Error::Unsupported("inter prediction"));

        match error {
            CodecError::Unsupported { format, detail } => {
                assert_eq!(format, ImageFormat::Heif);
                assert!(detail.contains("inter prediction"), "{detail}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn a_pixel_limit_is_reported_with_its_own_numbers() {
        let error = map_error(heic_rs::Error::PixelLimit {
            pixels: 400_000_000,
            max_pixels: 100_000_000,
        });

        let message = error.to_string();
        assert!(message.contains("400000000"), "{message}");
        assert!(message.contains("100000000"), "{message}");
    }

    #[test]
    fn a_truncated_file_is_a_decode_error() {
        let error = map_error(heic_rs::Error::Truncated("ftyp"));

        assert!(
            matches!(error, CodecError::Decode { .. }),
            "unexpected error: {error:?}"
        );
    }
}
