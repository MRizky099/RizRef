//! Phase-07 budget benchmark: loading and saving a 100-item board.
//!
//! The budgets (README, "Performance budgets"):
//!
//! - loading a 100-item board of ~200 MB originals reaches the first
//!   interactive frame in < 1 s using the embedded previews;
//! - saving that board incrementally takes < 500 ms (unchanged blobs are not
//!   rewritten).
//!
//! This bench measures the file-format half of both, without a window: it
//! builds such a board (100 assets of 2 MiB each — incompressible bytes, so
//! LZ4 and BLAKE3 pay full price — each with a 512 px preview), writes it
//! fresh, then times the operations the application performs:
//!
//! - `read_summary` — what the start view and recents read per card;
//! - `load` — the whole open: index, document, every blob read and hashed,
//!   every preview decoded, the scene rebuilt (the application's
//!   `open_board_now` is this plus the preview uploads, and its first frame is
//!   logged as "board interactive" and reported by `--bench-frames`);
//! - incremental save after moving one item (the common Ctrl+S);
//! - incremental save after adding one 2 MiB image.
//!
//! Each line is the best of `RUNS` warm runs; the board lives in the system
//! temp directory and is deleted at the end.
//!
//! ```bash
//! cargo bench -p rizref-format --bench board_io
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec2;
use rizref_codecs::{ImageBuffer, ImageFormat};
use rizref_core::{Asset, Camera, Scene, SceneId, Transform};
use rizref_format::{SaveRequest, load, read_summary, save};

/// Items (one asset each) on the board.
const ITEMS: usize = 100;
/// Bytes per original: 100 × 2 MiB ≈ 210 MB, the budget's "~200 MB".
const ORIGINAL_BYTES: usize = 2 * 1024 * 1024;
/// Warm runs per measurement; the best is reported.
const RUNS: usize = 5;

/// Deterministic incompressible bytes (xorshift), so LZ4 cannot cheat.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(len);
    out
}

/// A 512 × 384 preview with some structure, as a real one would have.
fn preview(seed: u64) -> ImageBuffer {
    let (w, h) = (512_u32, 384_u32);
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let s = seed as u32;
            pixels.extend_from_slice(&[
                ((x + s * 7) % 256) as u8,
                ((y + s * 13) % 256) as u8,
                ((x ^ y).wrapping_add(s) % 256) as u8,
                255,
            ]);
        }
    }
    ImageBuffer::from_rgba(w, h, pixels).expect("sized")
}

fn asset(seed: u64) -> Asset {
    let original = ImageBuffer::filled(4000, 3000, [0, 0, 0, 255]);
    Asset::from_decoded(
        Arc::from(noise(seed, ORIGINAL_BYTES)),
        ImageFormat::Jpeg,
        &original,
        preview(seed),
    )
}

fn best(label: &str, runs: usize, mut body: impl FnMut() -> Duration) -> Duration {
    let fastest = (0..runs).map(|_| body()).min().expect("at least one run");
    println!("{label:<44}{:>10.1} ms", fastest.as_secs_f64() * 1000.0);
    fastest
}

fn main() {
    let dir = std::env::temp_dir().join(format!("rizref-board-io-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("board.rizref");

    let mut scene = Scene::new(SceneId::random());
    for index in 0..ITEMS {
        let (id, _) = scene.insert_asset(asset(index as u64 + 1));
        let center = Vec2::new((index % 10) as f32 * 450.0, (index / 10) as f32 * 340.0);
        scene
            .insert_image(id, center, Vec2::new(400.0, 300.0))
            .expect("asset");
    }
    let camera = Camera::default();
    let originals = ITEMS * ORIGINAL_BYTES;
    println!(
        "board: {ITEMS} items, {:.0} MB of originals, best of {RUNS}",
        originals as f64 / 1e6
    );

    best("fresh save (Save As)", RUNS.min(3), || {
        let mut request = SaveRequest::new(&scene, camera, "bench");
        request.save_as = true;
        let start = Instant::now();
        save(&path, &request).expect("fresh save");
        start.elapsed()
    });
    let file_len = std::fs::metadata(&path).map_or(0, |m| m.len());
    println!("{:<44}{:>10.1} MB", "file size", file_len as f64 / 1e6);

    best("read_summary (start-view card)", RUNS, || {
        let start = Instant::now();
        let summary = read_summary(&path).expect("summary");
        std::hint::black_box(summary);
        start.elapsed()
    });

    best("load (blobs hashed, previews decoded)", RUNS, || {
        let start = Instant::now();
        let loaded = load(&path).expect("load");
        assert_eq!(loaded.scene.items().len(), ITEMS);
        start.elapsed()
    });

    // Incremental saves: each run changes the scene a little, as an edit does.
    let first = scene.items()[0].id();
    let mut nudge = 0.0_f32;
    best("incremental save, one item moved", RUNS, || {
        nudge += 10.0;
        let item = scene.item_mut(first).expect("item");
        item.set_transform(Transform::new(
            Vec2::new(nudge, 0.0),
            Vec2::new(400.0, 300.0),
        ));
        let request = SaveRequest::new(&scene, camera, "bench");
        let start = Instant::now();
        let report = save(&path, &request).expect("incremental save");
        let elapsed = start.elapsed();
        std::hint::black_box(report);
        elapsed
    });

    let mut seed = 10_000_u64;
    best("incremental save, one 2 MiB image added", RUNS, || {
        seed += 1;
        let (id, _) = scene.insert_asset(asset(seed));
        scene
            .insert_image(id, Vec2::new(-500.0, seed as f32), Vec2::new(400.0, 300.0))
            .expect("asset");
        let request = SaveRequest::new(&scene, camera, "bench");
        let start = Instant::now();
        save(&path, &request).expect("incremental save");
        start.elapsed()
    });

    let _ = std::fs::remove_dir_all(&dir);
}
