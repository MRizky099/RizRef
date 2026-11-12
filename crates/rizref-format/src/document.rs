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
    Asset, AssetId, Camera, Group, GroupChild, GroupId, Item, ItemId, NoteContent, NoteFormat,
    Rect, Scene, SceneId, TextSpan, TextStyle, Transform, clamp_wrap_width,
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
    /// How a note's `text` is interpreted: `"plain"` or `"markdown"`. Absent
    /// means plain, the v1 meaning. An unrecognised value also reads as plain
    /// (with a load warning) rather than failing the board.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_format: Option<String>,
    /// A note's opaque background, lowercase `#rrggbb`. Kept as written so an
    /// invalid value can be reported and ignored instead of failing the load.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_string"
    )]
    pub background: Option<String>,
    /// A note's content wrap width in world units, clamped to 16–10,000 on
    /// load. Absent means the note's inner box width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap_width: Option<f32>,
    /// A rich text item's styled runs: `[{"start", "end", "bold", "italic",
    /// "underline", "color": "#rrggbb"}]` over byte offsets of `text`. Kept
    /// as raw JSON so a malformed list is reported and ignored, never fatal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spans: Option<Value>,
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

/// Reads any JSON value as an optional string: a string stays itself, `null`
/// is absent, and any other value becomes its JSON text, which then fails
/// validation and is reported as a warning instead of failing the load.
fn lenient_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::Null => None,
        Value::String(text) => Some(text),
        other => Some(other.to_string()),
    })
}

/// The document spelling of a note format.
pub const fn text_format_name(format: NoteFormat) -> &'static str {
    match format {
        NoteFormat::Plain => "plain",
        NoteFormat::Markdown => "markdown",
        NoteFormat::Rich => "rich",
    }
}

/// Parses a document note format; `None` for an unrecognised value.
pub fn parse_text_format(name: &str) -> Option<NoteFormat> {
    match name {
        "plain" => Some(NoteFormat::Plain),
        "markdown" => Some(NoteFormat::Markdown),
        "rich" => Some(NoteFormat::Rich),
        _ => None,
    }
}

/// Serialises an opaque colour as lowercase `#rrggbb`.
pub fn format_background(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// Parses `#rrggbb` or `#rgb` (any case) into an opaque colour.
pub fn parse_background(text: &str) -> Option<[u8; 3]> {
    let hex = text.strip_prefix('#')?;
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |index: usize| u8::from_str_radix(&hex[index..=index], 16).ok();
    match hex.len() {
        6 => {
            let pair = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16).ok();
            Some([pair(0)?, pair(2)?, pair(4)?])
        }
        3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
        _ => None,
    }
}

impl ItemEntry {
    /// The note content this entry describes. Invalid optional members are
    /// ignored and reported in `warnings`; they never fail the load.
    pub fn note_content(&self, warnings: &mut Vec<String>) -> NoteContent {
        let format = match self.text_format.as_deref() {
            None => NoteFormat::Plain,
            Some(name) => parse_text_format(name).unwrap_or_else(|| {
                warnings.push(format!(
                    "note {} has unknown text_format {name:?}; read as plain text",
                    self.id
                ));
                NoteFormat::Plain
            }),
        };
        let background = self.background.as_deref().and_then(|text| {
            let parsed = parse_background(text);
            if parsed.is_none() {
                warnings.push(format!(
                    "note {} has invalid background {text:?}; using the default card",
                    self.id
                ));
            }
            parsed
        });
        let wrap_width = self.wrap_width.and_then(|width| {
            let clamped = clamp_wrap_width(width);
            if clamped.is_none() {
                warnings.push(format!(
                    "note {} has invalid wrap_width {width}; using the box width",
                    self.id
                ));
            }
            clamped
        });
        let text = self.text.clone().unwrap_or_default();
        let spans = match (&self.spans, format) {
            (Some(value), NoteFormat::Rich) => parse_spans(value).unwrap_or_else(|| {
                warnings.push(format!(
                    "note {} has invalid spans; its text is shown unstyled",
                    self.id
                ));
                Vec::new()
            }),
            _ => Vec::new(),
        };
        let spans = rizref_core::rich::normalize(&text, spans);
        NoteContent {
            text,
            format,
            background,
            wrap_width,
            spans,
        }
    }
}

/// Serialises styled runs for the document.
pub fn format_spans(spans: &[TextSpan]) -> Value {
    Value::Array(
        spans
            .iter()
            .map(|span| {
                let mut entry = serde_json::Map::new();
                entry.insert("start".into(), span.start.into());
                entry.insert("end".into(), span.end.into());
                for (key, on) in [
                    ("bold", span.style.bold),
                    ("italic", span.style.italic),
                    ("underline", span.style.underline),
                ] {
                    if on {
                        entry.insert(key.into(), Value::Bool(true));
                    }
                }
                if let Some(color) = span.style.color {
                    entry.insert("color".into(), Value::String(format_background(color)));
                }
                if let Some(size) = span.style.size {
                    entry.insert("size".into(), size.into());
                }
                Value::Object(entry)
            })
            .collect(),
    )
}

/// Parses the document's styled runs; `None` when the list is malformed.
pub fn parse_spans(value: &Value) -> Option<Vec<TextSpan>> {
    value
        .as_array()?
        .iter()
        .map(|entry| {
            let entry = entry.as_object()?;
            let offset =
                |key: &str| -> Option<usize> { usize::try_from(entry.get(key)?.as_u64()?).ok() };
            let flag = |key: &str| entry.get(key).and_then(Value::as_bool).unwrap_or(false);
            let color = match entry.get("color") {
                None | Some(Value::Null) => None,
                Some(value) => Some(parse_background(value.as_str()?)?),
            };
            let size = match entry.get("size") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let size = value
                        .as_f64()
                        .filter(|size| size.is_finite() && *size > 0.0)?;
                    Some(size.round().min(f64::from(u16::MAX)) as u16)
                }
            };
            Some(TextSpan {
                start: offset("start")?,
                end: offset("end")?,
                style: TextStyle {
                    bold: flag("bold"),
                    italic: flag("italic"),
                    underline: flag("underline"),
                    color,
                    size,
                },
            })
        })
        .collect()
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
    /// Recoverable problems the load ignored, such as an invalid note
    /// background; one human-readable line each.
    pub warnings: Vec<String>,
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
                let note = item.note_content();
                let kind = if note.is_some() {
                    ItemKind::Note
                } else {
                    ItemKind::Image
                };
                ItemEntry {
                    id: item.id().raw(),
                    kind,
                    asset: item.asset().map(|asset| asset.to_hex()),
                    text: note.as_ref().map(|note| note.text.clone()),
                    // Plain notes omit the member so a v1 board resaved without
                    // edits stays byte-identical in its note entries.
                    text_format: note
                        .as_ref()
                        .filter(|note| note.format != NoteFormat::Plain)
                        .map(|note| text_format_name(note.format).to_owned()),
                    background: note
                        .as_ref()
                        .and_then(|note| note.background)
                        .map(format_background),
                    wrap_width: note.as_ref().and_then(|note| note.wrap_width),
                    spans: note
                        .as_ref()
                        .filter(|note| !note.spans.is_empty())
                        .map(|note| format_spans(&note.spans)),
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

    /// Number of markdown notes.
    pub fn markdown_note_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| {
                item.kind == ItemKind::Note && item.text_format.as_deref() == Some("markdown")
            })
            .count()
    }

    /// Number of notes with a valid background override.
    pub fn styled_note_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| {
                item.kind == ItemKind::Note
                    && item
                        .background
                        .as_deref()
                        .and_then(parse_background)
                        .is_some()
            })
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
        let mut warnings = Vec::new();

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
                ItemKind::Note => Item::note_with(
                    id,
                    entry.note_content(&mut warnings),
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

        Ok(LoadedScene {
            scene,
            missing,
            warnings,
        })
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
    use rizref_core::{
        Asset, AssetId, Camera, GroupChild, Item, NoteContent, NoteFormat, Scene, SceneId,
        TextSpan, TextStyle,
    };
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
    fn a_v1_asset_without_provenance_loads_and_resaves_unchanged() {
        // An asset entry as phase 06 wrote it before any browser capture:
        // no origin, no source members at all.
        let hash = "ab".repeat(32);
        let json = format!(
            r#"{{
                "schema_version": 1,
                "scene_id": "0123456789abcdeffedcba9876543210",
                "assets": [
                    {{ "hash": "{hash}", "format": "png", "pixel_size": [2, 2], "byte_len": 9 }}
                ]
            }}"#
        );
        let document = SceneDocument::from_json(json.as_bytes()).expect("v1 document");
        let asset = &document.assets[0];
        assert_eq!(asset.origin, AssetOrigin::Unknown);
        assert_eq!(asset.source_path, None);
        assert_eq!(asset.source_url, None);
        assert!(asset.embedded);

        let written = String::from_utf8(document.to_json().expect("json")).expect("utf-8");
        assert!(!written.contains("source_path"), "{written}");
        assert!(!written.contains("source_url"), "{written}");
        assert!(written.contains(r#""origin":"unknown""#), "{written}");

        // And a 06b entry with a URL reads back exactly.
        let with_url = json.replace(
            r#""byte_len": 9"#,
            r#""byte_len": 9, "origin": "clipboard", "source_url": "https://e.com/a.png""#,
        );
        let document = SceneDocument::from_json(with_url.as_bytes()).expect("06b document");
        assert_eq!(document.schema_version, 1);
        assert_eq!(document.assets[0].origin, AssetOrigin::Clipboard);
        assert_eq!(
            document.assets[0].source_url.as_deref(),
            Some("https://e.com/a.png")
        );
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
            text_format: None,
            background: None,
            wrap_width: None,
            spans: None,
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

    fn note_entry(extra_json: &str) -> ItemEntry {
        let json = format!(
            r#"{{"id":9,"kind":"note","text":"hi","position":[0,0],"size":[100,50]{extra_json}}}"#
        );
        serde_json::from_str(&json).expect("entry parses")
    }

    #[test]
    fn note_fields_round_trip_through_a_document() {
        let mut scene = Scene::new(scene_id());
        let note = scene.insert_note("**bold**", Vec2::ZERO, Vec2::new(200.0, 100.0));
        let content = NoteContent {
            text: "**bold** <span style=\"color: red\">red</span>".into(),
            format: NoteFormat::Markdown,
            background: Some([0xfa, 0xf0, 0xe6]),
            wrap_width: Some(172.5),
            spans: Vec::new(),
        };
        scene.item_mut(note).expect("note").set_note(&content);
        let document = SceneDocument::from_scene(&scene, "n", &Camera::default(), Vec::new());

        let json = String::from_utf8(document.to_json().expect("json")).expect("utf-8");
        assert!(json.contains(r#""text_format":"markdown""#), "{json}");
        assert!(json.contains(r##""background":"#faf0e6""##), "{json}");
        assert!(json.contains(r#""wrap_width":172.5"#), "{json}");

        let back = SceneDocument::from_json(json.as_bytes()).expect("parses");
        let loaded = back
            .build_scene(&BTreeMap::new(), &BTreeMap::new())
            .expect("scene");
        assert!(loaded.warnings.is_empty());
        assert_eq!(
            loaded.scene.item(note).and_then(Item::note_content),
            Some(content)
        );
        assert_eq!(back.markdown_note_count(), 1);
        assert_eq!(back.styled_note_count(), 1);
    }

    #[test]
    fn rich_text_spans_round_trip_and_bad_spans_only_warn() {
        let mut scene = Scene::new(scene_id());
        let id = scene.insert_note("", Vec2::ZERO, Vec2::new(200.0, 40.0));
        let mut content = NoteContent::rich("bold red");
        content.spans = vec![
            TextSpan {
                start: 0,
                end: 4,
                style: TextStyle {
                    bold: true,
                    underline: true,
                    ..TextStyle::default()
                },
            },
            TextSpan {
                start: 5,
                end: 8,
                style: TextStyle {
                    color: Some([0xff, 0x00, 0x00]),
                    italic: true,
                    size: Some(48),
                    ..TextStyle::default()
                },
            },
        ];
        scene.item_mut(id).expect("note").set_note(&content);
        let document = SceneDocument::from_scene(&scene, "t", &Camera::default(), Vec::new());
        let json = String::from_utf8(document.to_json().expect("json")).expect("utf-8");
        assert!(json.contains(r#""text_format":"rich""#), "{json}");
        assert!(json.contains(r##""color":"#ff0000""##), "{json}");
        assert!(json.contains(r#""size":48"#), "{json}");
        let back = SceneDocument::from_json(json.as_bytes()).expect("parses");
        let loaded = back
            .build_scene(&BTreeMap::new(), &BTreeMap::new())
            .expect("scene");
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(
            loaded.scene.item(id).and_then(Item::note_content),
            Some(content)
        );

        let mut warnings = Vec::new();
        let bad = note_entry(r#","text_format":"rich","spans":[{"start":"x"}]"#);
        let content = bad.note_content(&mut warnings);
        assert!(content.spans.is_empty() && content.is_rich());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn a_note_without_the_new_fields_omits_them_and_loads_as_plain() {
        let entry = note_entry("");
        let mut warnings = Vec::new();
        let content = entry.note_content(&mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(content.format, NoteFormat::Plain);
        assert_eq!(content.background, None);
        assert_eq!(content.wrap_width, None);

        let mut scene = Scene::new(scene_id());
        let id = scene.insert_note("plain", Vec2::ZERO, Vec2::splat(80.0));
        let mut plain = scene.item(id).and_then(Item::note_content).expect("note");
        plain.format = NoteFormat::Plain;
        scene.item_mut(id).expect("note").set_note(&plain);
        let document = SceneDocument::from_scene(&scene, "n", &Camera::default(), Vec::new());
        let json = String::from_utf8(document.to_json().expect("json")).expect("utf-8");
        for member in ["text_format", "background", "wrap_width"] {
            assert!(!json.contains(member), "{member} must be omitted: {json}");
        }
    }

    #[test]
    fn an_invalid_background_is_ignored_with_a_warning() {
        for bad in [
            r#""red""#,
            r##""#12345""##,
            r##""#gg0000""##,
            "17",
            "[1,2,3]",
        ] {
            let entry = note_entry(&format!(r#","background":{bad}"#));
            let mut warnings = Vec::new();
            let content = entry.note_content(&mut warnings);
            assert_eq!(content.background, None, "{bad}");
            assert_eq!(warnings.len(), 1, "{bad}: {warnings:?}");
        }
        let entry = note_entry(r#","background":null"#);
        let mut warnings = Vec::new();
        assert_eq!(entry.note_content(&mut warnings).background, None);
        assert!(warnings.is_empty());
    }

    #[test]
    fn short_and_upper_case_backgrounds_are_normalised() {
        assert_eq!(super::parse_background("#FFF"), Some([255, 255, 255]));
        assert_eq!(super::parse_background("#302C25"), Some([0x30, 0x2c, 0x25]));
        assert_eq!(super::parse_background("302c25"), None);
        assert_eq!(super::parse_background("#é0000"), None);
        assert_eq!(super::format_background([0xab, 0x01, 0xff]), "#ab01ff");
    }

    #[test]
    fn wrap_width_is_clamped_on_load_and_an_unknown_format_reads_as_plain() {
        let mut warnings = Vec::new();
        assert_eq!(
            note_entry(r#","wrap_width":2"#)
                .note_content(&mut warnings)
                .wrap_width,
            Some(16.0)
        );
        assert_eq!(
            note_entry(r#","wrap_width":99999"#)
                .note_content(&mut warnings)
                .wrap_width,
            Some(10_000.0)
        );
        assert!(warnings.is_empty());
        assert_eq!(
            note_entry(r#","wrap_width":-3"#)
                .note_content(&mut warnings)
                .wrap_width,
            None
        );
        let content = note_entry(r#","text_format":"asciidoc""#).note_content(&mut warnings);
        assert_eq!(content.format, NoteFormat::Plain);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn a_v1_reader_keeps_the_new_note_members_as_unknown_fields() {
        // A v1 build's ItemEntry has no note members: simulate it by parsing
        // into a struct that only knows the v1 fields plus the flattened map.
        #[derive(serde::Deserialize, serde::Serialize)]
        struct V1Entry {
            id: u64,
            kind: String,
            text: Option<String>,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }
        let json = r##"{"id":2,"kind":"note","text":"# hi","text_format":"markdown","background":"#ffffff","wrap_width":120.0,"position":[0,0],"size":[10,10]}"##;
        let v1: V1Entry = serde_json::from_str(json).expect("v1 parses");
        for member in ["text_format", "background", "wrap_width"] {
            assert!(v1.extra.contains_key(member), "{member} lands in extra");
        }
        assert_eq!(v1.text.as_deref(), Some("# hi"));
        assert_eq!(v1.kind, "note");
        let resaved = serde_json::to_string(&v1).expect("resaves");
        let back: ItemEntry = serde_json::from_str(&resaved).expect("06a parses the resave");
        assert_eq!(back.text_format.as_deref(), Some("markdown"));
        assert_eq!(back.background.as_deref(), Some("#ffffff"));
        assert_eq!(back.wrap_width, Some(120.0));
        assert_eq!(back.id, 2);
    }

    #[test]
    fn unknown_note_members_still_round_trip_beside_the_new_ones() {
        let entry = note_entry(r##","background":"#000000","future":{"a":1}"##);
        assert!(entry.extra.contains_key("future"));
        let json = serde_json::to_string(&entry).expect("serialises");
        assert!(json.contains(r#""future":{"a":1}"#), "{json}");
        assert!(json.contains(r##""background":"#000000""##), "{json}");
    }
}
