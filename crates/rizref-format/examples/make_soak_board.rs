//! Writes a large board for the phase-07 soak: many real, decodable JPEGs.
//!
//! ```bash
//! cargo run --release -p rizref-format --example make_soak_board -- 500 2000 soak.rizref
//! rizref.exe --bench-frames=600 soak.rizref
//! ```
//!
//! Arguments: item count, original size in pixels on the long side (a 3:2
//! picture), output path. Every original is a distinct, noisy JPEG at quality
//! 92 — noise is what makes JPEG large, so the board's byte size scales the
//! way real photographs do (≈ 3–4 MB per 2000 × 1333 original, so 500 items
//! land near the plan's 2 GB). Items sit on a grid; the opening camera shows
//! a corner, so the frame bench pans across mostly culled items.

use std::sync::Arc;

use glam::{DVec2, Vec2};
use rizref_codecs::{ImageBuffer, ImageFormat, encode_jpeg};
use rizref_core::{Asset, Camera, Scene, SceneId, generate_preview};
use rizref_format::{SaveRequest, save};

fn picture(seed: u32, width: u32, height: u32) -> ImageBuffer {
    let mut state = seed.wrapping_mul(2_654_435_761) | 1;
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    let hue = [(seed * 37) % 256, (seed * 91) % 256, (seed * 53) % 256];
    for y in 0..height {
        for x in 0..width {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let grain = state & 0x3F;
            let gradient = x * 160 / width + y * 60 / height;
            pixels.extend_from_slice(&[
                ((hue[0] + gradient + grain) % 256) as u8,
                ((hue[1] + gradient / 2 + grain) % 256) as u8,
                ((hue[2] + grain) % 256) as u8,
                255,
            ]);
        }
    }
    ImageBuffer::from_rgba(width, height, pixels).expect("sized")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let items: u32 = args.first().and_then(|a| a.parse().ok()).unwrap_or(500);
    let side: u32 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(2000);
    let out = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "soak.rizref".to_owned());
    let (width, height) = (side, side * 2 / 3);

    let mut scene = Scene::new(SceneId::random());
    let mut bytes_total = 0_u64;
    let columns = (f64::from(items).sqrt().ceil() as u32).max(1);
    for index in 0..items {
        let image = picture(index + 1, width, height);
        let jpeg = encode_jpeg(&image, 92).expect("jpeg");
        bytes_total += jpeg.len() as u64;
        let asset = Asset::from_decoded(
            Arc::from(jpeg),
            ImageFormat::Jpeg,
            &image,
            generate_preview(&image),
        );
        let (id, _) = scene.insert_asset(asset);
        let center = Vec2::new(
            (index % columns) as f32 * (width as f32 * 1.1),
            (index / columns) as f32 * (height as f32 * 1.1),
        );
        scene
            .insert_image(id, center, Vec2::new(width as f32, height as f32))
            .expect("asset");
        if (index + 1) % 50 == 0 {
            eprintln!(
                "{} / {items} images, {:.0} MB",
                index + 1,
                bytes_total as f64 / 1e6
            );
        }
    }
    // Open on the top-left corner at 10%: a screenful of the grid.
    let camera = Camera::new(
        DVec2::new(f64::from(width) * 3.0, f64::from(height) * 3.0),
        0.1,
        0.0,
    );
    let report = save(&out, &SaveRequest::new(&scene, camera, "Soak")).expect("save");
    println!(
        "wrote {out}: {items} items, {:.0} MB of originals, file {:.0} MB",
        bytes_total as f64 / 1e6,
        report.file_len as f64 / 1e6
    );
}
