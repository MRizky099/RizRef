//! Encoding, for the two cases a board needs.
//!
//! Reference boards are re-encoded in exactly two places: the preview thumbnail
//! that travels inside a `.rizref` file, and an explicit "export as" from the
//! item menu. Both want a small, fast, broadly readable file, which is why only
//! PNG (lossless) and JPEG (small) are wired up — the two formats the files are
//! already dominated by.

use crate::buffer::ImageBuffer;
use crate::error::CodecError;
use crate::sniff::ImageFormat;

/// Encodes `image` as PNG, preserving alpha.
///
/// A zero-sized image is refused by the encoder: PNG requires at least one pixel
/// in each direction, and the caller — not this crate — decides what an empty
/// board should look like.
pub fn encode_png(image: &ImageBuffer) -> Result<Vec<u8>, CodecError> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width(), image.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| CodecError::Encode {
            format: ImageFormat::Png,
            reason: error.to_string(),
        })?;
        writer
            .write_image_data(image.pixels())
            .map_err(|error| CodecError::Encode {
                format: ImageFormat::Png,
                reason: error.to_string(),
            })?;
    }
    Ok(bytes)
}

/// Encodes `image` as JPEG at `quality` (1–100), discarding alpha.
///
/// The quality is clamped rather than rejected: it comes from a settings file
/// that a user may edit, and a board that saves must not fail over a cosmetic
/// preference. Alpha is dropped because the format has none — the preview is
/// drawn over the board's background colour, which is next to a checkerboard
/// what the user sees anyway.
pub fn encode_jpeg(image: &ImageBuffer, quality: u8) -> Result<Vec<u8>, CodecError> {
    let unsupported = || CodecError::TooLarge {
        width: image.width(),
        height: image.height(),
    };
    let width = u16::try_from(image.width()).map_err(|_| unsupported())?;
    let height = u16::try_from(image.height()).map_err(|_| unsupported())?;

    let mut bytes = Vec::new();
    jpeg_encoder::Encoder::new(&mut bytes, quality.clamp(1, 100))
        .encode(image.pixels(), width, height, jpeg_encoder::ColorType::Rgba)
        .map_err(|error| CodecError::Encode {
            format: ImageFormat::Jpeg,
            reason: error.to_string(),
        })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{encode_jpeg, encode_png};
    use crate::buffer::ImageBuffer;
    use crate::decode::decode;
    use crate::error::CodecError;
    use crate::sniff::{ImageFormat, sniff};

    fn gradient(width: u32, height: u32) -> ImageBuffer {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[
                    (x * 255 / width.max(1)) as u8,
                    (y * 255 / height.max(1)) as u8,
                    128,
                    255,
                ]);
            }
        }
        ImageBuffer::from_rgba(width, height, pixels).expect("gradient is the right size")
    }

    #[test]
    fn a_png_export_is_a_png_and_survives_a_round_trip() {
        let original = gradient(32, 16);

        let bytes = encode_png(&original).expect("encode png");

        assert_eq!(sniff(&bytes), Some(ImageFormat::Png));
        assert_eq!(decode(&bytes).expect("decode png"), original);
    }

    #[test]
    fn a_png_export_keeps_the_exact_dimensions_for_odd_sizes() {
        let original = gradient(31, 17);

        let decoded = decode(&encode_png(&original).expect("encode png")).expect("decode png");

        assert_eq!((decoded.width(), decoded.height()), (31, 17));
    }

    #[test]
    fn a_jpeg_export_is_a_jpeg_and_keeps_the_dimensions() {
        let original = gradient(24, 12);

        let bytes = encode_jpeg(&original, 90).expect("encode jpeg");

        assert_eq!(sniff(&bytes), Some(ImageFormat::Jpeg));
        let decoded = decode(&bytes).expect("decode jpeg");
        assert_eq!((decoded.width(), decoded.height()), (24, 12));
    }

    #[test]
    fn a_higher_quality_costs_more_bytes() {
        let original = gradient(64, 64);

        let low = encode_jpeg(&original, 20).expect("encode jpeg");
        let high = encode_jpeg(&original, 95).expect("encode jpeg");

        assert!(
            high.len() > low.len(),
            "quality 95 gave {} bytes, quality 20 gave {}",
            high.len(),
            low.len()
        );
    }

    #[test]
    fn out_of_range_quality_is_clamped_not_refused() {
        let original = gradient(8, 8);

        assert!(encode_jpeg(&original, 0).is_ok(), "0 clamps up to 1");
        assert!(
            encode_jpeg(&original, 255).is_ok(),
            "255 clamps down to 100"
        );
    }

    #[test]
    fn an_image_too_wide_for_the_format_is_reported() {
        let too_wide = ImageBuffer::filled(u32::from(u16::MAX) + 1, 1, [0, 0, 0, 255]);

        let error = encode_jpeg(&too_wide, 80).expect_err("JPEG dimensions do not fit");

        assert!(
            error.to_string().contains("too large"),
            "unexpected message: {error}"
        );
    }

    #[test]
    fn a_zero_sized_image_is_refused_rather_than_encoded_or_panicked_on() {
        // PNG requires at least one pixel in each direction. Whoever renders a
        // preview must decide what an empty board looks like; this crate must
        // not invent a 1x1 file behind their back.
        let empty = ImageBuffer::filled(0, 0, [0, 0, 0, 0]);

        assert!(matches!(
            encode_png(&empty),
            Err(CodecError::Encode {
                format: ImageFormat::Png,
                ..
            })
        ));
    }
}
