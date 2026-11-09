//! Skeleton benchmark for the canvas maths.
//!
//! This is a placeholder that makes `cargo bench` meaningful from phase 00 on:
//! it times the pure functions the render loop calls for every visible item and
//! every input event, so a regression shows up as a number instead of a feeling.
//!
//! It deliberately does **not** measure any phase-07 budget. Cold start, frame
//! latency, board loading, saving and binary size all need the full application
//! and are verified in phase 07. The budgets themselves are recorded in
//! `README.md` under "Performance budgets".

use std::hint::black_box;
use std::time::Instant;

use glam::{DVec2, Vec2};
use rizref_core::{Camera, Rect, Scene, SceneId};

/// Iterations per measurement.
const ITERATIONS: u32 = 200_000;

/// Warms up, then times `body` and prints the result as nanoseconds per call.
fn time(label: &str, iterations: u32, mut body: impl FnMut() -> f64) {
    for _ in 0..iterations / 10 {
        black_box(body());
    }

    let start = Instant::now();
    let mut checksum = 0.0_f64;
    for _ in 0..iterations {
        checksum += black_box(body());
    }
    let elapsed = start.elapsed();

    let nanos_per_call = elapsed.as_secs_f64() * 1.0e9 / f64::from(iterations);
    println!("{label:<30}{nanos_per_call:9.2} ns/op  (checksum {checksum:.0})");
}

fn main() {
    let viewport = DVec2::new(1920.0, 1080.0);
    let item_rect = Rect::from_min_max(Vec2::new(-320.0, -240.0), Vec2::new(320.0, 240.0));
    let mut camera = Camera::new(DVec2::new(128.0, -64.0), 2.0, 0.2);
    let mut ticks = 0_u32;

    println!("rizref-core canvas benchmark ({ITERATIONS} iterations per line)");

    time("Camera::world_to_screen", ITERATIONS, || {
        ticks = ticks.wrapping_add(1);
        let world = DVec2::new(f64::from(ticks % 4096), f64::from(ticks % 2048));
        camera.world_to_screen(world, viewport).length()
    });

    time("Camera::screen_to_world", ITERATIONS, || {
        ticks = ticks.wrapping_add(1);
        let screen = DVec2::new(f64::from(ticks % 1920), f64::from(ticks % 1080));
        camera.screen_to_world(screen, viewport).length()
    });

    time("Camera::zoom_at", ITERATIONS, || {
        camera.zoom_at(DVec2::new(960.0, 540.0), 1.0001, viewport);
        camera.eye.length()
    });

    time("Camera::pan_by_screen", ITERATIONS, || {
        camera.pan_by_screen(DVec2::new(0.5, -0.25));
        camera.eye.length()
    });

    time("Camera::visible_world_rect", ITERATIONS, || {
        f64::from(camera.visible_world_rect(viewport).area())
    });

    time("Rect::intersects (culling)", ITERATIONS, || {
        let visible = camera.visible_world_rect(viewport);
        f64::from(u8::from(visible.intersects(&item_rect)))
    });

    // The phase-07 soak shape: 500 items on a 25 x 20 grid of 640 x 480 cells,
    // with the camera looking at a corner. The visible set is what the render
    // loop instances each frame, so this line is the per-frame culling cost.
    let mut scene = Scene::new(SceneId::from_bytes([7; SceneId::BYTE_LEN]));
    for index in 0..500_u16 {
        let center = Vec2::new(f32::from(index % 25) * 700.0, f32::from(index / 25) * 540.0);
        scene.insert_note("", center, Vec2::new(640.0, 480.0));
    }
    let soak_camera = Camera::new(DVec2::new(1_000.0, 800.0), 0.5, 0.0);
    let soak_visible = soak_camera.visible_world_rect(viewport);
    println!(
        "soak board: {} items, {} visible",
        scene.items().len(),
        scene.visible_items(&soak_visible).count()
    );
    time("Scene::visible_items (500)", ITERATIONS / 100, || {
        let visible = soak_camera.visible_world_rect(viewport);
        scene.visible_items(&visible).count() as f64
    });

    time("Rect::union (board bounds)", ITERATIONS, || {
        ticks = ticks.wrapping_add(1);
        let offset = (ticks % 1024) as f32;
        f64::from(
            item_rect
                .translated(Vec2::new(offset, 0.0))
                .union(&item_rect)
                .area(),
        )
    });

    time("Rect::contains (hit-test)", ITERATIONS, || {
        ticks = ticks.wrapping_add(1);
        let point = Vec2::new((ticks % 640) as f32 - 320.0, 12.0);
        f64::from(u8::from(item_rect.contains(point)))
    });
}
