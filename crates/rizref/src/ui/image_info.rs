//! The image information dialog: a read-only modal for one image item.
//!
//! [`ImageInfo`] is a snapshot taken when the dialog opens — provenance,
//! format, sizes, hash, animation, preview and placement facts, all from
//! metadata already in memory. Nothing is decoded again: the preview drawn
//! in the panel is the asset's existing preview, copied into a premultiplied
//! pixmap once at open time.
//!
//! [`ImageInfoPanel`] is the widget: a pure layout, keyboard/pointer
//! handling that returns an [`InfoResult`] for the application to act on,
//! and painting through the shared [`Painter`].

use std::time::Duration;

use glam::DVec2;
use rizref_codecs::ImageFormat;
use rizref_core::{GroupChild, ItemId, Scene};
use rizref_format::{AssetOrigin, AssetProvenance};
use tiny_skia::Pixmap;

use crate::location::{self, LocationKind};
use crate::shortcuts::{Key, KeyChord};
use crate::ui::paint::{
    ACCENT, DIM_TEXT, HOVER_BG, PANEL_BORDER, Painter, TEXT, UiRect, pixmap_from_rgba,
};

/// Preferred panel size, logical pixels.
const PANEL_SIZE: (f32, f32) = (640.0, 430.0);
/// Outer padding.
const PADDING: f32 = 20.0;
/// Preview box side.
const PREVIEW_SIDE: f32 = 180.0;
/// One label/value row.
const ROW_HEIGHT: f32 = 21.0;
/// Label column width.
const LABEL_WIDTH: f32 = 88.0;
/// Footer button height.
const BUTTON_HEIGHT: f32 = 30.0;
/// Value text size.
const VALUE_SIZE: f32 = 11.5;

/// Everything the dialog shows about one image.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageInfo {
    /// The item the dialog describes.
    pub item: ItemId,
    /// File name from the path or URL, or `Pasted image`.
    pub title: String,
    /// Format of the original bytes.
    pub format: ImageFormat,
    /// The recorded location: the URL when one is recorded (even one that
    /// cannot be launched), else the path.
    pub location: Option<String>,
    /// Whether that location is a URL (for the copy button label).
    pub location_is_url: bool,
    /// The launchable location kind, if any.
    pub location_kind: Option<LocationKind>,
    /// Where the asset came from.
    pub origin: AssetOrigin,
    /// Decoded pixel size.
    pub pixels: (u32, u32),
    /// The item's extent on the board, world units.
    pub board_size: (f32, f32),
    /// Length of the original encoded bytes.
    pub byte_len: u64,
    /// The 64-digit BLAKE3 asset id.
    pub asset_id: String,
    /// Frame count and one loop's duration, for an animated asset.
    pub animation: Option<(usize, Duration)>,
    /// Preview pixel size.
    pub preview_size: (u32, u32),
    /// Rotation in degrees, clockwise.
    pub rotation_degrees: f32,
    /// Mirror flags.
    pub flip: (bool, bool),
    /// Opacity in `0..=1`.
    pub opacity: f32,
    /// 1-based position in the z-order (1 is the bottom) and the item count.
    pub z_order: (usize, usize),
    /// The group the item belongs to directly, if any.
    pub group: Option<u64>,
}

impl ImageInfo {
    /// The snapshot for image `item`, or `None` when it is not an image or
    /// its asset is not loaded (a missing linked asset).
    pub fn from_scene(
        scene: &Scene,
        item: ItemId,
        provenance: Option<&AssetProvenance>,
    ) -> Option<Self> {
        let entry = scene.item(item)?;
        let asset_id = entry.asset()?;
        let asset = scene.asset(asset_id)?;
        let empty = AssetProvenance::default();
        let provenance = provenance.unwrap_or(&empty);
        let url = provenance
            .source_url
            .as_deref()
            .filter(|url| !url.trim().is_empty());
        let path = provenance
            .source_path
            .as_deref()
            .filter(|path| !path.trim().is_empty());
        let location = url.or(path).map(str::to_owned);
        let size = entry.size();
        Some(Self {
            item,
            title: location
                .as_deref()
                .and_then(|value| display_name(value, url.is_some()))
                .unwrap_or_else(|| "Pasted image".to_owned()),
            format: asset.format(),
            location_is_url: url.is_some(),
            location,
            location_kind: location::location_kind(provenance),
            origin: provenance.origin,
            pixels: (asset.pixel_width(), asset.pixel_height()),
            board_size: (size.x, size.y),
            byte_len: asset.encoded().len() as u64,
            asset_id: asset_id.to_hex(),
            animation: asset
                .animation()
                .map(|animation| (animation.len(), animation.total_duration())),
            preview_size: (asset.preview().width(), asset.preview().height()),
            rotation_degrees: entry.rotation().to_degrees(),
            flip: (entry.flip_x(), entry.flip_y()),
            opacity: entry.opacity(),
            z_order: (
                scene.position(item).map_or(0, |index| index + 1),
                scene.items().len(),
            ),
            group: scene
                .groups()
                .parent_of(GroupChild::Item(item))
                .map(|group| group.raw()),
        })
    }

    /// The label/value rows, in display order. Values are complete; the
    /// painter elides them, accessibility publishes them whole.
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = vec![
            (
                "Location",
                self.location
                    .clone()
                    .unwrap_or_else(|| "No source location".to_owned()),
            ),
            ("Source", origin_label(self.origin).to_owned()),
            ("Format", format_name(self.format).to_owned()),
            ("Pixels", format!("{} × {}", self.pixels.0, self.pixels.1)),
            (
                "Board size",
                format!(
                    "{} × {} units",
                    trim_number(self.board_size.0),
                    trim_number(self.board_size.1)
                ),
            ),
            ("Scale", scale_text(self.board_size, self.pixels)),
            (
                "File size",
                format!("{} ({} bytes)", human_bytes(self.byte_len), self.byte_len),
            ),
            ("Asset ID", self.asset_id.clone()),
        ];
        if let Some((frames, duration)) = self.animation {
            rows.push((
                "Animation",
                format!("{frames} frames · {:.2} s per loop", duration.as_secs_f64()),
            ));
        }
        rows.push((
            "Preview",
            format!(
                "{} × {} · original embedded",
                self.preview_size.0, self.preview_size.1
            ),
        ));
        rows.push(("Rotation", rotation_text(self.rotation_degrees, self.flip)));
        rows.push((
            "Opacity",
            format!("{}%", (self.opacity.clamp(0.0, 1.0) * 100.0).round()),
        ));
        rows.push((
            "Order",
            format!("{} of {} (bottom to top)", self.z_order.0, self.z_order.1),
        ));
        rows.push((
            "Group",
            self.group
                .map_or_else(|| "None".to_owned(), |id| format!("Group {id}")),
        ));
        rows
    }

    /// The sentence under the title explaining the location.
    pub fn location_note(&self) -> &'static str {
        match (self.location_kind, &self.location) {
            (Some(LocationKind::Web), _) => "Web image",
            (Some(LocationKind::Local), _) => "Local file",
            (None, Some(_)) => "Not launchable: only http and https URLs open",
            (None, None) => "Pasted or dropped without a source location",
        }
    }

    /// The footer buttons that apply to this image, in order.
    pub fn buttons(&self) -> Vec<InfoButton> {
        let mut buttons = Vec::with_capacity(4);
        if self.location_kind.is_some() {
            buttons.push(InfoButton::Open);
        }
        if self.location.is_some() {
            buttons.push(InfoButton::CopyLocation);
        }
        buttons.push(InfoButton::CopyId);
        buttons.push(InfoButton::Close);
        buttons
    }

    /// The label of a footer button for this image.
    pub fn button_label(&self, button: InfoButton) -> &'static str {
        match button {
            InfoButton::Open => self
                .location_kind
                .map_or("Open location", |kind| match kind {
                    LocationKind::Local => "Open location",
                    LocationKind::Web => "Open URL",
                }),
            InfoButton::CopyLocation if self.location_is_url => "Copy URL",
            InfoButton::CopyLocation => "Copy path",
            InfoButton::CopyId => "Copy asset ID",
            InfoButton::Close => "Close",
        }
    }
}

/// A footer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoButton {
    /// Open the file location or URL.
    Open,
    /// Copy the full path or URL.
    CopyLocation,
    /// Copy the full asset id.
    CopyId,
    /// Close the dialog.
    Close,
}

/// What the dialog wants the application to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoResult {
    /// Nothing.
    None,
    /// The focus moved; repaint.
    Changed,
    /// A button was activated (Close included).
    Activate(InfoButton),
}

/// The open dialog.
#[derive(Debug, Clone)]
pub struct ImageInfoPanel {
    /// The snapshot.
    pub info: ImageInfo,
    /// The preview, premultiplied, or `None` for a placeholder.
    preview: Option<Pixmap>,
    /// Index into [`ImageInfo::buttons`].
    focused: usize,
}

impl ImageInfoPanel {
    /// A dialog for `info`, drawing `preview` (straight RGBA) when given.
    /// Close is focused, so Enter right after opening is harmless.
    pub fn new(info: ImageInfo, preview: Option<(u32, u32, &[u8])>) -> Self {
        let preview =
            preview.and_then(|(width, height, rgba)| pixmap_from_rgba(width, height, rgba));
        let focused = info.buttons().len() - 1;
        Self {
            info,
            preview,
            focused,
        }
    }

    /// The focused button.
    pub fn focused(&self) -> InfoButton {
        let buttons = self.info.buttons();
        buttons[self.focused.min(buttons.len() - 1)]
    }

    /// Handles one key.
    pub fn key(&mut self, chord: KeyChord) -> InfoResult {
        let count = self.info.buttons().len();
        match chord.key {
            Key::Escape => InfoResult::Activate(InfoButton::Close),
            Key::Tab if chord.shift => self.step(count - 1, count),
            Key::Left | Key::Up => self.step(count - 1, count),
            Key::Tab | Key::Right | Key::Down => self.step(1, count),
            Key::Enter | Key::Space => InfoResult::Activate(self.focused()),
            _ => InfoResult::None,
        }
    }

    fn step(&mut self, by: usize, count: usize) -> InfoResult {
        self.focused = (self.focused + by) % count;
        InfoResult::Changed
    }

    /// Moves the focus to the button under the pointer.
    pub fn pointer_move(&mut self, position: DVec2, viewport: DVec2) -> bool {
        match self.hit_button(position, viewport) {
            Some(index) if index != self.focused => {
                self.focused = index;
                true
            }
            _ => false,
        }
    }

    /// Handles a left click: a button activates, outside the panel closes,
    /// anywhere else inside does nothing.
    pub fn pointer_click(&mut self, position: DVec2, viewport: DVec2) -> InfoResult {
        if let Some(index) = self.hit_button(position, viewport) {
            self.focused = index;
            return InfoResult::Activate(self.focused());
        }
        let layout = Layout::new(viewport, &self.info);
        if layout.panel.contains(position.x as f32, position.y as f32) {
            InfoResult::None
        } else {
            InfoResult::Activate(InfoButton::Close)
        }
    }

    /// The button index under `position`.
    pub fn hit_button(&self, position: DVec2, viewport: DVec2) -> Option<usize> {
        Layout::new(viewport, &self.info)
            .buttons
            .iter()
            .position(|(rect, _)| rect.contains(position.x as f32, position.y as f32))
    }

    /// Paints the dialog above everything else.
    pub fn paint(&self, painter: &mut Painter, viewport: DVec2) {
        painter.fill_rect(
            UiRect::new(0.0, 0.0, viewport.x as f32, viewport.y as f32),
            [0, 0, 0, 150],
        );
        let layout = Layout::new(viewport, &self.info);
        let panel = layout.panel;
        painter.panel(panel, 10.0);

        // Header: title, format badge, location note.
        let badge = format_name(self.info.format);
        let badge_width = painter.measure(badge, 10.5) + 14.0;
        let title_room = panel.width - PADDING * 2.0 - badge_width - 10.0;
        let title = fit_middle(painter, &self.info.title, 17.0, title_room);
        painter.text(&title, panel.x + PADDING, panel.y + 14.0, 17.0, TEXT);
        let title_width = painter.measure(&title, 17.0);
        let badge_rect = UiRect::new(
            panel.x + PADDING + title_width + 10.0,
            panel.y + 17.0,
            badge_width,
            19.0,
        );
        painter.fill_round_rect(badge_rect, 5.0, [77, 158, 255, 60]);
        painter.text_centered(badge, badge_rect, badge_rect.y + 3.0, 10.5, ACCENT);
        painter.text(
            self.info.location_note(),
            panel.x + PADDING,
            panel.y + 40.0,
            11.0,
            DIM_TEXT,
        );

        // Preview, letterboxed over a checker-free neutral box.
        painter.fill_round_rect(layout.preview, 6.0, [28, 29, 35, 255]);
        match &self.preview {
            Some(pixmap) => painter.image(pixmap, layout.preview.inset(6.0)),
            None => painter.thumbnail_placeholder(layout.preview, badge),
        }

        // Label/value grid, clipped to the space above the footer.
        let value_width = panel.right() - PADDING - layout.value_x;
        let mut y = layout.grid_top;
        for (label, value) in self.info.rows() {
            if y + ROW_HEIGHT > layout.grid_bottom {
                break;
            }
            painter.text(label, layout.label_x, y, 11.0, DIM_TEXT);
            let shown = fit_middle(painter, &value, VALUE_SIZE, value_width);
            painter.text(&shown, layout.value_x, y, VALUE_SIZE, TEXT);
            y += ROW_HEIGHT;
        }

        // Footer: the keyboard hint takes the room left of the buttons, in
        // its longest form that fits (none at all in a narrow window).
        let hint_room = layout
            .buttons
            .first()
            .map_or(panel.width, |(rect, _)| rect.x - panel.x - PADDING - 10.0);
        if let Some(hint) = ["Esc closes · Tab moves · Enter activates", "Esc closes"]
            .into_iter()
            .find(|hint| painter.measure(hint, 10.5) <= hint_room)
        {
            painter.text(
                hint,
                panel.x + PADDING,
                panel.bottom() - PADDING - BUTTON_HEIGHT + 8.0,
                10.5,
                DIM_TEXT,
            );
        }
        for (index, (rect, button)) in layout.buttons.iter().enumerate() {
            let focused = index == self.focused;
            if focused {
                painter.fill_round_rect(*rect, 6.0, HOVER_BG);
            }
            painter.stroke_round_rect(*rect, 6.0, 1.0, if focused { ACCENT } else { PANEL_BORDER });
            painter.text_centered(
                self.info.button_label(*button),
                *rect,
                rect.y + 8.0,
                11.5,
                TEXT,
            );
        }
    }
}

/// The dialog geometry for one viewport.
#[derive(Debug, Clone, PartialEq)]
struct Layout {
    panel: UiRect,
    preview: UiRect,
    label_x: f32,
    value_x: f32,
    grid_top: f32,
    grid_bottom: f32,
    buttons: Vec<(UiRect, InfoButton)>,
}

impl Layout {
    fn new(viewport: DVec2, info: &ImageInfo) -> Self {
        let (viewport_w, viewport_h) = (viewport.x as f32, viewport.y as f32);
        // Clamp to the viewport rather than growing past it.
        let width = PANEL_SIZE.0.min(viewport_w - 16.0).max(300.0);
        let height = PANEL_SIZE.1.min(viewport_h - 16.0).max(240.0);
        let panel = UiRect::new(
            ((viewport_w - width) * 0.5).max(0.0),
            ((viewport_h - height) * 0.5).max(0.0),
            width,
            height,
        );
        let side = PREVIEW_SIDE.min(width * 0.3);
        let preview = UiRect::new(panel.x + PADDING, panel.y + 66.0, side, side);
        let label_x = preview.right() + 18.0;
        let value_x = label_x + LABEL_WIDTH;
        let footer_top = panel.bottom() - PADDING - BUTTON_HEIGHT;

        let mut buttons = Vec::new();
        let mut right = panel.right() - PADDING;
        for button in info.buttons().into_iter().rev() {
            let width = match button {
                InfoButton::Close => 74.0,
                InfoButton::CopyId => 112.0,
                InfoButton::CopyLocation | InfoButton::Open => 104.0,
            };
            let rect = UiRect::new(right - width, footer_top, width, BUTTON_HEIGHT);
            right = rect.x - 8.0;
            buttons.push((rect, button));
        }
        buttons.reverse();

        Self {
            panel,
            preview,
            label_x,
            value_x,
            grid_top: panel.y + 64.0,
            grid_bottom: footer_top - 8.0,
            buttons,
        }
    }
}

/// Middle-elides `text` with `…` until it fits `max_width` at `size`.
fn fit_middle(painter: &mut Painter, text: &str, size: f32, max_width: f32) -> String {
    if painter.measure(text, size) <= max_width {
        return text.to_owned();
    }
    let chars = text.chars().count();
    let (mut low, mut high) = (0usize, chars);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if painter.measure(&elide_middle(text, mid), size) <= max_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    elide_middle(text, low)
}

/// Keeps `keep` characters of `text` around a middle `…`.
pub fn elide_middle(text: &str, keep: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= keep {
        return text.to_owned();
    }
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// A human-readable byte count in binary units.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// The item's extent over its pixel size, as a percentage; both axes when
/// they differ by more than half a percent.
pub fn scale_text(board_size: (f32, f32), pixels: (u32, u32)) -> String {
    if pixels.0 == 0 || pixels.1 == 0 {
        return "—".to_owned();
    }
    let x = f64::from(board_size.0) / f64::from(pixels.0) * 100.0;
    let y = f64::from(board_size.1) / f64::from(pixels.1) * 100.0;
    if (x - y).abs() <= 0.5 {
        format!("{}%", trim_number(((x + y) * 0.5) as f32))
    } else {
        format!("{}% × {}%", trim_number(x as f32), trim_number(y as f32))
    }
}

/// One decimal, without a trailing `.0`.
fn trim_number(value: f32) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").map_or(text.clone(), str::to_owned)
}

/// Rotation and mirroring, in words.
fn rotation_text(degrees: f32, flip: (bool, bool)) -> String {
    let degrees = degrees.rem_euclid(360.0);
    let mirror = match flip {
        (false, false) => "not mirrored",
        (true, false) => "mirrored horizontally",
        (false, true) => "mirrored vertically",
        (true, true) => "mirrored both ways",
    };
    format!("{}° · {mirror}", trim_number(degrees))
}

/// The friendly name of an origin.
pub fn origin_label(origin: AssetOrigin) -> &'static str {
    match origin {
        AssetOrigin::File => "File",
        AssetOrigin::Clipboard => "Clipboard",
        AssetOrigin::Generated => "Generated",
        AssetOrigin::Unknown => "Unknown",
    }
}

/// The display name of a format.
pub fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Tga => "TGA",
        ImageFormat::Pnm => "PNM",
        ImageFormat::Qoi => "QOI",
        ImageFormat::Ico => "ICO",
        ImageFormat::Heif => "HEIC",
        ImageFormat::Avif => "AVIF",
        // The enum is non-exhaustive; a newer codec shows its extension.
        _ => format.extension(),
    }
}

/// The file name a path or URL names.
fn display_name(location: &str, is_url: bool) -> Option<String> {
    let name = if is_url {
        let without_suffix = location
            .split(['?', '#'])
            .next()
            .unwrap_or(location)
            .trim_end_matches('/');
        let (_, rest) = without_suffix
            .split_once("://")
            .unwrap_or(("", without_suffix));
        rest.split_once('/')
            .and_then(|(_, path)| path.rsplit('/').next())
            .or(Some(rest))
            .map(str::to_owned)
    } else {
        // Both separators: a board saved on Windows records `D:\refs\a.png`,
        // and on Linux or macOS that is still the file `a.png`, not a name
        // with backslashes in it.
        location
            .trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .map(str::to_owned)
    };
    name.filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;
    use rizref_codecs::ImageBuffer;
    use rizref_core::{Animation, AnimationFrame, Asset, Group, GroupId, SceneId};
    use std::sync::Arc;

    fn scene_with_image() -> (Scene, ItemId) {
        let mut scene = Scene::new(SceneId::random());
        let image = ImageBuffer::filled(400, 200, [10, 20, 30, 255]);
        let preview = ImageBuffer::filled(40, 20, [10, 20, 30, 255]);
        let asset = Asset::from_decoded(
            Arc::from(vec![7u8; 3000]),
            ImageFormat::Png,
            &image,
            preview,
        );
        let (asset_id, _) = scene.insert_asset(asset);
        let _below = scene.insert_note("note", Vec2::ZERO, Vec2::new(10.0, 10.0));
        let item = scene
            .insert_image(asset_id, Vec2::ZERO, Vec2::new(200.0, 100.0))
            .expect("asset exists");
        (scene, item)
    }

    fn row<'a>(rows: &'a [(&'static str, String)], label: &str) -> &'a str {
        rows.iter()
            .find(|(name, _)| *name == label)
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("row {label}"))
    }

    #[test]
    fn a_snapshot_covers_every_field_of_a_local_file() {
        let (mut scene, item) = scene_with_image();
        let group = GroupId::from_raw(4);
        scene.insert_group(Group::new(group, vec![GroupChild::Item(item)]));
        let provenance = AssetProvenance {
            origin: AssetOrigin::File,
            source_path: Some(r"D:\My Refs\hero shot.png".to_owned()),
            source_url: None,
        };
        let info = ImageInfo::from_scene(&scene, item, Some(&provenance)).expect("image");

        assert_eq!(info.title, "hero shot.png");
        assert_eq!(info.location_kind, Some(LocationKind::Local));
        assert_eq!(info.location_note(), "Local file");
        let rows = info.rows();
        assert_eq!(row(&rows, "Location"), r"D:\My Refs\hero shot.png");
        assert_eq!(row(&rows, "Source"), "File");
        assert_eq!(row(&rows, "Format"), "PNG");
        assert_eq!(row(&rows, "Pixels"), "400 × 200");
        assert_eq!(row(&rows, "Board size"), "200 × 100 units");
        assert_eq!(row(&rows, "Scale"), "50%");
        assert_eq!(row(&rows, "File size"), "2.9 KiB (3000 bytes)");
        assert_eq!(row(&rows, "Asset ID").len(), 64);
        assert_eq!(row(&rows, "Preview"), "40 × 20 · original embedded");
        assert_eq!(row(&rows, "Rotation"), "0° · not mirrored");
        assert_eq!(row(&rows, "Opacity"), "100%");
        assert_eq!(row(&rows, "Order"), "2 of 2 (bottom to top)");
        assert_eq!(row(&rows, "Group"), "Group 4");
        assert!(rows.iter().all(|(label, _)| *label != "Animation"));
        assert_eq!(
            info.buttons(),
            [
                InfoButton::Open,
                InfoButton::CopyLocation,
                InfoButton::CopyId,
                InfoButton::Close
            ]
        );
        assert_eq!(info.button_label(InfoButton::CopyLocation), "Copy path");
    }

    #[test]
    fn a_web_image_names_itself_from_the_url() {
        let (scene, item) = scene_with_image();
        let provenance = AssetProvenance {
            origin: AssetOrigin::Clipboard,
            source_path: None,
            source_url: Some("https://e.com/art/cat.jpg?w=800".to_owned()),
        };
        let info = ImageInfo::from_scene(&scene, item, Some(&provenance)).expect("image");
        assert_eq!(info.title, "cat.jpg");
        assert_eq!(info.location_kind, Some(LocationKind::Web));
        assert_eq!(row(&info.rows(), "Source"), "Clipboard");
        assert_eq!(info.button_label(InfoButton::CopyLocation), "Copy URL");
        assert_eq!(info.button_label(InfoButton::Open), "Open URL");
    }

    #[test]
    fn a_hostile_url_is_shown_but_not_offered_for_opening() {
        let (scene, item) = scene_with_image();
        let provenance = AssetProvenance {
            origin: AssetOrigin::Clipboard,
            source_path: None,
            source_url: Some("javascript:alert(1)".to_owned()),
        };
        let info = ImageInfo::from_scene(&scene, item, Some(&provenance)).expect("image");
        assert_eq!(row(&info.rows(), "Location"), "javascript:alert(1)");
        assert_eq!(info.location_kind, None);
        assert_eq!(
            info.location_note(),
            "Not launchable: only http and https URLs open"
        );
        assert_eq!(
            info.buttons(),
            [
                InfoButton::CopyLocation,
                InfoButton::CopyId,
                InfoButton::Close
            ]
        );
    }

    #[test]
    fn a_pasted_image_explains_why_there_is_no_location() {
        let (scene, item) = scene_with_image();
        let info = ImageInfo::from_scene(&scene, item, None).expect("image");
        assert_eq!(info.title, "Pasted image");
        assert_eq!(row(&info.rows(), "Location"), "No source location");
        assert_eq!(row(&info.rows(), "Source"), "Unknown");
        assert_eq!(
            info.location_note(),
            "Pasted or dropped without a source location"
        );
        assert_eq!(info.buttons(), [InfoButton::CopyId, InfoButton::Close]);
    }

    #[test]
    fn notes_and_unknown_items_have_no_snapshot() {
        let (scene, _) = scene_with_image();
        let note = scene.items()[0].id();
        assert!(ImageInfo::from_scene(&scene, note, None).is_none());
    }

    #[test]
    fn animation_facts_appear_for_animated_assets() {
        let mut scene = Scene::new(SceneId::random());
        let frame = |ms| {
            AnimationFrame::new(
                ImageBuffer::filled(4, 4, [0, 0, 0, 255]),
                Duration::from_millis(ms),
            )
        };
        let animation = Animation::new(vec![frame(100), frame(150)]).expect("frames");
        let image = ImageBuffer::filled(4, 4, [0, 0, 0, 255]);
        let asset = Asset::from_decoded(
            Arc::from(vec![1u8; 10]),
            ImageFormat::Gif,
            &image,
            image.clone(),
        )
        .with_animation(animation);
        let (asset_id, _) = scene.insert_asset(asset);
        let item = scene
            .insert_image(asset_id, Vec2::ZERO, Vec2::new(4.0, 4.0))
            .expect("asset");
        let info = ImageInfo::from_scene(&scene, item, None).expect("image");
        assert_eq!(row(&info.rows(), "Animation"), "2 frames · 0.25 s per loop");
        assert_eq!(row(&info.rows(), "Format"), "GIF");
    }

    #[test]
    fn formatting_helpers() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(2_516_582), "2.4 MiB");
        assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");

        assert_eq!(scale_text((200.0, 100.0), (400, 200)), "50%");
        assert_eq!(scale_text((1920.0, 1080.0), (1920, 1080)), "100%");
        assert_eq!(scale_text((150.0, 100.0), (100, 100)), "150% × 100%");
        assert_eq!(scale_text((1.0, 1.0), (0, 0)), "—");

        assert_eq!(
            rotation_text(-90.0, (true, false)),
            "270° · mirrored horizontally"
        );
        assert_eq!(
            rotation_text(12.34, (true, true)),
            "12.3° · mirrored both ways"
        );

        assert_eq!(origin_label(AssetOrigin::Generated), "Generated");
        for format in ImageFormat::ALL {
            assert!(!format_name(format).is_empty());
        }

        assert_eq!(elide_middle("abcdefghij", 4), "ab…ij");
        assert_eq!(elide_middle("abc", 10), "abc");
        assert_eq!(elide_middle("日本語テキスト", 3), "日本…ト");
        assert_eq!(elide_middle("abcdef", 0), "…");
    }

    #[test]
    fn display_names_come_from_the_last_segment() {
        assert_eq!(display_name(r"C:\a\b.png", false).as_deref(), Some("b.png"));
        assert_eq!(
            display_name("/home/u/b.png", false).as_deref(),
            Some("b.png")
        );
        assert_eq!(
            display_name("https://e.com/a/b.webp?x#y", true).as_deref(),
            Some("b.webp")
        );
        assert_eq!(display_name("https://e.com/a/", true).as_deref(), Some("a"));
        assert_eq!(
            display_name("https://e.com", true).as_deref(),
            Some("e.com")
        );
    }

    fn panel() -> ImageInfoPanel {
        let (scene, item) = scene_with_image();
        let provenance = AssetProvenance {
            origin: AssetOrigin::File,
            source_path: Some(r"D:\a.png".to_owned()),
            source_url: None,
        };
        let info = ImageInfo::from_scene(&scene, item, Some(&provenance)).expect("image");
        ImageInfoPanel::new(info, Some((1, 1, &[255, 0, 0, 255])))
    }

    fn chord(key: Key, shift: bool) -> KeyChord {
        KeyChord {
            key,
            ctrl: false,
            shift,
            alt: false,
        }
    }

    #[test]
    fn the_keyboard_cycles_buttons_without_a_trap() {
        let mut panel = panel();
        assert_eq!(panel.focused(), InfoButton::Close, "Close is the default");
        assert_eq!(panel.key(chord(Key::Tab, false)), InfoResult::Changed);
        assert_eq!(panel.focused(), InfoButton::Open, "Tab wraps around");
        assert_eq!(panel.key(chord(Key::Tab, true)), InfoResult::Changed);
        assert_eq!(panel.focused(), InfoButton::Close);
        panel.key(chord(Key::Left, false));
        assert_eq!(panel.focused(), InfoButton::CopyId);
        assert_eq!(
            panel.key(chord(Key::Enter, false)),
            InfoResult::Activate(InfoButton::CopyId)
        );
        assert_eq!(
            panel.key(chord(Key::Escape, false)),
            InfoResult::Activate(InfoButton::Close)
        );
        assert_eq!(panel.key(chord(Key::A, false)), InfoResult::None);
    }

    #[test]
    fn clicks_activate_buttons_and_outside_closes() {
        let mut panel = panel();
        let viewport = DVec2::new(1280.0, 800.0);
        let layout = Layout::new(viewport, &panel.info);
        let (rect, button) = layout.buttons[1];
        let inside = DVec2::new(f64::from(rect.x + 4.0), f64::from(rect.y + 4.0));
        assert!(panel.pointer_move(inside, viewport));
        assert_eq!(panel.focused(), button);
        assert_eq!(
            panel.pointer_click(inside, viewport),
            InfoResult::Activate(InfoButton::CopyLocation)
        );
        let in_panel = DVec2::new(
            f64::from(layout.panel.x + 30.0),
            f64::from(layout.panel.y + 30.0),
        );
        assert_eq!(panel.pointer_click(in_panel, viewport), InfoResult::None);
        assert_eq!(
            panel.pointer_click(DVec2::new(2.0, 2.0), viewport),
            InfoResult::Activate(InfoButton::Close)
        );
    }

    #[test]
    fn the_panel_clamps_to_a_small_viewport() {
        let info = panel().info;
        let layout = Layout::new(DVec2::new(500.0, 360.0), &info);
        assert!(layout.panel.width <= 484.0);
        assert!(layout.panel.height <= 344.0);
        assert!(layout.panel.x >= 0.0 && layout.panel.y >= 0.0);
        for (rect, _) in &layout.buttons {
            assert!(rect.x >= layout.panel.x, "buttons stay inside");
        }
        let big = Layout::new(DVec2::new(1920.0, 1080.0), &info);
        assert_eq!((big.panel.width, big.panel.height), PANEL_SIZE);
    }

    #[test]
    fn painting_does_not_panic_at_any_size() {
        let panel = panel();
        let mut engine = crate::text::TextEngine::new();
        for (width, height, scale) in [(1280u32, 800u32, 1.0f32), (420, 300, 1.0), (800, 600, 2.0)]
        {
            let mut pixmap = Pixmap::new(
                (width as f32 * scale) as u32,
                (height as f32 * scale) as u32,
            )
            .expect("allocates");
            let mut painter = Painter::new(&mut pixmap, &mut engine, scale);
            panel.paint(
                &mut painter,
                DVec2::new(f64::from(width), f64::from(height)),
            );
        }
    }
}
