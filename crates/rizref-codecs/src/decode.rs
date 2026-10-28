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
use crate::exif;
use crate::sniff::{ImageFormat, sniff};

/// Resource limits applied before a decoder is allowed to allocate pixels.
///
/// Reference boards import files from the internet, so a hostile or merely
/// absurd header must be refused *before* a bitmap is allocated. The default
/// caps a single image at 100 megapixels — larger than any camera or reference
/// sheet, and still over a gigabyte at one byte per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeLimits {
    /// Largest accepted `width × height` product.
    pub max_pixels: u64,
}

impl DecodeLimits {
    /// The default 100 MP cap.
    pub const DEFAULT: Self = Self {
        max_pixels: 100_000_000,
    };

    /// Whether `width × height` is within the cap.
    pub fn allows(self, width: u32, height: u32) -> bool {
        u64::from(width) * u64::from(height) <= self.max_pixels
    }
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Decodes `bytes` under the default [`DecodeLimits`], choosing the decoder
/// from the file's signature.
pub fn decode(bytes: &[u8]) -> Result<ImageBuffer, CodecError> {
    decode_with_limits(bytes, DecodeLimits::default())
}

/// Decodes `bytes` under explicit resource limits, choosing the decoder from
/// the file's signature.
pub fn decode_with_limits(bytes: &[u8], limits: DecodeLimits) -> Result<ImageBuffer, CodecError> {
    let format = sniff(bytes).ok_or(CodecError::Unrecognized)?;
    decode_as_with_limits(bytes, format, limits)
}

/// Decodes `bytes` that the caller already knows the format of.
///
/// Useful when the signature is absent (TGA) or when the caller has stronger
/// information than the first handful of bytes, such as an import job that
/// recorded the format when the file was read.
pub fn decode_as(bytes: &[u8], format: ImageFormat) -> Result<ImageBuffer, CodecError> {
    decode_as_with_limits(bytes, format, DecodeLimits::default())
}

/// Decodes `bytes` of a known format under explicit resource limits.
pub fn decode_as_with_limits(
    bytes: &[u8],
    format: ImageFormat,
    limits: DecodeLimits,
) -> Result<ImageBuffer, CodecError> {
    match format {
        ImageFormat::Png => decode_png(bytes, limits),
        ImageFormat::Jpeg => decode_jpeg(bytes, limits),
        ImageFormat::Gif => decode_gif(bytes, limits),
        ImageFormat::WebP => decode_webp(bytes, limits),
        ImageFormat::Bmp | ImageFormat::Tga | ImageFormat::Pnm | ImageFormat::Qoi => {
            decode_plain(bytes, format, limits)
        }
        ImageFormat::Ico => decode_ico(bytes, limits),
        ImageFormat::Heif => Err(CodecError::Unsupported {
            format,
            detail: "no pure-Rust decoder in this build; HEIC is an accepted gap until a \
                     pure-Rust decoder exists"
                .to_owned(),
        }),
    }
}

/// Refuses dimensions over the caller's cap before any pixels are allocated.
fn check_limits(width: u32, height: u32, limits: DecodeLimits) -> Result<(), CodecError> {
    if limits.allows(width, height) {
        Ok(())
    } else {
        Err(CodecError::TooLarge { width, height })
    }
}

/// The IHDR dimensions of a PNG, when the bytes open with a complete header.
///
/// A PNG begins: 8-byte signature, 4-byte chunk length, `IHDR`, then width and
/// height as big-endian `u32`. Reading those 24 bytes is enough to enforce the
/// cap without trusting the rest of the file.
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
    let header = bytes.get(..24)?;
    if header[..8] != SIGNATURE || &header[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(header[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(header[20..24].try_into().ok()?);
    Some((width, height))
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
fn decode_png(bytes: &[u8], limits: DecodeLimits) -> Result<ImageBuffer, CodecError> {
    // Read the IHDR dimensions directly: the decoder's own header pass refuses
    // a file without IDAT, which would turn "too large" into a vague decode
    // error before the cap is consulted.
    if let Some((width, height)) = png_dimensions(bytes) {
        check_limits(width, height, limits)?;
    }

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
    let info = reader.info();
    check_limits(info.width, info.height, limits)?;

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

/// JPEG, through `zune-jpeg`, with EXIF orientation applied.
fn decode_jpeg(bytes: &[u8], limits: DecodeLimits) -> Result<ImageBuffer, CodecError> {
    // Ask for RGBA directly: JPEG has no alpha, and letting the decoder expand
    // the chroma planes removes a BGR/RGB guess from this crate.
    let options = zune_jpeg::zune_core::options::DecoderOptions::default()
        .jpeg_set_out_colorspace(zune_jpeg::zune_core::colorspace::ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(bytes), options);

    // Read the frame header first so an oversized image is refused before the
    // decoder allocates its output buffer.
    decoder
        .decode_headers()
        .map_err(|error| failed(ImageFormat::Jpeg, error))?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| failed(ImageFormat::Jpeg, "image dimensions are missing"))?;
    check_limits(width as u32, height as u32, limits)?;

    let pixels = decoder
        .decode()
        .map_err(|error| failed(ImageFormat::Jpeg, error))?;

    let image = finish(ImageFormat::Jpeg, width as u32, height as u32, pixels)?;
    Ok(exif::apply_orientation(
        image,
        exif::jpeg_orientation(bytes),
    ))
}

/// GIF, through the `gif` crate; only the first frame is used.
fn decode_gif(bytes: &[u8], limits: DecodeLimits) -> Result<ImageBuffer, CodecError> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);

    let mut decoder = options
        .read_info(bytes)
        .map_err(|error| failed(ImageFormat::Gif, error))?;
    check_limits(
        u32::from(decoder.width()),
        u32::from(decoder.height()),
        limits,
    )?;

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
fn decode_webp(bytes: &[u8], limits: DecodeLimits) -> Result<ImageBuffer, CodecError> {
    let mut decoder = image_webp::WebPDecoder::new(Cursor::new(bytes))
        .map_err(|error| failed(ImageFormat::WebP, error))?;
    let (width, height) = decoder.dimensions();
    check_limits(width, height, limits)?;
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

    let image = finish(ImageFormat::WebP, width, height, rgba)?;
    Ok(exif::apply_orientation(
        image,
        exif::webp_orientation(bytes),
    ))
}

/// BMP, TGA, PNM and QOI, through the trimmed `image` backends.
///
/// Grouped into one call because they share a shape: a simple container with no
/// colour management to speak of, and no need for a dedicated pure-Rust crate.
fn decode_plain(
    bytes: &[u8],
    format: ImageFormat,
    limits: DecodeLimits,
) -> Result<ImageBuffer, CodecError> {
    let backend = match format {
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        ImageFormat::Tga => image::ImageFormat::Tga,
        ImageFormat::Pnm => image::ImageFormat::Pnm,
        ImageFormat::Qoi => image::ImageFormat::Qoi,
        _ => return Err(CodecError::Unrecognized),
    };

    // `into_dimensions` reads the header only: the cap must be enforced before
    // `load_from_memory_with_format` allocates the bitmap.
    let (width, height) = image::ImageReader::with_format(Cursor::new(bytes), backend)
        .into_dimensions()
        .map_err(|error| failed(format, error))?;
    check_limits(width, height, limits)?;

    let decoded = image::load_from_memory_with_format(bytes, backend)
        .map_err(|error| failed(format, error))?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();

    finish(format, width, height, rgba.into_raw())
}

/// ICO, through the `ico` crate; the largest embedded image wins.
fn decode_ico(bytes: &[u8], limits: DecodeLimits) -> Result<ImageBuffer, CodecError> {
    let directory =
        ico::IconDir::read(Cursor::new(bytes)).map_err(|error| failed(ImageFormat::Ico, error))?;

    let best = directory
        .entries()
        .iter()
        .max_by_key(|entry| u64::from(entry.width()) * u64::from(entry.height()))
        .ok_or_else(|| failed(ImageFormat::Ico, "icon contains no images"))?;
    check_limits(best.width(), best.height(), limits)?;
    check_ico_entry(best, limits)?;

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

/// Length of the `BITMAPINFOHEADER` an ICO entry starts with.
const ICO_BMP_HEADER_LEN: usize = 40;

/// Validates an entry against this crate's contract before `ico` sees it.
///
/// The directory's width and height are only a claim: the real dimensions live
/// in the entry, so the megapixel cap has to be checked there too. The palette
/// walk exists because `ico` 0.6 indexes its colour table with the raw pixel
/// index and never clamps it — a 4-bit icon declaring a two-colour palette
/// panics on the first pixel above colour 1 (found by the `decode_ico` fuzz
/// target). Refusing the entry turns that into a decode error.
fn check_ico_entry(entry: &ico::IconDirEntry, limits: DecodeLimits) -> Result<(), CodecError> {
    let data = entry.data();
    if let Some((width, height)) = png_dimensions(data) {
        return check_limits(width, height, limits);
    }
    check_ico_bitmap(data, limits)
}

/// Validates one `BITMAPINFOHEADER` entry: real dimensions and palette indices.
fn check_ico_bitmap(data: &[u8], limits: DecodeLimits) -> Result<(), CodecError> {
    let Some(header) = data.get(..ICO_BMP_HEADER_LEN) else {
        // Too short to be a bitmap: the decoder reports it.
        return Ok(());
    };
    let header_len = u32::from_le_bytes(header[0..4].try_into().expect("four bytes"));
    if header_len as usize != ICO_BMP_HEADER_LEN {
        return Ok(());
    }

    let width = i32::from_le_bytes(header[4..8].try_into().expect("four bytes"));
    let height = i32::from_le_bytes(header[8..12].try_into().expect("four bytes"));
    let bits = u16::from_le_bytes(header[14..16].try_into().expect("two bytes"));
    let colors_used = u32::from_le_bytes(header[32..36].try_into().expect("four bytes"));

    // The stored height counts the colour rows and the alpha mask, hence /2.
    // A malformed header is the decoder's to reject before it reads pixels.
    if width <= 0 || height <= 0 || height % 2 != 0 {
        return Ok(());
    }
    let pixel_height = (height / 2) as u32;
    check_limits(width as u32, pixel_height, limits)?;

    let palette_max = match bits {
        1 => 2,
        4 => 16,
        8 => 256,
        // 16-, 24- and 32-bit entries index no palette, and unknown depths are
        // rejected by the decoder.
        _ => return Ok(()),
    };
    let colors = if colors_used == 0 || colors_used > palette_max {
        palette_max
    } else {
        colors_used
    };
    let colors = colors as usize;
    let palette_end = ICO_BMP_HEADER_LEN + colors * 4;
    if data.len() < palette_end {
        // A truncated palette makes the decoder return an error; it never
        // indexes past what it read.
        return Ok(());
    }

    let width = width as usize;
    let bits = bits as usize;
    let row_bytes = (width * bits).div_ceil(8);
    let stride = row_bytes.div_ceil(4) * 4;

    // `ico` unpacks a 4-bit row without honouring the image width: an odd
    // width writes one pixel past the end of the bottom row and panics. No
    // validation of the data can make that decode safe, so such an entry is
    // reported as unsupported instead.
    if bits == 4 && !width.is_multiple_of(2) {
        return Err(failed(
            ImageFormat::Ico,
            "4-bit icon has an odd width, which its decoder cannot unpack",
        ));
    }

    let check = |index: u8| -> Result<(), CodecError> {
        if usize::from(index) >= colors {
            return Err(failed(
                ImageFormat::Ico,
                format!("indexed icon uses colour {index} but its palette holds {colors}"),
            ));
        }
        Ok(())
    };

    let mut offset = palette_end;
    for _row in 0..pixel_height as usize {
        // `ico` reads a row byte by byte and only meets the end of the data on
        // the next read, so a *partial* row is still indexed. Scan whatever is
        // present rather than requiring the whole row: a truncated fuzz input
        // panicked exactly this way.
        let available = data.len().saturating_sub(offset).min(row_bytes);
        let row_data = &data[offset..offset + available];
        // The per-depth loops mirror `ico` exactly, including its quirks: the
        // 4-bit arm ignores the width and unpacks both nibbles of every byte.
        match bits {
            1 => {
                let mut col = 0;
                for &byte in row_data {
                    for bit in 0..8 {
                        check((byte >> (7 - bit)) & 1)?;
                        col += 1;
                        if col == width {
                            break;
                        }
                    }
                }
            }
            4 => {
                for &byte in row_data {
                    check((byte >> 4) & 0xF)?;
                    check(byte & 0xF)?;
                }
            }
            _ => {
                for &byte in row_data {
                    check(byte)?;
                }
            }
        }
        offset = offset.saturating_add(stride);
        if offset >= data.len() {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DecodeLimits, decode, decode_as, decode_with_limits};
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

    /// Builds a `BITMAPINFOHEADER` entry for the palette and size checks.
    fn bmp_entry(
        width: i32,
        height: i32,
        bits: u16,
        colors_used: u32,
        palette: usize,
        pixels: &[u8],
    ) -> Vec<u8> {
        let mut entry = vec![0u8; 40];
        entry[0..4].copy_from_slice(&40_u32.to_le_bytes());
        entry[4..8].copy_from_slice(&width.to_le_bytes());
        entry[8..12].copy_from_slice(&height.to_le_bytes());
        entry[14..16].copy_from_slice(&bits.to_le_bytes());
        entry[32..36].copy_from_slice(&colors_used.to_le_bytes());
        for _ in 0..palette {
            entry.extend_from_slice(&[0, 0, 0, 0]);
        }
        entry.extend_from_slice(pixels);
        entry
    }

    #[test]
    fn a_4_bit_icon_indexing_past_its_palette_is_refused() {
        // 4 x 4 pixels at 4 bits each: two bytes a row, padded to four. Every
        // nibble indexes colour 2, but the palette only holds two entries.
        let entry = bmp_entry(4, 8, 4, 2, 2, &[0x22; 16]);

        let error = super::check_ico_bitmap(&entry, DecodeLimits::default())
            .expect_err("colour 2 is outside a two-colour palette");

        assert!(
            error.to_string().contains("palette holds 2"),
            "unexpected: {error}"
        );
    }

    #[test]
    fn the_same_icon_within_its_palette_is_accepted() {
        let entry = bmp_entry(4, 8, 4, 2, 2, &[0x11; 16]);

        super::check_ico_bitmap(&entry, DecodeLimits::default()).expect("indices 1 are valid");
    }

    #[test]
    fn a_1_bit_icon_with_a_one_colour_palette_is_refused() {
        // The high bit is set, so the first pixel indexes colour 1.
        let entry = bmp_entry(4, 8, 1, 1, 1, &[0x80; 4]);

        assert!(super::check_ico_bitmap(&entry, DecodeLimits::default()).is_err());
    }

    #[test]
    fn a_truncated_palette_is_left_to_the_decoder() {
        let entry = bmp_entry(4, 8, 4, 16, 1, &[]);

        super::check_ico_bitmap(&entry, DecodeLimits::default())
            .expect("a short palette is already a decode error");
    }

    #[test]
    fn a_partial_pixel_row_is_still_scanned_for_bad_indices() {
        // The fuzz reproducer: a 64-pixel row whose data stops after three
        // bytes, the last of which indexes colour 4 in a one-colour palette.
        let entry = bmp_entry(64, 2, 4, 1, 1, &[0x00, 0x00, 0x04]);

        assert!(
            super::check_ico_bitmap(&entry, DecodeLimits::default()).is_err(),
            "a truncated row must not skip the bytes that are present"
        );
    }

    #[test]
    fn an_8_bit_icon_is_scanned_without_a_shift_overflow() {
        // 8 bits is the depth where a naive `1u8 << bits` mask panics; the
        // second fuzz reproducer hit exactly that in this validator.
        let entry = bmp_entry(2, 2, 8, 1, 1, &[0x01, 0x00]);

        let error = super::check_ico_bitmap(&entry, DecodeLimits::default())
            .expect_err("colour 1 is outside a one-colour palette");

        assert!(error.to_string().contains("palette holds 1"), "{error}");
    }

    #[test]
    fn a_4_bit_icon_with_an_odd_width_is_refused() {
        // `ico` writes one pixel past the buffer for odd 4-bit rows, so the
        // entry must be reported before it reaches the crate.
        let entry = bmp_entry(3, 2, 4, 16, 16, &[0u8; 8]);

        let error = super::check_ico_bitmap(&entry, DecodeLimits::default())
            .expect_err("an odd 4-bit row cannot be unpacked safely");

        assert!(error.to_string().contains("odd width"), "{error}");
    }

    #[test]
    fn a_lying_directory_size_cannot_bypass_the_megapixel_cap() {
        // The directory claims 4 x 4; the entry itself says 20 000 x 20 000.
        let entry = bmp_entry(20_000, 40_000, 32, 0, 0, &[]);

        let error = super::check_ico_bitmap(&entry, DecodeLimits::default())
            .expect_err("400 MP is over the default cap");

        assert!(matches!(
            error,
            CodecError::TooLarge {
                width: 20_000,
                height: 20_000
            }
        ));
    }

    #[test]
    fn a_true_colour_entry_needs_no_palette_walk() {
        let entry = bmp_entry(4, 8, 32, 0, 0, &[]);

        super::check_ico_bitmap(&entry, DecodeLimits::default())
            .expect("a 32-bit entry indexes no palette");
    }

    /// Wraps one entry in the smallest legal `ICONDIR`.
    fn ico_file(entry: &[u8], width: u8, height: u8, bits: u16) -> Vec<u8> {
        let mut file = vec![0, 0, 1, 0, 1, 0];
        file.extend_from_slice(&[width, height, 0, 0]);
        file.extend_from_slice(&1_u16.to_le_bytes());
        file.extend_from_slice(&bits.to_le_bytes());
        file.extend_from_slice(&(entry.len() as u32).to_le_bytes());
        file.extend_from_slice(&22_u32.to_le_bytes());
        file.extend_from_slice(entry);
        file
    }

    /// Deterministic pseudo-random source, so the sweep is reproducible.
    fn next(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state >> 33
    }

    #[test]
    fn a_generated_ico_sweep_never_panics() {
        // A miniature deterministic fuzzer: the `ico` crate has several
        // unchecked index paths, and every one the soak found is in this
        // space. Five thousand entries run in milliseconds, so a regression
        // shows up in `cargo test` instead of a 10-minute CI soak.
        let depths = [1_u16, 4, 8, 16, 24, 32];
        let mut state = 0x2545_f491_4f6c_dd1d_u64;

        for _ in 0..20_000 {
            let width = (next(&mut state) % 80) as i32 + 1;
            let height = ((next(&mut state) % 20) as i32 + 1) * 2;
            let bits = depths[(next(&mut state) % depths.len() as u64) as usize];
            let colors_used = (next(&mut state) % 300) as u32;
            let palette = (next(&mut state) % 300) as usize;
            let pixel_len = (next(&mut state) % 400) as usize;
            let mut pixels = vec![0_u8; pixel_len];
            for byte in &mut pixels {
                *byte = next(&mut state) as u8;
            }

            let entry = bmp_entry(width, height, bits, colors_used, palette, &pixels);
            let file = ico_file(
                &entry,
                width.min(255) as u8,
                (height / 2).min(255) as u8,
                bits,
            );

            let outcome = std::panic::catch_unwind(|| decode(&file));
            assert!(
                outcome.is_ok(),
                "decode panicked on width {width}, height {height}, bits {bits}, \
                 colours {colors_used}, palette {palette}, {pixel_len} pixel bytes"
            );
        }
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

    #[test]
    fn a_header_over_the_megapixel_cap_is_refused_before_decoding() {
        // A real IHDR but no image data: the dimensions are readable, so the
        // cap can answer without ever allocating the 1.6 GB bitmap.
        let mut bytes = Vec::new();
        {
            let encoder = png::Encoder::new(&mut bytes, 20_000, 20_000);
            let _writer = encoder.write_header().expect("header");
        }

        let error = decode_with_limits(&bytes, DecodeLimits::default())
            .expect_err("400 MP is over the default cap");

        assert!(
            matches!(
                error,
                CodecError::TooLarge {
                    width: 20_000,
                    height: 20_000
                }
            ),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn the_megapixel_cap_is_exclusive_of_nothing_under_it() {
        let original = sample();
        let bytes = png_of(&original);

        let limits = DecodeLimits {
            max_pixels: u64::from(original.width()) * u64::from(original.height()),
        };
        assert!(decode_with_limits(&bytes, limits).is_ok());

        let below = DecodeLimits {
            max_pixels: limits.max_pixels - 1,
        };
        assert!(matches!(
            decode_with_limits(&bytes, below),
            Err(CodecError::TooLarge { .. })
        ));
    }

    #[test]
    fn exif_orientation_is_baked_into_the_decoded_jpeg() {
        let original = ImageBuffer::filled(4, 2, [90, 100, 110, 255]);
        let mut bytes = jpeg_of(&original, 95);

        // Insert an APP1 EXIF segment (little-endian TIFF, orientation 6 =
        // rotate 90 degrees clockwise) right after SOI.
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&42_u16.to_le_bytes());
        tiff.extend_from_slice(&8_u32.to_le_bytes());
        tiff.extend_from_slice(&1_u16.to_le_bytes());
        tiff.extend_from_slice(&0x0112_u16.to_le_bytes());
        tiff.extend_from_slice(&3_u16.to_le_bytes());
        tiff.extend_from_slice(&1_u32.to_le_bytes());
        tiff.extend_from_slice(&6_u16.to_le_bytes());
        tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&0_u32.to_le_bytes());

        let mut segment = vec![0xFF, 0xE1];
        let length = (2 + 6 + tiff.len()) as u16;
        segment.extend_from_slice(&length.to_be_bytes());
        segment.extend_from_slice(b"Exif\0\0");
        segment.extend_from_slice(&tiff);

        let mut with_exif = bytes[..2].to_vec();
        with_exif.extend_from_slice(&segment);
        with_exif.extend_from_slice(&bytes[2..]);
        bytes = with_exif;

        let decoded = decode(&bytes).expect("decode rotated jpeg");

        assert_eq!(
            (decoded.width(), decoded.height()),
            (2, 4),
            "orientation 6 swaps the dimensions"
        );
    }
}
