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

use cosmic_text::{Attrs, Buffer, Color, Editor, FontSystem, Metrics, Shaping, SwashCache, Wrap};
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
        #[cfg_attr(windows, allow(unused_mut))]
        let mut fonts = FontSystem::new();
        #[cfg(unix)]
        add_fallback_font(&mut fonts);
        Self {
            fonts,
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

    /// Draws wrapped text into a rectangle starting at `x`, `y`.
    ///
    /// Used for note text when the note is not being edited: a word-wrapped
    /// buffer shaped to the note's text width, clipped to the pixmap.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_block(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        font_size: f32,
        color: [u8; 4],
    ) {
        let metrics = Metrics::new(font_size, (font_size * 1.4).ceil());
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        buffer.set_wrap(Wrap::Word);
        buffer.set_size(Some(width.max(1.0)), Some(height.max(1.0)));
        buffer.set_text(text, &Attrs::new(), Shaping::Advanced, None);
        self.draw_buffer(pixmap, &mut buffer, x, y, color);
    }

    /// Draws a shaped buffer at `x`, `y`, blending into the pixmap.
    pub fn draw_buffer(
        &mut self,
        pixmap: &mut Pixmap,
        buffer: &mut Buffer,
        x: f32,
        y: f32,
        color: [u8; 4],
    ) {
        let tint = Color::rgba(color[0], color[1], color[2], color[3]);
        let origin = (x as i32, y as i32);
        let clip = [0, 0, pixmap.width() as i32, pixmap.height() as i32];
        buffer.draw(
            &mut self.fonts,
            &mut self.swash,
            tint,
            |gx, gy, w, h, pixel| {
                fill_blend(pixmap, origin, [gx, gy], [w, h], pixel, clip);
            },
        );
    }

    /// Draws a note edit session, including its cursor and selection.
    ///
    /// The editor owns the buffer and its caret; this method supplies the font
    /// system and glyph cache and blends the result into the note's pixmap.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_editor(
        &mut self,
        pixmap: &mut Pixmap,
        editor: &mut Editor<'static>,
        x: f32,
        y: f32,
        text_color: [u8; 4],
        cursor_color: [u8; 4],
        selection_color: [u8; 4],
        selected_text_color: [u8; 4],
        clip: [i32; 4],
    ) {
        let origin = (x as i32, y as i32);
        editor.draw(
            &mut self.fonts,
            &mut self.swash,
            Color::rgba(text_color[0], text_color[1], text_color[2], text_color[3]),
            Color::rgba(
                cursor_color[0],
                cursor_color[1],
                cursor_color[2],
                cursor_color[3],
            ),
            Color::rgba(
                selection_color[0],
                selection_color[1],
                selection_color[2],
                selection_color[3],
            ),
            Color::rgba(
                selected_text_color[0],
                selected_text_color[1],
                selected_text_color[2],
                selected_text_color[3],
            ),
            |gx, gy, w, h, pixel| fill_blend(pixmap, origin, [gx, gy], [w, h], pixel, clip),
        );
    }

    /// Paints a buffer shaped in world units at `scale` pixels per world unit.
    ///
    /// The glyphs are not re-shaped: each laid-out glyph is rasterised at the
    /// scaled size (`LayoutGlyph::physical`), so line breaks and positions are
    /// exactly the ones the layout measured. `origin` is the buffer's top-left
    /// in pixels, `clip` is `[x0, y0, x1, y1]` in pixels, and `color` maps a
    /// glyph's metadata (its span index) to straight RGBA.
    pub fn draw_scaled(
        &mut self,
        pixmap: &mut Pixmap,
        buffer: &Buffer,
        origin: (f32, f32),
        scale: f32,
        clip: [i32; 4],
        mut color: impl FnMut(usize) -> Option<[u8; 4]>,
    ) {
        let width = pixmap.width() as i32;
        let height = pixmap.height() as i32;
        let x0 = clip[0].max(0);
        let y0 = clip[1].max(0);
        let x1 = clip[2].min(width);
        let y1 = clip[3].min(height);
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let Some(rgba) = color(glyph.metadata) else {
                    continue;
                };
                let physical = glyph.physical((origin.0, origin.1 + run.line_y * scale), scale);
                let base = Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]);
                let data = pixmap.data_mut();
                self.swash.with_pixels(
                    &mut self.fonts,
                    physical.cache_key,
                    base,
                    |gx, gy, pixel| {
                        let px = physical.x + gx;
                        let py = physical.y + gy;
                        if px < x0 || py < y0 || px >= x1 || py >= y1 {
                            return;
                        }
                        blend_premultiplied(
                            data,
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
    }

    /// The font system, for callers that drive a cosmic-text [`Editor`].
    pub fn font_system_mut(&mut self) -> &mut FontSystem {
        &mut self.fonts
    }

    /// The shaped width of one line of text at `font_size`, in pixels.
    ///
    /// Used by the shell to size panels, menu columns and right-aligned
    /// accelerator labels. Shaping here is deliberate: guessing from character
    /// counts drifts with font and DPI, and the shell is custom-painted, so
    /// there is no layout engine to ask.
    pub fn measure(&mut self, text: &str, font_size: f32) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let metrics = Metrics::new(font_size, (font_size * 1.35).ceil());
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        buffer.set_text(text, &Attrs::new(), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fonts, false);
        buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0_f32, f32::max)
    }
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Blends a `size` rectangle of one colour at `at` (relative to `origin`)
/// into the pixmap, inside `clip` (`[x0, y0, x1, y1]`).
///
/// cosmic-text's draw callback reports glyph coverage as 1 x 1 rectangles,
/// but selection highlights and text decorations (underline) as wider ones;
/// painting only their first pixel lost both.
fn fill_blend(
    pixmap: &mut Pixmap,
    origin: (i32, i32),
    at: [i32; 2],
    size: [u32; 2],
    color: Color,
    clip: [i32; 4],
) {
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let x0 = (origin.0 + at[0]).max(clip[0].max(0));
    let y0 = (origin.1 + at[1]).max(clip[1].max(0));
    let x1 = (origin.0 + at[0])
        .saturating_add(size[0] as i32)
        .min(clip[2].min(width));
    let y1 = (origin.1 + at[1])
        .saturating_add(size[1] as i32)
        .min(clip[3].min(height));
    let data = pixmap.data_mut();
    for py in y0..y1 {
        for px in x0..x1 {
            blend_premultiplied(
                data,
                (py as usize * width as usize + px as usize) * 4,
                color.r(),
                color.g(),
                color.b(),
                color.a(),
            );
        }
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

/// Adds the bundled last-resort UI face on Linux and macOS.
///
/// `cosmic-text` (with `fontconfig` off) scans the system font directories
/// itself; a minimal or headless Linux may have none at all, and then every
/// label would be boxes. KaTeX_SansSerif (SIL OFL 1.1) is already compiled
/// into the binary for math notes, so it costs nothing to register it as the
/// face shaping falls back to when nothing else covers a glyph. Windows always
/// has Segoe UI and keeps its phase-07 font list exactly.
#[cfg(unix)]
fn add_fallback_font(fonts: &mut FontSystem) {
    match ratex_katex_fonts::ttf_bytes("KaTeX_SansSerif-Regular.ttf") {
        Some(bytes) => fonts.db_mut().load_font_data(bytes.into_owned()),
        None => tracing::warn!("the bundled fallback font is missing"),
    }
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
    fn rectangles_from_the_shaper_are_filled_not_dotted() {
        // A selection highlight or underline arrives as one wide rectangle.
        let mut pixmap = Pixmap::new(20, 10).expect("pixmap");
        super::fill_blend(
            &mut pixmap,
            (2, 1),
            [1, 2],
            [10, 3],
            cosmic_text::Color::rgba(255, 0, 0, 255),
            [0, 0, 20, 10],
        );
        let red = pixmap
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] == 255 && p[3] == 255)
            .count();
        assert_eq!(red, 30);
        // Clipped at the pixmap edge, never out of bounds.
        super::fill_blend(
            &mut pixmap,
            (15, 8),
            [0, 0],
            [100, 100],
            cosmic_text::Color::rgba(0, 255, 0, 255),
            [0, 0, 20, 10],
        );
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
