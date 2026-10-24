//! The CPU-side raster surface: the HUD and the fallback path.
//!
//! Two jobs, one surface type:
//!
//! * **HUD rasterisation.** Grid lines, item outlines, selection handles, notes
//!   and the comment markers are drawn as vectors into a pixmap and uploaded as
//!   one texture per frame, which keeps the GPU pipelines to the item quads.
//! * **Fallback.** On a machine where a GPU surface cannot be created at all,
//!   the same rasteriser can paint a whole frame, so the application degrades to
//!   slow rather than to broken.
//!
//! Phase 00 proves the rasteriser links and produces pixels. The drawing itself
//! arrives with the canvas in phase 02 and the HUD in phase 04/05.
//!
//! Note the storage layout: tiny-skia keeps **premultiplied** RGBA, so a pixel
//! read back here is not in the same form as the straight-alpha pixels
//! `rizref-codecs` produces from a decoded image. Both halves are documented at
//! their own boundary, and the conversion belongs where the two meet.

use tiny_skia::{Color, Pixmap};

/// Bytes per pixel in a tiny-skia pixmap.
///
/// Named rather than written as `4` in six places, because the day the layout
/// changes is the day every stride in this file has to change with it.
const BYTES_PER_PIXEL: usize = 4;

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
}

#[cfg(test)]
mod tests {
    use super::HudSurface;

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
}
