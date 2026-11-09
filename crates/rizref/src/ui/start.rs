//! The launch state: wordmark, drop zone, quick actions and recent boards.
//!
//! Layout is computed once per size ([`StartLayout`]) and used by both the
//! painter and hit-testing, so a button can never be drawn in one place and
//! clicked in another. Cards read the [`Recents`] store and draw the board
//! thumbnail recorded there; a card without one (or whose bytes do not
//! decode) draws a monogram placeholder.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use tiny_skia::Pixmap;

use crate::recents::Recents;
use crate::ui::menu::Action;
use crate::ui::paint::{
    ACCENT, DIM_TEXT, DROP_BORDER, PANEL_BG, PANEL_BORDER, Painter, TEXT, UiRect, WINDOW_BG,
};

/// The content width cap.
const CONTENT_MAX: f32 = 980.0;

/// Card size.
const CARD_WIDTH: f32 = 216.0;
/// Card height.
const CARD_HEIGHT: f32 = 158.0;
/// Gap between cards and rows.
const CARD_GAP: f32 = 16.0;

/// A clickable region on the start view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartControl {
    /// Create an empty board.
    NewBoard,
    /// Open the image dialog.
    OpenImages,
    /// Open a board file.
    OpenBoard,
    /// Open a recent board.
    Recent(usize),
}

/// The start view geometry for one viewport size.
#[derive(Debug, Clone)]
pub struct StartLayout {
    /// The content column.
    pub content: UiRect,
    /// The large drop target.
    pub drop_zone: UiRect,
    /// The quick-action buttons.
    pub buttons: Vec<(UiRect, StartControl)>,
    /// Recent-board cards, in list order.
    pub cards: Vec<UiRect>,
    /// Top of the "Recent boards" heading.
    pub heading_y: f32,
    /// Baseline of the wordmark.
    pub wordmark_y: f32,
    /// Baseline of the tagline.
    pub tagline_y: f32,
    /// Whether the footer hint fits below the content.
    pub footer_visible: bool,
}

impl StartLayout {
    /// Computes the layout for a viewport in logical pixels.
    pub fn new(viewport: glam::DVec2, recent_count: usize) -> Self {
        let width = (viewport.x - 96.0).clamp(360.0, f64::from(CONTENT_MAX)) as f32;
        let x = ((viewport.x - f64::from(width)) * 0.5) as f32;
        let content = UiRect::new(x, 0.0, width, viewport.y as f32);
        let compact = viewport.y < 560.0;
        let wordmark_y = if compact { 26.0 } else { 46.0 };
        let tagline_y = wordmark_y + 34.0;
        let drop_zone = UiRect::new(
            x,
            if compact { 86.0 } else { 128.0 },
            width,
            if compact { 112.0 } else { 146.0 },
        );

        let button_width = 150.0;
        let button_height = if compact { 34.0 } else { 38.0 };
        let gap = 12.0;
        let total = button_width * 3.0 + gap * 2.0;
        let start_x = content.x + (content.width - total) * 0.5;
        let y = drop_zone.bottom() + if compact { 16.0 } else { 18.0 };
        let buttons = vec![
            (
                UiRect::new(start_x, y, button_width, button_height),
                StartControl::NewBoard,
            ),
            (
                UiRect::new(start_x + button_width + gap, y, button_width, button_height),
                StartControl::OpenImages,
            ),
            (
                UiRect::new(
                    start_x + (button_width + gap) * 2.0,
                    y,
                    button_width,
                    button_height,
                ),
                StartControl::OpenBoard,
            ),
        ];

        let heading_y = y + button_height + if compact { 20.0 } else { 38.0 };
        let card_top = heading_y + if compact { 22.0 } else { 28.0 };
        let available = (viewport.y as f32 - card_top - 14.0).max(48.0);
        let card_height = available.min(CARD_HEIGHT);
        let columns =
            (((content.width + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).floor() as usize).clamp(1, 4);
        let mut cards = Vec::new();
        for index in 0..recent_count {
            let column = index % columns;
            let row = index / columns;
            cards.push(UiRect::new(
                content.x + column as f32 * (CARD_WIDTH + CARD_GAP),
                card_top + row as f32 * (card_height + CARD_GAP),
                CARD_WIDTH,
                card_height,
            ));
        }

        Self {
            content,
            drop_zone,
            buttons,
            cards,
            heading_y,
            wordmark_y,
            tagline_y,
            footer_visible: viewport.y > 560.0,
        }
    }

    /// The control at `position`, if any.
    pub fn hit(&self, position: glam::DVec2) -> Option<StartControl> {
        let (x, y) = (position.x as f32, position.y as f32);
        for (rect, control) in &self.buttons {
            if rect.contains(x, y) {
                return Some(*control);
            }
        }
        for (index, rect) in self.cards.iter().enumerate() {
            if rect.contains(x, y) {
                return Some(StartControl::Recent(index));
            }
        }
        None
    }

    /// The index of the card at `position`, for the card context menu.
    pub fn card_at(&self, position: glam::DVec2) -> Option<usize> {
        self.cards
            .iter()
            .position(|rect| rect.contains(position.x as f32, position.y as f32))
    }
}

/// Paints the whole start view.
pub fn paint(
    painter: &mut Painter,
    viewport: glam::DVec2,
    recents: &Recents,
    layout: &StartLayout,
) {
    painter.clear(WINDOW_BG);

    // A very soft vertical tint so the empty page has a horizon.
    painter.fill_rect(
        UiRect::new(0.0, 0.0, viewport.x as f32, viewport.y as f32 * 0.55),
        [30, 31, 38, 255],
    );

    painter.text("RizRef", 64.0, layout.wordmark_y, 31.0, TEXT);
    painter.text(
        "A reference board that stays out of your way",
        65.0,
        layout.tagline_y,
        13.5,
        DIM_TEXT,
    );

    // Drop zone.
    painter.fill_round_rect(layout.drop_zone, 12.0, [31, 33, 41, 255]);
    painter.stroke_round_rect(layout.drop_zone, 12.0, 1.0, DROP_BORDER);
    let height = layout.drop_zone.height;
    painter.text_centered(
        "Drop images here",
        layout.drop_zone,
        layout.drop_zone.y + height * 0.32,
        18.0,
        TEXT,
    );
    painter.text_centered(
        "or use Browse — they land on a fresh board",
        layout.drop_zone,
        layout.drop_zone.y + height * 0.53,
        12.5,
        DIM_TEXT,
    );
    painter.text_centered(
        "PNG · JPEG · WebP · GIF · BMP · TGA · PNM · QOI · ICO · HEIC · AVIF",
        layout.drop_zone,
        layout.drop_zone.y + height * 0.72,
        10.0,
        [122, 127, 140, 255],
    );

    // Quick actions.
    for (rect, control) in &layout.buttons {
        let (label, primary) = match control {
            StartControl::NewBoard => ("New board", true),
            StartControl::OpenImages => ("Open images…", false),
            StartControl::OpenBoard => ("Open board…", false),
            StartControl::Recent(_) => continue,
        };
        if primary {
            painter.fill_round_rect(*rect, 8.0, ACCENT);
            painter.text_centered(label, *rect, rect.y + 10.0, 13.0, [255, 255, 255, 255]);
        } else {
            painter.fill_round_rect(*rect, 8.0, PANEL_BG);
            painter.stroke_round_rect(*rect, 8.0, 1.0, PANEL_BORDER);
            painter.text_centered(label, *rect, rect.y + 10.0, 13.0, TEXT);
        }
    }

    painter.text(
        "Recent boards",
        layout.content.x,
        layout.heading_y,
        15.0,
        TEXT,
    );

    if recents.is_empty() {
        painter.text(
            "No recent boards yet — your saved boards will appear here.",
            layout.content.x,
            layout.heading_y + 32.0,
            12.5,
            DIM_TEXT,
        );
    }

    let now = crate::recents::now_unix();
    for (index, rect) in layout.cards.iter().enumerate() {
        let Some(entry) = recents.entries.get(index) else {
            break;
        };
        painter.fill_round_rect(*rect, 9.0, [33, 35, 43, 255]);
        painter.stroke_round_rect(*rect, 9.0, 1.0, PANEL_BORDER);
        let preview = UiRect::new(
            rect.x + 8.0,
            rect.y + 8.0,
            rect.width - 16.0,
            (rect.height - 58.0).max(24.0),
        );
        let monogram = entry
            .display_name()
            .chars()
            .next()
            .map_or("R".to_owned(), |first| first.to_uppercase().to_string());
        match entry.preview.as_deref().and_then(preview_pixmap) {
            Some(pixmap) => {
                painter.fill_round_rect(preview, 6.0, [46, 49, 60, 255]);
                painter.image(&pixmap, preview);
            }
            None => painter.thumbnail_placeholder(preview, &monogram),
        }
        painter.text(
            &entry.display_name(),
            rect.x + 10.0,
            rect.y + rect.height - 48.0,
            12.5,
            TEXT,
        );
        painter.text(
            &format!("{} items · {}", entry.item_count, entry.edited_label(now)),
            rect.x + 10.0,
            rect.y + rect.height - 27.0,
            10.5,
            DIM_TEXT,
        );
    }

    if layout.footer_visible {
        painter.text(
            "F1 shortcuts · Ctrl+O open images · right-click a recent board to remove it",
            layout.content.x,
            viewport.y as f32 - 30.0,
            10.5,
            [124, 128, 140, 255],
        );
        painter.text_right(
            &format!("rizref {}", env!("CARGO_PKG_VERSION")),
            viewport.x as f32 - 28.0,
            viewport.y as f32 - 30.0,
            10.5,
            [124, 128, 140, 255],
        );
    }
}

/// The action a control runs.
pub fn action_for(control: StartControl, recents: &Recents) -> Option<Action> {
    match control {
        StartControl::NewBoard => Some(Action::NewBoard),
        StartControl::OpenImages => Some(Action::OpenImages),
        StartControl::OpenBoard => Some(Action::OpenBoard),
        StartControl::Recent(index) => recents
            .entries
            .get(index)
            .map(|entry| Action::OpenRecent(entry.path_buf())),
    }
}

thread_local! {
    /// Decoded card previews, keyed by a hash of their encoded bytes, so a
    /// repaint does not decode a dozen JPEGs. Painting happens on the UI
    /// thread only.
    static PREVIEWS: RefCell<HashMap<u64, Option<Pixmap>>> = RefCell::new(HashMap::new());
}

/// The card picture for a recent entry's stored preview bytes, decoded once.
fn preview_pixmap(bytes: &[u8]) -> Option<Pixmap> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let key = hasher.finish();
    PREVIEWS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() > 4 * crate::recents::RECENT_LIMIT {
            cache.clear();
        }
        cache
            .entry(key)
            .or_insert_with(|| {
                let image = rizref_codecs::decode(bytes).ok()?;
                // An empty board saves a 1 x 1 thumbnail: the monogram says
                // more than a flat colour.
                if image.width() < 2 || image.height() < 2 {
                    return None;
                }
                crate::ui::paint::pixmap_from_rgba(image.width(), image.height(), image.pixels())
            })
            .clone()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_stored_preview_decodes_for_its_card_and_garbage_does_not() {
        let image = rizref_codecs::ImageBuffer::filled(40, 20, [200, 30, 30, 255]);
        let jpeg = rizref_codecs::encode_jpeg(&image, 80).expect("jpeg");

        let pixmap = super::preview_pixmap(&jpeg).expect("decodes");
        assert_eq!((pixmap.width(), pixmap.height()), (40, 20));
        assert!(
            super::preview_pixmap(b"not an image").is_none(),
            "monogram instead"
        );
    }

    use super::{StartControl, StartLayout};
    use crate::recents::Recents;
    use glam::DVec2;

    #[test]
    fn buttons_and_cards_hit_test_where_they_are_drawn() {
        let layout = StartLayout::new(DVec2::new(1280.0, 800.0), 3);

        for (rect, control) in &layout.buttons {
            assert_eq!(
                layout.hit(DVec2::new(
                    f64::from(rect.x + rect.width * 0.5),
                    f64::from(rect.y + rect.height * 0.5)
                )),
                Some(*control)
            );
        }
        for (index, rect) in layout.cards.iter().enumerate() {
            assert_eq!(
                layout.hit(DVec2::new(f64::from(rect.x + 2.0), f64::from(rect.y + 2.0))),
                Some(StartControl::Recent(index))
            );
        }
        assert_eq!(layout.hit(DVec2::new(5.0, 5.0)), None);
    }

    #[test]
    fn cards_wrap_into_rows_and_stay_inside_the_content_column() {
        let layout = StartLayout::new(DVec2::new(1280.0, 900.0), 7);
        assert_eq!(layout.cards.len(), 7);
        for card in &layout.cards {
            assert!(card.x >= layout.content.x - 0.01);
            assert!(card.right() <= layout.content.right() + 0.01);
        }
        assert!(
            layout.cards[4].y > layout.cards[0].y,
            "the fifth card wraps to a new row"
        );
    }

    #[test]
    fn a_narrow_window_still_has_one_column() {
        let layout = StartLayout::new(DVec2::new(400.0, 700.0), 2);
        assert_eq!(layout.cards.len(), 2);
        assert_eq!(layout.cards[0].x, layout.cards[1].x);
        assert!(layout.cards[1].y > layout.cards[0].y);
    }

    #[test]
    fn recent_actions_carry_the_exact_path() {
        let mut recents = Recents::default();
        recents.record("C:/boards/a.rizref", 1, None, 0);
        let action = super::action_for(StartControl::Recent(0), &recents);
        assert_eq!(
            action,
            Some(crate::ui::menu::Action::OpenRecent(
                "C:/boards/a.rizref".into()
            ))
        );
    }
}
