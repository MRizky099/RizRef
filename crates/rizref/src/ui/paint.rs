//! Shared painting helpers for the custom shell.
//!
//! All shell geometry is expressed in logical pixels; the painter multiplies by
//! the monitor scale once, so callers never think about DPI. Panels are drawn
//! with tiny-skia into the cached screen layer, and text goes through
//! [`TextEngine`] (cosmic-text shaping + swash rasterisation).

use tiny_skia::{
    Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Rect as SkiaRect, Stroke, Transform,
};

use crate::text::TextEngine;

/// Shell colour palette (straight-alpha RGBA).
pub const WINDOW_BG: [u8; 4] = [22, 22, 27, 255];
/// Panel fill.
pub const PANEL_BG: [u8; 4] = [36, 37, 44, 248];
/// Slightly lighter row hover.
pub const HOVER_BG: [u8; 4] = [52, 55, 66, 255];
/// Panel edge.
pub const PANEL_BORDER: [u8; 4] = [74, 78, 92, 255];
/// Primary text.
pub const TEXT: [u8; 4] = [236, 237, 242, 255];
/// Secondary text and accelerators.
pub const DIM_TEXT: [u8; 4] = [154, 158, 171, 255];
/// Disabled text.
pub const DISABLED_TEXT: [u8; 4] = [104, 106, 116, 255];
/// Accent blue, matching the canvas selection overlay.
pub const ACCENT: [u8; 4] = [77, 158, 255, 255];
/// Soft accent fill for selection.
pub const ACCENT_SOFT: [u8; 4] = [77, 158, 255, 48];
/// Warning tint for ghost/click-through indicators.
pub const GHOST_ACCENT: [u8; 4] = [255, 190, 92, 230];
/// Drop-zone border.
pub const DROP_BORDER: [u8; 4] = [92, 97, 112, 255];

/// A rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiRect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl UiRect {
    /// A rectangle from its corner and size.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Right edge.
    pub const fn right(self) -> f32 {
        self.x + self.width
    }

    /// Bottom edge.
    pub const fn bottom(self) -> f32 {
        self.y + self.height
    }

    /// Whether `(x, y)` lies inside, edges included.
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x <= self.right() && y <= self.bottom()
    }

    /// The rectangle inset by `amount` on every side.
    pub fn inset(self, amount: f32) -> Self {
        Self::new(
            self.x + amount,
            self.y + amount,
            (self.width - amount * 2.0).max(0.0),
            (self.height - amount * 2.0).max(0.0),
        )
    }
}

/// Draws shell UI into a device-pixel pixmap from logical coordinates.
pub struct Painter<'a> {
    pixmap: &'a mut Pixmap,
    engine: &'a mut TextEngine,
    scale: f32,
}

impl<'a> Painter<'a> {
    /// A painter for a pixmap and its monitor scale.
    pub fn new(pixmap: &'a mut Pixmap, engine: &'a mut TextEngine, scale: f32) -> Self {
        Self {
            pixmap,
            engine,
            scale: scale.max(1.0),
        }
    }

    /// Fills the whole surface with one colour.
    pub fn clear(&mut self, rgba: [u8; 4]) {
        self.pixmap.fill(color(rgba));
    }

    /// Fills a rectangle.
    pub fn fill_rect(&mut self, rect: UiRect, rgba: [u8; 4]) {
        let Some(skia) = skia_rect(rect, self.scale) else {
            return;
        };
        let paint = solid_paint(rgba);
        self.pixmap
            .fill_rect(skia, &paint, Transform::identity(), None);
    }

    /// Fills a rounded rectangle.
    pub fn fill_round_rect(&mut self, rect: UiRect, radius: f32, rgba: [u8; 4]) {
        let Some(path) = round_path(rect, radius, self.scale) else {
            return;
        };
        self.pixmap.fill_path(
            &path,
            &solid_paint(rgba),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    /// Strokes a rounded rectangle.
    pub fn stroke_round_rect(&mut self, rect: UiRect, radius: f32, width: f32, rgba: [u8; 4]) {
        let Some(path) = round_path(rect, radius, self.scale) else {
            return;
        };
        let stroke = Stroke {
            width: width * self.scale,
            ..Stroke::default()
        };
        self.pixmap.stroke_path(
            &path,
            &solid_paint(rgba),
            &stroke,
            Transform::identity(),
            None,
        );
    }

    /// Strokes a straight line with round caps.
    pub fn line(&mut self, from: (f32, f32), to: (f32, f32), width: f32, rgba: [u8; 4]) {
        let mut builder = PathBuilder::new();
        builder.move_to(from.0 * self.scale, from.1 * self.scale);
        builder.line_to(to.0 * self.scale, to.1 * self.scale);
        let Some(path) = builder.finish() else {
            return;
        };
        let stroke = Stroke {
            width: (width * self.scale).max(1.0),
            line_cap: LineCap::Round,
            ..Stroke::default()
        };
        self.pixmap.stroke_path(
            &path,
            &solid_paint(rgba),
            &stroke,
            Transform::identity(),
            None,
        );
    }

    /// Fills a rounded panel with a soft shadow and a 1 px border.
    pub fn panel(&mut self, rect: UiRect, radius: f32) {
        self.fill_round_rect(
            UiRect::new(rect.x + 3.0, rect.y + 5.0, rect.width, rect.height),
            radius,
            [0, 0, 0, 90],
        );
        self.fill_round_rect(rect, radius, PANEL_BG);
        self.stroke_round_rect(rect, radius, 1.0, PANEL_BORDER);
    }

    /// Draws one line of text with its top-left at `(x, y)`.
    pub fn text(&mut self, text: &str, x: f32, y: f32, size: f32, rgba: [u8; 4]) {
        if text.is_empty() {
            return;
        }
        self.engine.draw_str(
            self.pixmap,
            text,
            x * self.scale,
            y * self.scale,
            size * self.scale,
            rgba,
        );
    }

    /// Draws one line right-aligned to `right`.
    pub fn text_right(&mut self, text: &str, right: f32, y: f32, size: f32, rgba: [u8; 4]) {
        let width = self.engine.measure(text, size * self.scale) / self.scale;
        self.text(text, right - width, y, size, rgba);
    }

    /// Draws one line horizontally centred in `rect`.
    pub fn text_centered(&mut self, text: &str, rect: UiRect, y: f32, size: f32, rgba: [u8; 4]) {
        let width = self.engine.measure(text, size * self.scale) / self.scale;
        self.text(text, rect.x + (rect.width - width) * 0.5, y, size, rgba);
    }

    /// The shaped width of `text` in logical pixels.
    pub fn measure(&mut self, text: &str, size: f32) -> f32 {
        self.engine.measure(text, size * self.scale) / self.scale
    }

    /// Draws a thumbnail placeholder: a rounded box with a large monogram.
    pub fn thumbnail_placeholder(&mut self, rect: UiRect, monogram: &str) {
        self.fill_round_rect(rect, 6.0, [46, 49, 60, 255]);
        let size = (rect.height * 0.42).clamp(16.0, 34.0);
        let width = self.measure(monogram, size);
        let y = rect.y + (rect.height - size * 1.2) * 0.5;
        self.text(
            monogram,
            rect.x + (rect.width - width) * 0.5,
            y,
            size,
            [120, 128, 148, 255],
        );
    }
}

/// A tiny-skia colour from straight-alpha bytes.
fn color(rgba: [u8; 4]) -> Color {
    Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3])
}

/// A solid paint.
fn solid_paint(rgba: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color(rgba));
    paint.anti_alias = true;
    paint
}

/// Maps a logical rectangle onto device pixels.
fn skia_rect(rect: UiRect, scale: f32) -> Option<SkiaRect> {
    SkiaRect::from_xywh(
        rect.x * scale,
        rect.y * scale,
        (rect.width * scale).max(0.0),
        (rect.height * scale).max(0.0),
    )
}

/// Builds a rounded-rectangle path in device pixels.
fn round_path(rect: UiRect, radius: f32, scale: f32) -> Option<tiny_skia::Path> {
    let skia = skia_rect(rect, scale)?;
    let (x, y, width, height) = (skia.x(), skia.y(), skia.width(), skia.height());
    let radius = (radius * scale).min(width.min(height) * 0.5);
    let mut builder = PathBuilder::new();
    builder.move_to(x + radius, y);
    builder.line_to(x + width - radius, y);
    builder.quad_to(x + width, y, x + width, y + radius);
    builder.line_to(x + width, y + height - radius);
    builder.quad_to(x + width, y + height, x + width - radius, y + height);
    builder.line_to(x + radius, y + height);
    builder.quad_to(x, y + height, x, y + height - radius);
    builder.line_to(x, y + radius);
    builder.quad_to(x, y, x + radius, y);
    builder.close();
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::{Painter, TEXT, UiRect};
    use crate::text::TextEngine;
    use tiny_skia::Pixmap;

    #[test]
    fn rect_math_includes_edges_and_insets() {
        let rect = UiRect::new(10.0, 20.0, 100.0, 40.0);
        assert!(rect.contains(10.0, 20.0));
        assert!(rect.contains(110.0, 60.0));
        assert!(!rect.contains(111.0, 60.0));

        let inset = rect.inset(5.0);
        assert_eq!(inset, UiRect::new(15.0, 25.0, 90.0, 30.0));
    }

    #[test]
    fn painting_at_scale_inks_the_expected_pixels() {
        let mut pixmap = Pixmap::new(200, 100).expect("allocates");
        let mut engine = TextEngine::new();
        {
            let mut painter = Painter::new(&mut pixmap, &mut engine, 2.0);
            painter.clear([0, 0, 0, 255]);
            painter.fill_rect(UiRect::new(10.0, 10.0, 20.0, 10.0), [255, 0, 0, 255]);
            painter.text("A", 10.0, 30.0, 12.0, TEXT);
        }

        // Logical (10,10) means device (20,20).
        let pixel = pixmap.pixel(20, 20).expect("in bounds");
        assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (255, 0, 0));
        assert_eq!(
            pixmap.pixel(10, 10).expect("in bounds").red(),
            0,
            "logical origin was scaled"
        );
    }

    #[test]
    fn measuring_is_positive_and_monotonic() {
        let mut engine = TextEngine::new();
        let short = engine.measure("Edit", 13.0);
        let long = engine.measure("Edit a really long label", 13.0);
        assert!(short > 0.0);
        assert!(long > short);
    }
}
