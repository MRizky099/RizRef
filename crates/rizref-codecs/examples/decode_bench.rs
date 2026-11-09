//! The runtime side of the binary-size trade-off: decode speed per opt-level.
//!
//! Phase 07 builds the release binary at `opt-level = "s"` to meet the 15 MB
//! budget. This example times what that could cost users — decoding a 12 MP
//! photo-like JPEG and PNG — so the choice is measured, not assumed:
//!
//! ```bash
//! cargo run --release -p rizref-codecs --example decode_bench
//! cargo run --release -p rizref-codecs --example decode_bench --config 'profile.release.opt-level="s"'
//! ```
//!
//! Each line is the best of five runs.

use std::time::{Duration, Instant};

use rizref_codecs::{ImageBuffer, decode, encode_jpeg, encode_png};

fn photo(width: u32, height: u32) -> ImageBuffer {
    let mut state = 0x2545_F491_u32;
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let grain = state & 0x1F;
            pixels.extend_from_slice(&[
                ((x * 255 / width + grain) % 256) as u8,
                ((y * 255 / height + grain) % 256) as u8,
                (((x + y) * 127 / (width + height) + grain) % 256) as u8,
                255,
            ]);
        }
    }
    ImageBuffer::from_rgba(width, height, pixels).expect("sized")
}

fn best(label: &str, mut body: impl FnMut()) -> Duration {
    let fastest = (0..5)
        .map(|_| {
            let start = Instant::now();
            body();
            start.elapsed()
        })
        .min()
        .expect("runs");
    println!("{label:<34}{:>9.1} ms", fastest.as_secs_f64() * 1000.0);
    fastest
}

fn main() {
    let image = photo(4000, 3000);
    let jpeg = encode_jpeg(&image, 90).expect("jpeg");
    let png = encode_png(&image).expect("png");
    println!(
        "12 MP test image: JPEG {:.1} MB, PNG {:.1} MB",
        jpeg.len() as f64 / 1e6,
        png.len() as f64 / 1e6
    );
    best("decode JPEG 4000x3000", || {
        std::hint::black_box(decode(&jpeg).expect("decode"));
    });
    best("decode PNG 4000x3000", || {
        std::hint::black_box(decode(&png).expect("decode"));
    });
    best("encode JPEG q90 (previews)", || {
        std::hint::black_box(encode_jpeg(&image, 90).expect("encode"));
    });
}
