//! Styled runs for text items: bold, italic, underline and colour over byte
//! ranges of a plain string.
//!
//! A text item stores its words as ordinary UTF-8 (`text`) plus a list of
//! [`TextSpan`]s. Keeping the words plain means search, accessibility and an
//! older RizRef (which reads an unknown format as plain text) all still see
//! the text; only the styling is extra.
//!
//! Spans are kept *normalised*: sorted, non-overlapping, non-empty, on char
//! boundaries, never the default style, and with equal neighbours merged.
//! Every operation here returns a normalised list, so two texts that look the
//! same compare equal and hash to the same texture.

use serde::{Deserialize, Serialize};

/// The style of one run of characters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextStyle {
    /// Heavy weight.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    /// Slanted face.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    /// A line under the glyphs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    /// An explicit colour; `None` is the item's default ink.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    /// An explicit font size in world units; `None` is
    /// [`DEFAULT_TEXT_SIZE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u16>,
}

/// The font size of text without an explicit size, in world units.
pub const DEFAULT_TEXT_SIZE: u16 = 24;

/// Smallest stored font size, in world units.
pub const MIN_TEXT_SIZE: u16 = 4;

/// Largest stored font size, in world units.
pub const MAX_TEXT_SIZE: u16 = 1000;

/// The sizes the larger/smaller buttons step through.
pub const TEXT_SIZE_STEPS: [u16; 22] = [
    6, 8, 10, 12, 14, 16, 18, 20, 24, 28, 32, 36, 40, 48, 56, 64, 72, 96, 128, 160, 200, 256,
];

/// The next size up (`larger`) or down from `size`, past the ends of
/// [`TEXT_SIZE_STEPS`] by a quarter at a time.
pub fn step_text_size(size: u16, larger: bool) -> u16 {
    let next = if larger {
        TEXT_SIZE_STEPS
            .iter()
            .copied()
            .find(|step| *step > size)
            .unwrap_or_else(|| size.saturating_add((size / 4).max(1)))
    } else {
        TEXT_SIZE_STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < size)
            .unwrap_or_else(|| size.saturating_sub((size / 4).max(1)))
    };
    next.clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE)
}

/// A stored size: clamped, with the default spelled as `None`.
pub fn normalize_size(size: Option<u16>) -> Option<u16> {
    size.map(|size| size.clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE))
        .filter(|size| *size != DEFAULT_TEXT_SIZE)
}

/// `spans` with every character's font size multiplied by `factor`
/// (characters at the default size included), rounded and clamped: what a
/// text item's content becomes when it is scaled by `factor`.
pub fn scale_sizes(text: &str, spans: &[TextSpan], factor: f32) -> Vec<TextSpan> {
    if !factor.is_finite() || factor <= 0.0 {
        return spans.to_vec();
    }
    apply_style(text, spans, 0..text.len(), |style| {
        let scaled = (f32::from(style.size_or_default()) * factor)
            .round()
            .clamp(f32::from(MIN_TEXT_SIZE), f32::from(MAX_TEXT_SIZE));
        style.size = normalize_size(Some(scaled as u16));
    })
}

impl TextStyle {
    /// The font size in world units.
    pub fn size_or_default(&self) -> u16 {
        self.size.unwrap_or(DEFAULT_TEXT_SIZE)
    }

    /// Whether this is the unstyled default.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A style over the byte range `start..end` of the item's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextSpan {
    /// First byte, inclusive.
    pub start: usize,
    /// Last byte, exclusive.
    pub end: usize,
    /// The style of every character in the range.
    #[serde(flatten)]
    pub style: TextStyle,
}

/// The style of the character starting at byte `index`.
pub fn style_at(spans: &[TextSpan], index: usize) -> TextStyle {
    spans
        .iter()
        .find(|span| span.start <= index && index < span.end)
        .map_or_else(TextStyle::default, |span| span.style)
}

/// The style a caret at byte `index` types with: the character before it,
/// or the one after it at the start of the text.
pub fn typing_style(text: &str, spans: &[TextSpan], index: usize) -> TextStyle {
    let index = floor_char(text, index.min(text.len()));
    match text[..index].char_indices().next_back() {
        Some((before, _)) => style_at(spans, before),
        None => style_at(spans, 0),
    }
}

/// Whether every character in `range` satisfies `test` (an empty range asks
/// about nothing and is `false`).
pub fn all_in_range(
    text: &str,
    spans: &[TextSpan],
    range: std::ops::Range<usize>,
    test: impl Fn(&TextStyle) -> bool,
) -> bool {
    let start = floor_char(text, range.start.min(text.len()));
    let end = ceil_char(text, range.end.min(text.len()));
    if start >= end {
        return false;
    }
    // Line breaks are never styled (each line keeps its own styling), so a
    // selection across lines must not be judged by them: counting the
    // unstyled `\n` made "all bold" false, and the toggle turned bold on
    // again instead of off.
    let mut cuts = vec![start, end];
    for (index, _) in text[start..end].match_indices('\n') {
        cuts.push(start + index);
        cuts.push(start + index + 1);
    }
    let mut judged = segments(text, spans, &cuts)
        .into_iter()
        .filter(|(s, e, _)| *s >= start && *e <= end)
        .filter(|(s, e, _)| text[*s..*e].chars().any(|c| c != '\n'))
        .peekable();
    judged.peek().is_some() && judged.all(|(_, _, style)| test(&style))
}

/// Applies `change` to the style of every character in `range`.
pub fn apply_style(
    text: &str,
    spans: &[TextSpan],
    range: std::ops::Range<usize>,
    change: impl Fn(&mut TextStyle),
) -> Vec<TextSpan> {
    let start = floor_char(text, range.start.min(text.len()));
    let end = ceil_char(text, range.end.min(text.len()));
    let mut out = Vec::new();
    for (s, e, mut style) in segments(text, spans, &[start, end]) {
        if s >= start && e <= end {
            change(&mut style);
        }
        out.push(TextSpan {
            start: s,
            end: e,
            style,
        });
    }
    normalize(text, out)
}

/// Sorts, clamps, de-overlaps (a later span wins) and merges `spans`.
pub fn normalize(text: &str, spans: Vec<TextSpan>) -> Vec<TextSpan> {
    let clamped: Vec<TextSpan> = spans
        .into_iter()
        .map(|span| TextSpan {
            start: floor_char(text, span.start.min(text.len())),
            end: ceil_char(text, span.end.min(text.len())),
            style: TextStyle {
                size: normalize_size(span.style.size),
                ..span.style
            },
        })
        .filter(|span| span.start < span.end)
        .collect();
    // Paint the spans in order onto the boundary segments: later ones win.
    let mut cuts: Vec<usize> = clamped.iter().flat_map(|s| [s.start, s.end]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out: Vec<TextSpan> = Vec::new();
    for pair in cuts.windows(2) {
        let (s, e) = (pair[0], pair[1]);
        let style = clamped
            .iter()
            .rev()
            .find(|span| span.start <= s && e <= span.end)
            .map(|span| span.style);
        let Some(style) = style.filter(|style| !style.is_default()) else {
            continue;
        };
        match out.last_mut() {
            Some(last) if last.end == s && last.style == style => last.end = e,
            _ => out.push(TextSpan {
                start: s,
                end: e,
                style,
            }),
        }
    }
    out
}

/// Maps spans through an edit that replaced `removed` bytes at `at` with
/// `inserted` bytes, as one splice of the text. Used to keep styling attached
/// to its words when plain text is edited outside the styled editor.
pub fn splice(spans: &[TextSpan], at: usize, removed: usize, inserted: usize) -> Vec<TextSpan> {
    let end = at + removed;
    let shift = |index: usize| -> usize {
        if index <= at {
            index
        } else if index >= end {
            index - removed + inserted
        } else {
            at
        }
    };
    spans
        .iter()
        .map(|span| TextSpan {
            start: shift(span.start),
            end: shift(span.end),
            style: span.style,
        })
        .filter(|span| span.start < span.end)
        .collect()
}

/// The text cut into `(start, end, style)` pieces at every span boundary and
/// at `extra` cuts, covering the whole text.
fn segments(text: &str, spans: &[TextSpan], extra: &[usize]) -> Vec<(usize, usize, TextStyle)> {
    let mut cuts = vec![0, text.len()];
    cuts.extend(spans.iter().flat_map(|s| [s.start, s.end]));
    cuts.extend(extra.iter().copied());
    cuts.retain(|cut| *cut <= text.len());
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .map(|pair| (pair[0], pair[1], style_at(spans, pair[0])))
        .collect()
}

fn floor_char(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::{
        TextSpan, TextStyle, all_in_range, apply_style, normalize, splice, style_at, typing_style,
    };

    const BOLD: TextStyle = TextStyle {
        bold: true,
        italic: false,
        underline: false,
        color: None,
        size: None,
    };

    fn span(start: usize, end: usize, style: TextStyle) -> TextSpan {
        TextSpan { start, end, style }
    }

    #[test]
    fn applying_bold_to_a_word_makes_one_span() {
        let text = "hello world";
        let spans = apply_style(text, &[], 6..11, |s| s.bold = true);
        assert_eq!(spans, vec![span(6, 11, BOLD)]);
        assert!(style_at(&spans, 7).bold);
        assert!(!style_at(&spans, 2).bold);
    }

    #[test]
    fn neighbours_with_the_same_style_merge_and_defaults_vanish() {
        let text = "abcdef";
        let spans = apply_style(text, &[span(0, 3, BOLD)], 3..6, |s| s.bold = true);
        assert_eq!(spans, vec![span(0, 6, BOLD)]);
        let cleared = apply_style(text, &spans, 0..6, |s| s.bold = false);
        assert!(cleared.is_empty());
    }

    #[test]
    fn a_change_inside_a_span_splits_it() {
        let text = "abcdef";
        let spans = apply_style(text, &[span(0, 6, BOLD)], 2..4, |s| s.italic = true);
        assert_eq!(spans.len(), 3);
        assert!(spans.iter().all(|s| s.style.bold));
        assert!(spans[1].style.italic && spans[1].start == 2 && spans[1].end == 4);
    }

    #[test]
    fn ranges_snap_to_char_boundaries() {
        let text = "aé b";
        // Byte 2 is inside 'é' (bytes 1..3).
        let spans = apply_style(text, &[], 2..3, |s| s.underline = true);
        assert_eq!(spans[0].start, 1);
        assert_eq!(spans[0].end, 3);
        let normal = normalize(text, vec![span(0, 99, BOLD)]);
        assert_eq!(normal, vec![span(0, text.len(), BOLD)]);
    }

    #[test]
    fn all_in_range_decides_the_toggle_direction() {
        let text = "abcdef";
        let spans = vec![span(0, 3, BOLD)];
        assert!(all_in_range(text, &spans, 0..3, |s| s.bold));
        assert!(!all_in_range(text, &spans, 0..4, |s| s.bold));
        assert!(!all_in_range(text, &spans, 2..2, |s| s.bold));
    }

    #[test]
    fn a_selection_across_lines_ignores_the_unstyled_line_breaks() {
        // "Roses are red,\nRoses are blue" with each line bold, as the editor
        // stores it: the newline between them carries no style.
        let text = "ab\ncd";
        let spans = vec![span(0, 2, BOLD), span(3, 5, BOLD)];
        assert!(all_in_range(text, &spans, 0..5, |s| s.bold));
        // One unbolded letter still makes the whole selection "not all".
        let spans = vec![span(0, 2, BOLD), span(4, 5, BOLD)];
        assert!(!all_in_range(text, &spans, 0..5, |s| s.bold));
        // A selection of only a line break has nothing to judge.
        assert!(!all_in_range(text, &[], 2..3, |s| !s.bold));
        // Blank lines in the middle are skipped too.
        let text = "ab\n\ncd";
        let spans = vec![span(0, 2, BOLD), span(4, 6, BOLD)];
        assert!(all_in_range(text, &spans, 0..6, |s| s.bold));
    }

    #[test]
    fn typing_continues_the_style_before_the_caret() {
        let text = "abcdef";
        let spans = vec![span(0, 3, BOLD)];
        assert!(typing_style(text, &spans, 3).bold);
        assert!(!typing_style(text, &spans, 4).bold);
        assert!(
            typing_style(text, &spans, 0).bold,
            "start takes the first char"
        );
    }

    #[test]
    fn splice_moves_spans_with_their_words() {
        let spans = vec![span(4, 8, BOLD)];
        assert_eq!(splice(&spans, 0, 0, 2), vec![span(6, 10, BOLD)]);
        assert_eq!(splice(&spans, 4, 4, 0), vec![]);
        assert_eq!(splice(&spans, 10, 0, 3), spans);
    }

    #[test]
    fn colour_serialises_compactly() {
        let style = TextStyle {
            color: Some([255, 0, 0]),
            ..TextStyle::default()
        };
        let json = serde_json::to_string(&style).expect("serialises");
        assert_eq!(json, r#"{"color":[255,0,0]}"#);
    }

    #[test]
    fn scaling_multiplies_every_size_including_the_default() {
        use super::scale_sizes;
        let text = "big small";
        let spans = vec![span(
            0,
            3,
            TextStyle {
                size: Some(48),
                ..TextStyle::default()
            },
        )];
        let scaled = scale_sizes(text, &spans, 2.0);
        assert_eq!(style_at(&scaled, 0).size, Some(96));
        assert_eq!(style_at(&scaled, 5).size, Some(48), "24 doubled");
        let back = scale_sizes(text, &scaled, 0.5);
        assert_eq!(back, spans, "halving undoes it");
        assert_eq!(scale_sizes(text, &spans, f32::NAN), spans);
    }

    #[test]
    fn sizes_step_and_the_default_is_stored_as_none() {
        use super::{DEFAULT_TEXT_SIZE, MAX_TEXT_SIZE, normalize_size, step_text_size};
        assert_eq!(step_text_size(24, true), 28);
        assert_eq!(step_text_size(24, false), 20);
        assert_eq!(step_text_size(25, false), 24);
        assert_eq!(step_text_size(256, true), 320);
        assert_eq!(step_text_size(6, false), 5);
        assert_eq!(step_text_size(4, false), 4);
        assert_eq!(step_text_size(MAX_TEXT_SIZE, true), MAX_TEXT_SIZE);
        assert_eq!(normalize_size(Some(DEFAULT_TEXT_SIZE)), None);
        assert_eq!(normalize_size(Some(1)), Some(4));
        let text = "abc";
        let spans = apply_style(text, &[], 0..3, |s| s.size = Some(DEFAULT_TEXT_SIZE));
        assert!(spans.is_empty(), "the default size is no style at all");
    }
}
