//! The math backend: RaTeX behind a small, cached facade.
//!
//! RaTeX is KaTeX-compatible and pure Rust. It is a 0.1.x crate, so everything
//! it touches stays in this module: the rest of the note pipeline only sees a
//! [`MathBox`] (em metrics, for layout) and a [`MathMask`] (an alpha coverage
//! mask, for painting). The mask is rendered black-on-transparent and tinted at
//! composite time, so a span colour change never re-rasterises an equation.
//!
//! RaTeX only exposes PNG output, so a mask goes through the PNG decoder that
//! `rizref-codecs` already ships. Its KaTeX fonts are embedded (`embed-fonts`)
//! and loaded lazily by RaTeX on the first render, so a board without math pays
//! nothing at startup.
//!
//! Errors are cached like successes: an invalid equation is parsed once, shows
//! its source in the error style, and is never retried every frame. RaTeX runs
//! inside `catch_unwind`, so a backend panic on hostile input degrades to the
//! same error state instead of taking the application down.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use ratex_layout::{LayoutOptions, layout, to_display_list, validate_layout};
use ratex_render::{RenderOptions, render_to_png};
use ratex_types::color::Color;
use ratex_types::display_item::DisplayList;
use ratex_types::math_style::MathStyle;

/// Commands that would reach outside the note (files, URLs) and are refused.
const REFUSED_COMMANDS: [&str; 5] = [
    "\\includegraphics",
    "\\href",
    "\\url",
    "\\input",
    "\\include",
];

/// Most cached entries of each kind before the cache is cleared.
const CACHE_LIMIT: usize = 1024;

/// An equation's metrics in em (multiples of its font size).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MathBox {
    /// Advance width.
    pub width: f32,
    /// Extent above the baseline.
    pub height: f32,
    /// Extent below the baseline.
    pub depth: f32,
}

/// A rendered equation: coverage only, tinted when composited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathMask {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Distance from the top row to the baseline, in pixels.
    pub baseline: u32,
    /// One coverage byte per pixel, row-major.
    pub alpha: Vec<u8>,
}

type BoxKey = (String, bool);
type MaskKey = (String, bool, u32);

/// Caches equation metrics and masks for every note.
#[derive(Default)]
pub struct MathCache {
    boxes: HashMap<BoxKey, Result<MathBox, String>>,
    masks: HashMap<MaskKey, Option<Arc<MathMask>>>,
}

impl MathCache {
    /// An empty cache.
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    /// The equation's em metrics, or the reason it cannot be typeset.
    pub fn measure(&mut self, tex: &str, display: bool) -> Result<MathBox, String> {
        let key = (tex.to_owned(), display);
        if let Some(result) = self.boxes.get(&key) {
            return result.clone();
        }
        if self.boxes.len() >= CACHE_LIMIT {
            self.boxes.clear();
        }
        let result = display_list(tex, display).map(|list| MathBox {
            width: list.width as f32,
            height: list.height as f32,
            depth: list.depth as f32,
        });
        if let Err(reason) = &result {
            tracing::debug!(tex, reason, "equation falls back to its source");
        }
        self.boxes.insert(key, result.clone());
        result
    }

    /// The equation rendered at `font_px` pixels per em, or `None` when it
    /// cannot be typeset or rasterised.
    pub fn mask(&mut self, tex: &str, display: bool, font_px: f32) -> Option<Arc<MathMask>> {
        let px = font_px.round().clamp(1.0, 2048.0) as u32;
        let key = (tex.to_owned(), display, px);
        if let Some(mask) = self.masks.get(&key) {
            return mask.clone();
        }
        if self.masks.len() >= CACHE_LIMIT {
            self.masks.clear();
        }
        let mask = display_list(tex, display)
            .ok()
            .and_then(|list| rasterise(&list, px as f32))
            .map(Arc::new);
        self.masks.insert(key, mask.clone());
        mask
    }

    /// Number of cached masks.
    #[cfg(test)]
    pub fn mask_count(&self) -> usize {
        self.masks.len()
    }
}

/// Parse and lay out `tex`, guarding against refused commands and panics.
fn display_list(tex: &str, display: bool) -> Result<DisplayList, String> {
    if let Some(command) = REFUSED_COMMANDS
        .iter()
        .find(|command| tex.contains(*command))
    {
        return Err(format!("{command} is not supported in notes"));
    }
    let tex = tex.to_owned();
    catch_unwind(AssertUnwindSafe(move || {
        let ast = ratex_parser::parser::parse(&tex).map_err(|error| error.to_string())?;
        let style = if display {
            MathStyle::Display
        } else {
            MathStyle::Text
        };
        let options = LayoutOptions::default()
            .with_style(style)
            .with_color(Color::BLACK);
        let root = layout(&ast, &options);
        validate_layout(&root)?;
        let list = to_display_list(&root);
        let finite = list.width.is_finite() && list.height.is_finite() && list.depth.is_finite();
        if !finite || list.width < 0.0 {
            return Err("equation has no finite size".to_owned());
        }
        Ok(list)
    }))
    .unwrap_or_else(|_| Err("the math backend panicked".to_owned()))
}

/// Renders a display list to an alpha mask at `font_px` pixels per em.
fn rasterise(list: &DisplayList, font_px: f32) -> Option<MathMask> {
    let options = RenderOptions {
        font_size: font_px,
        padding: 0.0,
        background_color: Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        },
        font_dir: String::new(),
        device_pixel_ratio: 1.0,
    };
    let png = catch_unwind(AssertUnwindSafe(|| render_to_png(list, &options)))
        .ok()?
        .ok()?;
    let image = rizref_codecs::decode(&png).ok()?;
    let alpha = image
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| pixel[3])
        .collect();
    let baseline = (list.height as f32 * font_px).round().max(0.0) as u32;
    Some(MathMask {
        width: image.width(),
        height: image.height(),
        baseline: baseline.min(image.height()),
        alpha,
    })
}

#[cfg(test)]
mod tests {
    use super::MathCache;

    #[test]
    fn a_fraction_typesets_with_embedded_fonts_and_a_transparent_background() {
        // The phase spike: parse → layout → raster with no font directory.
        let mut cache = MathCache::new();
        let metrics = cache.measure("\\frac{a}{b}", true).expect("typesets");
        assert!(metrics.width > 0.3 && metrics.height > 0.5 && metrics.depth > 0.2);

        let mask = cache.mask("\\frac{a}{b}", true, 32.0).expect("rasterises");
        assert_eq!(mask.alpha.len(), (mask.width * mask.height) as usize);
        let covered = mask.alpha.iter().filter(|alpha| **alpha > 128).count();
        let empty = mask.alpha.iter().filter(|alpha| **alpha == 0).count();
        assert!(covered > 20, "glyphs and the fraction bar are drawn");
        assert!(empty > covered, "the background stays transparent");
        assert!(mask.baseline > 0 && mask.baseline < mask.height);
        assert!((mask.height as f32 - (metrics.height + metrics.depth) * 32.0).abs() <= 2.0);
    }

    #[test]
    fn the_acceptance_equations_typeset() {
        let mut cache = MathCache::new();
        let inline = cache.measure("E=mc^2", false).expect("inline");
        assert!(inline.width > 1.5);
        let display = cache
            .measure("\\int_0^1 x^2\\,dx = \\tfrac{1}{3}", true)
            .expect("display");
        assert!(display.height > inline.height, "display limits are taller");
        assert!(cache.mask("E=mc^2", false, 16.0).is_some());
    }

    #[test]
    fn errors_are_cached_and_refused_commands_never_reach_the_backend() {
        let mut cache = MathCache::new();
        assert!(cache.measure("\\frac{a}{", false).is_err());
        assert!(cache.measure("\\frac{a}{", false).is_err(), "cached error");
        assert!(cache.mask("\\frac{a}{", false, 16.0).is_none());
        let refused = cache
            .measure("\\includegraphics{c:/secret.png}", false)
            .expect_err("refused");
        assert!(refused.contains("not supported"));
    }

    #[test]
    fn masks_are_cached_per_pixel_size() {
        let mut cache = MathCache::new();
        let a = cache.mask("x", false, 16.0).expect("mask");
        let b = cache.mask("x", false, 16.2).expect("same rounded size");
        assert!(std::sync::Arc::ptr_eq(&a, &b));
        let c = cache.mask("x", false, 32.0).expect("bigger");
        assert!(c.height > a.height);
        assert_eq!(cache.mask_count(), 2);
    }
}
