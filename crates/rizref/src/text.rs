//! Text shaping and rasterisation for the HUD.
//!
//! One [`TextEngine`] is created for the window and reused: building the font
//! database means reading the system's font registry and, on Windows, opening
//! every face it lists. Doing that per frame is the difference between a HUD
//! that costs microseconds and one that costs milliseconds.
//!
//! Phase 02 uses the engine for the provisional HUD readout, rasterised into
//! the tiny-skia CPU layer; the real text pipeline (layout, notes, menus)
//! arrives with the UI phases.

use cosmic_text::{Attrs, Buffer, Color, FontSystem, Metrics, Shaping, SwashCache};
use tiny_skia::Pixmap;

/// Owns the font database, the shaping cache and the glyph raster cache.
pub struct TextEngine {
    fonts: FontSystem,
    swash: SwashCache,
}

impl TextEngine {
    /// Loads the system font database.
    ///
    /// Never fails: `cosmic-text` falls back to an empty database when it cannot
    /// read the font registry, and an empty database still renders the built-in
    /// fallback. A board full of images should not refuse to open because a font
    /// directory is unreadable.
    pub fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
            swash: SwashCache::new(),
        }
    }

    /// How many font faces are available.
    ///
    /// Reported at startup and worth having in a bug report: "the labels are
    /// boxes" is almost always a font database that came up empty.
    pub fn font_count(&self) -> usize {
        self.fonts.db().len()
    }

    /// Draws one line of text into a tiny-skia pixmap at `x`, `y` (pixels from
    /// the top-left), blended over what is already there.
    ///
    /// Shaping goes through cosmic-text, rasterisation through swash, and each
    /// covered pixel is alpha-blended into the pixmap's premultiplied data.
    /// This is the provisional HUD path; it is deliberately simple rather than
    /// fast, and the readout is a few dozen glyphs.
    pub fn draw_str(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        font_size: f32,
        color: [u8; 4],
    ) {
        let metrics = Metrics::new(font_size, (font_size * 1.35).ceil());
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        buffer.set_text(text, &Attrs::new(), Shaping::Advanced, None);

        let tint = Color::rgba(color[0], color[1], color[2], color[3]);
        let offset_x = x as i32;
        let offset_y = y as i32;
        let width = pixmap.width() as i32;
        let height = pixmap.height() as i32;

        // The callback fires once per covered pixel with the coverage in the
        // colour's alpha; coordinates are relative to the text origin.
        buffer.draw(
            &mut self.fonts,
            &mut self.swash,
            tint,
            |gx, gy, _, _, pixel| {
                let px = offset_x + gx;
                let py = offset_y + gy;
                if px < 0 || py < 0 || px >= width || py >= height {
                    return;
                }
                blend_premultiplied(
                    pixmap.data_mut(),
                    (py as usize * width as usize + px as usize) * 4,
                    pixel.r(),
                    pixel.g(),
                    pixel.b(),
                    pixel.a(),
                );
            },
        );
    }
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Blends one straight-alpha pixel into premultiplied RGBA data at `offset`.
fn blend_premultiplied(data: &mut [u8], offset: usize, r: u8, g: u8, b: u8, a: u8) {
    if a == 255 {
        data[offset] = r;
        data[offset + 1] = g;
        data[offset + 2] = b;
        data[offset + 3] = 255;
        return;
    }
    let alpha = u16::from(a);
    let inverse = 255 - alpha;
    let blend = |dst: u8, src: u8| -> u8 {
        // src straight → premultiplied, then over the premultiplied dst.
        let src_premul = u16::from(src) * alpha / 255;
        let out = src_premul + u16::from(dst) * inverse / 255;
        out.min(255) as u8
    };
    data[offset] = blend(data[offset], r);
    data[offset + 1] = blend(data[offset + 1], g);
    data[offset + 2] = blend(data[offset + 2], b);
    data[offset + 3] = blend(data[offset + 3], 255);
}

#[cfg(test)]
mod tests {
    use super::{TextEngine, blend_premultiplied};
    use tiny_skia::Pixmap;

    #[test]
    fn the_system_font_database_is_not_empty() {
        // A machine with no readable fonts at all is broken enough that this
        // failing is more useful than the HUD silently drawing nothing.
        assert!(TextEngine::new().font_count() > 0);
    }

    #[test]
    fn creating_an_engine_twice_is_allowed() {
        // Guards against a hidden process-wide singleton: the font system will
        // have to be constructible per document once boards can be reopened.
        let first = TextEngine::new();
        let second = TextEngine::new();

        assert_eq!(first.font_count(), second.font_count());
    }

    #[test]
    fn drawing_text_puts_opaque_pixels_on_the_surface() {
        let mut engine = TextEngine::new();
        let mut pixmap = Pixmap::new(120, 30).expect("small pixmap allocates");

        engine.draw_str(&mut pixmap, "100%", 2.0, 4.0, 14.0, [240, 240, 244, 255]);

        let any = pixmap
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] > 0)
            .count();
        let solid = pixmap
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] > 200)
            .count();
        assert!(any > 100, "four glyphs cover plenty of pixels, got {any}");
        assert!(
            solid > 20,
            "and a good share of them are near-opaque, got {solid}"
        );
    }

    #[test]
    fn drawing_outside_the_surface_clips_instead_of_panicking() {
        let mut engine = TextEngine::new();
        let mut pixmap = Pixmap::new(20, 10).expect("small pixmap allocates");

        engine.draw_str(&mut pixmap, "zoom 100%", -4.0, 6.0, 14.0, [255, 0, 0, 255]);
        engine.draw_str(&mut pixmap, "zoom 100%", 0.0, 8.0, 14.0, [255, 0, 0, 255]);
    }

    #[test]
    fn blending_keeps_premultiplied_channels_in_range() {
        let mut data = [10u8, 20, 30, 255];
        blend_premultiplied(&mut data, 0, 200, 200, 200, 128);

        assert!(data[0] <= data[3] && data[1] <= data[3] && data[2] <= data[3]);
        assert_eq!(data[3], 255, "opaque over opaque stays opaque");
    }

    #[test]
    fn blending_over_transparent_stores_the_source_premultiplied() {
        let mut data = [0u8, 0, 0, 0];
        blend_premultiplied(&mut data, 0, 240, 240, 244, 230);

        // tiny-skia storage is premultiplied: rgb scaled by the coverage.
        assert_eq!(data[0], (240u16 * 230 / 255) as u8);
        assert_eq!(data[1], (240u16 * 230 / 255) as u8);
        assert_eq!(data[2], (244u16 * 230 / 255) as u8);
        assert_eq!(data[3], 230);
    }
}
