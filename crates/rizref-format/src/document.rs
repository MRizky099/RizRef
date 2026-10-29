//! The scene document stored in the `DOC ` chunk.
//!
//! The document is JSON on purpose. It is the part of a board that has to stay
//! inspectable and forgiving: fields added by a newer build are skipped rather
//! than rejected, a missing optional field falls back to a default instead of
//! failing the load, and `rizref inspect` can summarise a board with ordinary
//! JSON tooling. A board of a few hundred items costs a few tens of kilobytes;
//! if a future profiling run ever makes that a bottleneck, the chunk framing
//! already allows a `DOC2` binary chunk as an additive change.
//!
//! # Decoupling from `rizref-core`
//!
//! These types mirror the core scene model instead of serialising it directly.
//! The on-disk schema is a contract: changing an internal struct must not
//! silently change the bytes of every saved board, and the document needs
//! fields — asset hashes, provenance, a schema revision — that the in-memory
//! model deliberately does not carry. Conversion happens at the edge, in
//! [`SceneDocument::build_scene`] and [`SceneDocument::from_scene`].
//!
//! # Forward compatibility
//!
//! Every struct keeps unknown members in an `extra` map and writes them back
//! out, so a board saved by a newer build survives a round trip through an
//! older one. `schema_version` guards changes of *meaning*; unknown members are
//! only additive.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use glam::{DVec2, Vec2};
use rizref_codecs::{ImageBuffer, ImageFormat};
use rizref_core::{
    Asset, AssetId, Camera, Group, GroupChild, GroupId, Item, ItemId, Rect, Scene, SceneId,
    Transform,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::FormatError;

/// Document revision this build writes.
pub const SCHEMA_VERSION: u16 = 1;

/// Revision used by `#[serde(default = …)]`.
#[must_use]
pub const fn current_schema() -> u16 {
    SCHEMA_VERSION
}

/// Everything a board remembers besides its original image bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneDocument {
    /// Revision of this document. Bumped when a field changes meaning.
    #[serde(default = "current_schema")]
    pub schema_version: u16,
    /// Board identity, 32 lowercase hex digits; equals the header's scene id.
    #[serde(default)]
    pub scene_id: String,
    /// The view the board was saved with.
    #[serde(default)]
    pub camera: ViewEntry,
    /// Bounding box of the saved scene in world units, `[x0, y0, x1, y1]`.
    ///
    /// Informational: the live rectangle is recomputed from the items. It is
    /// stored so an external tool can frame a board without doing geometry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_rect: Option<[f64; 4]>,
    /// Items in z-order; the last entry is drawn on top.
    #[serde(default)]
    pub items: Vec<ItemEntry>,
    /// Group membership, recursive.
    #[serde(default)]
    pub groups: Vec<GroupEntry>,
    /// Every asset the board references.
    #[serde(default)]
    pub assets: Vec<AssetEntry>,
    /// Board-level display facts.
    #[serde(default)]
    pub board: BoardEntry,
    /// Members written by a newer build; preserved verbatim on resave.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

/// The saved camera.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewEntry {
    /// World point at the viewport centre.
    #[serde(default)]
    pub eye: [f64; 2],
    /// Viewport pixels per world unit.
    #[serde(default = "unit_zoom")]
    pub zoom: f64,
    /// View rotation in radians, clockwise on screen.
    #[serde(default)]
    pub rotation: f64,
    /// Members from a newer build.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

const fn unit_zoom() -> f64 {
    1.0
}

impl Default for ViewEntry {
    fn default() -> Self {
        Self {
            eye: [0.0, 0.0],
            zoom: 1.0,
            rotation: 0.0,
            extra: BTreeMap::new(),
        }
    }
}

impl ViewEntry {
    /// The saved view of `camera`.
    pub fn from_camera(camera: &Camera) -> Self {
        Self {
            eye: [camera.eye.x, camera.eye.y],
            zoom: camera.zoom,
            rotation: camera.rotation,
            extra: BTreeMap::new(),
        }
    }

    /// A core camera, with zoom clamped to the supported range.
    pub fn to_camera(&self) -> Camera {
        Camera::new(
            DVec2::new(self.eye[0], self.eye[1]),
            self.zoom,
            self.rotation,
        )
    }
}

/// Board-level display facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BoardEntry {
    /// Display name. The `NAME` chunk is authoritative; this is a fallback for
    /// tools that only read the document.
    #[serde(default)]
    pub name: String,
    /// Members from a newer build.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

/// One item: an image pointing at an asset, or a note carrying text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemEntry {
    /// Stable item id inside this scene.
    pub id: u64,
    /// What the item shows.
    pub kind: ItemKind,
    /// Content hash of the asset, for image items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// Note text, for note items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Centre in world units.
    pub position: [f32; 2],
    /// Extent in world units.
    pub size: [f32; 2],
    /// Rotation in radians, clockwise on screen.
    #[serde(default)]
    pub rotation: f32,
    /// Opacity in `0..=1`.
    #[serde(default = "unit_opacity")]
    pub opacity: f32,
    /// Horizontal and vertical mirror flags.
    #[serde(default)]
    pub flip: [bool; 2],
    /// Members from a newer build.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

const fn unit_opacity() -> f32 {
    1.0
}

/// What an item shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// An image backed by a content-addressed asset.
    Image,
    /// A text note drawn by the application.
    Note,
}

/// One group node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupEntry {
    /// Stable group id inside this scene.
    pub id: u64,
    /// Members, in order.
    pub children: Vec<ChildEntry>,
    /// Members from a newer build.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

/// One member of a group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildEntry {
    /// Whether `id` names an item or a nested group.
    pub kind: ChildKind,
    /// The item or group id.
    pub id: u64,
    /// Members from a newer build.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

/// What a group child points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildKind {
    /// A leaf item.
    Item,
    /// A nested group.
    Group,
}

/// One asset: content identity, decoded facts and provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetEntry {
    /// BLAKE3 of the original encoded bytes, 64 lowercase hex digits.
    pub hash: String,
    /// Image format of the original bytes, as a canonical extension.
    pub format: String,
    /// Decoded pixel size.
    pub pixel_size: [u32; 2],
    /// Length of the original bytes.
    pub byte_len: u64,
    /// Where the asset came from.
    #[serde(default)]
    pub origin: AssetOrigin,
    /// Original file path, a hint only; never load-bearing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// Original URL, a hint only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    /// Whether the original bytes are embedded in a `BLOB` chunk.
    #[serde(default = "embedded_default")]
    pub embedded: bool,
    /// The encoded preview stored in a `PREV` chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<PreviewEntry>,
    /// Members from a newer build.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

const fn embedded_default() -> bool {
    true
}

/// Metadata of one encoded asset preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewEntry {
    /// Encoded image format, as a canonical extension.
    pub format: String,
    /// Pixel size of the preview.
    pub pixel_size: [u32; 2],
    /// Encoded length in bytes.
    pub byte_len: u64,
    /// Members from a newer build.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

/// Where an asset came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AssetOrigin {
    /// Imported from a file.
    File,
    /// Pasted from the clipboard or dropped as a virtual file.
    Clipboard,
    /// Generated inside the application.
    Generated,
    /// Unknown, as for a board written by an earlier build.
    #[default]
    Unknown,
}

impl AssetOrigin {
    /// Every origin this build writes.
    pub const ALL: [Self; 4] = [Self::File, Self::Clipboard, Self::Generated, Self::Unknown];
}

/// A scene reconstructed from a document, plus the assets that could not be.
#[derive(Debug)]
pub struct LoadedScene {
    /// The reconstructed scene. Items whose linked asset is missing are kept;
    /// the application can show a placeholder while offering a relink.
    pub scene: Scene,
    /// Assets the document declares as linked but whose bytes were absent.
    pub missing: Vec<AssetId>,
}

impl SceneDocument {
    /// An empty document for `scene_id`.
    pub fn new(scene_id: SceneId, name: &str, camera: &Camera) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            scene_id: scene_id.to_hex(),
            camera: ViewEntry::from_camera(camera),
            scene_rect: None,
            items: Vec::new(),
            groups: Vec::new(),
            assets: Vec::new(),
            board: BoardEntry {
                name: name.to_owned(),
                extra: BTreeMap::new(),
            },
            extra: BTreeMap::new(),
        }
    }

    /// A document describing `scene`, with asset entries supplied by the
    /// writer.
    pub fn from_scene(scene: &Scene, name: &str, camera: &Camera, assets: Vec<AssetEntry>) -> Self {
        let mut document = Self::new(scene.id, name, camera);

        document.items = scene
            .items()
            .iter()
            .map(|item| {
                let transform = item.transform();
                let (kind, asset, text) = match item.asset() {
                    Some(asset) => (ItemKind::Image, Some(asset.to_hex()), None),
                    None => (
                        ItemKind::Note,
                        None,
                        Some(item.text().unwrap_or_default().to_owned()),
                    ),
                };
                ItemEntry {
                    id: item.id().raw(),
                    kind,
                    asset,
                    text,
                    position: [transform.center().x, transform.center().y],
                    size: [transform.size().x, transform.size().y],
                    rotation: transform.rotation(),
                    opacity: item.opacity(),
                    flip: [item.flip_x(), item.flip_y()],
                    extra: BTreeMap::new(),
                }
            })
            .collect();

        document.groups = scene
            .groups()
            .iter()
            .map(|group| GroupEntry {
                id: group.id().raw(),
                children: group
                    .children()
                    .iter()
                    .map(|child| match child {
                        GroupChild::Item(item) => ChildEntry {
                            kind: ChildKind::Item,
                            id: item.raw(),
                            extra: BTreeMap::new(),
                        },
                        GroupChild::Group(group) => ChildEntry {
                            kind: ChildKind::Group,
                            id: group.raw(),
                            extra: BTreeMap::new(),
                        },
                    })
                    .collect(),
                extra: BTreeMap::new(),
            })
            .collect();

        document.scene_rect = scene_rect(scene).map(|rect| {
            [
                f64::from(rect.min.x),
                f64::from(rect.min.y),
                f64::from(rect.max().x),
                f64::from(rect.max().y),
            ]
        });

        document.assets = assets;
        document
    }

    /// Serializes the document to UTF-8 JSON.
    pub fn to_json(&self) -> Result<Vec<u8>, FormatError> {
        serde_json::to_vec(self).map_err(|error| FormatError::Document {
            reason: error.to_string(),
        })
    }

    /// Parses UTF-8 JSON, running the schema migration scaffold.
    pub fn from_json(bytes: &[u8]) -> Result<Self, FormatError> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|error| FormatError::Document {
                reason: error.to_string(),
            })?;
        Self::from_value(value)
    }

    /// Parses an already decoded JSON value, checking its schema revision.
    ///
    /// The match is the migration scaffold: a future revision adds an arm that
    /// rewrites the value to the next shape before falling through. A document
    /// from a newer build is refused rather than guessed at.
    pub fn from_value(value: Value) -> Result<Self, FormatError> {
        let found = value
            .get("schema_version")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let found = u16::try_from(found).unwrap_or(u16::MAX);
        match found {
            1 => serde_json::from_value(value).map_err(|error| FormatError::Document {
                reason: error.to_string(),
            }),
            0 => Err(FormatError::UnsupportedSchema {
                found: 0,
                supported: SCHEMA_VERSION,
            }),
            other => Err(FormatError::UnsupportedSchema {
                found: other,
                supported: SCHEMA_VERSION,
            }),
        }
    }

    /// The scene id, or an error if the document's is malformed.
    pub fn scene_id(&self) -> Result<SceneId, FormatError> {
        SceneId::from_hex(&self.scene_id).ok_or_else(|| FormatError::Document {
            reason: format!("scene id {:?} is not 32 hex digits", self.scene_id),
        })
    }

    /// The saved camera.
    pub fn camera(&self) -> Camera {
        self.camera.to_camera()
    }

    /// The board name, trimmed of a trailing NUL if a writer left one.
    pub fn name(&self) -> &str {
        self.board.name.trim_end_matches('\0')
    }

    /// Number of image items.
    pub fn image_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.kind == ItemKind::Image)
            .count()
    }

    /// Number of note items.
    pub fn note_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.kind == ItemKind::Note)
            .count()
    }

    /// Rebuilds the core scene from the document, its blobs and its previews.
    ///
    /// `blobs` maps every embedded asset hash to its original bytes. `previews`
    /// maps hashes to decoded preview images; an asset without one gets a
    /// one-pixel transparent placeholder, which the renderer replaces as soon
    /// as the full bitmap is decoded.
    pub fn build_scene(
        &self,
        blobs: &BTreeMap<AssetId, Arc<[u8]>>,
        previews: &BTreeMap<AssetId, ImageBuffer>,
    ) -> Result<LoadedScene, FormatError> {
        let scene_id = self.scene_id()?;
        let mut scene = Scene::new(scene_id);
        let mut declared = BTreeSet::new();
        let mut available = BTreeSet::new();
        let mut missing = Vec::new();

        for entry in &self.assets {
            let id = AssetId::from_hex(&entry.hash).ok_or_else(|| FormatError::Document {
                reason: format!("asset hash {:?} is not 64 hex digits", entry.hash),
            })?;
            if !declared.insert(id) {
                return Err(FormatError::Document {
                    reason: format!("asset {id} is declared twice"),
                });
            }
            let Some(bytes) = blobs.get(&id) else {
                if entry.embedded {
                    return Err(FormatError::MissingBlob {
                        hash: entry.hash.clone(),
                    });
                }
                missing.push(id);
                continue;
            };
            let format = ImageFormat::from_extension(&entry.format).ok_or_else(|| {
                FormatError::Document {
                    reason: format!("asset {id} names unknown format {:?}", entry.format),
                }
            })?;
            let preview = previews
                .get(&id)
                .cloned()
                .unwrap_or_else(placeholder_preview);
            let asset = Asset::from_meta(
                Arc::clone(bytes),
                format,
                *id.as_bytes(),
                entry.pixel_size[0],
                entry.pixel_size[1],
                preview,
            );
            scene.insert_asset(asset);
            available.insert(id);
        }

        let mut item_ids = BTreeSet::new();
        for (index, entry) in self.items.iter().enumerate() {
            let id = ItemId::from_raw(entry.id);
            if !item_ids.insert(id) {
                return Err(FormatError::Document {
                    reason: format!("item id {} appears twice", entry.id),
                });
            }
            let mut transform = Transform::new(
                Vec2::new(entry.position[0], entry.position[1]),
                Vec2::new(entry.size[0], entry.size[1]),
            )
            .with_rotation(entry.rotation);
            // `Transform::new` normalises the extent; do not let a document
            // smuggle a negative size through a later setter.
            transform.set_size(Vec2::new(entry.size[0], entry.size[1]));

            let mut item = match entry.kind {
                ItemKind::Image => {
                    let hash = entry
                        .asset
                        .as_deref()
                        .ok_or_else(|| FormatError::Document {
                            reason: format!("image item {} has no asset hash", entry.id),
                        })?;
                    let asset = AssetId::from_hex(hash).ok_or_else(|| FormatError::Document {
                        reason: format!("item {} asset hash {hash:?} is invalid", entry.id),
                    })?;
                    if !declared.contains(&asset) {
                        return Err(FormatError::Document {
                            reason: format!(
                                "item {} references asset {asset}, which the document does not declare",
                                entry.id
                            ),
                        });
                    }
                    Item::image(id, asset, transform.center(), transform.size())
                }
                ItemKind::Note => Item::note(
                    id,
                    entry.text.clone().unwrap_or_default(),
                    transform.center(),
                    transform.size(),
                ),
            };
            item.set_transform(transform);
            item.set_opacity(entry.opacity);
            item.set_flip(entry.flip[0], entry.flip[1]);
            scene.insert_item_at(index, item);
        }

        let mut group_ids = BTreeSet::new();
        for entry in &self.groups {
            let id = GroupId::from_raw(entry.id);
            if !group_ids.insert(id) {
                return Err(FormatError::Document {
                    reason: format!("group id {} appears twice", entry.id),
                });
            }
        }
        for entry in &self.groups {
            let mut children = Vec::with_capacity(entry.children.len());
            for child in &entry.children {
                match child.kind {
                    ChildKind::Item => {
                        if !item_ids.contains(&ItemId::from_raw(child.id)) {
                            return Err(FormatError::Document {
                                reason: format!(
                                    "group {} contains item {}, which does not exist",
                                    entry.id, child.id
                                ),
                            });
                        }
                        children.push(GroupChild::Item(ItemId::from_raw(child.id)));
                    }
                    ChildKind::Group => {
                        if !group_ids.contains(&GroupId::from_raw(child.id)) {
                            return Err(FormatError::Document {
                                reason: format!(
                                    "group {} contains group {}, which does not exist",
                                    entry.id, child.id
                                ),
                            });
                        }
                        children.push(GroupChild::Group(GroupId::from_raw(child.id)));
                    }
                }
            }
            scene.insert_group(Group::new(GroupId::from_raw(entry.id), children));
        }

        // `available` is intentionally not used after construction: a missing
        // linked asset keeps its items so the relink flow has something to
        // reattach. It exists to make the distinction explicit at the boundary.
        let _ = available;

        Ok(LoadedScene { scene, missing })
    }
}

/// Union of every item's world rectangle.
pub fn scene_rect(scene: &Scene) -> Option<Rect> {
    let mut rect: Option<Rect> = None;
    for item in scene.items() {
        let item_rect = item.world_rect();
        rect = Some(match rect {
            None => item_rect,
            Some(current) => Rect::from_min_max(
                Vec2::new(
                    current.min.x.min(item_rect.min.x),
                    current.min.y.min(item_rect.min.y),
                ),
                Vec2::new(
                    current.max().x.max(item_rect.max().x),
                    current.max().y.max(item_rect.max().y),
                ),
            ),
        });
    }
    rect
}

/// A one-pixel transparent preview, used when an asset has no stored preview.
pub fn placeholder_preview() -> ImageBuffer {
    ImageBuffer::filled(1, 1, [0, 0, 0, 0])
}

#[cfg(test)]
mod tests {
    use super::{
        AssetEntry, AssetOrigin, ChildEntry, ChildKind, GroupEntry, ItemEntry, ItemKind,
        PreviewEntry, SCHEMA_VERSION, SceneDocument,
    };
    use glam::{DVec2, Vec2};
    use rizref_codecs::ImageBuffer;
    use rizref_core::{Asset, AssetId, Camera, GroupChild, Scene, SceneId};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn scene_id() -> SceneId {
        SceneId::from_hex("0123456789abcdeffedcba9876543210").expect("valid id")
    }

    fn asset(bytes: &[u8]) -> Asset {
        Asset::from_decoded(
            Arc::from(bytes),
            rizref_codecs::ImageFormat::Png,
            &ImageBuffer::filled(4, 3, [10, 20, 30, 255]),
            ImageBuffer::filled(2, 1, [10, 20, 30, 255]),
        )
    }

    fn sample_document() -> (SceneDocument, Scene, AssetId) {
        let mut scene = Scene::new(scene_id());
        let (asset_id, _) = scene.insert_asset(asset(b"original png bytes"));
        let first = scene
            .insert_image(asset_id, Vec2::new(10.0, 20.0), Vec2::new(100.0, 50.0))
            .expect("asset stored");
        let note = scene.insert_note("study the hand", Vec2::new(200.0, 0.0), Vec2::splat(80.0));
        let group = scene.allocate_group_id();
        scene.attach_group(group, vec![GroupChild::Item(first), GroupChild::Item(note)]);
        let camera = Camera::new(DVec2::new(3.0, -4.0), 1.5, 0.25);
        let entry = AssetEntry {
            hash: asset_id.to_hex(),
            format: "png".to_owned(),
            pixel_size: [4, 3],
            byte_len: 18,
            origin: AssetOrigin::File,
            source_path: Some("refs/hands.png".to_owned()),
            source_url: None,
            embedded: true,
            preview: Some(PreviewEntry {
                format: "jpeg".to_owned(),
                pixel_size: [2, 1],
                byte_len: 42,
                extra: BTreeMap::new(),
            }),
            extra: BTreeMap::new(),
        };
        (
            SceneDocument::from_scene(&scene, "Hands", &camera, vec![entry]),
            scene,
            asset_id,
        )
    }

    #[test]
    fn a_document_round_trips_through_json() {
        let (document, _, _) = sample_document();

        let json = document.to_json().expect("serialises");
        let back = SceneDocument::from_json(&json).expect("parses");

        assert_eq!(back, document);
    }

    #[test]
    fn scene_items_groups_and_camera_survive_the_conversion() {
        let (document, original, asset_id) = sample_document();
        let mut blobs = BTreeMap::new();
        blobs.insert(asset_id, Arc::<[u8]>::from(&b"original png bytes"[..]));

        let loaded = document
            .build_scene(&blobs, &BTreeMap::new())
            .expect("scene");

        assert_eq!(loaded.missing, Vec::<AssetId>::new());
        assert_eq!(loaded.scene.id, original.id);
        assert_eq!(loaded.scene.items().len(), original.items().len());
        for (left, right) in loaded.scene.items().iter().zip(original.items()) {
            assert_eq!(left.id(), right.id());
            assert_eq!(left.center(), right.center());
            assert_eq!(left.size(), right.size());
            assert_eq!(left.rotation(), right.rotation());
            assert_eq!(left.opacity(), right.opacity());
            assert_eq!(left.asset(), right.asset());
            assert_eq!(left.text(), right.text());
        }
        assert_eq!(loaded.scene.groups(), original.groups());
        assert_eq!(loaded.scene.assets().len(), 1);
        assert_eq!(
            loaded
                .scene
                .asset(asset_id)
                .expect("asset")
                .encoded()
                .as_ref(),
            b"original png bytes"
        );
        assert_eq!(document.camera().eye, DVec2::new(3.0, -4.0));
        assert_eq!(document.camera().zoom, 1.5);
    }

    #[test]
    fn unknown_members_are_ignored_and_preserved() {
        let json = br##"{
            "schema_version": 1,
            "scene_id": "0123456789abcdeffedcba9876543210",
            "board": { "name": "Future board", "accent": "#ff8800" },
            "items": [
                { "id": 1, "kind": "note", "text": "hi", "position": [0,0],
                  "size": [10,10], "colour": "amber" }
            ],
            "assets": [
                { "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                  "format": "png", "pixel_size": [1,1], "byte_len": 4,
                  "origin": "file", "embedded": false, "future_flag": true }
            ],
            "colour_management": "display-p3"
        }"##;

        let document = SceneDocument::from_json(json).expect("unknown members are tolerated");

        assert_eq!(document.board.name, "Future board");
        assert_eq!(
            document
                .extra
                .get("colour_management")
                .and_then(|value| value.as_str()),
            Some("display-p3")
        );
        assert_eq!(document.board.extra.len(), 1);
        assert_eq!(document.items[0].extra.len(), 1);
        assert_eq!(document.assets[0].extra.len(), 1);

        let written = document.to_json().expect("serialises");
        let value: serde_json::Value = serde_json::from_slice(&written).expect("json");
        assert_eq!(value["colour_management"], "display-p3");
        assert_eq!(value["board"]["accent"], "#ff8800");
        assert_eq!(value["items"][0]["colour"], "amber");
        assert_eq!(value["assets"][0]["future_flag"], true);
    }

    #[test]
    fn absent_optional_members_fall_back_to_defaults() {
        let document = SceneDocument::from_json(
            br#"{
                "schema_version": 1,
                "scene_id": "0123456789abcdeffedcba9876543210",
                "items": [
                    { "id": 1, "kind": "note", "text": "x", "position": [0,0], "size": [5,5] }
                ]
            }"#,
        )
        .expect("minimal document");

        assert_eq!(document.camera.zoom, 1.0);
        assert_eq!(document.items[0].opacity, 1.0);
        assert_eq!(document.items[0].flip, [false, false]);
        assert!(document.assets.is_empty());
    }

    #[test]
    fn a_newer_schema_is_refused_with_its_revision() {
        let error = SceneDocument::from_json(
            br#"{ "schema_version": 2, "scene_id": "0123456789abcdeffedcba9876543210" }"#,
        )
        .expect_err("must not parse");

        assert!(matches!(
            error,
            crate::error::FormatError::UnsupportedSchema {
                found: 2,
                supported: SCHEMA_VERSION
            }
        ));
    }

    #[test]
    fn a_missing_schema_is_refused() {
        let error = SceneDocument::from_json(b"{}").expect_err("must not parse");

        assert!(matches!(
            error,
            crate::error::FormatError::UnsupportedSchema { found: 0, .. }
        ));
    }

    #[test]
    fn a_linked_asset_without_bytes_is_reported_not_fatal() {
        let mut document = SceneDocument::new(scene_id(), "Linked", &Camera::default());
        document.assets.push(AssetEntry {
            hash: "bb".repeat(32),
            format: "png".to_owned(),
            pixel_size: [10, 10],
            byte_len: 4,
            origin: AssetOrigin::File,
            source_path: Some("missing.png".to_owned()),
            source_url: None,
            embedded: false,
            preview: None,
            extra: BTreeMap::new(),
        });
        document.items.push(ItemEntry {
            id: 1,
            kind: ItemKind::Image,
            asset: Some("bb".repeat(32)),
            text: None,
            position: [0.0, 0.0],
            size: [10.0, 10.0],
            rotation: 0.0,
            opacity: 1.0,
            flip: [false, false],
            extra: BTreeMap::new(),
        });

        let loaded = document
            .build_scene(&BTreeMap::new(), &BTreeMap::new())
            .expect("linked board loads");

        assert_eq!(loaded.missing.len(), 1);
        assert_eq!(
            loaded.scene.items().len(),
            1,
            "the item survives for relinking"
        );
        assert!(loaded.scene.assets().is_empty());
    }

    #[test]
    fn an_embedded_asset_without_bytes_is_an_error() {
        let mut document = SceneDocument::new(scene_id(), "Broken", &Camera::default());
        document.assets.push(AssetEntry {
            hash: "cc".repeat(32),
            format: "png".to_owned(),
            pixel_size: [10, 10],
            byte_len: 4,
            origin: AssetOrigin::Unknown,
            source_path: None,
            source_url: None,
            embedded: true,
            preview: None,
            extra: BTreeMap::new(),
        });

        let error = document
            .build_scene(&BTreeMap::new(), &BTreeMap::new())
            .expect_err("must not load");

        assert!(matches!(
            error,
            crate::error::FormatError::MissingBlob { .. }
        ));
    }

    #[test]
    fn a_group_referencing_a_missing_item_is_refused() {
        let mut document = SceneDocument::new(scene_id(), "Broken", &Camera::default());
        document.groups.push(GroupEntry {
            id: 1,
            children: vec![ChildEntry {
                kind: ChildKind::Item,
                id: 99,
                extra: BTreeMap::new(),
            }],
            extra: BTreeMap::new(),
        });

        let error = document
            .build_scene(&BTreeMap::new(), &BTreeMap::new())
            .expect_err("must not load");

        assert!(error.to_string().contains("does not exist"));
    }

    #[test]
    fn the_scene_rect_covers_every_item() {
        let (document, _, _) = sample_document();

        let rect = document.scene_rect.expect("items have geometry");
        assert!(rect[0] <= -40.0);
        assert!(rect[1] <= -25.0);
        assert!(rect[2] >= 240.0);
        assert!(rect[3] >= 30.0);
    }
}
