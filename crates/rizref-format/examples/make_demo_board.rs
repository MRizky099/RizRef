//! Regenerates `docs/demo.rizref`, the sample board for the README and the
//! landing screenshot.
//!
//! Every image on it is drawn here, in code — a value scale, a colour wheel, a
//! two-point perspective grid and a layered landscape study — so the board
//! carries no third-party artwork. The images are dedicated to the public
//! domain (CC0 1.0) along with this program's output. The notes exercise the
//! 06a markdown fields: headings, lists, a coloured card and inline math.
//!
//! ```bash
//! cargo run -p rizref-format --example make_demo_board
//! ```
//!
//! The output is deterministic except for the scene id and timestamps, so a
//! rerun changes the file's bytes but not what it shows.

use std::f32::consts::TAU;
use std::sync::Arc;

use glam::{DVec2, Vec2};
use rizref_codecs::{ImageBuffer, ImageFormat, encode_jpeg, encode_png};
use rizref_core::generate_preview;
use rizref_core::{Asset, Camera, NoteContent, Rect, Scene, SceneId};
use rizref_format::{SaveRequest, save};

fn image(width: u32, height: u32, shade: impl Fn(f32, f32) -> [u8; 4]) -> ImageBuffer {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let u = (x as f32 + 0.5) / width as f32;
            let v = (y as f32 + 0.5) / height as f32;
            pixels.extend_from_slice(&shade(u, v));
        }
    }
    ImageBuffer::from_rgba(width, height, pixels).expect("sized buffer")
}

fn hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i as i32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [u8; 4] {
    let c = |i: usize| (a[i] + (b[i] - a[i]) * t.clamp(0.0, 1.0)) as u8;
    [c(0), c(1), c(2), 255]
}

/// Ten flat value steps, black to white.
fn value_scale() -> ImageBuffer {
    image(1000, 140, |u, _| {
        let step = (u * 10.0).floor().min(9.0) / 9.0;
        let g = (step * 255.0).round() as u8;
        [g, g, g, 255]
    })
}

/// A hue/saturation wheel on a transparent square.
fn colour_wheel() -> ImageBuffer {
    image(560, 560, |u, v| {
        let (x, y) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
        let r = (x * x + y * y).sqrt();
        if r > 1.0 {
            return [0, 0, 0, 0];
        }
        let hue = (y.atan2(x) / TAU).rem_euclid(1.0);
        let [r8, g8, b8] = hsv(hue, r, 1.0);
        let edge = ((1.0 - r) * 280.0).min(1.0);
        [r8, g8, b8, (edge * 255.0) as u8]
    })
}

/// Two-point perspective: lines fanning from two vanishing points on a
/// horizon, over paper.
fn perspective_grid() -> ImageBuffer {
    let (w, h) = (900.0_f32, 600.0_f32);
    let horizon = 0.42;
    let vps = [(-0.15_f32, horizon), (1.15_f32, horizon)];
    image(900, 600, |u, v| {
        let paper = [244.0, 240.0, 228.0];
        let mut ink: f32 = 0.0;
        if ((v - horizon) * h).abs() < 1.2 {
            ink = 1.0;
        }
        for (vx, vy) in vps {
            let angle = ((v - vy) * h).atan2((u - vx) * w);
            let spacing = 0.035;
            let phase = (angle / spacing).rem_euclid(1.0);
            let dist = ((u - vx) * w).hypot((v - vy) * h);
            let width = (spacing * dist).max(1.0);
            let d = phase.min(1.0 - phase) * width;
            ink = ink.max((1.0 - d / 0.9).clamp(0.0, 1.0) * 0.55);
        }
        mix(paper, [70.0, 90.0, 140.0], ink)
    })
}

/// Atmospheric perspective: hills lighten and cool with distance.
fn landscape() -> ImageBuffer {
    image(960, 540, |u, v| {
        let sky = mix([250.0, 196.0, 140.0], [120.0, 160.0, 210.0], 1.0 - v * 1.6);
        let mut colour = [f32::from(sky[0]), f32::from(sky[1]), f32::from(sky[2])];
        let layers: [(f32, f32, f32, [f32; 3]); 4] = [
            (0.52, 0.05, 3.0, [150.0, 160.0, 190.0]),
            (0.60, 0.06, 5.0, [110.0, 124.0, 160.0]),
            (0.70, 0.07, 7.0, [70.0, 90.0, 110.0]),
            (0.82, 0.05, 11.0, [36.0, 54.0, 52.0]),
        ];
        for (base, amp, freq, tint) in layers {
            let ridge = base - amp * ((u * freq).sin() * 0.6 + (u * freq * 2.3 + 1.0).sin() * 0.4);
            if v > ridge {
                colour = tint;
            }
        }
        let sun = ((u - 0.72).powi(2) * 3.2 + (v - 0.33).powi(2)).sqrt();
        if sun < 0.06 && v < 0.5 {
            colour = [255.0, 236.0, 190.0];
        }
        [colour[0] as u8, colour[1] as u8, colour[2] as u8, 255]
    })
}

fn add(scene: &mut Scene, picture: &ImageBuffer, jpeg: bool, center: Vec2, width: f32) {
    let (bytes, format) = if jpeg {
        (encode_jpeg(picture, 88).expect("jpeg"), ImageFormat::Jpeg)
    } else {
        (encode_png(picture).expect("png"), ImageFormat::Png)
    };
    let asset = Asset::from_decoded(Arc::from(bytes), format, picture, generate_preview(picture));
    let (id, _) = scene.insert_asset(asset);
    let height = width * picture.height() as f32 / picture.width() as f32;
    scene
        .insert_image(id, center, Vec2::new(width, height))
        .expect("asset is on the board");
}

fn note(scene: &mut Scene, content: NoteContent, center: Vec2, size: Vec2) {
    let id = scene.insert_note("", center, size);
    scene.item_mut(id).expect("note").set_note(&content);
}

fn main() {
    let mut scene = Scene::new(SceneId::random());
    add(&mut scene, &landscape(), true, Vec2::new(0.0, 0.0), 960.0);
    add(
        &mut scene,
        &perspective_grid(),
        true,
        Vec2::new(940.0, -60.0),
        760.0,
    );
    add(
        &mut scene,
        &colour_wheel(),
        false,
        Vec2::new(-780.0, -120.0),
        420.0,
    );
    add(
        &mut scene,
        &value_scale(),
        false,
        Vec2::new(80.0, 400.0),
        900.0,
    );

    note(
        &mut scene,
        NoteContent::markdown(
            "# Demo board\n\nEverything here was drawn by code \
             (CC0). Try:\n\n- **wheel** to zoom, middle-drag to pan\n- \
             **F1** for shortcuts, **F3** for frame times\n- drop your own \
             references anywhere",
        ),
        Vec2::new(-780.0, 300.0),
        Vec2::new(380.0, 300.0),
    );
    let mut values = NoteContent::markdown(
        "## Values\n\nSquint: the far hills sit around step 6–7, \
         the foreground near 2. Contrast falls off as $C \\propto e^{-kd}$.",
    );
    values.background = Some([255, 236, 160]);
    note(
        &mut scene,
        values,
        Vec2::new(940.0, 400.0),
        Vec2::new(420.0, 220.0),
    );
    let mut perspective = NoteContent::markdown(
        "## Two-point\n\nVanishing points sit *off* the canvas, on the horizon.",
    );
    perspective.background = Some([46, 64, 110]);
    note(
        &mut scene,
        perspective,
        Vec2::new(1_480.0, -420.0),
        Vec2::new(300.0, 160.0),
    );

    let bounds = Rect::from_min_max(Vec2::new(-1_000.0, -520.0), Vec2::new(1_640.0, 560.0));
    let camera = Camera::fit_rect(bounds, DVec2::new(1_280.0, 720.0), 24.0);
    let request = SaveRequest::new(&scene, camera, "Demo board");

    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/demo.rizref");
    let report = save(&out, &request).expect("save the demo board");
    println!(
        "wrote {} ({} bytes, {} items)",
        out.display(),
        report.file_len,
        scene.items().len()
    );
}
