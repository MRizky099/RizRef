//! `rizref_thumb.dll` — the Windows Explorer thumbnail provider for `.rizref`.
//!
//! Explorer asks a registered *thumbnail handler* for a picture of a file. This
//! crate is that handler: a classic in-process COM server implementing
//! `IInitializeWithStream` + `IThumbnailProvider`, the same integration point
//! PureRef's thumbnail DLL uses for `.pur` files (observed in the study).
//!
//! The work is small by design of the format (phase 06): open the container
//! through the stream, follow the footer to the index, decode the `THMB` chunk
//! — or, for a board without one, composite the stored per-asset previews —
//! and scale the picture to the size Explorer asked for. Original images are
//! never read, so a 2 GB board costs a few small reads. Notes appear as cards
//! in their stored background colour (the 06a `background` field), because
//! both paths go through `rizref-format`'s compositor.
//!
//! Layout:
//! - [`render`] and [`scale_to_fit`]: the pure picture pipeline, unit-tested;
//! - `com` (Windows only): the COM classes, the stream adapter and the two DLL
//!   exports Explorer calls (`DllGetClassObject`, `DllCanUnloadNow`).
//!
//! Registration lives in the application (`rizref register`), which writes the
//! `CLSID` below under `HKCU\Software\Classes` (or `HKLM` for the installer).

// MSVC's linker notes (LNK4104) that the two COM exports "should be PRIVATE",
// i.e. kept out of the import library. Nothing links against this DLL — COM
// finds the exports by name through `LoadLibrary` — so the note is harmless,
// and rustc writes the export list itself, leaving no `.def` file to mark.
#![allow(linker_messages)]

use std::io::{Read, Seek};

use rizref_codecs::ImageBuffer;
use rizref_format::BoardReader;

#[cfg(windows)]
mod com;

/// The provider's class id, as Explorer and the registry spell it.
pub const CLSID_STRING: &str = "{6F1A3C52-8E4B-4D2A-9C7E-3B5D2F18A4C9}";

/// The provider's class id, as a 128-bit value (`CLSID_STRING` in numbers).
pub const CLSID_U128: u128 = 0x6F1A3C52_8E4B_4D2A_9C7E_3B5D2F18A4C9;

/// The shell's thumbnail-handler category, the `ShellEx` subkey a file type
/// names its provider under.
pub const THUMBNAIL_HANDLER_CATEGORY: &str = "{e357fccd-a995-4576-b01f-234630154e96}";

/// Background a board is composited onto when it has no embedded thumbnail:
/// the application's dark canvas colour.
pub const FALLBACK_BACKGROUND: [u8; 4] = [24, 24, 28, 255];

/// A thumbnail ready for a DIB section: premultiplied BGRA, top-down rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes of premultiplied B, G, R, A.
    pub bgra: Vec<u8>,
    /// Whether any pixel is not fully opaque (Explorer's `WTSAT_ARGB`).
    pub has_alpha: bool,
}

/// Reads the board in `source` and renders its thumbnail fitted into a
/// `max_side` square, the contract of `IThumbnailProvider::GetThumbnail`.
pub fn render<S: Read + Seek>(source: S, max_side: u32) -> Result<Thumbnail, String> {
    let mut reader = BoardReader::open(source, "shell stream").map_err(|e| e.to_string())?;
    let image = reader
        .thumbnail_image(FALLBACK_BACKGROUND)
        .map_err(|e| e.to_string())?;
    Ok(to_bgra(&scale_to_fit(&image, max_side)))
}

/// Scales `image` so its longer side is `max_side`, keeping the aspect ratio.
///
/// Downscaling averages every source pixel that falls in a destination pixel
/// (a box filter, with partial coverage weighted), which is what keeps a
/// 512 px thumbnail legible at 96 px; upscaling is bilinear. A zero
/// `max_side` is treated as one.
pub fn scale_to_fit(image: &ImageBuffer, max_side: u32) -> ImageBuffer {
    let (sw, sh) = (image.width().max(1), image.height().max(1));
    let max_side = max_side.max(1);
    let scale = f64::from(max_side) / f64::from(sw.max(sh));
    let dw = ((f64::from(sw) * scale).round() as u32).clamp(1, max_side);
    let dh = ((f64::from(sh) * scale).round() as u32).clamp(1, max_side);
    if (dw, dh) == (sw, sh) {
        return image.clone();
    }
    let src = image.pixels();
    let fetch = |x: u32, y: u32| -> [f64; 4] {
        let i = ((y.min(sh - 1) * sw + x.min(sw - 1)) * 4) as usize;
        let a = f64::from(src[i + 3]) / 255.0;
        // Premultiply while filtering so transparent pixels do not bleed
        // their (meaningless) colour into the edges.
        [
            f64::from(src[i]) * a,
            f64::from(src[i + 1]) * a,
            f64::from(src[i + 2]) * a,
            a,
        ]
    };
    let mut out = Vec::with_capacity((dw * dh * 4) as usize);
    let (fx, fy) = (f64::from(sw) / f64::from(dw), f64::from(sh) / f64::from(dh));
    for y in 0..dh {
        for x in 0..dw {
            let mut acc = [0.0; 4];
            if fx > 1.0 || fy > 1.0 {
                // Box filter over the source footprint of this pixel.
                let (x0, x1) = (f64::from(x) * fx, f64::from(x + 1) * fx);
                let (y0, y1) = (f64::from(y) * fy, f64::from(y + 1) * fy);
                let mut weight = 0.0;
                let mut sy = y0.floor() as u32;
                while f64::from(sy) < y1 && sy < sh {
                    let wy = (y1.min(f64::from(sy + 1)) - y0.max(f64::from(sy))).max(0.0);
                    let mut sx = x0.floor() as u32;
                    while f64::from(sx) < x1 && sx < sw {
                        let wx = (x1.min(f64::from(sx + 1)) - x0.max(f64::from(sx))).max(0.0);
                        let p = fetch(sx, sy);
                        let w = wx * wy;
                        for c in 0..4 {
                            acc[c] += p[c] * w;
                        }
                        weight += w;
                        sx += 1;
                    }
                    sy += 1;
                }
                if weight > 0.0 {
                    for value in &mut acc {
                        *value /= weight;
                    }
                }
            } else {
                // Bilinear upscale, pixel centres aligned.
                let gx = ((f64::from(x) + 0.5) * fx - 0.5).max(0.0);
                let gy = ((f64::from(y) + 0.5) * fy - 0.5).max(0.0);
                let (ix, iy) = (gx.floor() as u32, gy.floor() as u32);
                let (tx, ty) = (gx - f64::from(ix), gy - f64::from(iy));
                let (a, b) = (fetch(ix, iy), fetch(ix + 1, iy));
                let (c, d) = (fetch(ix, iy + 1), fetch(ix + 1, iy + 1));
                for k in 0..4 {
                    let top = a[k] + (b[k] - a[k]) * tx;
                    let bottom = c[k] + (d[k] - c[k]) * tx;
                    acc[k] = top + (bottom - top) * ty;
                }
            }
            let alpha = acc[3];
            let un = |v: f64| {
                if alpha > 0.0 {
                    (v / alpha).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                }
            };
            out.extend_from_slice(&[
                un(acc[0]),
                un(acc[1]),
                un(acc[2]),
                (alpha * 255.0).round().clamp(0.0, 255.0) as u8,
            ]);
        }
    }
    ImageBuffer::from_rgba(dw, dh, out).expect("buffer sized from its dimensions")
}

/// Converts straight RGBA to premultiplied BGRA, the 32-bit DIB layout.
pub fn to_bgra(image: &ImageBuffer) -> Thumbnail {
    let mut bgra = Vec::with_capacity(image.pixels().len());
    let mut has_alpha = false;
    for px in image.pixels().chunks_exact(4) {
        let a = u16::from(px[3]);
        has_alpha |= px[3] != 255;
        let pm = |c: u8| ((u16::from(c) * a + 127) / 255) as u8;
        bgra.extend_from_slice(&[pm(px[2]), pm(px[1]), pm(px[0]), px[3]]);
    }
    Thumbnail {
        width: image.width(),
        height: image.height(),
        bgra,
        has_alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::{CLSID_STRING, CLSID_U128, scale_to_fit, to_bgra};
    use rizref_codecs::ImageBuffer;

    #[test]
    fn the_class_id_spellings_agree() {
        let hex: String = CLSID_STRING
            .chars()
            .filter(char::is_ascii_hexdigit)
            .collect();
        assert_eq!(u128::from_str_radix(&hex, 16), Ok(CLSID_U128));
    }

    #[test]
    fn scaling_keeps_the_aspect_and_hits_the_requested_side() {
        let wide = ImageBuffer::filled(512, 256, [10, 20, 30, 255]);

        let small = scale_to_fit(&wide, 96);
        assert_eq!((small.width(), small.height()), (96, 48));
        assert_eq!(small.pixel(10, 10), Some([10, 20, 30, 255]));

        let large = scale_to_fit(&ImageBuffer::filled(40, 80, [1, 2, 3, 255]), 256);
        assert_eq!((large.width(), large.height()), (128, 256));
    }

    #[test]
    fn downscaling_averages_rather_than_samples() {
        // A 2 x 1 black/white image shrunk to one pixel is mid-grey.
        let image =
            ImageBuffer::from_rgba(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255]).expect("buffer");

        let one = scale_to_fit(&image, 1);

        let [r, g, b, a] = one.pixel(0, 0).expect("pixel");
        assert_eq!(a, 255);
        assert!((127..=128).contains(&r) && r == g && g == b, "got {r}");
    }

    #[test]
    fn transparent_pixels_do_not_bleed_colour() {
        // Opaque red next to transparent green: the average must stay red,
        // half covered.
        let image =
            ImageBuffer::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 0]).expect("buffer");

        let [r, g, _, a] = scale_to_fit(&image, 1).pixel(0, 0).expect("pixel");

        assert_eq!((r, g), (255, 0));
        assert!((127..=128).contains(&a));
    }

    #[test]
    fn bgra_is_premultiplied_and_reports_alpha() {
        let image = ImageBuffer::from_rgba(2, 1, vec![200, 100, 50, 255, 200, 100, 50, 128])
            .expect("buffer");

        let thumb = to_bgra(&image);

        assert_eq!(&thumb.bgra[..4], &[50, 100, 200, 255]);
        assert_eq!(&thumb.bgra[4..], &[25, 50, 100, 128]);
        assert!(thumb.has_alpha);
        assert!(!to_bgra(&ImageBuffer::filled(1, 1, [9, 9, 9, 255])).has_alpha);
    }

    #[test]
    fn a_saved_board_renders_through_a_stream() {
        use rizref_core::{Camera, Scene, SceneId};
        let mut scene = Scene::new(SceneId::random());
        let note = scene.insert_note("n", glam_vec(0.0, 0.0), glam_vec(200.0, 100.0));
        let mut content = scene
            .item(note)
            .and_then(rizref_core::Item::note_content)
            .expect("note");
        content.background = Some([220, 30, 30]);
        scene.item_mut(note).expect("note").set_note(&content);
        let dir = std::env::temp_dir().join(format!("rizref-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("b.rizref");
        rizref_format::save(
            &path,
            &rizref_format::SaveRequest::new(&scene, Camera::default(), "b"),
        )
        .expect("save");

        let file = std::fs::File::open(&path).expect("open");
        let thumb = super::render(file, 64).expect("render");

        assert_eq!((thumb.width, thumb.height), (64, 32));
        // The middle of the card is the stored red, as BGRA.
        let i = ((16 * 64 + 32) * 4) as usize;
        let [b, g, r, a] = [
            thumb.bgra[i],
            thumb.bgra[i + 1],
            thumb.bgra[i + 2],
            thumb.bgra[i + 3],
        ];
        assert!(
            r > 180 && g < 80 && b < 80 && a == 255,
            "got {r} {g} {b} {a}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn glam_vec(x: f32, y: f32) -> glam::Vec2 {
        glam::Vec2::new(x, y)
    }
}
