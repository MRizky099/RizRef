//! The scene model: what a board contains and how it is identified.

use std::fmt::{self, Write as _};

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::asset::{Asset, AssetId, AssetStore};
use crate::item::ImageItem;

/// Identity of a board: 16 raw bytes rendered as 32 lowercase hex digits.
///
/// The reference format carries a 32-hex scene id in its header; RizRef keeps
/// that shape so an id can be typed, logged and compared without ambiguity. It
/// is created once when a board is made and never changes afterwards, which is
/// what lets recents, autosave files and the thumbnail cache be keyed by id
/// instead of by file path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SceneId([u8; 16]);

impl SceneId {
    /// Length of the raw id in bytes.
    pub const BYTE_LEN: usize = 16;

    /// Length of the hex rendering in characters.
    pub const HEX_LEN: usize = 32;

    /// Wraps raw bytes, for example the first 16 bytes of a hash.
    pub const fn from_bytes(bytes: [u8; Self::BYTE_LEN]) -> Self {
        Self(bytes)
    }

    /// The raw bytes.
    pub const fn as_bytes(&self) -> &[u8; Self::BYTE_LEN] {
        &self.0
    }

    /// The id as 32 lowercase hex digits.
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(Self::HEX_LEN);
        for byte in self.0 {
            write!(out, "{byte:02x}").expect("writing to a String cannot fail");
        }
        out
    }

    /// Parses exactly 32 hex digits, accepting upper- and lowercase.
    ///
    /// Returns `None` for any other length or for a non-hex character, so a
    /// corrupt header is reported as a clean error instead of a partial id.
    pub fn from_hex(text: &str) -> Option<Self> {
        let digits = text.as_bytes();
        if digits.len() != Self::HEX_LEN {
            return None;
        }
        let mut raw = [0u8; Self::BYTE_LEN];
        for (pair, slot) in digits.as_chunks::<2>().0.iter().zip(raw.iter_mut()) {
            *slot = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
        }
        Some(Self(raw))
    }
}

impl fmt::Display for SceneId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

/// Value of a single hex digit, or `None` if the byte is not one.
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Identifier of one item (image, note or group) inside a scene.
///
/// Ids are handed out per scene and stay stable across save/load and across
/// undo/redo, so a drawn frame, a selection or a group membership can hold one
/// without re-resolving it by index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ItemId(u64);

impl ItemId {
    /// Wraps a raw identifier, as read from a file.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw identifier, as written to a file.
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A board: the ordered set of items the user arranged, plus the assets they
/// show.
///
/// `items` is an ordered list rather than a map on purpose: z-order *is* the
/// iteration order, and a board is small enough (hundreds of items) that a
/// linear scan beats a second index. Assets are content-addressed and shared;
/// several items can point at the same one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// Identity of the board, written verbatim into the `.rizref` header.
    pub id: SceneId,
    /// Items in z-order; the last entry is drawn on top.
    pub items: Vec<ImageItem>,
    /// The next raw item id handed out.
    #[serde(default = "Scene::first_item_raw")]
    next_item: u64,
    /// Content-addressed originals. Phase 06 gives them their on-disk chunk
    /// form; the in-memory map is what every item resolves through.
    #[serde(skip)]
    assets: AssetStore,
}

impl Scene {
    /// An empty board with the given identity.
    pub fn new(id: SceneId) -> Self {
        Self {
            id,
            items: Vec::new(),
            next_item: Self::first_item_raw(),
            assets: AssetStore::new(),
        }
    }

    /// First raw item id a new scene hands out.
    const fn first_item_raw() -> u64 {
        1
    }

    /// Number of items on the board.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the board has no items yet.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether `item` is on the board.
    pub fn contains(&self, item: ItemId) -> bool {
        self.items.iter().any(|entry| entry.id() == item)
    }

    /// The items in z-order.
    pub fn items(&self) -> &[ImageItem] {
        &self.items
    }

    /// The item with `id`, if present.
    pub fn item(&self, id: ItemId) -> Option<&ImageItem> {
        self.items.iter().find(|item| item.id() == id)
    }

    /// The item with `id` for mutation, if present.
    pub fn item_mut(&mut self, id: ItemId) -> Option<&mut ImageItem> {
        self.items.iter_mut().find(|item| item.id() == id)
    }

    /// The stored assets.
    pub fn assets(&self) -> &AssetStore {
        &self.assets
    }

    /// The asset with `id`, if present.
    pub fn asset(&self, id: AssetId) -> Option<&Asset> {
        self.assets.get(id)
    }

    /// Stores `asset` unless an equal one is present, returning its id and
    /// whether this call inserted it.
    pub fn insert_asset(&mut self, asset: Asset) -> (AssetId, bool) {
        self.assets.intern(asset)
    }

    /// Adds an item on top of the stack, returning its id.
    ///
    /// Returns `None` when `asset` is not in the scene: an item pointing at
    /// nothing would draw as a hole and fail every later save, so the model
    /// refuses it rather than repairing it at render time. The caller is
    /// responsible for updating any [`SceneRectTracker`] it keeps.
    ///
    /// [`SceneRectTracker`]: crate::SceneRectTracker
    pub fn insert_image(&mut self, asset: AssetId, center: Vec2, size: Vec2) -> Option<ItemId> {
        if !self.assets.contains(asset) {
            return None;
        }
        // A deserialised scene may not know its high-water mark; never reuse an
        // id that an existing item already holds.
        let highest = self
            .items
            .iter()
            .map(|item| item.id().raw())
            .max()
            .unwrap_or(0);
        self.next_item = self.next_item.max(highest + 1);

        let id = ItemId::from_raw(self.next_item);
        self.next_item += 1;
        self.items.push(ImageItem::new(id, asset, center, size));
        Some(id)
    }

    /// Adds a fully built item on top of the stack.
    ///
    /// Used by undo/redo, which restores an item with its original id and
    /// transform, and by tests that need exact control over the payload.
    pub fn push(&mut self, item: ImageItem) {
        self.next_item = self.next_item.max(item.id().raw() + 1);
        self.items.push(item);
    }

    /// Removes `item`, returning the z-position it had.
    ///
    /// The position is returned rather than ignored because undo has to put the
    /// item back where it was, not on top.
    pub fn remove(&mut self, item: ItemId) -> Option<usize> {
        let position = self.items.iter().position(|entry| entry.id() == item)?;
        self.items.remove(position);
        Some(position)
    }
}

#[cfg(test)]
mod tests {
    use super::{ItemId, Scene, SceneId};
    use crate::asset::{Asset, AssetId};
    use crate::item::ImageItem;
    use glam::Vec2;
    use rizref_codecs::{ImageBuffer, ImageFormat};
    use std::sync::Arc;

    fn scene_id() -> SceneId {
        SceneId::from_bytes([0xab; SceneId::BYTE_LEN])
    }

    fn insert_asset(scene: &mut Scene, bytes: &[u8]) -> AssetId {
        let image = ImageBuffer::filled(4, 3, [1, 2, 3, 255]);
        let asset = Asset::from_decoded(
            Arc::from(bytes),
            ImageFormat::Png,
            &image,
            ImageBuffer::filled(2, 1, [1, 2, 3, 255]),
        );
        scene.insert_asset(asset).0
    }

    #[test]
    fn scene_id_round_trips_through_hex() {
        let id = scene_id();

        let hex = id.to_hex();

        assert_eq!(hex.len(), SceneId::HEX_LEN);
        assert_eq!(hex, "ab".repeat(16));
        assert_eq!(SceneId::from_hex(&hex), Some(id));
        assert_eq!(id.to_string(), hex);
    }

    #[test]
    fn scene_id_accepts_uppercase_and_rejects_junk() {
        let id = SceneId::from_bytes([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ]);

        assert_eq!(
            SceneId::from_hex("0123456789ABCDEFFEDCBA9876543210"),
            Some(id)
        );
        assert_eq!(id.to_hex(), "0123456789abcdeffedcba9876543210");

        assert_eq!(SceneId::from_hex(""), None, "empty input");
        assert_eq!(SceneId::from_hex(&"0".repeat(31)), None, "too short");
        assert_eq!(SceneId::from_hex(&"0".repeat(33)), None, "too long");
        assert_eq!(SceneId::from_hex(&"z".repeat(32)), None, "not hex");
    }

    #[test]
    fn new_board_is_empty() {
        let scene = Scene::new(scene_id());

        assert!(scene.is_empty());
        assert_eq!(scene.len(), 0);
        assert_eq!(scene.id, scene_id());
    }

    #[test]
    fn items_keep_their_z_order() {
        let mut scene = Scene::new(scene_id());
        let asset = insert_asset(&mut scene, b"shared");
        let bottom = scene
            .insert_image(asset, Vec2::new(1.0, 2.0), Vec2::new(10.0, 10.0))
            .expect("asset is stored");
        let top = scene
            .insert_image(asset, Vec2::new(30.0, 2.0), Vec2::new(10.0, 10.0))
            .expect("asset is stored");

        assert_eq!(scene.len(), 2);
        assert!(!scene.is_empty());
        assert_eq!(
            scene.items.iter().map(ImageItem::id).collect::<Vec<_>>(),
            vec![bottom, top]
        );
        assert!(scene.contains(bottom) && scene.contains(top));
        assert_eq!(
            scene.item(top).expect("top item").center(),
            Vec2::new(30.0, 2.0)
        );
    }

    #[test]
    fn two_items_can_share_one_asset() {
        let mut scene = Scene::new(scene_id());
        let asset = insert_asset(&mut scene, b"one original");

        let first = scene
            .insert_image(asset, Vec2::ZERO, Vec2::splat(4.0))
            .expect("first");
        let second = scene
            .insert_image(asset, Vec2::splat(10.0), Vec2::splat(4.0))
            .expect("second");

        assert_ne!(first, second);
        assert_eq!(scene.len(), 2);
        assert_eq!(scene.assets().len(), 1, "one asset, two items");
    }

    #[test]
    fn inserting_an_unknown_asset_is_refused() {
        let mut scene = Scene::new(scene_id());
        let missing = AssetId::from_checksum([9; 32]);

        assert_eq!(
            scene.insert_image(missing, Vec2::ZERO, Vec2::splat(1.0)),
            None
        );
        assert!(scene.is_empty());
    }

    #[test]
    fn remove_reports_the_z_position_it_took_away() {
        let mut scene = Scene::new(scene_id());
        let asset = insert_asset(&mut scene, b"three items");
        let items: Vec<ItemId> = (0..3)
            .map(|index| {
                scene
                    .insert_image(asset, Vec2::splat(index as f32 * 10.0), Vec2::splat(4.0))
                    .expect("asset is stored")
            })
            .collect();

        assert_eq!(scene.remove(items[1]), Some(1));
        assert_eq!(
            scene.items.iter().map(ImageItem::id).collect::<Vec<_>>(),
            vec![items[0], items[2]]
        );
        assert_eq!(scene.remove(items[1]), None, "removing twice is a no-op");
        assert!(!scene.contains(items[1]));
    }
}
