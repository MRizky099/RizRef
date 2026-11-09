//! Regenerates the two icons embedded in `rizref.exe` from the source artwork:
//!
//! - `assets/app_icon.png` → `assets/rizref.ico`, the application icon
//!   (`RT_GROUP_ICON` 1: the exe, taskbar, title bar, Alt-Tab, installer and
//!   jump list);
//! - `assets/app_document_icon.png` → `assets/rizref_document.ico`, the
//!   `.rizref` file icon (`RT_GROUP_ICON` 2, `DefaultIcon = rizref.exe,1`),
//!   which the Explorer thumbnail DLL also shows for a board with no picture.
//!
//! Each source is trimmed to its opaque artwork, centred on a transparent
//! square with a small margin (the document page is taller than wide and is
//! padded, never stretched), and resampled to every size with a Lanczos-3
//! filter in premultiplied colour, so transparent pixels never bleed into the
//! edges. The small sizes (32 px and under) get a light unsharp mask, which
//! keeps the outlines from going soft. Every size is stored as PNG inside the
//! ICO, which Windows has read since Vista. Pure Rust: the PNGs go through
//! `rizref-codecs`.
//!
//! ```bash
//! cargo run -p rizref --example make_icon
//! ```
//!
//! With `--sheet <out.png>` it also writes a contact sheet of the 16, 32, 48
//! and 256 px renditions of both icons on light and dark backgrounds.

use std::path::{Path, PathBuf};

use rizref_codecs::ImageBuffer;

/// The sizes Explorer, the taskbar, the title bar and Alt-Tab ask for, at
/// 100–250% display scaling.
const SIZES: [u32; 10] = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

/// Transparent margin around the artwork, as a fraction of the square side.
const MARGIN: f32 = 0.03;

/// Sizes at or under this get the unsharp mask.
const SHARPEN_UP_TO: u32 = 32;

/// Sizes at or under these use the simplified mark: at 16–24 px the column
/// of reference tiles and the lens contents are noise, so the app icon keeps
/// only the magnifier and its star; the document's small inner mark is noise
/// up to 32 px, so it shows that magnifier large on its page instead.
const APP_SIMPLIFY_UP_TO: u32 = 24;
const DOCUMENT_SIMPLIFY_UP_TO: u32 = 32;

/// The magnifier in `app_icon.png` (1254 px), in source pixels: the lens
/// circle with its rim, the handle as a capsule, and the star.
const LENS: ([f32; 2], f32) = ([695.0, 575.0], 401.0);
const HANDLE: ([f32; 2], [f32; 2], f32) = ([800.0, 860.0], [1060.0, 1150.0], 82.0);
const STAR: ([f32; 2], f32) = ([965.0, 258.0], 132.0);

/// In `app_document_icon.png` (1086 x 1448): the small mark on the page, the
/// page's fill colour that covers it, and where the simplified icon puts the
/// large magnifier instead (inside the page, clear of the folded corner).
const DOC_MARK: [usize; 4] = [270, 600, 830, 1225];
const DOC_PAGE_FILL: [u8; 4] = [1, 35, 79, 255];
const DOC_MAGNIFIER: [usize; 4] = [241, 470, 607, 780];

/// Premultiplied RGBA in 0..=1, row-major.
struct Canvas {
    width: usize,
    height: usize,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn from_image(image: &ImageBuffer) -> Self {
        let px = image
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let a = f32::from(p[3]) / 255.0;
                [
                    f32::from(p[0]) / 255.0 * a,
                    f32::from(p[1]) / 255.0 * a,
                    f32::from(p[2]) / 255.0 * a,
                    a,
                ]
            })
            .collect();
        Self {
            width: image.width() as usize,
            height: image.height() as usize,
            px,
        }
    }

    fn to_image(&self) -> ImageBuffer {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            let a = p[3].clamp(0.0, 1.0);
            let un = |c: f32| {
                if a > 0.0 {
                    ((c / a).clamp(0.0, 1.0) * 255.0).round() as u8
                } else {
                    0
                }
            };
            out.extend_from_slice(&[un(p[0]), un(p[1]), un(p[2]), (a * 255.0).round() as u8]);
        }
        ImageBuffer::from_rgba(self.width as u32, self.height as u32, out).expect("sized buffer")
    }

    /// The bounding box of pixels with visible alpha.
    fn opaque_bounds(&self) -> (usize, usize, usize, usize) {
        let (mut x0, mut y0, mut x1, mut y1) = (self.width, self.height, 0, 0);
        for y in 0..self.height {
            for x in 0..self.width {
                if self.px[y * self.width + x][3] > 2.0 / 255.0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        assert!(x1 > x0 && y1 > y0, "the artwork is fully transparent");
        (x0, y0, x1, y1)
    }

    /// The artwork trimmed and centred on a transparent square.
    fn squared(&self) -> Self {
        let (x0, y0, x1, y1) = self.opaque_bounds();
        let (w, h) = (x1 - x0, y1 - y0);
        let inner = w.max(h);
        let side = (inner as f32 / (1.0 - 2.0 * MARGIN)).round() as usize;
        let (ox, oy) = ((side - w) / 2, (side - h) / 2);
        let mut px = vec![[0.0; 4]; side * side];
        for y in 0..h {
            for x in 0..w {
                px[(y + oy) * side + x + ox] = self.px[(y + y0) * self.width + x + x0];
            }
        }
        Self {
            width: side,
            height: side,
            px,
        }
    }

    /// Keeps only the pixels inside `inside` (with a 2 px soft edge measured
    /// by the signed distance it returns: negative inside).
    fn masked(&self, distance: impl Fn(f32, f32) -> f32) -> Self {
        let mut px = self.px.clone();
        for y in 0..self.height {
            for x in 0..self.width {
                let d = distance(x as f32 + 0.5, y as f32 + 0.5);
                let keep = (0.5 - d / 2.0).clamp(0.0, 1.0);
                for c in &mut px[y * self.width + x] {
                    *c *= keep;
                }
            }
        }
        Self {
            width: self.width,
            height: self.height,
            px,
        }
    }

    /// The opaque artwork only, cropped to its bounds.
    fn trimmed(&self) -> Self {
        let (x0, y0, x1, y1) = self.opaque_bounds();
        let width = x1 - x0;
        let px = (y0..y1)
            .flat_map(|y| self.px[y * self.width + x0..y * self.width + x1].to_vec())
            .collect();
        Self {
            width,
            height: y1 - y0,
            px,
        }
    }

    fn fill_rect(&mut self, [x0, y0, x1, y1]: [usize; 4], rgba: [u8; 4]) {
        let a = f32::from(rgba[3]) / 255.0;
        let colour = [
            f32::from(rgba[0]) / 255.0 * a,
            f32::from(rgba[1]) / 255.0 * a,
            f32::from(rgba[2]) / 255.0 * a,
            a,
        ];
        for y in y0..y1 {
            for x in x0..x1 {
                self.px[y * self.width + x] = colour;
            }
        }
    }

    /// Composites premultiplied `src` over `self` at `(x0, y0)`.
    fn draw(&mut self, src: &Self, x0: usize, y0: usize) {
        for y in 0..src.height {
            for x in 0..src.width {
                let s = src.px[y * src.width + x];
                let d = &mut self.px[(y0 + y) * self.width + x0 + x];
                for c in 0..4 {
                    d[c] = s[c] + d[c] * (1.0 - s[3]);
                }
            }
        }
    }

    fn resized(&self, width: usize, height: usize) -> Self {
        let horizontal = resample_rows(self, width);
        resample_columns(&horizontal, height)
    }

    /// A light unsharp mask: original + amount × (original − 3×3 blur).
    fn sharpened(&self, amount: f32) -> Self {
        let (w, h) = (self.width, self.height);
        let at = |x: isize, y: isize| {
            let x = x.clamp(0, w as isize - 1) as usize;
            let y = y.clamp(0, h as isize - 1) as usize;
            self.px[y * w + x]
        };
        let kernel = [1.0, 2.0, 1.0];
        let mut px = Vec::with_capacity(self.px.len());
        for y in 0..h as isize {
            for x in 0..w as isize {
                let mut blur = [0.0; 4];
                for (dy, ky) in kernel.iter().enumerate() {
                    for (dx, kx) in kernel.iter().enumerate() {
                        let p = at(x + dx as isize - 1, y + dy as isize - 1);
                        for c in 0..4 {
                            blur[c] += p[c] * kx * ky / 16.0;
                        }
                    }
                }
                let p = at(x, y);
                let mut out = [0.0; 4];
                for c in 0..4 {
                    out[c] = (p[c] + amount * (p[c] - blur[c])).clamp(0.0, 1.0);
                }
                // Premultiplied colour may not exceed its alpha.
                for c in 0..3 {
                    out[c] = out[c].min(out[3]);
                }
                px.push(out);
            }
        }
        Self {
            width: w,
            height: h,
            px,
        }
    }
}

fn lanczos3(x: f32) -> f32 {
    let x = x.abs();
    if x < 1.0e-6 {
        1.0
    } else if x < 3.0 {
        let pi_x = std::f32::consts::PI * x;
        3.0 * pi_x.sin() * (pi_x / 3.0).sin() / (pi_x * pi_x)
    } else {
        0.0
    }
}

/// Normalised Lanczos-3 weights for each destination sample along one axis.
fn weights(source: usize, dest: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = source as f32 / dest as f32;
    let support = 3.0 * scale.max(1.0);
    let stretch = scale.max(1.0);
    (0..dest)
        .map(|i| {
            let centre = (i as f32 + 0.5) * scale;
            let first = ((centre - support).floor().max(0.0)) as usize;
            let last = ((centre + support).ceil() as usize).min(source);
            let mut taps: Vec<f32> = (first..last)
                .map(|s| lanczos3((s as f32 + 0.5 - centre) / stretch))
                .collect();
            let sum: f32 = taps.iter().sum();
            for tap in &mut taps {
                *tap /= sum;
            }
            (first, taps)
        })
        .collect()
}

fn resample_rows(src: &Canvas, width: usize) -> Canvas {
    let table = weights(src.width, width);
    let mut px = vec![[0.0; 4]; width * src.height];
    for y in 0..src.height {
        for (x, (first, taps)) in table.iter().enumerate() {
            let mut acc = [0.0; 4];
            for (k, tap) in taps.iter().enumerate() {
                let p = src.px[y * src.width + first + k];
                for c in 0..4 {
                    acc[c] += p[c] * tap;
                }
            }
            px[y * width + x] = acc.map(|v| v.clamp(0.0, 1.0));
        }
    }
    Canvas {
        width,
        height: src.height,
        px,
    }
}

fn resample_columns(src: &Canvas, height: usize) -> Canvas {
    let table = weights(src.height, height);
    let mut px = vec![[0.0; 4]; src.width * height];
    for (y, (first, taps)) in table.iter().enumerate() {
        for x in 0..src.width {
            let mut acc = [0.0; 4];
            for (k, tap) in taps.iter().enumerate() {
                let p = src.px[(first + k) * src.width + x];
                for c in 0..4 {
                    acc[c] += p[c] * tap;
                }
            }
            let mut out = acc.map(|v| v.clamp(0.0, 1.0));
            for c in 0..3 {
                out[c] = out[c].min(out[3]);
            }
            px[y * src.width + x] = out;
        }
    }
    Canvas {
        width: src.width,
        height,
        px,
    }
}

fn load(source: &Path) -> Canvas {
    let bytes = std::fs::read(source).unwrap_or_else(|e| panic!("{}: {e}", source.display()));
    Canvas::from_image(&rizref_codecs::decode(&bytes).expect("decodable PNG"))
}

/// The magnifier and star cut out of the app artwork.
fn magnifier(app: &Canvas) -> Canvas {
    app.masked(|x, y| {
        let circle = |([cx, cy], r): ([f32; 2], f32)| (x - cx).hypot(y - cy) - r;
        let ([ax, ay], [bx, by], r) = HANDLE;
        let (dx, dy) = (bx - ax, by - ay);
        let t = (((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        let capsule = (x - ax - t * dx).hypot(y - ay - t * dy) - r;
        circle(LENS).min(capsule).min(circle(STAR))
    })
    .trimmed()
}

/// The document page with the magnifier drawn large in place of its small
/// mark.
fn simplified_document(document: &Canvas, app: &Canvas) -> Canvas {
    let mut page = Canvas {
        width: document.width,
        height: document.height,
        px: document.px.clone(),
    };
    page.fill_rect(DOC_MARK, DOC_PAGE_FILL);
    let [x, y, w, h] = DOC_MAGNIFIER;
    let mark = magnifier(app);
    // Fit the cut-out into the box, keeping its aspect, centred.
    let scale = (w as f32 / mark.width as f32).min(h as f32 / mark.height as f32);
    let (mw, mh) = (
        (mark.width as f32 * scale).round() as usize,
        (mark.height as f32 * scale).round() as usize,
    );
    page.draw(&mark.resized(mw, mh), x + (w - mw) / 2, y + (h - mh) / 2);
    page
}

/// Every size of one icon: `full` above `simplify_up_to`, `simple` at and
/// below it.
fn renditions(full: &Canvas, simple: &Canvas, simplify_up_to: u32) -> Vec<(u32, ImageBuffer)> {
    let full = full.squared();
    let simple = simple.squared();
    SIZES
        .iter()
        .map(|&size| {
            let source = if size <= simplify_up_to {
                &simple
            } else {
                &full
            };
            let mut canvas = source.resized(size as usize, size as usize);
            if size <= SHARPEN_UP_TO {
                canvas = canvas.sharpened(if size <= 20 { 0.45 } else { 0.3 });
            }
            (size, canvas.to_image())
        })
        .collect()
}

/// An ICO file holding each image as PNG.
fn ico(images: &[(u32, ImageBuffer)]) -> Vec<u8> {
    let pngs: Vec<(u32, Vec<u8>)> = images
        .iter()
        .map(|(size, image)| (*size, rizref_codecs::encode_png(image).expect("png")))
        .collect();
    // ICONDIR, one ICONDIRENTRY per image, then the PNG payloads.
    let mut out = Vec::new();
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (size, png) in &pngs {
        let side = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1_u16.to_le_bytes());
        out.extend_from_slice(&32_u16.to_le_bytes());
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in &pngs {
        out.extend_from_slice(png);
    }
    out
}

/// Draws `image` straight-alpha over `canvas` (RGBA, `stride` pixels wide).
fn blit(canvas: &mut [u8], stride: usize, image: &ImageBuffer, x0: usize, y0: usize) {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let src = image.pixels();
    for y in 0..h {
        for x in 0..w {
            let s = (y * w + x) * 4;
            let d = ((y0 + y) * stride + x0 + x) * 4;
            let a = u32::from(src[s + 3]);
            for c in 0..3 {
                let over = u32::from(src[s + c]);
                let under = u32::from(canvas[d + c]);
                canvas[d + c] = ((over * a + under * (255 - a) + 127) / 255) as u8;
            }
        }
    }
}

/// The review sheet: for each icon a row of 16/32/48/256 px on light and on
/// dark, each small size also shown 4× enlarged (nearest neighbour) so the
/// pixels can be judged.
fn contact_sheet(icons: &[&[(u32, ImageBuffer)]], out: &Path) {
    let shown = [16_u32, 32, 48, 256];
    let backgrounds = [[243_u8, 243, 243], [32, 32, 36]];
    let pad = 16_usize;
    // Per background: actual sizes, then 4x zooms of 16/32/48.
    let row_width: usize = shown.iter().map(|&s| s as usize + pad).sum::<usize>()
        + [16_usize, 32, 48]
            .iter()
            .map(|s| s * 4 + pad)
            .sum::<usize>()
        + pad;
    let row_height = 256 + 2 * pad;
    let width = row_width;
    let height = row_height * icons.len() * backgrounds.len();
    let mut canvas = vec![0_u8; width * height * 4];
    for (row, (icon, bg)) in icons
        .iter()
        .flat_map(|icon| backgrounds.iter().map(move |bg| (icon, bg)))
        .enumerate()
    {
        let top = row * row_height;
        for y in top..top + row_height {
            for x in 0..width {
                let d = (y * width + x) * 4;
                canvas[d..d + 4].copy_from_slice(&[bg[0], bg[1], bg[2], 255]);
            }
        }
        let mut x = pad;
        for size in shown {
            let image = &icon.iter().find(|(s, _)| *s == size).expect("size").1;
            blit(
                &mut canvas,
                width,
                image,
                x,
                top + pad + (256 - size as usize) / 2,
            );
            x += size as usize + pad;
        }
        for size in [16_u32, 32, 48] {
            let image = &icon.iter().find(|(s, _)| *s == size).expect("size").1;
            let zoomed = zoom(image, 4);
            let side = size as usize * 4;
            blit(&mut canvas, width, &zoomed, x, top + pad + (256 - side) / 2);
            x += side + pad;
        }
    }
    let sheet = ImageBuffer::from_rgba(width as u32, height as u32, canvas).expect("sized");
    std::fs::create_dir_all(out.parent().expect("parent")).expect("sheet dir");
    std::fs::write(out, rizref_codecs::encode_png(&sheet).expect("png")).expect("write sheet");
    println!("wrote {}", out.display());
}

fn zoom(image: &ImageBuffer, factor: u32) -> ImageBuffer {
    let (w, h) = (image.width(), image.height());
    let src = image.pixels();
    let mut out = Vec::with_capacity((w * h * factor * factor * 4) as usize);
    for y in 0..h * factor {
        for x in 0..w * factor {
            let s = (((y / factor) * w + x / factor) * 4) as usize;
            out.extend_from_slice(&src[s..s + 4]);
        }
    }
    ImageBuffer::from_rgba(w * factor, h * factor, out).expect("sized")
}

fn main() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let app_art = load(&assets.join("app_icon.png"));
    let document_art = load(&assets.join("app_document_icon.png"));
    let app = renditions(&app_art, &magnifier(&app_art), APP_SIMPLIFY_UP_TO);
    let document = renditions(
        &document_art,
        &simplified_document(&document_art, &app_art),
        DOCUMENT_SIMPLIFY_UP_TO,
    );
    for (images, name) in [(&app, "rizref.ico"), (&document, "rizref_document.ico")] {
        let bytes = ico(images);
        let out = assets.join(name);
        std::fs::write(&out, &bytes).expect("write icon");
        println!(
            "wrote {} ({} bytes, {} sizes)",
            out.display(),
            bytes.len(),
            images.len()
        );
    }

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--sheet" {
            let out = PathBuf::from(args.next().expect("--sheet needs a path"));
            contact_sheet(&[&app, &document], &out);
        }
    }
}
