//! The scene model: what a board contains and how it is identified.

use std::fmt::{self, Write as _};

use serde::{Deserialize, Serialize};

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

/// A board: the ordered set of items the user arranged.
///
/// Phase 00 keeps the identity and the z-order only — the item payload (source
/// bytes, transform, group membership, note text) arrives with the canvas and
/// manipulation phases. Keeping `items` as an ordered list rather than a map is
/// deliberate: z-order *is* the iteration order, and a board is small enough
/// (hundreds of items) that a linear scan beats a second index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// Identity of the board, written verbatim into the `.rizref` header.
    pub id: SceneId,
    /// Item ids in z-order; the last entry is drawn on top.
    pub items: Vec<ItemId>,
}

impl Scene {
    /// An empty board with the given identity.
    pub fn new(id: SceneId) -> Self {
        Self {
            id,
            items: Vec::new(),
        }
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
        self.items.contains(&item)
    }

    /// Adds an item on top of the stack.
    pub fn push(&mut self, item: ItemId) {
        self.items.push(item);
    }

    /// Removes `item`, returning the z-position it had.
    ///
    /// The position is returned rather than ignored because undo has to put the
    /// item back where it was, not on top.
    pub fn remove(&mut self, item: ItemId) -> Option<usize> {
        let position = self.items.iter().position(|entry| *entry == item)?;
        self.items.remove(position);
        Some(position)
    }
}

#[cfg(test)]
mod tests {
    use super::{ItemId, Scene, SceneId};

    fn scene_id() -> SceneId {
        SceneId::from_bytes([0xab; SceneId::BYTE_LEN])
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
        let (bottom, top) = (ItemId::from_raw(1), ItemId::from_raw(2));

        scene.push(bottom);
        scene.push(top);

        assert_eq!(scene.len(), 2);
        assert!(!scene.is_empty());
        assert_eq!(scene.items, vec![bottom, top]);
        assert!(scene.contains(bottom) && scene.contains(top));
    }

    #[test]
    fn remove_reports_the_z_position_it_took_away() {
        let mut scene = Scene::new(scene_id());
        let items = [
            ItemId::from_raw(1),
            ItemId::from_raw(2),
            ItemId::from_raw(3),
        ];
        for item in items {
            scene.push(item);
        }

        assert_eq!(scene.remove(items[1]), Some(1));
        assert_eq!(scene.items, vec![items[0], items[2]]);
        assert_eq!(scene.remove(items[1]), None, "removing twice is a no-op");
        assert!(!scene.contains(items[1]));
    }
}
