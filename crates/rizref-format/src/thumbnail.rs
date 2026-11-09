//! Board thumbnails and preview compositing.
//!
//! A `.rizref` file carries its own thumbnail so the start view (and later the
//! Explorer provider) can show what a board looks like without opening it. The
//! thumbnail is a bounding-box composite of the board's items, fitted into at
//! most [`MAX_THUMBNAIL_SIDE`] pixels.
//!
//! Compositing uses each asset's stored preview rather than its full original.
//! That is a deliberate trade: a preview is at most 512 px and is already
//! resident in memory, so saving a 500 MB board does not decode 500 MB of
//! pixels just to draw a picture 512 px wide. The result is saved as JPEG when
//! the board background is opaque, and PNG when a caller asks for transparency.
//!
//! Notes are drawn as cards in their stored background colour (or the default
//! thumbnail card when they have none); their text is not rendered. A
//! thumbnail is a navigation aid, not a readable export.

use glam::Vec2;
use rizref_codecs::{ImageBuffer, encode_jpeg, encode_png};
use rizref_core::{Item, Scene};
use tiny_skia::{Color, FilterQuality, IntSize, Paint, Pixmap, PixmapPaint, Transform};

use crate::document::scene_rect;
use crate::error::FormatError;
use crate::preview::Preview;

/// Longest side of a board thumbnail, in pixels.
pub const MAX_THUMBNAIL_SIDE: u32 = 512;

/// JPEG quality used for an opaque thumbnail.
pub const THUMBNAIL_QUALITY: u8 = 85;

/// Colour a note is drawn with in a thumbnail, before item opacity.
const NOTE_COLOUR: [u8; 4] = [228, 214, 160, 235];

/// Renders `scene` into a thumbnail of at most `max_side` pixels.
///
/// `background` is the canvas clear colour the application draws behind the
/// board. A fully transparent background selects PNG output; anything else is
/// encoded as JPEG, which is several times smaller for a picture that will be
/// shown a few hundred pixels wide.
pub fn generate(scene: &Scene, background: [u8; 4], max_side: u32) -> Result<Preview, FormatError> {
    let image = composite(scene, background, max_side, None)?;
    let opaque = background[3] == 255;
    let bytes = if opaque {
        encode_jpeg(&image, THUMBNAIL_QUALITY)
    } else {
        encode_png(&image)
    }
    .map_err(|error| FormatError::Preview {
        reason: error.to_string(),
    })?;
    Ok(Preview::new(image.width(), image.height(), bytes))
}

/// Composites the scene at a chosen scale, for thumbnails and image export.
///
/// `max_side` bounds the output when `exact_scale` is `None`; an exact scale is
/// used by export, where the caller chose a pixel size. Items are drawn in
/// z-order with their opacity, rotation and mirror flags.
pub fn composite(
    scene: &Scene,
    background: [u8; 4],
    max_side: u32,
    exact_scale: Option<f32>,
) -> Result<ImageBuffer, FormatError> {
    let Some(bounds) = scene_rect(scene) else {
        return Ok(ImageBuffer::filled(1, 1, background));
    };
    let width = f64::from(bounds.size.x);
    let height = f64::from(bounds.size.y);
    if width <= 0.0 || height <= 0.0 {
        return Ok(ImageBuffer::filled(1, 1, background));
    }

    let max_side = max_side.max(1);
    let scale = exact_scale.unwrap_or_else(|| {
        let fit = (f64::from(max_side) / width).min(f64::from(max_side) / height);
        // Do not magnify a small board: a 40 px sketch board should stay 40 px.
        fit.min(1.0) as f32
    });
    if !scale.is_finite() || scale <= 0.0 {
        return Err(FormatError::Preview {
            reason: format!("invalid composite scale {scale}"),
        });
    }

    let target_width = ((width as f32 * scale).ceil() as u32).max(1);
    let target_height = ((height as f32 * scale).ceil() as u32).max(1);
    let byte_len =
        ImageBuffer::byte_len(target_width, target_height).ok_or_else(|| FormatError::Preview {
            reason: format!("thumbnail {target_width} x {target_height} is too large"),
        })?;
    // A guard against an accidental huge export from an absurd scene rect.
    const MAX_COMPOSITE_BYTES: usize = 1 << 28;
    if byte_len > MAX_COMPOSITE_BYTES {
        return Err(FormatError::Preview {
            reason: format!(
                "composite of {target_width} x {target_height} exceeds the {} byte limit",
                MAX_COMPOSITE_BYTES
            ),
        });
    }

    let Some(mut pixmap) = Pixmap::new(target_width, target_height) else {
        return Err(FormatError::Preview {
            reason: format!("cannot allocate a {target_width} x {target_height} pixmap"),
        });
    };
    pixmap.fill(Color::from_rgba8(
        background[0],
        background[1],
        background[2],
        background[3],
    ));

    for item in scene.items() {
        draw_item(&mut pixmap, scene, item, bounds.min, scale);
    }

    let straight = straight_rgba(&pixmap);
    ImageBuffer::from_rgba(target_width, target_height, straight).ok_or_else(|| {
        FormatError::Preview {
            reason: "composited pixel buffer has the wrong length".to_owned(),
        }
    })
}

/// Draws one item into `pixmap`.
fn draw_item(pixmap: &mut Pixmap, scene: &Scene, item: &Item, origin: Vec2, scale: f32) {
    let center = (item.center() - origin) * scale;
    let size = item.size() * scale;
    let rotation_deg = item.rotation().to_degrees();
    let opacity = item.opacity().clamp(0.0, 1.0);
    if opacity <= 0.0 || size.x <= 0.0 || size.y <= 0.0 {
        return;
    }

    if let Some(asset_id) = item.asset() {
        let Some(asset) = scene.asset(asset_id) else {
            return;
        };
        let preview = asset.preview().image();
        let Some(preview_pixmap) = premultiplied(preview) else {
            return;
        };
        let (px, py) = (preview.width() as f32, preview.height() as f32);
        let mut transform = Transform::from_translate(center.x, center.y)
            .pre_concat(Transform::from_rotate(rotation_deg));
        if item.flip_x() {
            transform = transform.pre_concat(Transform::from_scale(-1.0, 1.0));
        }
        if item.flip_y() {
            transform = transform.pre_concat(Transform::from_scale(1.0, -1.0));
        }
        transform = transform
            .pre_concat(Transform::from_scale(size.x / px, size.y / py))
            .pre_concat(Transform::from_translate(-px * 0.5, -py * 0.5));
        let paint = PixmapPaint {
            opacity,
            blend_mode: tiny_skia::BlendMode::SourceOver,
            quality: FilterQuality::Bilinear,
        };
        pixmap.draw_pixmap(0, 0, preview_pixmap.as_ref(), &paint, transform, None);
    } else {
        // A note: a card without its text, rotated with the item.
        let Some(rect) = tiny_skia::Rect::from_xywh(-size.x * 0.5, -size.y * 0.5, size.x, size.y)
        else {
            return;
        };
        let (fill, border) = note_card_colours(item);
        let transform = Transform::from_translate(center.x, center.y)
            .pre_concat(Transform::from_rotate(rotation_deg));
        let mut paint = Paint {
            shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                fill[0],
                fill[1],
                fill[2],
                (f32::from(fill[3]) * opacity).round() as u8,
            )),
            ..Paint::default()
        };
        pixmap.fill_rect(rect, &paint, transform, None);
        if let Some(border) = border {
            paint.shader = tiny_skia::Shader::SolidColor(Color::from_rgba8(
                border[0],
                border[1],
                border[2],
                (255.0 * opacity).round() as u8,
            ));
            let path = tiny_skia::PathBuilder::from_rect(rect);
            let stroke = tiny_skia::Stroke {
                width: 1.0,
                ..tiny_skia::Stroke::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, transform, None);
        }
    }
}

/// The fill and border colours of a note card. A note with a stored
/// background is painted in that opaque colour with a border nudged towards
/// its contrasting ink; a note without one keeps the phase-06 card.
fn note_card_colours(item: &Item) -> ([u8; 4], Option<[u8; 3]>) {
    match item.note_content().and_then(|note| note.background) {
        Some([r, g, b]) => {
            let luminance = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
            let toward: f32 = if luminance < 128.0 { 255.0 } else { 0.0 };
            let mix = |channel: u8| (f32::from(channel) * 0.75 + toward * 0.25).round() as u8;
            ([r, g, b, 255], Some([mix(r), mix(g), mix(b)]))
        }
        None => (NOTE_COLOUR, None),
    }
}

/// Converts straight-alpha RGBA to a premultiplied pixmap for tiny-skia.
fn premultiplied(image: &ImageBuffer) -> Option<Pixmap> {
    let width = image.width();
    let height = image.height();
    let size = IntSize::from_wh(width, height)?;
    let mut data = Vec::with_capacity(ImageBuffer::byte_len(width, height)?);
    for pixel in image.pixels().as_chunks::<4>().0 {
        let alpha = u32::from(pixel[3]);
        data.push(((u32::from(pixel[0]) * alpha + 127) / 255) as u8);
        data.push(((u32::from(pixel[1]) * alpha + 127) / 255) as u8);
        data.push(((u32::from(pixel[2]) * alpha + 127) / 255) as u8);
        data.push(pixel[3]);
    }
    Pixmap::from_vec(data, size)
}

/// Converts tiny-skia's premultiplied pixels back to straight-alpha RGBA.
fn straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixmap.data().len());
    for pixel in pixmap.data().as_chunks::<4>().0 {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 || alpha == 255 {
            out.extend_from_slice(pixel);
            continue;
        }
        for channel in &pixel[..3] {
            out.push(((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8);
        }
        out.push(pixel[3]);
    }
    out
}

/// Composites the board into a PNG at `path`.
///
/// The export is built from the stored previews, at up to `max_side` pixels on
/// the longest side, and written through a temporary file so a failed export
/// never leaves a half-written PNG over an existing one. A full-resolution
/// export that decodes every original is a renderer feature, not a container
/// feature; this entry point exists so File → Export produces something real
/// and correctly composed from the first phase the format lands in.
pub fn export_png(
    path: impl AsRef<std::path::Path>,
    scene: &Scene,
    background: [u8; 4],
    max_side: u32,
) -> Result<(), FormatError> {
    let path = path.as_ref();
    let image = composite(scene, background, max_side, None)?;
    let png = encode_png(&image).map_err(|error| FormatError::Preview {
        reason: error.to_string(),
    })?;
    let temporary = path.with_extension("png.tmp");
    std::fs::write(&temporary, &png)
        .map_err(|error| FormatError::io("write", temporary.display().to_string(), error))?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(FormatError::io(
            "replace",
            path.display().to_string(),
            error,
        ));
    }
    Ok(())
}

/// Encodes an asset preview, choosing a codec from whether it has alpha.
///
/// Opaque previews become JPEG; a preview with transparency becomes PNG so the
/// checkerboard and the board background show through. The returned format
/// string is the canonical extension used in the scene document.
pub(crate) fn encode_preview(image: &ImageBuffer) -> Result<(String, Vec<u8>), FormatError> {
    let has_alpha = image
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[3] != 255);
    let bytes = if has_alpha {
        encode_png(image)
    } else {
        encode_jpeg(image, THUMBNAIL_QUALITY)
    }
    .map_err(|error| FormatError::Preview {
        reason: error.to_string(),
    })?;
    let format = if has_alpha { "png" } else { "jpg" };
    Ok((format.to_owned(), bytes))
}

/// Decodes an encoded asset preview back to pixels.
pub(crate) fn decode_preview(bytes: &[u8]) -> Result<ImageBuffer, FormatError> {
    rizref_codecs::decode(bytes).map_err(|error| FormatError::Preview {
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{MAX_THUMBNAIL_SIDE, composite, decode_preview, encode_preview, generate};
    use glam::Vec2;
    use rizref_codecs::{ImageBuffer, ImageFormat};
    use rizref_core::{Asset, AssetId, Scene, SceneId};
    use std::sync::Arc;

    fn scene() -> (Scene, AssetId) {
        let mut scene = Scene::new(SceneId::from_bytes([1; 16]));
        let pixels = ImageBuffer::filled(8, 6, [200, 40, 40, 255]);
        let asset = Asset::from_decoded(
            Arc::from(&b"blob"[..]),
            ImageFormat::Png,
            &pixels,
            ImageBuffer::filled(4, 3, [200, 40, 40, 255]),
        );
        let (id, _) = scene.insert_asset(asset);
        scene
            .insert_image(id, Vec2::new(400.0, 300.0), Vec2::new(200.0, 150.0))
            .expect("asset stored");
        scene.insert_note("note", Vec2::new(-200.0, 100.0), Vec2::splat(120.0));
        (scene, id)
    }

    #[test]
    fn a_thumbnail_fits_the_documented_side() {
        let (scene, _) = scene();

        let thumb = generate(&scene, [24, 24, 28, 255], MAX_THUMBNAIL_SIDE).expect("thumbnail");

        assert!(thumb.width <= MAX_THUMBNAIL_SIDE);
        assert!(thumb.height <= MAX_THUMBNAIL_SIDE);
        assert!(!thumb.bytes.is_empty());
        assert!(rizref_codecs::sniff(&thumb.bytes).is_some());
    }

    #[test]
    fn a_small_board_is_not_magnified() {
        let (scene, _) = scene();
        // A single 40 px item on an otherwise empty board.
        let mut small = Scene::new(SceneId::from_bytes([3; 16]));
        let pixels = ImageBuffer::filled(4, 4, [1, 2, 3, 255]);
        let asset = Asset::from_decoded(
            Arc::from(&b"small"[..]),
            ImageFormat::Png,
            &pixels,
            ImageBuffer::filled(2, 2, [1, 2, 3, 255]),
        );
        let (id, _) = small.insert_asset(asset);
        small
            .insert_image(id, Vec2::ZERO, Vec2::splat(40.0))
            .expect("asset");
        let thumb = generate(&small, [0, 0, 0, 255], MAX_THUMBNAIL_SIDE).expect("thumbnail");

        assert_eq!(thumb.width, 40);
        assert_eq!(thumb.height, 40);

        // The larger sample still fits.
        let big = generate(&scene, [0, 0, 0, 255], MAX_THUMBNAIL_SIDE).expect("thumbnail");
        assert!(big.width <= MAX_THUMBNAIL_SIDE);
    }

    #[test]
    fn an_empty_board_still_produces_a_thumbnail() {
        let scene = Scene::new(SceneId::from_bytes([2; 16]));

        let thumb = generate(&scene, [10, 10, 10, 255], MAX_THUMBNAIL_SIDE).expect("thumbnail");

        assert_eq!((thumb.width, thumb.height), (1, 1));
        assert!(!thumb.bytes.is_empty());
    }

    #[test]
    fn an_exact_scale_is_honoured() {
        let (scene, _) = scene();

        let image =
            composite(&scene, [0, 0, 0, 255], MAX_THUMBNAIL_SIDE, Some(0.5)).expect("composite");

        // Scene rect is 760 x 335 (note at -260..-140, image at 300..500).
        assert!((image.width() as i32 - 380).abs() <= 1);
        assert!((image.height() as i32 - 168).abs() <= 1);
    }

    fn pixel(image: &ImageBuffer, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * image.width() + x) as usize) * 4;
        image.pixels()[offset..offset + 4]
            .try_into()
            .expect("4 channels")
    }

    fn single_item(preview: ImageBuffer, size: Vec2, rotation: f32, flip_x: bool) -> Scene {
        let mut scene = Scene::new(SceneId::from_bytes([9; 16]));
        let asset = Asset::from_decoded(
            Arc::from(&b"pixels"[..]),
            ImageFormat::Png,
            &preview,
            preview.clone(),
        );
        let (id, _) = scene.insert_asset(asset);
        let item = scene.insert_image(id, Vec2::ZERO, size).expect("asset");
        let item = scene.item_mut(item).expect("item");
        let mut transform = item.transform().with_rotation(rotation);
        transform.set_center(Vec2::ZERO);
        item.set_transform(transform);
        item.set_flip(flip_x, false);
        scene
    }

    #[test]
    fn rotation_and_flip_are_applied_to_the_composite() {
        // A red 4 x 2 preview stretched to 40 x 10 and turned a quarter turn:
        // the board becomes a 10 x 40 vertical bar, not a horizontal one.
        let red = ImageBuffer::filled(4, 2, [220, 30, 30, 255]);
        let scene = single_item(
            red,
            Vec2::new(40.0, 10.0),
            std::f32::consts::FRAC_PI_2,
            false,
        );
        let image = composite(&scene, [0, 0, 0, 255], 64, Some(1.0)).expect("composite");

        assert!(
            image.width() >= 10 && image.width() <= 11,
            "width {}",
            image.width()
        );
        assert_eq!(image.height(), 40);
        assert_eq!(
            pixel(&image, 5, 2),
            [220, 30, 30, 255],
            "top of the vertical bar"
        );
        assert_eq!(pixel(&image, 5, 20), [220, 30, 30, 255], "centre");
        assert_eq!(
            pixel(&image, 1, 20),
            [220, 30, 30, 255],
            "left edge of the bar"
        );

        // A 4 x 2 preview with a red left half and a blue right half, flipped:
        // the blue half must end up on the left.
        let mut split = Vec::with_capacity(4 * 2 * 4);
        for _y in 0..2 {
            split.extend_from_slice(&[220, 30, 30, 255]);
            split.extend_from_slice(&[220, 30, 30, 255]);
            split.extend_from_slice(&[30, 30, 220, 255]);
            split.extend_from_slice(&[30, 30, 220, 255]);
        }
        let split = ImageBuffer::from_rgba(4, 2, split).expect("pixels");
        let scene = single_item(split, Vec2::new(4.0, 2.0), 0.0, true);
        let image = composite(&scene, [0, 0, 0, 255], 64, Some(1.0)).expect("composite");

        assert_eq!(pixel(&image, 0, 1), [30, 30, 220, 255]);
        assert_eq!(pixel(&image, 3, 1), [220, 30, 30, 255]);
    }

    #[test]
    fn previews_round_trip_through_their_encoder() {
        let opaque = ImageBuffer::filled(4, 3, [10, 20, 30, 255]);
        let transparent = ImageBuffer::filled(4, 3, [10, 20, 30, 128]);

        let (opaque_format, opaque_bytes) = encode_preview(&opaque).expect("encode");
        let (alpha_format, alpha_bytes) = encode_preview(&transparent).expect("encode");

        assert_eq!(opaque_format, "jpg");
        assert_eq!(alpha_format, "png");
        let decoded = decode_preview(&opaque_bytes).expect("decode");
        assert_eq!((decoded.width(), decoded.height()), (4, 3));
        assert_eq!((decode_preview(&alpha_bytes).expect("decode").width()), 4);
    }

    #[test]
    fn a_note_card_is_painted_in_its_stored_background() {
        let mut scene = Scene::new(SceneId::from_bytes([2; 16]));
        let id = scene.insert_note("n", Vec2::ZERO, Vec2::splat(100.0));
        let mut content = scene
            .item(id)
            .and_then(rizref_core::Item::note_content)
            .expect("note");
        content.background = Some([20, 120, 220]);
        scene.item_mut(id).expect("note").set_note(&content);

        let image = composite(&scene, [0, 0, 0, 255], 100, None).expect("composite");
        let centre = (50 * image.width() + 50) as usize * 4;
        assert_eq!(&image.pixels()[centre..centre + 4], &[20, 120, 220, 255]);

        content.background = None;
        scene.item_mut(id).expect("note").set_note(&content);
        let image = composite(&scene, [0, 0, 0, 255], 100, None).expect("composite");
        assert_ne!(
            &image.pixels()[centre..centre + 3],
            &[20, 120, 220],
            "a note without a background keeps the default card"
        );
    }
}
