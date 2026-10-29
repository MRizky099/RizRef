//! The CPU-side raster surface: the HUD and the fallback path.
//!
//! Two jobs, one surface type:
//!
//! * **HUD rasterisation.** The zoom readout is shaped and rasterised through
//!   [`crate::text::TextEngine`] into a pixmap and uploaded as one texture per
//!   frame, which keeps the GPU pipelines to the item quads until the text
//!   pipeline lands with the UI phases.
//! * **Fallback.** On a machine where a GPU surface cannot be created at all,
//!   the same rasteriser can paint a whole frame, so the application degrades to
//!   slow rather than to broken.
//!
//! Note the storage layout: tiny-skia keeps **premultiplied** RGBA, so a pixel
//! read back here is not in the same form as the straight-alpha pixels
//! `rizref-codecs` produces from a decoded image. Both halves are documented at
//! their own boundary, and the conversion belongs where the two meet — which
//! today is [`HudSurface::straight_rgba`], the GPU upload.

use tiny_skia::{Color, Pixmap};

use crate::text::TextEngine;

/// Bytes per pixel in a tiny-skia pixmap.
///
/// Named rather than written as `4` in six places, because the day the layout
/// changes is the day every stride in this file has to change with it.
const BYTES_PER_PIXEL: usize = 4;

/// Left inset, in surface pixels, of the readout text.
const READOUT_MARGIN_X: f32 = 10.0;

/// Top inset, in surface pixels, of the readout text.
const READOUT_MARGIN_Y: f32 = 9.0;

/// Readout font size in surface pixels.
const READOUT_FONT_SIZE: f32 = 14.0;

/// A CPU raster target sized in device pixels.
pub struct HudSurface {
    pixmap: Pixmap,
}

impl HudSurface {
    /// Allocates a transparent surface of `width` × `height` device pixels.
    ///
    /// Returns `None` when the size cannot be allocated, which is the same
    /// condition wgpu would report as a surface creation failure; the caller
    /// decides whether to fall back to a smaller surface or to tell the user.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        Some(Self {
            pixmap: Pixmap::new(width, height)?,
        })
    }

    /// Width in device pixels.
    pub fn width(&self) -> u32 {
        self.pixmap.width()
    }

    /// Height in device pixels.
    pub fn height(&self) -> u32 {
        self.pixmap.height()
    }

    /// Bytes per row, as the GPU upload needs to know it.
    pub fn stride(&self) -> usize {
        self.pixmap.width() as usize * BYTES_PER_PIXEL
    }

    /// Clears the whole surface to one colour.
    pub fn clear(&mut self, rgba: [u8; BYTES_PER_PIXEL]) {
        self.pixmap
            .fill(Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]));
    }

    /// The stored value of one pixel, or `None` when out of bounds.
    ///
    /// The bytes are premultiplied by alpha, exactly as tiny-skia stores them
    /// and as the GPU expects them.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; BYTES_PER_PIXEL]> {
        if x >= self.width() || y >= self.height() {
            return None;
        }
        let offset = y as usize * self.stride() + x as usize * BYTES_PER_PIXEL;
        let channels = self.pixmap.data().get(offset..offset + BYTES_PER_PIXEL)?;
        Some([channels[0], channels[1], channels[2], channels[3]])
    }

    /// Draws the provisional readout (zoom, rotation, cursor world position)
    /// into a cleared surface, ready for the per-frame GPU upload.
    pub fn draw_readout(&mut self, text: &str, engine: &mut TextEngine) {
        self.clear([0, 0, 0, 0]);
        engine.draw_str(
            &mut self.pixmap,
            text,
            READOUT_MARGIN_X,
            READOUT_MARGIN_Y,
            READOUT_FONT_SIZE,
            [235, 235, 240, 230],
        );
    }

    /// The surface as straight-alpha RGBA bytes, as the GPU upload wants them.
    ///
    /// tiny-skia stores premultiplied pixels; the divide-back is rounded, which
    /// for antialiased HUD glyphs is well inside any visible tolerance.
    pub fn straight_rgba(&self) -> Vec<u8> {
        straight_rgba(&self.pixmap)
    }
}

/// Converts a tiny-skia pixmap into straight-alpha RGBA bytes.
pub fn straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = pixmap.data().to_vec();
    for pixel in out.as_chunks_mut::<BYTES_PER_PIXEL>().0 {
        let alpha = u16::from(pixel[3]);
        match alpha {
            0 => {
                pixel[..3].fill(0);
            }
            255 => {}
            _ => {
                for channel in pixel.iter_mut().take(3) {
                    *channel = ((u16::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
        }
    }
    out
}

/// The stored premultiplied bytes of a surface (test helper).
#[cfg(test)]
fn surface_pixmap_data(surface: &HudSurface) -> Vec<u8> {
    surface.pixmap.data().to_vec()
}

#[cfg(test)]
mod tests {
    use super::HudSurface;
    use crate::text::TextEngine;

    const OPAQUE_RED: [u8; 4] = [200, 10, 10, 255];

    #[test]
    fn a_new_surface_is_transparent() {
        let surface = HudSurface::new(3, 2).expect("small surface allocates");

        assert_eq!(surface.width(), 3);
        assert_eq!(surface.height(), 2);
        assert_eq!(surface.stride(), 12);
        assert_eq!(surface.pixel(0, 0), Some([0, 0, 0, 0]));
        assert_eq!(surface.pixel(2, 1), Some([0, 0, 0, 0]));
    }

    #[test]
    fn clear_paints_every_pixel() {
        let mut surface = HudSurface::new(2, 1).expect("small surface allocates");

        surface.clear(OPAQUE_RED);

        assert_eq!(surface.pixel(0, 0), Some(OPAQUE_RED));
        assert_eq!(surface.pixel(1, 0), Some(OPAQUE_RED));
    }

    #[test]
    fn out_of_bounds_pixels_are_none_not_a_panic() {
        let surface = HudSurface::new(1, 1).expect("small surface allocates");

        assert_eq!(surface.pixel(1, 0), None);
        assert_eq!(surface.pixel(0, 1), None);
    }

    #[test]
    fn a_zero_sized_surface_is_refused() {
        assert!(HudSurface::new(0, 10).is_none());
        assert!(HudSurface::new(10, 0).is_none());
    }

    #[test]
    fn the_readout_inks_pixels_and_straight_rgba_matches_them() {
        let mut surface = HudSurface::new(200, 40).expect("readout surface allocates");
        let mut engine = TextEngine::new();

        surface.draw_readout("100% · 0° · (0.0, 0.0)", &mut engine);

        let inked = (0..surface.width())
            .flat_map(|x| (0..surface.height()).map(move |y| (x, y)))
            .filter(|&(x, y)| surface.pixel(x, y).is_some_and(|p| p[3] > 0))
            .count();
        assert!(inked > 100, "a readout should ink pixels, got {inked}");

        // Alpha survives un-premultiplication; fully covered pixels come back
        // exactly as the pixmap stores them; partially covered pixels only
        // come back brighter than their premultiplied value, never darker.
        let straight = surface.straight_rgba();
        let stored_pixels = super::surface_pixmap_data(&surface);
        for (straight, stored) in straight
            .as_chunks::<4>()
            .0
            .iter()
            .zip(stored_pixels.as_chunks::<4>().0)
        {
            assert_eq!(straight[3], stored[3], "alpha passes through untouched");
            if stored[3] == 255 {
                assert_eq!(&straight[..3], &stored[..3]);
            } else if stored[3] > 0 {
                assert!(straight[0] >= stored[0] && straight[1] >= stored[1]);
            }
        }
    }

    #[test]
    fn straight_rgba_un_premultiplies() {
        let mut surface = HudSurface::new(1, 1).expect("1x1 surface allocates");
        // Opaque red survives round-tripping exactly.
        surface.clear(OPAQUE_RED);
        assert_eq!(surface.straight_rgba(), vec![200, 10, 10, 255]);
    }

    #[test]
    fn transparent_pixels_stay_transparent_and_black() {
        let surface = HudSurface::new(1, 1).expect("1x1 surface allocates");
        assert_eq!(surface.straight_rgba(), vec![0, 0, 0, 0]);
    }
}
