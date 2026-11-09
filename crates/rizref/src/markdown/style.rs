//! `<span style="…">` declarations and the note colour rules.
//!
//! Only two CSS properties are honoured, and only on `<span>`: `color` (any
//! syntax `csscolorparser` accepts) and `font-size` in `px`. Everything else
//! is ignored. That keeps the "HTML" in a note safe by construction: there is
//! no HTML engine, no stylesheet and no URL anywhere in this path.
//!
//! The contrast helpers implement the WCAG relative-luminance formula. A note's
//! default ink is whichever of two fixed inks contrasts more with its
//! background; text with an explicit span colour is never adjusted.

/// Smallest honoured `font-size`, in world units at 100% zoom.
pub const MIN_SPAN_FONT_SIZE: f32 = 6.0;

/// Largest honoured `font-size`, in world units at 100% zoom.
pub const MAX_SPAN_FONT_SIZE: f32 = 200.0;

/// The warm light ink used on dark backgrounds, `#f2eee8`.
pub const LIGHT_INK: [u8; 3] = [0xf2, 0xee, 0xe8];

/// The near-black ink used on light backgrounds, `#1b1b1b`.
pub const DARK_INK: [u8; 3] = [0x1b, 0x1b, 0x1b];

/// The application accent, used for links when it contrasts enough.
pub const ACCENT: [u8; 3] = [0xff, 0xce, 0x6e];

/// The accent adapted for light backgrounds.
pub const DARK_ACCENT: [u8; 3] = [0x1f, 0x5f, 0xbf];

/// What one `<span style>` asks for.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpanStyle {
    /// An explicit straight-alpha RGBA colour.
    pub color: Option<[u8; 4]>,
    /// A font size in world units, already clamped.
    pub font_size: Option<f32>,
}

impl SpanStyle {
    /// `inner` layered over `self`: inner declarations win for their range.
    pub fn layered(self, inner: Self) -> Self {
        Self {
            color: inner.color.or(self.color),
            font_size: inner.font_size.or(self.font_size),
        }
    }
}

/// Parses a `style` attribute value. Unknown properties and invalid values
/// are ignored; the last valid duplicate wins.
pub fn parse_declarations(style: &str) -> SpanStyle {
    let mut out = SpanStyle::default();
    for declaration in style.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let value = value
            .strip_suffix("!important")
            .map_or(value, str::trim_end);
        match property.trim().to_ascii_lowercase().as_str() {
            "color" => match parse_color(value) {
                Some(color) => out.color = Some(color),
                None => tracing::debug!(value, "ignoring an invalid span colour"),
            },
            "font-size" => match parse_font_size(value) {
                Some(size) => out.font_size = Some(size),
                None => tracing::debug!(value, "ignoring an unsupported span font-size"),
            },
            _ => {}
        }
    }
    out
}

/// A CSS colour as straight-alpha RGBA.
pub fn parse_color(value: &str) -> Option<[u8; 4]> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 {
        return None;
    }
    csscolorparser::parse(value)
        .ok()
        .map(|color| color.clamp().to_rgba8())
}

/// A `px` font size, clamped to the supported range. `%`, `em`, `rem`,
/// viewport units and bare numbers are not accepted.
pub fn parse_font_size(value: &str) -> Option<f32> {
    let number = value.trim().strip_suffix("px")?.trim_end();
    let size: f32 = number.parse().ok()?;
    size.is_finite()
        .then(|| size.clamp(MIN_SPAN_FONT_SIZE, MAX_SPAN_FONT_SIZE))
}

/// Extracts the `style` attribute from an opening `<span …>` tag, or
/// `Some("")` for a bare `<span>`. `None` when `tag` is not a span opener.
pub fn span_open(tag: &str) -> Option<&str> {
    let rest = tag.strip_prefix('<')?.strip_suffix('>')?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let name_len = rest
        .find(|c: char| c.is_ascii_whitespace())
        .unwrap_or(rest.len());
    if !rest[..name_len].eq_ignore_ascii_case("span") {
        return None;
    }
    let mut attrs = rest[name_len..].trim_start();
    let mut style = "";
    while !attrs.is_empty() {
        let name_end = attrs
            .find(|c: char| c == '=' || c.is_ascii_whitespace())
            .unwrap_or(attrs.len());
        let name = &attrs[..name_end];
        attrs = attrs[name_end..].trim_start();
        let mut value = "";
        if let Some(after) = attrs.strip_prefix('=') {
            let after = after.trim_start();
            let (parsed, rest) = match after.chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let body = &after[1..];
                    let end = body.find(quote)?;
                    (&body[..end], &body[end + 1..])
                }
                _ => {
                    let end = after
                        .find(|c: char| c.is_ascii_whitespace())
                        .unwrap_or(after.len());
                    (&after[..end], &after[end..])
                }
            };
            value = parsed;
            attrs = rest.trim_start();
        }
        if name.eq_ignore_ascii_case("style") {
            style = value;
        }
        if name.is_empty() {
            break;
        }
    }
    Some(style)
}

/// Whether `tag` is a closing `</span>`.
pub fn is_span_close(tag: &str) -> bool {
    tag.strip_prefix("</")
        .and_then(|rest| rest.strip_suffix('>'))
        .is_some_and(|name| name.trim().eq_ignore_ascii_case("span"))
}

/// WCAG relative luminance of an sRGB colour.
pub fn relative_luminance(rgb: [u8; 3]) -> f32 {
    let channel = |value: u8| {
        let c = f32::from(value) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2])
}

/// WCAG contrast ratio between two colours, `1..=21`.
pub fn contrast_ratio(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (light, dark) = if la > lb { (la, lb) } else { (lb, la) };
    (light + 0.05) / (dark + 0.05)
}

/// The colours a note derives from its background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotePalette {
    /// The card colour.
    pub background: [u8; 3],
    /// Default text colour.
    pub ink: [u8; 3],
    /// Whether the ink is the light one (the background is dark).
    pub light_ink: bool,
    /// Link colour, or `None` to use underlined ink.
    pub link: Option<[u8; 3]>,
    /// The card border.
    pub border: [u8; 3],
    /// Caret colour while editing.
    pub caret: [u8; 3],
    /// Selection highlight while editing.
    pub selection: [u8; 4],
}

impl NotePalette {
    /// Derives every colour from `background`.
    pub fn for_background(background: [u8; 3]) -> Self {
        let light = contrast_ratio(LIGHT_INK, background);
        let dark = contrast_ratio(DARK_INK, background);
        let light_ink = light >= dark;
        let ink = if light_ink { LIGHT_INK } else { DARK_INK };
        let accent = if light_ink { ACCENT } else { DARK_ACCENT };
        let link = (contrast_ratio(accent, background) >= 3.0).then_some(accent);
        Self {
            background,
            ink,
            light_ink,
            link,
            border: mix(background, ink, 0.28),
            caret: accent,
            selection: if light_ink {
                [66, 92, 136, 220]
            } else {
                [150, 190, 245, 220]
            },
        }
    }

    /// `ink` with an alpha, straight RGBA.
    pub fn ink_alpha(&self, alpha: f32) -> [u8; 4] {
        [
            self.ink[0],
            self.ink[1],
            self.ink[2],
            (255.0 * alpha.clamp(0.0, 1.0)).round() as u8,
        ]
    }
}

/// Linear blend from `a` towards `b` by `t`.
pub fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    [lerp(a[0], b[0]), lerp(a[1], b[1]), lerp(a[2], b[2])]
}

#[cfg(test)]
mod tests {
    use super::{
        DARK_INK, LIGHT_INK, NotePalette, SpanStyle, contrast_ratio, is_span_close,
        parse_declarations, parse_font_size, span_open,
    };

    #[test]
    fn the_requested_examples_parse() {
        let red = parse_declarations(span_open(r#"<span style="color: red;">"#).expect("span"));
        assert_eq!(red.color, Some([255, 0, 0, 255]));
        let hex = parse_declarations(span_open(r#"<span style="color: #ff5733;">"#).expect("span"));
        assert_eq!(hex.color, Some([0xff, 0x57, 0x33, 255]));
        let size = parse_declarations("font-size: 24px");
        assert_eq!(size.font_size, Some(24.0));
        assert_eq!(size.color, None);
    }

    #[test]
    fn colour_syntaxes_keep_their_alpha() {
        assert_eq!(
            parse_declarations("color:#0f08").color,
            Some([0, 255, 0, 136])
        );
        assert_eq!(
            parse_declarations("color: rgba(10, 20, 30, 0.5)").color,
            Some([10, 20, 30, 128])
        );
        assert_eq!(
            parse_declarations("color: hsl(0, 100%, 50%)").color,
            Some([255, 0, 0, 255])
        );
        assert_eq!(parse_declarations("color: nonsense").color, None);
    }

    #[test]
    fn only_px_sizes_are_honoured_and_clamped() {
        assert_eq!(parse_font_size("2px"), Some(6.0));
        assert_eq!(parse_font_size("900px"), Some(200.0));
        assert_eq!(parse_font_size(" 12.5px "), Some(12.5));
        for bad in ["12", "1.5em", "2rem", "50%", "10vw", "NaNpx", "px"] {
            assert_eq!(parse_font_size(bad), None, "{bad}");
        }
    }

    #[test]
    fn unknown_properties_are_ignored_and_the_last_duplicate_wins() {
        let style = parse_declarations(
            "background: url(http://x); color: red; position: fixed; color: blue; ;;; :x",
        );
        assert_eq!(style.color, Some([0, 0, 255, 255]));
        assert_eq!(style.font_size, None);
    }

    #[test]
    fn inner_spans_override_outer_ones_per_property() {
        let outer = SpanStyle {
            color: Some([1, 2, 3, 255]),
            font_size: Some(20.0),
        };
        let inner = SpanStyle {
            color: Some([9, 9, 9, 255]),
            font_size: None,
        };
        assert_eq!(
            outer.layered(inner),
            SpanStyle {
                color: Some([9, 9, 9, 255]),
                font_size: Some(20.0),
            }
        );
    }

    #[test]
    fn span_tags_are_recognised_strictly() {
        assert_eq!(span_open("<span>"), Some(""));
        assert_eq!(
            span_open("<SPAN class='a' style='color:red'>"),
            Some("color:red")
        );
        assert_eq!(span_open("<span style=color:red>"), Some("color:red"));
        assert_eq!(span_open("<spanx>"), None);
        assert_eq!(span_open("<div style='color:red'>"), None);
        assert_eq!(
            span_open("<span style=\"color:red>"),
            None,
            "unterminated quote"
        );
        assert!(is_span_close("</span>"));
        assert!(is_span_close("</SPAN >"));
        assert!(!is_span_close("</div>"));
    }

    #[test]
    fn the_ink_contrasts_with_the_background() {
        let dark = NotePalette::for_background([0x30, 0x2c, 0x25]);
        assert_eq!(dark.ink, LIGHT_INK);
        assert!(dark.light_ink);
        let light = NotePalette::for_background([0xfa, 0xf6, 0xe0]);
        assert_eq!(light.ink, DARK_INK);
        assert!(contrast_ratio(light.ink, light.background) > 4.5);
        for rgb in [
            [0, 0, 0],
            [255, 255, 255],
            [128, 128, 128],
            [255, 0, 0],
            [0, 0, 255],
        ] {
            let palette = NotePalette::for_background(rgb);
            // The better of the two inks; pure red tops out near 4.3:1.
            assert!(contrast_ratio(palette.ink, rgb) >= 4.0, "{rgb:?}");
            if let Some(link) = palette.link {
                assert!(contrast_ratio(link, rgb) >= 3.0, "{rgb:?}");
            }
        }
    }
}
