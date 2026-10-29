//! The scene model: what a board contains and how it is identified.

use std::collections::BTreeSet;
use std::fmt::{self, Write as _};

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::asset::{Asset, AssetId, AssetStore};
use crate::group::{Group, GroupChild, GroupId, Groups};
use crate::item::Item;

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

    /// A fresh UUID version 4 identifier.
    ///
    /// The bits a UUID v4 reserves are set correctly (version nibble `4`,
    /// variant bits `10`), so ids are recognisable to other tools. Entropy
    /// comes from the clock, the process id, a process-wide counter and the
    /// address of a stack value, hashed with BLAKE3: enough to make a collision
    /// between boards created on one machine practically impossible, without
    /// pulling in a random-number crate for a value that is never a secret.
    pub fn random() -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let stack_marker = 0_u8;
        let mut entropy = Vec::with_capacity(32);
        entropy.extend_from_slice(&nanos.to_le_bytes());
        entropy.extend_from_slice(&u64::from(std::process::id()).to_le_bytes());
        entropy.extend_from_slice(&counter.to_le_bytes());
        entropy.extend_from_slice(&(std::ptr::addr_of!(stack_marker) as usize).to_le_bytes());
        let digest = blake3::hash(&entropy);
        let mut raw = [0u8; Self::BYTE_LEN];
        raw.copy_from_slice(&digest.as_bytes()[..Self::BYTE_LEN]);
        raw[6] = (raw[6] & 0x0f) | 0x40;
        raw[8] = (raw[8] & 0x3f) | 0x80;
        Self(raw)
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
/// show and the groups that tie items together.
///
/// `items` is an ordered list rather than a map on purpose: z-order *is* the
/// iteration order, and a board is small enough (hundreds of items) that a
/// linear scan beats a second index. Assets are content-addressed and shared;
/// several items can point at the same one. Groups are a separate tree and do
/// not affect draw order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// Identity of the board, written verbatim into the `.rizref` header.
    pub id: SceneId,
    /// Items in z-order; the last entry is drawn on top.
    pub items: Vec<Item>,
    /// Group membership. Recursive from day one; the v1 interface creates one
    /// level.
    #[serde(default)]
    groups: Groups,
    /// The next raw item id handed out. Not serialised: it is a session
    /// high-water mark, and leaving it out is what lets undo-all reproduce the
    /// saved bytes exactly. A loaded scene re-derives it from the items it has.
    #[serde(skip)]
    next_item: u64,
    /// The next raw group id handed out, likewise not serialised.
    #[serde(skip)]
    next_group: u64,
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
            groups: Groups::new(),
            next_item: Self::first_id(),
            next_group: Self::first_id(),
            assets: AssetStore::new(),
        }
    }

    /// First raw id a new scene hands out.
    const fn first_id() -> u64 {
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
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The items in z-order, for mutation.
    pub fn items_mut(&mut self) -> &mut [Item] {
        &mut self.items
    }

    /// The item with `id`, if present.
    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.iter().find(|item| item.id() == id)
    }

    /// The item with `id` for mutation, if present.
    pub fn item_mut(&mut self, id: ItemId) -> Option<&mut Item> {
        self.items.iter_mut().find(|item| item.id() == id)
    }

    /// The z-position of `id`, if present.
    pub fn position(&self, id: ItemId) -> Option<usize> {
        self.items.iter().position(|item| item.id() == id)
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

    /// Replaces an asset with the same content id, returning the previous one.
    ///
    /// Used when a deferred decode discovers that a GIF is animated: the
    /// original bytes are identical, only the attached frames are new.
    pub fn replace_asset(&mut self, asset: Asset) -> Option<Asset> {
        self.assets.replace(asset)
    }

    /// Allocates the next item id, keeping a session high-water mark.
    ///
    /// A deserialised scene may not know its high-water mark; never reuse an id
    /// that an existing item already holds.
    pub fn allocate_item_id(&mut self) -> ItemId {
        let highest = self
            .items
            .iter()
            .map(|item| item.id().raw())
            .max()
            .unwrap_or(0);
        self.next_item = self.next_item.max(highest + 1);
        let id = ItemId::from_raw(self.next_item);
        self.next_item += 1;
        id
    }

    /// Adds an image item on top of the stack, returning its id.
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
        let id = self.allocate_item_id();
        self.items.push(Item::image(id, asset, center, size));
        Some(id)
    }

    /// Adds a note on top of the stack, returning its id.
    pub fn insert_note(&mut self, text: impl Into<String>, center: Vec2, size: Vec2) -> ItemId {
        let id = self.allocate_item_id();
        self.items.push(Item::note(id, text, center, size));
        id
    }

    /// Inserts a fully built item at `index` (clamped), as undo and redo do.
    pub fn insert_item_at(&mut self, index: usize, item: Item) {
        self.next_item = self.next_item.max(item.id().raw() + 1);
        let index = index.min(self.items.len());
        self.items.insert(index, item);
    }

    /// Removes the item at `index`, returning it.
    pub fn remove_at(&mut self, index: usize) -> Option<Item> {
        (index < self.items.len()).then(|| self.items.remove(index))
    }

    /// Removes `item`, returning the z-position it had.
    ///
    /// The position is returned rather than ignored because undo has to put the
    /// item back where it was, not on top.
    pub fn remove(&mut self, item: ItemId) -> Option<usize> {
        let position = self.position(item)?;
        self.items.remove(position);
        Some(position)
    }

    /// Reorders the items to match `order`.
    ///
    /// Ids missing from `order` keep their relative order after the listed
    /// ones, so a stale order can never drop items from the board.
    pub fn reorder(&mut self, order: &[ItemId]) {
        let mut remaining = std::mem::take(&mut self.items);
        let mut ordered = Vec::with_capacity(remaining.len());
        for id in order {
            if let Some(index) = remaining.iter().position(|item| item.id() == *id) {
                ordered.push(remaining.remove(index));
            }
        }
        ordered.append(&mut remaining);
        self.items = ordered;
    }

    /// The group tree.
    pub fn groups(&self) -> &Groups {
        &self.groups
    }

    /// The group tree for mutation, through the commands' helpers.
    pub fn groups_mut(&mut self) -> &mut Groups {
        &mut self.groups
    }

    /// The group with `id`, if present.
    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.get(id)
    }

    /// Allocates the next group id.
    pub fn allocate_group_id(&mut self) -> GroupId {
        let highest = self
            .groups
            .iter()
            .map(|group| group.id().raw())
            .max()
            .unwrap_or(0);
        self.next_group = self.next_group.max(highest + 1);
        let id = GroupId::from_raw(self.next_group);
        self.next_group += 1;
        id
    }

    /// Inserts a group node, keeping the id high-water mark.
    pub fn insert_group(&mut self, group: Group) {
        self.next_group = self.next_group.max(group.id().raw() + 1);
        self.groups.insert(group);
    }

    /// Removes the group node, leaving its children parentless.
    pub fn remove_group(&mut self, id: GroupId) -> Option<Group> {
        self.groups.remove(id)
    }

    /// Creates a group and detaches its children from their previous parents.
    ///
    /// Returns the memberships that were removed, in detach order: undo walks
    /// them in reverse, which restores the original child indices even when two
    /// members shared a parent.
    pub fn attach_group(
        &mut self,
        id: GroupId,
        children: Vec<GroupChild>,
    ) -> Vec<(GroupId, usize, GroupChild)> {
        let mut seen = BTreeSet::new();
        let mut detached = Vec::new();
        let mut members = Vec::new();
        for child in children {
            if !seen.insert(child) {
                continue;
            }
            if let Some((parent, index)) = self.groups.detach(child) {
                detached.push((parent, index, child));
            }
            members.push(child);
        }
        self.insert_group(Group::new(id, members));
        detached
    }

    /// Removes `child` from its parent, returning the parent and slot.
    pub fn detach_child(&mut self, child: GroupChild) -> Option<(GroupId, usize)> {
        self.groups.detach(child)
    }

    /// Adds `child` to `parent` at `index` (clamped).
    pub fn attach_child(
        &mut self,
        parent: Option<GroupId>,
        child: GroupChild,
        index: Option<usize>,
    ) {
        self.groups.attach(parent, child, index);
    }

    /// The leaf items under `group`.
    pub fn group_leaves(&self, group: GroupId) -> Vec<ItemId> {
        self.groups.leaves(group)
    }

    /// The items a click on `item` should select: the whole outermost group,
    /// or the item itself when it is not grouped.
    pub fn expand_item(&self, item: ItemId) -> BTreeSet<ItemId> {
        match self.groups.top_group_of_item(item) {
            Some(group) => self.groups.leaves(group).into_iter().collect(),
            None => std::iter::once(item).collect(),
        }
    }

    /// Expands a set of item ids to whole groups.
    pub fn expand_selection(&self, items: impl IntoIterator<Item = ItemId>) -> BTreeSet<ItemId> {
        let mut out = BTreeSet::new();
        for item in items {
            out.extend(self.expand_item(item));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{ItemId, Scene, SceneId};
    use crate::asset::{Asset, AssetId};
    use crate::group::{GroupChild, GroupId};
    use crate::item::Item;
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
    fn random_scene_ids_are_uuid_v4_and_not_repeated() {
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..64 {
            let id = SceneId::random();
            let bytes = id.as_bytes();
            assert_eq!(bytes[6] & 0xf0, 0x40, "UUID version nibble");
            assert_eq!(bytes[8] & 0xc0, 0x80, "UUID variant bits");
            assert!(seen.insert(id), "random ids must not repeat");
        }
    }

    #[test]
    fn new_board_is_empty() {
        let scene = Scene::new(scene_id());

        assert!(scene.is_empty());
        assert_eq!(scene.len(), 0);
        assert_eq!(scene.id, scene_id());
        assert!(scene.groups().is_empty());
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
            scene.items.iter().map(Item::id).collect::<Vec<_>>(),
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
            scene.items.iter().map(Item::id).collect::<Vec<_>>(),
            vec![items[0], items[2]]
        );
        assert_eq!(scene.remove(items[1]), None, "removing twice is a no-op");
        assert!(!scene.contains(items[1]));
    }

    #[test]
    fn notes_are_items_without_assets() {
        let mut scene = Scene::new(scene_id());

        let note = scene.insert_note("study this", Vec2::ZERO, Vec2::splat(100.0));

        assert!(scene.item(note).expect("note").is_note());
        assert_eq!(scene.item(note).expect("note").text(), Some("study this"));
        assert!(scene.assets().is_empty());
    }

    #[test]
    fn inserting_at_a_position_restores_z_order() {
        let mut scene = Scene::new(scene_id());
        let asset = insert_asset(&mut scene, b"three items");
        let ids: Vec<ItemId> = (0..3)
            .map(|index| {
                scene
                    .insert_image(asset, Vec2::splat(index as f32), Vec2::splat(4.0))
                    .expect("asset is stored")
            })
            .collect();

        let removed = scene.remove_at(1).expect("the middle item");
        assert_eq!(removed.id(), ids[1]);
        scene.insert_item_at(1, removed);
        assert_eq!(
            scene.items.iter().map(Item::id).collect::<Vec<_>>(),
            ids,
            "the item is back in the middle, not on top"
        );
    }

    #[test]
    fn reorder_puts_listed_ids_first_and_keeps_the_rest() {
        let mut scene = Scene::new(scene_id());
        let asset = insert_asset(&mut scene, b"three items");
        let ids: Vec<ItemId> = (0..3)
            .map(|index| {
                scene
                    .insert_image(asset, Vec2::splat(index as f32), Vec2::splat(4.0))
                    .expect("asset is stored")
            })
            .collect();

        scene.reorder(&[ids[2], ids[0]]);

        assert_eq!(
            scene.items.iter().map(Item::id).collect::<Vec<_>>(),
            vec![ids[2], ids[0], ids[1]]
        );
    }

    #[test]
    fn a_click_on_a_grouped_item_expands_to_the_group() {
        let mut scene = Scene::new(scene_id());
        let a = scene.insert_note("a", Vec2::ZERO, Vec2::splat(10.0));
        let b = scene.insert_note("b", Vec2::splat(20.0), Vec2::splat(10.0));
        let c = scene.insert_note("c", Vec2::splat(40.0), Vec2::splat(10.0));
        let group = scene.allocate_group_id();
        scene.attach_group(group, vec![GroupChild::Item(a), GroupChild::Item(b)]);

        let expanded = scene.expand_item(b);
        assert_eq!(expanded, [a, b].into_iter().collect());
        assert_eq!(scene.expand_item(c), [c].into_iter().collect());
    }

    #[test]
    fn attaching_a_group_detaches_members_from_their_previous_parents() {
        let mut scene = Scene::new(scene_id());
        let a = scene.insert_note("a", Vec2::ZERO, Vec2::splat(10.0));
        let b = scene.insert_note("b", Vec2::splat(20.0), Vec2::splat(10.0));
        let first = scene.allocate_group_id();
        scene.attach_group(first, vec![GroupChild::Item(a), GroupChild::Item(b)]);

        let second = scene.allocate_group_id();
        let detached = scene.attach_group(second, vec![GroupChild::Item(b)]);

        assert_eq!(detached.len(), 1);
        assert_eq!(scene.group_leaves(first), vec![a], "b left its old group");
        assert_eq!(scene.group_leaves(second), vec![b]);
        assert_eq!(
            scene.groups().top_group_of_item(b),
            Some(second),
            "and belongs to the new one"
        );
    }

    #[test]
    fn group_ids_are_not_serialised_so_undo_can_be_byte_identical() {
        let mut scene = Scene::new(scene_id());
        scene.insert_note("a", Vec2::ZERO, Vec2::splat(10.0));
        let before = serde_json::to_vec(&scene).expect("serialises");

        let id = scene.allocate_item_id();
        scene.push_note_with_id(id);
        let after = serde_json::to_vec(&scene).expect("serialises");
        assert_ne!(before, after);

        scene.remove(id);
        assert_eq!(
            serde_json::to_vec(&scene).expect("serialises"),
            before,
            "removing the item restores the serialised bytes"
        );
    }

    impl Scene {
        /// Test helper: push a note with an explicit id.
        fn push_note_with_id(&mut self, id: ItemId) {
            self.insert_item_at(
                self.items.len(),
                Item::note(id, "x", Vec2::ZERO, Vec2::splat(10.0)),
            );
        }
    }

    #[test]
    fn a_nested_group_walk_is_bounded() {
        let mut scene = Scene::new(scene_id());
        let item = scene.insert_note("a", Vec2::ZERO, Vec2::splat(10.0));
        let inner = scene.allocate_group_id();
        scene.attach_group(inner, vec![GroupChild::Item(item)]);
        let outer = scene.allocate_group_id();
        scene.attach_group(outer, vec![GroupChild::Group(inner)]);

        assert_eq!(scene.group_leaves(outer), vec![item]);
        assert_eq!(
            scene.groups().top_group_of_item(item),
            Some(GroupId::from_raw(outer.raw()))
        );
    }
}
