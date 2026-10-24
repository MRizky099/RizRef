//! Decoding an image into the canonical RGBA buffer.
//!
//! Two entry points: [`decode`] sniffs the format from the bytes, and
//! [`decode_as`] is for callers that already know it — a file opened through the
//! open dialog, or a resumed import whose format came from the extension.
//!
//! Decoders are instructed to normalise rather than to hand back their native
//! layout: palette, grayscale, 16-bit and alpha-channel variants all come out as
//! 8-bit RGBA, so the caller has one case to handle instead of nine.

use std::io::Cursor;

use crate::buffer::ImageBuffer;
use crate::error::CodecError;
use crate::sniff::{ImageFormat, sniff};

/// Decodes `bytes`, choosing the decoder from the file's signature.
pub fn decode(bytes: &[u8]) -> Result<ImageBuffer, CodecError> {
    let format = sniff(bytes).ok_or(CodecError::Unrecognized)?;
    decode_as(bytes, format)
}

/// Decodes `bytes` that the caller already knows the format of.
///
/// Useful when the signature is absent (TGA) or when the caller has stronger
/// information than the first handful of bytes, such as an import job that
/// recorded the format when the file was read.
pub fn decode_as(bytes: &[u8], format: ImageFormat) -> Result<ImageBuffer, CodecError> {
    match format {
        ImageFormat::Png => decode_png(bytes),
        ImageFormat::Jpeg => decode_jpeg(bytes),
        ImageFormat::Gif => decode_gif(bytes),
        ImageFormat::WebP => decode_webp(bytes),
        ImageFormat::Bmp | ImageFormat::Tga | ImageFormat::Pnm | ImageFormat::Qoi => {
            decode_plain(bytes, format)
        }
        ImageFormat::Ico => decode_ico(bytes),
        ImageFormat::Heif => Err(CodecError::Unsupported {
            format,
            detail: "no pure-Rust decoder in this build; HEIC is an accepted gap until a \
                     pure-Rust decoder exists"
                .to_owned(),
        }),
    }
}

/// Reports a decoder failure, keeping the format for the message.
fn failed(format: ImageFormat, reason: impl std::fmt::Display) -> CodecError {
    CodecError::Decode {
        format,
        reason: reason.to_string(),
    }
}

/// Assembles the result, refusing to hand back a buffer that does not match.
///
/// A decoder that returns the wrong number of bytes has a bug, and this is the
/// last place where that can be noticed before the renderer reads past a row.
fn finish(
    format: ImageFormat,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
) -> Result<ImageBuffer, CodecError> {
    ImageBuffer::from_rgba(width, height, pixels).ok_or_else(|| CodecError::Decode {
        format,
        reason: format!("decoder returned a buffer that does not match {width} x {height} RGBA"),
    })
}

/// Converts a buffer with fewer channels per pixel into RGBA.
///
/// Used for the layouts a decoder can legitimately hand back after
/// normalisation: grayscale, grayscale with alpha, and RGB.
fn expand_channels(source: &[u8], channels: usize, alpha: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(source.len() / channels * ImageBuffer::BYTES_PER_PIXEL);
    for texel in source.chunks_exact(channels) {
        match channels {
            1 => out.extend_from_slice(&[texel[0], texel[0], texel[0], alpha]),
            2 => out.extend_from_slice(&[texel[0], texel[0], texel[0], texel[1]]),
            3 => out.extend_from_slice(&[texel[0], texel[1], texel[2], alpha]),
            _ => out.extend_from_slice(texel),
        }
    }
    out
}

/// PNG, through the `png` crate.
fn decode_png(bytes: &[u8]) -> Result<ImageBuffer, CodecError> {
    // Both the PNG and JPEG decoders need a seekable reader; a borrowed slice
    // wrapped in a cursor is exactly that, with nothing copied.
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    // Expand palette and low-bit-depth images, and 16-bit samples, into 8-bit
    // channels: the canonical buffer only has one depth.
    decoder.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );

    let mut reader = decoder
        .read_info()
        .map_err(|error| failed(ImageFormat::Png, error))?;
    let mut buffer = vec![
        0_u8;
        reader.output_buffer_size().ok_or_else(|| failed(
            ImageFormat::Png,
            "image dimensions overflow this platform"
        ))?
    ];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| failed(ImageFormat::Png, error))?;
    buffer.truncate(info.buffer_size());

    let rgba = match reader.output_color_type().0 {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => expand_channels(&buffer, 3, u8::MAX),
        png::ColorType::GrayscaleAlpha => expand_channels(&buffer, 2, u8::MAX),
        png::ColorType::Grayscale => expand_channels(&buffer, 1, u8::MAX),
        png::ColorType::Indexed => {
            // Unreachable after EXPAND, but a decoder is the wrong place to panic.
            return Err(failed(
                ImageFormat::Png,
                "indexed output was not expanded by the decoder",
            ));
        }
    };

    finish(ImageFormat::Png, info.width, info.height, rgba)
}

/// JPEG, through `zune-jpeg`.
fn decode_jpeg(bytes: &[u8]) -> Result<ImageBuffer, CodecError> {
    // Ask for RGBA directly: JPEG has no alpha, and letting the decoder expand
    // the chroma planes removes a BGR/RGB guess from this crate.
    let options = zune_jpeg::zune_core::options::DecoderOptions::default()
        .jpeg_set_out_colorspace(zune_jpeg::zune_core::colorspace::ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(bytes), options);

    let pixels = decoder
        .decode()
        .map_err(|error| failed(ImageFormat::Jpeg, error))?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| failed(ImageFormat::Jpeg, "image dimensions are missing"))?;

    finish(ImageFormat::Jpeg, width as u32, height as u32, pixels)
}

/// GIF, through the `gif` crate; only the first frame is used.
fn decode_gif(bytes: &[u8]) -> Result<ImageBuffer, CodecError> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);

    let mut decoder = options
        .read_info(bytes)
        .map_err(|error| failed(ImageFormat::Gif, error))?;
    let frame = decoder
        .read_next_frame()
        .map_err(|error| failed(ImageFormat::Gif, error))?
        .ok_or_else(|| failed(ImageFormat::Gif, "file contains no frames"))?;

    finish(
        ImageFormat::Gif,
        u32::from(frame.width),
        u32::from(frame.height),
        frame.buffer.to_vec(),
    )
}

/// WebP, through `image-webp`; lossy and lossless are both handled.
fn decode_webp(bytes: &[u8]) -> Result<ImageBuffer, CodecError> {
    let mut decoder = image_webp::WebPDecoder::new(Cursor::new(bytes))
        .map_err(|error| failed(ImageFormat::WebP, error))?;
    let (width, height) = decoder.dimensions();
    let has_alpha = decoder.has_alpha();

    // The decoder writes 4 bytes per pixel only when the file carries alpha;
    // otherwise it writes RGB, which is why the size is taken from it rather
    // than computed from the dimensions.
    let expected = decoder
        .output_buffer_size()
        .ok_or_else(|| failed(ImageFormat::WebP, "image dimensions overflow this platform"))?;
    let mut buffer = vec![0_u8; expected];
    decoder
        .read_image(&mut buffer)
        .map_err(|error| failed(ImageFormat::WebP, error))?;

    let rgba = if has_alpha {
        buffer
    } else {
        expand_channels(&buffer, 3, u8::MAX)
    };

    finish(ImageFormat::WebP, width, height, rgba)
}

/// BMP, TGA, PNM and QOI, through the trimmed `image` backends.
///
/// Grouped into one call because they share a shape: a simple container with no
/// colour management to speak of, and no need for a dedicated pure-Rust crate.
fn decode_plain(bytes: &[u8], format: ImageFormat) -> Result<ImageBuffer, CodecError> {
    let backend = match format {
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        ImageFormat::Tga => image::ImageFormat::Tga,
        ImageFormat::Pnm => image::ImageFormat::Pnm,
        ImageFormat::Qoi => image::ImageFormat::Qoi,
        _ => return Err(CodecError::Unrecognized),
    };

    let decoded = image::load_from_memory_with_format(bytes, backend)
        .map_err(|error| failed(format, error))?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();

    finish(format, width, height, rgba.into_raw())
}

/// ICO, through the `ico` crate; the largest embedded image wins.
fn decode_ico(bytes: &[u8]) -> Result<ImageBuffer, CodecError> {
    let directory =
        ico::IconDir::read(Cursor::new(bytes)).map_err(|error| failed(ImageFormat::Ico, error))?;

    let best = directory
        .entries()
        .iter()
        .max_by_key(|entry| u64::from(entry.width()) * u64::from(entry.height()))
        .ok_or_else(|| failed(ImageFormat::Ico, "icon contains no images"))?;

    let image = best
        .decode()
        .map_err(|error| failed(ImageFormat::Ico, error))?;

    finish(
        ImageFormat::Ico,
        image.width(),
        image.height(),
        image.rgba_data().to_vec(),
    )
}

#[cfg(test)]
mod tests {
    use super::{decode, decode_as};
    use crate::buffer::ImageBuffer;
    use crate::error::CodecError;
    use crate::sniff::ImageFormat;

    /// A 4×2 test image with a gradient and a transparent corner.
    fn sample() -> ImageBuffer {
        let mut pixels = Vec::with_capacity(4 * 2 * 4);
        for y in 0..2_u32 {
            for x in 0..4_u32 {
                let alpha = if x == 0 && y == 0 { 0 } else { 255 };
                pixels.extend_from_slice(&[
                    (x * 60) as u8,
                    (y * 90) as u8,
                    ((x + y) * 30) as u8,
                    alpha,
                ]);
            }
        }
        ImageBuffer::from_rgba(4, 2, pixels).expect("4 x 2 RGBA")
    }

    fn png_of(image: &ImageBuffer) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, image.width(), image.height());
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("header");
            writer.write_image_data(image.pixels()).expect("pixels");
        }
        bytes
    }

    fn jpeg_of(image: &ImageBuffer, quality: u8) -> Vec<u8> {
        let mut bytes = Vec::new();
        jpeg_encoder::Encoder::new(&mut bytes, quality)
            .encode(
                image.pixels(),
                image.width() as u16,
                image.height() as u16,
                jpeg_encoder::ColorType::Rgba,
            )
            .expect("jpeg encode");
        bytes
    }

    fn webp_of(image: &ImageBuffer) -> Vec<u8> {
        let mut bytes = Vec::new();
        image_webp::WebPEncoder::new(&mut bytes)
            .encode(
                image.pixels(),
                image.width(),
                image.height(),
                image_webp::ColorType::Rgba8,
            )
            .expect("lossless webp encode");
        bytes
    }

    fn gif_of(image: &ImageBuffer) -> Vec<u8> {
        let mut pixels = image.pixels().to_vec();
        let mut bytes = Vec::new();
        {
            let mut encoder =
                gif::Encoder::new(&mut bytes, image.width() as u16, image.height() as u16, &[])
                    .expect("gif encoder");
            let frame = gif::Frame::from_rgba_speed(
                image.width() as u16,
                image.height() as u16,
                &mut pixels,
                10,
            );
            encoder.write_frame(&frame).expect("gif frame");
        }
        bytes
    }

    #[test]
    fn png_round_trips_exactly_including_alpha() {
        let original = sample();

        let bytes = png_of(&original);
        let decoded = decode(&bytes).expect("decode png");

        assert_eq!(decoded, original);
        assert_eq!(
            decode_as(&bytes, ImageFormat::Png).expect("decode as png"),
            original
        );
    }

    #[test]
    fn a_grayscale_png_is_expanded_to_rgba() {
        // 1-bit grayscale, the layout a scanned reference sheet often arrives in.
        let bytes = {
            let mut out = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut out, 2, 1);
                encoder.set_color(png::ColorType::Grayscale);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().expect("header");
                writer.write_image_data(&[0, 200]).expect("pixels");
            }
            out
        };

        let decoded = decode(&bytes).expect("decode grayscale png");

        assert_eq!(decoded.pixel(0, 0), Some([0, 0, 0, 255]));
        assert_eq!(decoded.pixel(1, 0), Some([200, 200, 200, 255]));
    }

    #[test]
    fn jpeg_round_trips_within_lossy_tolerance() {
        let original = ImageBuffer::filled(16, 16, [120, 40, 200, 255]);

        let decoded = decode(&jpeg_of(&original, 95)).expect("decode jpeg");

        assert_eq!((decoded.width(), decoded.height()), (16, 16));
        let pixel = decoded.pixel(8, 8).expect("inside the image");
        for (channel, expected) in pixel.iter().zip([120_u8, 40, 200, 255]) {
            let difference = channel.abs_diff(expected);
            assert!(difference <= 8, "channel off by {difference}: {pixel:?}");
        }
    }

    #[test]
    fn gif_decodes_its_first_frame() {
        let original = ImageBuffer::filled(8, 4, [10, 20, 30, 255]);

        let decoded = decode(&gif_of(&original)).expect("decode gif");

        assert_eq!((decoded.width(), decoded.height()), (8, 4));
        assert_eq!(decoded.pixels().len(), 8 * 4 * 4);
    }

    #[test]
    fn webp_round_trips_losslessly() {
        let original = sample();

        let bytes = webp_of(&original);
        let decoded = decode(&bytes).expect("decode webp");

        assert_eq!(decoded, original);
    }

    #[test]
    fn bmp_round_trips_through_the_plain_backend() {
        use image::ImageEncoder;

        let original = sample();
        let mut bytes = Vec::new();
        image::codecs::bmp::BmpEncoder::new(&mut bytes)
            .write_image(
                original.pixels(),
                original.width(),
                original.height(),
                image::ExtendedColorType::Rgba8,
            )
            .expect("bmp encode");

        let decoded = decode(&bytes).expect("decode bmp");

        assert_eq!(decoded, original);
    }

    #[test]
    fn pnm_is_decoded_from_a_hand_written_ppm() {
        // The simplest real PNM: ASCII header, binary RGB triples.
        let mut bytes = b"P6\n2 1\n255\n".to_vec();
        bytes.extend_from_slice(&[255, 0, 0, 0, 128, 255]);

        let decoded = decode(&bytes).expect("decode ppm");

        assert_eq!((decoded.width(), decoded.height()), (2, 1));
        assert_eq!(decoded.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(decoded.pixel(1, 0), Some([0, 128, 255, 255]));
    }

    #[test]
    fn tga_is_decoded_when_the_caller_knows_the_format() {
        use image::ImageEncoder;

        let original = sample();
        let mut bytes = Vec::new();
        image::codecs::tga::TgaEncoder::new(&mut bytes)
            .write_image(
                original.pixels(),
                original.width(),
                original.height(),
                image::ExtendedColorType::Rgba8,
            )
            .expect("tga encode");

        // TGA has no signature, so sniffing cannot find it...
        assert_eq!(crate::sniff(&bytes), None);
        // ...but the extension-led path decodes it.
        assert_eq!(
            decode_as(&bytes, ImageFormat::Tga).expect("decode tga"),
            original
        );
    }

    #[test]
    fn ico_uses_the_largest_embedded_image() {
        let small = ico::IconImage::from_rgba_data(2, 2, vec![0; 2 * 2 * 4]);
        let large_pixels: Vec<u8> = (0..4 * 4 * 4)
            .map(|index| if index % 4 == 3 { 255 } else { 200 })
            .collect();
        let large = ico::IconImage::from_rgba_data(4, 4, large_pixels.clone());

        let mut directory = ico::IconDir::new(ico::ResourceType::Icon);
        directory.add_entry(ico::IconDirEntry::encode_as_bmp(&small).expect("bmp entry"));
        directory.add_entry(ico::IconDirEntry::encode(&large).expect("entry"));
        let mut bytes = Vec::new();
        directory.write(&mut bytes).expect("write ico");

        let decoded = decode(&bytes).expect("decode ico");

        assert_eq!((decoded.width(), decoded.height()), (4, 4));
        assert_eq!(decoded.pixel(0, 0), Some([200, 200, 200, 255]));
    }

    #[test]
    fn a_heic_is_recognised_and_explained_rather_than_called_corrupt() {
        let mut bytes = 24_u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"ftypheic");
        bytes.extend_from_slice(&[0; 8]);

        let error = decode(&bytes).expect_err("HEIC is not decodable yet");

        assert!(
            matches!(
                error,
                CodecError::Unsupported {
                    format: ImageFormat::Heif,
                    ..
                }
            ),
            "unexpected error: {error:?}"
        );
        assert!(error.to_string().contains("not supported yet"));
    }

    #[test]
    fn unknown_and_truncated_input_is_reported_not_panicked_on() {
        assert_eq!(decode(b"").expect_err("empty"), CodecError::Unrecognized);
        assert_eq!(
            decode(b"# a note, not an image").expect_err("text"),
            CodecError::Unrecognized
        );

        // A PNG signature followed by nonsense: recognised, then damaged.
        let mut damaged = b"\x89PNG\r\n\x1a\n".to_vec();
        damaged.extend_from_slice(&[0x11; 64]);
        assert!(matches!(
            decode(&damaged),
            Err(CodecError::Decode {
                format: ImageFormat::Png,
                ..
            })
        ));
    }
}
