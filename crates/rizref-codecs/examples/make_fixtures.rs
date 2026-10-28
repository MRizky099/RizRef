//! Writes the codec fixture set used by `tests/fixtures.rs`.
//!
//! Run once after cloning, or whenever a fixture needs to be refreshed:
//!
//! ```text
//! cargo run -p rizref-codecs --example make_fixtures
//! ```
//!
//! The files land in `crates/rizref-codecs/tests/data/` and are committed. They
//! are generated from real encoders (the same third-party backends the decoder
//! tests use) rather than hand-typed headers, so the integration tests exercise
//! the byte layouts actual writers produce: palette and 16-bit PNG, baseline,
//! grayscale and progressive JPEG, lossy and lossless WebP, an animated GIF,
//! BMP/TGA/PNM/QOI, a multi-size ICO and a JPEG carrying EXIF orientation 6.

use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    std::fs::create_dir_all(&dir).expect("create tests/data");

    let rgba = gradient(8, 6);
    std::fs::write(dir.join("rgba8.png"), png_rgba(&rgba)).expect("write rgba8.png");
    std::fs::write(dir.join("palette.png"), png_palette()).expect("write palette.png");
    std::fs::write(dir.join("gray16.png"), png_gray16()).expect("write gray16.png");

    std::fs::write(dir.join("baseline.jpg"), jpeg_rgba(&rgba, false)).expect("write baseline.jpg");
    std::fs::write(dir.join("progressive.jpg"), jpeg_rgba(&rgba, true))
        .expect("write progressive.jpg");
    std::fs::write(dir.join("gray.jpg"), jpeg_gray()).expect("write gray.jpg");
    std::fs::write(
        dir.join("exif-rotated.jpg"),
        jpeg_with_orientation(&rgba, 6),
    )
    .expect("write exif-rotated.jpg");

    std::fs::write(dir.join("lossless.webp"), webp_lossless(&rgba)).expect("write lossless.webp");
    // `lossy.webp` is committed as a real camera-style sample: the pure-Rust
    // encoder in this workspace writes lossless only, and fabricating a valid
    // VP8 bitstream by hand would test the fixture, not the decoder.
    std::fs::write(dir.join("animated.gif"), animated_gif()).expect("write animated.gif");

    std::fs::write(dir.join("plain.bmp"), bmp(&rgba)).expect("write plain.bmp");
    std::fs::write(dir.join("plain.tga"), tga(&rgba)).expect("write plain.tga");
    std::fs::write(dir.join("plain.ppm"), ppm()).expect("write plain.ppm");
    std::fs::write(dir.join("plain.qoi"), qoi(&rgba)).expect("write plain.qoi");
    std::fs::write(dir.join("multi.ico"), ico()).expect("write multi.ico");

    println!("fixtures written to {}", dir.display());
}

/// An RGBA gradient with a translucent corner.
fn gradient(width: u32, height: u32) -> rizref_codecs::ImageBuffer {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let alpha = if x == 0 && y == 0 { 96 } else { 255 };
            pixels.extend_from_slice(&[
                (x * 255 / width.max(1)) as u8,
                (y * 255 / height.max(1)) as u8,
                128,
                alpha,
            ]);
        }
    }
    rizref_codecs::ImageBuffer::from_rgba(width, height, pixels).expect("gradient size")
}

fn png_rgba(image: &rizref_codecs::ImageBuffer) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width(), image.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(image.pixels()).expect("png data");
    }
    bytes
}

fn png_palette() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2, 2);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_palette(vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0]);
        encoder.set_trns(vec![255, 128, 255, 255]);
        let mut writer = encoder.write_header().expect("palette header");
        writer
            .write_image_data(&[0, 1, 2, 3])
            .expect("palette data");
    }
    bytes
}

fn png_gray16() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Sixteen);
        let mut writer = encoder.write_header().expect("gray16 header");
        // Big-endian samples: 0x0000 and 0xFFFF.
        writer
            .write_image_data(&[0x00, 0x00, 0xFF, 0xFF])
            .expect("gray16 data");
    }
    bytes
}

fn jpeg_rgba(image: &rizref_codecs::ImageBuffer, progressive: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut bytes, 92);
    encoder.set_progressive(progressive);
    encoder
        .encode(
            image.pixels(),
            image.width() as u16,
            image.height() as u16,
            jpeg_encoder::ColorType::Rgba,
        )
        .expect("jpeg encode");
    bytes
}

fn jpeg_gray() -> Vec<u8> {
    let mut bytes = Vec::new();
    jpeg_encoder::Encoder::new(&mut bytes, 90)
        .encode(&[0, 64, 128, 255], 4, 1, jpeg_encoder::ColorType::Luma)
        .expect("gray jpeg encode");
    bytes
}

/// Encodes a JPEG and injects a minimal EXIF APP1 carrying `orientation`.
fn jpeg_with_orientation(image: &rizref_codecs::ImageBuffer, orientation: u16) -> Vec<u8> {
    let jpeg = jpeg_rgba(image, false);

    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II");
    tiff.extend_from_slice(&42_u16.to_le_bytes());
    tiff.extend_from_slice(&8_u32.to_le_bytes());
    tiff.extend_from_slice(&1_u16.to_le_bytes());
    tiff.extend_from_slice(&0x0112_u16.to_le_bytes());
    tiff.extend_from_slice(&3_u16.to_le_bytes());
    tiff.extend_from_slice(&1_u32.to_le_bytes());
    tiff.extend_from_slice(&orientation.to_le_bytes());
    tiff.extend_from_slice(&[0, 0]);
    tiff.extend_from_slice(&0_u32.to_le_bytes());

    let mut app1 = vec![0xFF, 0xE1];
    let length = (2 + 6 + tiff.len()) as u16;
    app1.extend_from_slice(&length.to_be_bytes());
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(&tiff);

    let mut bytes = jpeg[..2].to_vec();
    bytes.extend_from_slice(&app1);
    bytes.extend_from_slice(&jpeg[2..]);
    bytes
}

fn webp_lossless(image: &rizref_codecs::ImageBuffer) -> Vec<u8> {
    let mut bytes = Vec::new();
    image_webp::WebPEncoder::new(&mut bytes)
        .encode(
            image.pixels(),
            image.width(),
            image.height(),
            image_webp::ColorType::Rgba8,
        )
        .expect("webp encode");
    bytes
}

fn animated_gif() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, 4, 4, &[]).expect("gif encoder");
        for (colour, delay) in [([255u8, 0, 0, 255], 10u16), ([0, 0, 255, 255], 10)] {
            let mut pixels = Vec::new();
            for _ in 0..16 {
                pixels.extend_from_slice(&colour);
            }
            let mut frame = gif::Frame::from_rgba_speed(4, 4, &mut pixels, 10);
            frame.delay = delay;
            encoder.write_frame(&frame).expect("gif frame");
        }
    }
    bytes
}

fn bmp(image: &rizref_codecs::ImageBuffer) -> Vec<u8> {
    use image::ImageEncoder;
    let mut bytes = Vec::new();
    image::codecs::bmp::BmpEncoder::new(&mut bytes)
        .write_image(
            image.pixels(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .expect("bmp encode");
    bytes
}

fn tga(image: &rizref_codecs::ImageBuffer) -> Vec<u8> {
    use image::ImageEncoder;
    let mut bytes = Vec::new();
    image::codecs::tga::TgaEncoder::new(&mut bytes)
        .write_image(
            image.pixels(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .expect("tga encode");
    bytes
}

fn ppm() -> Vec<u8> {
    let mut bytes = b"P6\n2 1\n255\n".to_vec();
    bytes.extend_from_slice(&[255, 0, 0, 0, 128, 255]);
    bytes
}

fn qoi(image: &rizref_codecs::ImageBuffer) -> Vec<u8> {
    use image::ImageEncoder;
    let mut bytes = Vec::new();
    image::codecs::qoi::QoiEncoder::new(&mut bytes)
        .write_image(
            image.pixels(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .expect("qoi encode");
    bytes
}

fn ico() -> Vec<u8> {
    let small = ico::IconImage::from_rgba_data(2, 2, vec![40; 2 * 2 * 4]);
    let large_pixels: Vec<u8> = (0..4 * 4 * 4)
        .map(|index| if index % 4 == 3 { 255 } else { 200 })
        .collect();
    let large = ico::IconImage::from_rgba_data(4, 4, large_pixels);

    let mut directory = ico::IconDir::new(ico::ResourceType::Icon);
    directory.add_entry(ico::IconDirEntry::encode_as_bmp(&small).expect("small entry"));
    directory.add_entry(ico::IconDirEntry::encode(&large).expect("large entry"));

    let mut bytes = Vec::new();
    directory.write(&mut bytes).expect("ico write");
    bytes
}
