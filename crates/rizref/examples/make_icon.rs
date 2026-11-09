//! Regenerates `crates/rizref/assets/rizref.ico`, the application and
//! `.rizref` file icon.
//!
//! The mark is drawn in code, so the icon has a reviewable source and no
//! binary art of unknown provenance: three reference cards fanned on a dark
//! rounded tile, the front one showing a tiny landscape. Every size is drawn
//! from the same vector description at its own resolution (no downscaled
//! bitmap), and stored as PNG inside the ICO, which Windows has read since
//! Vista.
//!
//! ```bash
//! cargo run -p rizref --example make_icon
//! ```

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// The sizes Explorer, the taskbar and the title bar ask for.
const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> tiny_skia::Path {
    let mut pb = PathBuilder::new();
    let k = 0.552_284_8 * r;
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().expect("closed path")
}

fn paint(rgba: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]));
    paint.anti_alias = true;
    paint
}

/// Draws the mark on a 256-unit design grid scaled to `size` pixels.
fn draw(size: u32) -> Pixmap {
    let mut pixmap = Pixmap::new(size, size).expect("non-zero size");
    let s = size as f32 / 256.0;
    let base = Transform::from_scale(s, s);

    // The tile.
    let tile = rounded_rect(8.0, 8.0, 240.0, 240.0, 52.0);
    pixmap.fill_path(
        &tile,
        &paint([31, 36, 48, 255]),
        FillRule::Winding,
        base,
        None,
    );

    // Three cards fanned around the tile centre.
    let card = rounded_rect(-62.0, -46.0, 124.0, 92.0, 12.0);
    let cards: [([u8; 4], f32, f32, f32); 3] = [
        ([58, 123, 213, 255], -16.0, 100.0, 108.0),
        ([224, 164, 58, 255], 9.0, 150.0, 116.0),
        ([242, 242, 242, 255], -3.0, 126.0, 150.0),
    ];
    for (index, (colour, degrees, cx, cy)) in cards.into_iter().enumerate() {
        let at = base.pre_translate(cx, cy).pre_rotate(degrees);
        // A soft shadow keeps the cards apart at small sizes.
        let shadow = at.pre_translate(0.0, 5.0);
        pixmap.fill_path(
            &card,
            &paint([0, 0, 0, 90]),
            FillRule::Winding,
            shadow,
            None,
        );
        pixmap.fill_path(&card, &paint(colour), FillRule::Winding, at, None);
        if index == 2 {
            // The front card is a picture: sky, a sun and two hills.
            let inner = rounded_rect(-52.0, -36.0, 104.0, 72.0, 6.0);
            pixmap.fill_path(
                &inner,
                &paint([120, 176, 228, 255]),
                FillRule::Winding,
                at,
                None,
            );
            let mut sun = PathBuilder::new();
            sun.push_circle(26.0, -14.0, 10.0);
            let sun = sun.finish().expect("circle");
            pixmap.fill_path(
                &sun,
                &paint([255, 214, 102, 255]),
                FillRule::Winding,
                at,
                None,
            );
            let mut hills = PathBuilder::new();
            hills.move_to(-52.0, 36.0);
            hills.line_to(-18.0, -6.0);
            hills.line_to(6.0, 18.0);
            hills.line_to(22.0, 4.0);
            hills.line_to(52.0, 36.0);
            hills.close();
            let hills = hills.finish().expect("hills");
            pixmap.fill_path(
                &hills,
                &paint([46, 102, 74, 255]),
                FillRule::Winding,
                at,
                None,
            );
        }
    }
    if size >= 48 {
        // A hairline rim on the large sizes; at 16–32 px it would only blur.
        let stroke = Stroke {
            width: 3.0,
            ..Stroke::default()
        };
        pixmap.stroke_path(&tile, &paint([255, 255, 255, 28]), &stroke, base, None);
    }
    pixmap
}

fn main() {
    let images: Vec<(u32, Vec<u8>)> = SIZES
        .iter()
        .map(|&size| (size, draw(size).encode_png().expect("png")))
        .collect();

    // ICONDIR, one ICONDIRENTRY per image, then the PNG payloads.
    let mut ico = Vec::new();
    ico.extend_from_slice(&0_u16.to_le_bytes());
    ico.extend_from_slice(&1_u16.to_le_bytes());
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, png) in &images {
        let side = if *size >= 256 { 0 } else { *size as u8 };
        ico.extend_from_slice(&[side, side, 0, 0]);
        ico.extend_from_slice(&1_u16.to_le_bytes());
        ico.extend_from_slice(&32_u16.to_le_bytes());
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in &images {
        ico.extend_from_slice(png);
    }

    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/rizref.ico");
    std::fs::create_dir_all(out.parent().expect("parent")).expect("assets dir");
    std::fs::write(&out, &ico).expect("write icon");
    println!(
        "wrote {} ({} bytes, {} sizes)",
        out.display(),
        ico.len(),
        images.len()
    );
}
