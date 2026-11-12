//! Writing a `.rizref` file: fresh save, incremental append and compaction.
//!
//! # Save shapes
//!
//! * **Fresh** (Save As, or a first save): write header, name, thumbnail,
//!   document, blobs, previews, index and footer into `<file>.tmp`, flush it,
//!   then rename it over the target. A crash at any point leaves the previous
//!   file untouched, because the target's bytes are never modified in place.
//!
//! * **Incremental**: append only the chunks that changed — usually the
//!   document and the index, sometimes the thumbnail — then a new index and a
//!   new footer. Unchanged blobs and previews are referenced by offset and are
//!   not copied. Old indexes and footers remain in the file as dead space; the
//!   last valid index is what a reader uses.
//!
//! * **Compaction**: a full rewrite, triggered when appending would add more
//!   than twice the live bytes, or whenever appending is impossible (the file
//!   was recovered, is foreign, or names another scene). It is also what Save
//!   As does.
//!
//! The threshold is deliberately simple and predictable: compact when the file
//! would grow beyond three times its live bytes (equivalently, dead space over
//! twice live). A board with one changed note in a 500 MB file appends a few
//! kilobytes; a board whose document is rewritten a thousand times without new
//! images eventually pays one copy of its blobs to drop the dead indexes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rizref_core::{AssetId, Camera, Scene, SceneId};

use crate::checksum;
use crate::chunk::{CHUNK_HEADER_LEN, ChunkHeader, ChunkKind};
use crate::compression::{self, Compression};
use crate::document::{AssetEntry, AssetOrigin, PreviewEntry, SceneDocument};
use crate::error::FormatError;
use crate::header::{HEADER_LEN, Header};
use crate::index::{BoardIndex, ChunkRef, Footer, INDEX_SCHEMA_VERSION, IndexAsset};
use crate::preview::Preview;
use crate::reader::BoardReader;
use crate::thumbnail::{self, MAX_THUMBNAIL_SIDE};

/// How a save wrote the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveMode {
    /// The file did not exist; a new container was written.
    Fresh,
    /// New chunks were appended to an existing clean file.
    Appended,
    /// The file was rewritten to drop dead chunks.
    Compacted,
}

/// Where an asset came from and where it was before that.
///
/// Hints only: the board loads from embedded bytes and never needs the path.
/// A later relink flow may present it when an asset is not embedded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssetProvenance {
    /// The kind of source.
    pub origin: AssetOrigin,
    /// Original file path.
    pub source_path: Option<String>,
    /// Original URL.
    pub source_url: Option<String>,
}

/// Everything a save needs besides the output path.
#[derive(Debug)]
pub struct SaveRequest<'a> {
    /// The scene to write.
    pub scene: &'a Scene,
    /// The camera to restore on open.
    pub camera: Camera,
    /// Board display name, stored in a `NAME` chunk.
    pub name: &'a str,
    /// Origin hints by asset.
    pub provenance: &'a BTreeMap<AssetId, AssetProvenance>,
    /// Canvas colour behind a generated thumbnail.
    pub background: [u8; 4],
    /// The current time in Unix milliseconds.
    pub now_ms: u64,
    /// Force a fresh write, as Save As does.
    pub save_as: bool,
    /// A pre-rendered thumbnail. `None` lets the writer composite one.
    pub thumbnail: Option<Preview>,
    /// The document last read from disk, when the scene came from a board.
    ///
    /// A fresh write regenerates the document from the scene, but keeps the
    /// unknown members and link-only asset entries of this one, so Save As and
    /// compaction do not lose information a newer build stored.
    pub old_document: Option<&'a SceneDocument>,
}

impl<'a> SaveRequest<'a> {
    /// A save request with a generated thumbnail and no provenance.
    pub fn new(scene: &'a Scene, camera: Camera, name: &'a str) -> Self {
        Self {
            scene,
            camera,
            name,
            provenance: empty_provenance(),
            background: [24, 24, 28, 255],
            now_ms: crate::header::now_ms(),
            save_as: false,
            thumbnail: None,
            old_document: None,
        }
    }
}

fn empty_provenance() -> &'static BTreeMap<AssetId, AssetProvenance> {
    static EMPTY: std::sync::OnceLock<BTreeMap<AssetId, AssetProvenance>> =
        std::sync::OnceLock::new();
    EMPTY.get_or_init(BTreeMap::new)
}

/// What a save did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveReport {
    /// How the file was written.
    pub mode: SaveMode,
    /// Bytes written in this call.
    pub bytes_written: u64,
    /// Size of the file afterwards.
    pub file_len: u64,
    /// Generation written into the index.
    pub generation: u64,
    /// Identity of the board.
    pub scene_id: SceneId,
}

/// Saves `request.scene` to `path`.
///
/// An existing clean file with the same scene id is appended to when that is
/// cheaper than a rewrite; every other case writes a fresh file atomically.
pub fn save(path: impl AsRef<Path>, request: &SaveRequest<'_>) -> Result<SaveReport, FormatError> {
    let path = path.as_ref();
    let existed = path.is_file();
    // A read-only target would only fail at the final rename, after the whole
    // board was written to a temporary file; say so up front instead.
    if existed && std::fs::metadata(path).is_ok_and(|meta| meta.permissions().readonly()) {
        return Err(FormatError::Denied {
            operation: "save",
            path: path.display().to_string(),
            reason: "the file is read-only".to_owned(),
        });
    }
    let existing = if request.save_as {
        None
    } else {
        load_existing(path, request.scene.id)
    };

    if let Some(existing) = existing {
        let plan = plan_append(&existing, request)?;
        // Dead space is everything in the file that the new index does not
        // reference. A file of live + dead bytes, plus this save, stays under
        // three times live exactly while dead space is under twice live — the
        // documented threshold, counted without trusting a running total.
        let projected = existing.file_len.saturating_add(plan.new_bytes);
        if projected <= plan.live_bytes.saturating_mul(3) {
            return write_append(path, &existing, &plan);
        }
        return write_fresh(
            path,
            request,
            Some(&existing),
            SaveMode::Compacted,
            request.old_document,
        );
    }

    // A file that exists but cannot be appended to (recovered, foreign, or a
    // different scene) is replaced by a full write: that is a compaction in
    // every sense except that its old bytes were not reusable.
    let mode = if existed && !request.save_as {
        SaveMode::Compacted
    } else {
        SaveMode::Fresh
    };
    write_fresh(path, request, None, mode, request.old_document)
}

/// A loaded, clean file that an append can build on.
struct Existing {
    header: Header,
    index: BoardIndex,
    document: SceneDocument,
    file_len: u64,
}

/// Loads `path` when it is a clean board of `scene_id`.
fn load_existing(path: &Path, scene_id: SceneId) -> Option<Existing> {
    if !path.is_file() {
        return None;
    }
    let mut reader = BoardReader::open_path(path).ok()?;
    if reader.recovered() || !reader.header().has_footer() || reader.header().scene_id != scene_id {
        return None;
    }
    let document = reader.document().ok()?;
    Some(Existing {
        header: reader.header(),
        index: reader.index().clone(),
        document,
        file_len: reader.file_len(),
    })
}

/// One chunk that will be written, with the offset it will occupy.
struct PlannedChunk {
    offset: u64,
    kind: ChunkKind,
    compression: Compression,
    decompressed_len: u64,
    payload: Payload,
    digest: [u8; 32],
}

/// The bytes a planned chunk writes after its frame.
enum Payload {
    /// Already-encoded bytes, compressed or raw.
    Memory(Vec<u8>),
    /// A content hash followed by the original image bytes, streamed.
    Blob { hash: [u8; 32], bytes: Arc<[u8]> },
}

impl Payload {
    fn stored_len(&self) -> u64 {
        match self {
            Self::Memory(bytes) => bytes.len() as u64,
            Self::Blob { bytes, .. } => 32 + bytes.len() as u64,
        }
    }

    fn stored(&self) -> &[u8] {
        match self {
            Self::Memory(bytes) => bytes,
            Self::Blob { .. } => &[],
        }
    }
}

impl PlannedChunk {
    fn frame_len(&self) -> u64 {
        CHUNK_HEADER_LEN as u64 + self.payload.stored_len()
    }

    fn reference(&self) -> ChunkRef {
        ChunkRef {
            offset: self.offset,
            tag: String::from_utf8_lossy(&self.kind.tag()).into_owned(),
            stored_len: self.payload.stored_len(),
            decompressed_len: self.decompressed_len,
            compression: self.compression.code(),
            digest: checksum::to_hex(&self.digest),
            extra: BTreeMap::new(),
        }
    }
}

/// A complete append.
struct AppendPlan {
    chunks: Vec<PlannedChunk>,
    generation: u64,
    /// Live bytes of the board the plan describes.
    live_bytes: u64,
    /// Bytes this save adds to the file.
    new_bytes: u64,
}

/// A complete fresh write.
struct FreshPlan {
    chunks: Vec<PlannedChunk>,
    generation: u64,
}

/// Encodes every asset preview and returns its document metadata and payloads.
type PreviewPlan = (
    BTreeMap<String, PreviewEntry>,
    Vec<(String, [u8; 32], Arc<[u8]>)>,
);

fn encode_previews(scene: &Scene) -> Result<PreviewPlan, FormatError> {
    let mut meta = BTreeMap::new();
    let mut payloads = Vec::new();
    let mut assets: Vec<_> = scene.assets().iter().collect();
    assets.sort_by_key(|(id, _)| id.to_hex());
    for (id, asset) in assets {
        let hash = id.to_hex();
        let (format, bytes) = thumbnail::encode_preview(asset.preview().image())?;
        meta.insert(
            hash.clone(),
            PreviewEntry {
                format,
                pixel_size: [asset.preview().width(), asset.preview().height()],
                byte_len: bytes.len() as u64,
                extra: BTreeMap::new(),
            },
        );
        payloads.push((hash, *id.as_bytes(), Arc::from(bytes)));
    }
    Ok((meta, payloads))
}

fn plan_append(existing: &Existing, request: &SaveRequest<'_>) -> Result<AppendPlan, FormatError> {
    let start = existing.file_len;
    let mut chunks: Vec<PlannedChunk> = Vec::new();
    let mut offset = start;

    let old_assets: BTreeMap<&str, &AssetEntry> = existing
        .document
        .assets
        .iter()
        .map(|entry| (entry.hash.as_str(), entry))
        .collect();
    let old_index_assets: BTreeMap<&str, &IndexAsset> = existing
        .index
        .assets
        .iter()
        .map(|asset| (asset.hash.as_str(), asset))
        .collect();

    let mut preview_meta: BTreeMap<String, PreviewEntry> = BTreeMap::new();
    let mut new_previews: BTreeMap<String, ([u8; 32], Arc<[u8]>)> = BTreeMap::new();
    for (id, asset) in request.scene.assets().iter() {
        let hash = id.to_hex();
        if let Some(entry) = old_assets.get(hash.as_str())
            && let Some(preview) = &entry.preview
            && old_index_assets
                .get(hash.as_str())
                .is_some_and(|indexed| indexed.preview.is_some())
        {
            preview_meta.insert(hash, preview.clone());
            continue;
        }
        // Only an asset whose preview is not already stored pays for encoding.
        let (format, bytes) = thumbnail::encode_preview(asset.preview().image())?;
        preview_meta.insert(
            hash.clone(),
            PreviewEntry {
                format,
                pixel_size: [asset.preview().width(), asset.preview().height()],
                byte_len: bytes.len() as u64,
                extra: BTreeMap::new(),
            },
        );
        new_previews.insert(hash, (*id.as_bytes(), Arc::from(bytes)));
    }

    let assets = build_asset_entries(request, &existing.document, &preview_meta);
    let mut document = SceneDocument::from_scene(
        request.scene,
        request.name,
        &request.camera,
        assets.into_values().collect(),
    );
    document.board.name = request.name.to_owned();
    carry_unknown_fields(&mut document, &existing.document);
    let document_bytes = document.to_json()?;
    let document_digest = checksum::digest(&document_bytes);

    let name_bytes = request.name.as_bytes();
    let name_ref = if existing.index.name.as_ref().is_some_and(|reference| {
        reference.digest == checksum::to_hex(&checksum::digest(name_bytes))
    }) {
        existing.index.name.clone()
    } else {
        let chunk = plan_memory(ChunkKind::Name, name_bytes, true, offset)?;
        offset += chunk.frame_len();
        let reference = chunk.reference();
        chunks.push(chunk);
        Some(reference)
    };

    let thumbnail = match &request.thumbnail {
        Some(thumbnail) => thumbnail.clone(),
        None => thumbnail::generate(request.scene, request.background, MAX_THUMBNAIL_SIDE)?,
    };
    let thumbnail_payload = thumbnail_payload(&thumbnail);
    let thumb_digest = checksum::digest(&thumbnail_payload);
    let thumbnail_ref = if existing
        .index
        .thumbnail
        .as_ref()
        .is_some_and(|reference| reference.digest == checksum::to_hex(&thumb_digest))
    {
        existing.index.thumbnail.clone()
    } else {
        let chunk = plan_memory(ChunkKind::Thumbnail, &thumbnail_payload, true, offset)?;
        offset += chunk.frame_len();
        let reference = chunk.reference();
        chunks.push(chunk);
        Some(reference)
    };

    let document_ref = if existing.index.document.digest == checksum::to_hex(&document_digest) {
        existing.index.document.clone()
    } else {
        let chunk = plan_memory(ChunkKind::Document, &document_bytes, true, offset)?;
        offset += chunk.frame_len();
        let reference = chunk.reference();
        chunks.push(chunk);
        reference
    };

    let mut blob_refs: BTreeMap<String, ChunkRef> = BTreeMap::new();
    for entry in &document.assets {
        let Some(id) = AssetId::from_hex(&entry.hash) else {
            continue;
        };
        if let Some(existing_ref) = old_index_assets
            .get(entry.hash.as_str())
            .and_then(|asset| asset.blob.clone())
        {
            blob_refs.insert(entry.hash.clone(), existing_ref);
            continue;
        }
        if !entry.embedded {
            continue;
        }
        let Some(asset) = request.scene.asset(id) else {
            continue;
        };
        let chunk = plan_blob(id, asset.encoded(), offset)?;
        offset += chunk.frame_len();
        blob_refs.insert(entry.hash.clone(), chunk.reference());
        chunks.push(chunk);
    }

    let mut preview_refs: BTreeMap<String, ChunkRef> = BTreeMap::new();
    for entry in &document.assets {
        if let Some(existing_ref) = old_index_assets
            .get(entry.hash.as_str())
            .and_then(|asset| asset.preview.clone())
        {
            preview_refs.insert(entry.hash.clone(), existing_ref);
            continue;
        }
        let Some((hash_bytes, bytes)) = new_previews.remove(&entry.hash) else {
            continue;
        };
        let chunk = plan_preview(hash_bytes, bytes, offset)?;
        offset += chunk.frame_len();
        preview_refs.insert(entry.hash.clone(), chunk.reference());
        chunks.push(chunk);
    }

    let generation = existing.index.generation.saturating_add(1);
    // `appended_bytes` counts dead space added since the last full save,
    // excluding this save's own index and footer. That keeps the number
    // independent of its own encoded length (the index is written after the
    // content and would otherwise need a fixed-point iteration).
    let appended_before_index = existing.index.appended_bytes.saturating_add(offset - start);
    let index = BoardIndex {
        schema_version: INDEX_SCHEMA_VERSION,
        generation,
        created_ms: existing.header.created_ms,
        modified_ms: request.now_ms,
        appended_bytes: appended_before_index,
        writer: writer_label(),
        name: name_ref,
        document: document_ref,
        thumbnail: thumbnail_ref,
        assets: index_assets(&document, &blob_refs, &preview_refs),
        extra: BTreeMap::new(),
    };
    let (index_chunk, footer_chunk) = plan_index(&index, offset)?;
    let index_bytes = index_chunk.frame_len();
    let footer_bytes = footer_chunk.frame_len();
    offset += index_bytes + footer_bytes;
    chunks.push(index_chunk);
    chunks.push(footer_chunk);

    let new_bytes = offset - start;
    let live_bytes = u64::from(existing.header.header_size)
        .saturating_add(index.live_chunk_bytes())
        .saturating_add(index_bytes)
        .saturating_add(footer_bytes);

    Ok(AppendPlan {
        chunks,
        generation,
        live_bytes,
        new_bytes,
    })
}

fn plan_fresh(
    request: &SaveRequest<'_>,
    header: Header,
    generation: u64,
    old_document: Option<&SceneDocument>,
) -> Result<FreshPlan, FormatError> {
    let mut chunks: Vec<PlannedChunk> = Vec::new();
    let mut offset = HEADER_LEN as u64;

    let name_chunk = plan_memory(ChunkKind::Name, request.name.as_bytes(), true, offset)?;
    offset += name_chunk.frame_len();
    let name_ref = name_chunk.reference();
    chunks.push(name_chunk);

    let thumbnail = match &request.thumbnail {
        Some(thumbnail) => thumbnail.clone(),
        None => thumbnail::generate(request.scene, request.background, MAX_THUMBNAIL_SIDE)?,
    };
    let thumbnail_payload = thumbnail_payload(&thumbnail);
    let thumbnail_chunk = plan_memory(ChunkKind::Thumbnail, &thumbnail_payload, true, offset)?;
    offset += thumbnail_chunk.frame_len();
    let thumbnail_ref = thumbnail_chunk.reference();
    chunks.push(thumbnail_chunk);

    let (preview_meta, preview_payloads) = encode_previews(request.scene)?;
    let empty = SceneDocument::new(request.scene.id, request.name, &request.camera);
    let old = old_document.unwrap_or(&empty);
    let assets = build_asset_entries(request, old, &preview_meta);
    let mut document = SceneDocument::from_scene(
        request.scene,
        request.name,
        &request.camera,
        assets.into_values().collect(),
    );
    document.board.name = request.name.to_owned();
    carry_unknown_fields(&mut document, old);
    let document_bytes = document.to_json()?;
    let document_chunk = plan_memory(ChunkKind::Document, &document_bytes, true, offset)?;
    offset += document_chunk.frame_len();
    let document_ref = document_chunk.reference();
    chunks.push(document_chunk);

    let mut blob_refs: BTreeMap<String, ChunkRef> = BTreeMap::new();
    let mut sorted_assets: Vec<_> = request.scene.assets().iter().collect();
    sorted_assets.sort_by_key(|(id, _)| id.to_hex());
    for (id, asset) in sorted_assets {
        let chunk = plan_blob(*id, asset.encoded(), offset)?;
        offset += chunk.frame_len();
        blob_refs.insert(id.to_hex(), chunk.reference());
        chunks.push(chunk);
    }

    let mut preview_refs: BTreeMap<String, ChunkRef> = BTreeMap::new();
    for (hash, hash_bytes, bytes) in preview_payloads {
        let chunk = plan_preview(hash_bytes, bytes, offset)?;
        offset += chunk.frame_len();
        preview_refs.insert(hash, chunk.reference());
        chunks.push(chunk);
    }

    let index = BoardIndex {
        schema_version: INDEX_SCHEMA_VERSION,
        generation,
        created_ms: header.created_ms,
        modified_ms: header.modified_ms,
        appended_bytes: 0,
        writer: writer_label(),
        name: Some(name_ref),
        document: document_ref,
        thumbnail: Some(thumbnail_ref),
        assets: index_assets(&document, &blob_refs, &preview_refs),
        extra: BTreeMap::new(),
    };
    let (index_chunk, footer_chunk) = plan_index(&index, offset)?;
    chunks.push(index_chunk);
    chunks.push(footer_chunk);

    Ok(FreshPlan { chunks, generation })
}

/// Plans the index and footer that close a file at `offset`.
fn plan_index(
    index: &BoardIndex,
    offset: u64,
) -> Result<(PlannedChunk, PlannedChunk), FormatError> {
    let payload = index.encode_payload()?;
    let index_chunk = plan_memory(ChunkKind::Index, &payload, true, offset)?;
    let footer = Footer {
        generation: index.generation,
        index_offset: index_chunk.offset,
        index_len: index_chunk.payload.stored_len(),
        index_digest: checksum::digest(index_chunk.payload.stored()),
    };
    let footer_chunk = plan_memory(
        ChunkKind::Footer,
        &footer.encode(),
        false,
        offset + index_chunk.frame_len(),
    )?;
    Ok((index_chunk, footer_chunk))
}

fn index_assets(
    document: &SceneDocument,
    blobs: &BTreeMap<String, ChunkRef>,
    previews: &BTreeMap<String, ChunkRef>,
) -> Vec<IndexAsset> {
    document
        .assets
        .iter()
        .map(|entry| IndexAsset {
            hash: entry.hash.clone(),
            blob: blobs.get(&entry.hash).cloned(),
            preview: previews.get(&entry.hash).cloned(),
            extra: BTreeMap::new(),
        })
        .collect()
}

fn build_asset_entries(
    request: &SaveRequest<'_>,
    old_document: &SceneDocument,
    preview_meta: &BTreeMap<String, PreviewEntry>,
) -> BTreeMap<String, AssetEntry> {
    let old: BTreeMap<&str, &AssetEntry> = old_document
        .assets
        .iter()
        .map(|entry| (entry.hash.as_str(), entry))
        .collect();
    let referenced: BTreeSet<String> = request
        .scene
        .items()
        .iter()
        .filter_map(|item| item.asset().map(|asset| asset.to_hex()))
        .collect();

    let mut entries = BTreeMap::new();
    for (id, asset) in request.scene.assets().iter() {
        let hash = id.to_hex();
        if !referenced.contains(&hash) {
            continue;
        }
        let previous = old.get(hash.as_str()).copied();
        let mut entry = previous.cloned().unwrap_or(AssetEntry {
            hash: hash.clone(),
            format: asset.format().extension().to_owned(),
            pixel_size: [asset.pixel_width(), asset.pixel_height()],
            byte_len: asset.encoded().len() as u64,
            origin: AssetOrigin::Unknown,
            source_path: None,
            source_url: None,
            embedded: true,
            preview: None,
            extra: BTreeMap::new(),
        });
        entry.format = asset.format().extension().to_owned();
        entry.pixel_size = [asset.pixel_width(), asset.pixel_height()];
        entry.byte_len = asset.encoded().len() as u64;
        entry.embedded = true;
        entry.preview = preview_meta.get(&hash).cloned().or(entry.preview);
        if let Some(provenance) = request.provenance.get(id) {
            entry.origin = provenance.origin;
            entry.source_path = provenance.source_path.clone();
            entry.source_url = provenance.source_url.clone();
        }
        entries.insert(hash, entry);
    }

    // Items whose linked asset is not in the store keep their old entry, so a
    // missing-link board can be saved again without losing the link.
    for hash in referenced {
        if entries.contains_key(&hash) {
            continue;
        }
        if let Some(previous) = old.get(hash.as_str()) {
            entries.insert(hash, (*previous).clone());
        }
    }
    entries
}

/// Copies unknown members from `old` into the regenerated `new` document.
fn carry_unknown_fields(new: &mut SceneDocument, old: &SceneDocument) {
    new.extra = old.extra.clone();
    new.board.extra = old.board.extra.clone();
    new.camera.extra = old.camera.extra.clone();

    let item_extras: BTreeMap<u64, BTreeMap<String, serde_json::Value>> = old
        .items
        .iter()
        .map(|item| (item.id, item.extra.clone()))
        .collect();
    for item in &mut new.items {
        if let Some(extra) = item_extras.get(&item.id) {
            item.extra = extra.clone();
        }
    }

    let group_extras: BTreeMap<u64, BTreeMap<String, serde_json::Value>> = old
        .groups
        .iter()
        .map(|group| (group.id, group.extra.clone()))
        .collect();
    let child_extras: BTreeMap<(u64, String, u64), BTreeMap<String, serde_json::Value>> = old
        .groups
        .iter()
        .flat_map(|group| {
            group.children.iter().map(move |child| {
                (
                    (group.id, format!("{:?}", child.kind), child.id),
                    child.extra.clone(),
                )
            })
        })
        .collect();
    for group in &mut new.groups {
        if let Some(extra) = group_extras.get(&group.id) {
            group.extra = extra.clone();
        }
        for child in &mut group.children {
            if let Some(extra) =
                child_extras.get(&(group.id, format!("{:?}", child.kind), child.id))
            {
                child.extra = extra.clone();
            }
        }
    }
}

fn writer_label() -> String {
    format!("rizref {}", env!("CARGO_PKG_VERSION"))
}

/// A `THMB` payload: pixel size then encoded image bytes.
fn thumbnail_payload(thumbnail: &Preview) -> Vec<u8> {
    let mut payload = Vec::with_capacity(8 + thumbnail.bytes.len());
    payload.extend_from_slice(&thumbnail.width.to_le_bytes());
    payload.extend_from_slice(&thumbnail.height.to_le_bytes());
    payload.extend_from_slice(&thumbnail.bytes);
    payload
}

fn plan_memory(
    kind: ChunkKind,
    bytes: &[u8],
    compress: bool,
    offset: u64,
) -> Result<PlannedChunk, FormatError> {
    compression::check_len(bytes.len() as u64)?;
    let (compression, stored) = if compress {
        compression::choose(bytes)?
    } else {
        compression::store_raw(bytes)?
    };
    Ok(PlannedChunk {
        offset,
        kind,
        compression,
        decompressed_len: bytes.len() as u64,
        payload: Payload::Memory(stored),
        digest: checksum::digest(bytes),
    })
}

fn plan_blob(id: AssetId, bytes: &Arc<[u8]>, offset: u64) -> Result<PlannedChunk, FormatError> {
    compression::check_len(32 + bytes.len() as u64)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(id.as_bytes());
    hasher.update(bytes);
    Ok(PlannedChunk {
        offset,
        kind: ChunkKind::Blob,
        compression: Compression::Raw,
        decompressed_len: 32 + bytes.len() as u64,
        payload: Payload::Blob {
            hash: *id.as_bytes(),
            bytes: Arc::clone(bytes),
        },
        digest: *hasher.finalize().as_bytes(),
    })
}

fn plan_preview(
    hash: [u8; 32],
    bytes: Arc<[u8]>,
    offset: u64,
) -> Result<PlannedChunk, FormatError> {
    let mut payload = Vec::with_capacity(32 + bytes.len());
    payload.extend_from_slice(&hash);
    payload.extend_from_slice(&bytes);
    plan_memory(ChunkKind::Preview, &payload, false, offset)
}

fn write_chunk<W: Write>(writer: &mut W, chunk: &PlannedChunk) -> std::io::Result<()> {
    let header = ChunkHeader::new(
        chunk.kind,
        chunk.compression,
        chunk.payload.stored_len(),
        chunk.decompressed_len,
    );
    writer.write_all(&header.encode())?;
    match &chunk.payload {
        Payload::Memory(bytes) => writer.write_all(bytes),
        Payload::Blob { hash, bytes } => {
            writer.write_all(hash)?;
            writer.write_all(bytes)
        }
    }
}

fn write_append(
    path: &Path,
    existing: &Existing,
    plan: &AppendPlan,
) -> Result<SaveReport, FormatError> {
    let mut file = OpenOptions::new()
        .append(true)
        .read(true)
        .open(path)
        .map_err(|error| FormatError::io("append", path.display().to_string(), error))?;
    let start = file
        .seek(SeekFrom::End(0))
        .map_err(|error| FormatError::io("seek", path.display().to_string(), error))?;
    if start != existing.file_len {
        return Err(FormatError::Io {
            operation: "append",
            path: path.display().to_string(),
            reason: "the file changed while it was being saved".to_owned(),
        });
    }
    for chunk in &plan.chunks {
        write_chunk(&mut file, chunk)
            .map_err(|error| FormatError::io("append", path.display().to_string(), error))?;
    }
    file.sync_all()
        .map_err(|error| FormatError::io("flush", path.display().to_string(), error))?;
    let file_len = start + plan.new_bytes;
    Ok(SaveReport {
        mode: SaveMode::Appended,
        bytes_written: plan.new_bytes,
        file_len,
        generation: plan.generation,
        scene_id: existing.header.scene_id,
    })
}

fn write_fresh(
    path: &Path,
    request: &SaveRequest<'_>,
    existing: Option<&Existing>,
    mode: SaveMode,
    previous_document: Option<&SceneDocument>,
) -> Result<SaveReport, FormatError> {
    let created_ms = existing.map_or(request.now_ms, |existing| existing.header.created_ms);
    let generation = existing.map_or(1, |existing| existing.index.generation.saturating_add(1));
    let mut header = Header::new(request.scene.id, created_ms);
    header.modified_ms = request.now_ms;
    header = header.with_flag(Header::FLAG_HAS_FOOTER);

    let old_document = existing
        .map(|existing| &existing.document)
        .or(previous_document);
    let plan = plan_fresh(request, header, generation, old_document)?;
    let temporary = temporary_path(path);
    let result = write_temporary(&temporary, path, &header, &plan);
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    let file_len = result?;
    Ok(SaveReport {
        mode,
        bytes_written: file_len,
        file_len,
        generation: plan.generation,
        scene_id: request.scene.id,
    })
}

fn write_temporary(
    temporary: &Path,
    target: &Path,
    header: &Header,
    plan: &FreshPlan,
) -> Result<u64, FormatError> {
    let mut file = File::create(temporary)
        .map_err(|error| FormatError::io("create", temporary.display().to_string(), error))?;
    file.write_all(&header.encode())
        .map_err(|error| FormatError::io("write", temporary.display().to_string(), error))?;
    let mut len = HEADER_LEN as u64;
    for chunk in &plan.chunks {
        write_chunk(&mut file, chunk)
            .map_err(|error| FormatError::io("write", temporary.display().to_string(), error))?;
        len += chunk.frame_len();
    }
    file.sync_all()
        .map_err(|error| FormatError::io("flush", temporary.display().to_string(), error))?;
    drop(file);
    replace(temporary, target)
        .map_err(|error| FormatError::io("replace", target.display().to_string(), error))?;
    Ok(len)
}

/// How often a refused rename is retried, and the first pause between tries.
const REPLACE_ATTEMPTS: u32 = 6;
const REPLACE_BACKOFF: std::time::Duration = std::time::Duration::from_millis(40);

/// Renames `temporary` over `target`, retrying briefly when the target is
/// held open. On Windows a search indexer, an antivirus scan, a sync client or
/// Explorer's own thumbnail provider can hold a board for a few milliseconds,
/// and a rename over it fails with "access denied" in that window. Six tries
/// with doubling pauses wait at most about 1.2 s before the error surfaces.
fn replace(temporary: &Path, target: &Path) -> std::io::Result<()> {
    let mut pause = REPLACE_BACKOFF;
    let mut attempt = 1;
    loop {
        match std::fs::rename(temporary, target) {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt < REPLACE_ATTEMPTS
                    && error.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                std::thread::sleep(pause);
                pause *= 2;
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("board.rizref");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{name}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::{AppendPlan, SaveMode, SaveRequest, load_existing, plan_append, save, write_chunk};
    use glam::{DVec2, Vec2};
    use rizref_codecs::{ImageBuffer, encode_jpeg, encode_png};
    use rizref_core::{Asset, Camera, GroupChild, Scene, SceneId};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rizref-format-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn board(bytes: &[u8], side: u32) -> (Scene, Camera) {
        let mut scene = Scene::new(SceneId::random());
        let asset = Asset::from_decoded(
            std::sync::Arc::from(bytes),
            rizref_codecs::ImageFormat::Png,
            &ImageBuffer::filled(32, 24, [200, 40, 40, 255]),
            ImageBuffer::filled(side, side, [200, 40, 40, 255]),
        );
        let (id, _) = scene.insert_asset(asset);
        scene
            .insert_image(id, Vec2::new(100.0, 100.0), Vec2::new(64.0, 48.0))
            .expect("asset");
        scene.insert_note("a note", Vec2::new(-100.0, 50.0), Vec2::splat(80.0));
        (scene, Camera::new(DVec2::new(10.0, 20.0), 1.25, 0.1))
    }

    #[test]
    fn a_read_only_target_is_refused_before_anything_is_written() {
        let dir = temp_dir("read-only");
        let path = dir.join("locked.rizref");
        let (scene, camera) = board(b"bytes", 8);
        save(&path, &request(&scene, &camera, "locked")).expect("first save");
        let mut permissions = std::fs::metadata(&path).expect("meta").permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions.clone()).expect("read-only");
        let before = std::fs::read(&path).expect("read");

        let mut changed = scene.clone();
        changed.insert_note("more", Vec2::ZERO, Vec2::splat(40.0));
        let mut fresh = request(&changed, &camera, "locked");
        fresh.save_as = true;
        let error = save(&path, &fresh).expect_err("read-only");

        assert!(error.is_denied(), "{error}");
        assert!(error.to_string().contains("read-only"), "{error}");
        assert_eq!(std::fs::read(&path).expect("read"), before, "untouched");
        assert!(!dir.join("locked.rizref.tmp").exists(), "no temporary left");
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        let _ = std::fs::set_permissions(&path, permissions);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_rename_over_a_briefly_locked_target_is_retried() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp_dir("locked");
        let path = dir.join("busy.rizref");
        let (scene, camera) = board(b"bytes", 8);
        save(&path, &request(&scene, &camera, "busy")).expect("first save");
        // Hold the board open without FILE_SHARE_DELETE, as an indexer can,
        // and let go after ~100 ms.
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1) // FILE_SHARE_READ only
            .open(&path)
            .expect("hold");
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            drop(holder);
        });

        let mut fresh = request(&scene, &camera, "busy");
        fresh.save_as = true;
        let report = save(&path, &fresh).expect("the retry outlasts the lock");

        release.join().expect("holder thread");
        assert_eq!(report.mode, SaveMode::Fresh);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_shell_thumbnail_reads_the_embedded_picture_or_composites_previews() {
        let dir = temp_dir("shell-thumb");
        let path = dir.join("board.rizref");
        let (mut scene, camera) = board(b"original bytes", 8);
        let note = scene.insert_note("coloured", Vec2::new(-300.0, -200.0), Vec2::splat(90.0));
        let mut content = scene
            .item(note)
            .and_then(rizref_core::Item::note_content)
            .expect("note");
        content.background = Some([10, 200, 30]);
        scene.item_mut(note).expect("note").set_note(&content);
        save(&path, &request(&scene, &camera, "shell")).expect("save");

        let mut reader = crate::BoardReader::open_path(&path).expect("open");
        let embedded = reader
            .thumbnail_image([24, 24, 28, 255])
            .expect("thumbnail");
        let composite = reader
            .preview_composite([24, 24, 28, 255])
            .expect("composite");

        assert_eq!(
            (embedded.width(), embedded.height()),
            (composite.width(), composite.height()),
            "the fallback draws the same picture size the save embedded"
        );
        // The coloured note sits in the top-left corner of the board's box;
        // both pictures paint it green, from the 06a `background` field.
        let corner = |image: &ImageBuffer| {
            let [r, g, b, _] = image.pixel(4, 4).expect("in bounds");
            g > 150 && r < 80 && b < 80
        };
        assert!(
            corner(&composite),
            "composite paints the stored note colour"
        );
        assert!(corner(&embedded), "the embedded thumbnail does too");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn request<'a>(scene: &'a Scene, camera: &'a Camera, name: &'a str) -> SaveRequest<'a> {
        SaveRequest::new(scene, *camera, name)
    }

    fn encoded(seed: u8, jpeg: bool) -> (Arc<[u8]>, rizref_codecs::ImageFormat) {
        let image = ImageBuffer::filled(
            16 + u32::from(seed % 8),
            12 + u32::from(seed % 5),
            [seed, seed.wrapping_mul(3), seed.wrapping_mul(7), 255],
        );
        if jpeg {
            (
                Arc::from(encode_jpeg(&image, 80).expect("jpeg")),
                rizref_codecs::ImageFormat::Jpeg,
            )
        } else {
            (
                Arc::from(encode_png(&image).expect("png")),
                rizref_codecs::ImageFormat::Png,
            )
        }
    }

    fn large_board() -> Scene {
        let mut scene = Scene::new(SceneId::random());
        let mut assets = Vec::new();
        for index in 0..12_u8 {
            let (bytes, format) = encoded(index, index % 2 == 1);
            let pixels = ImageBuffer::filled(
                16 + u32::from(index % 8),
                12 + u32::from(index % 5),
                [index, index.wrapping_mul(3), index.wrapping_mul(7), 255],
            );
            let asset = Asset::from_decoded(
                bytes,
                format,
                &pixels,
                ImageBuffer::filled(8, 6, [index, index, index, 255]),
            );
            assets.push(scene.insert_asset(asset).0);
        }
        let mut item_ids = Vec::new();
        for index in 0..198 {
            let asset = assets[index % assets.len()];
            let id = scene
                .insert_image(
                    asset,
                    Vec2::new((index % 20) as f32 * 90.0, (index / 20) as f32 * 70.0),
                    Vec2::new(80.0, 60.0),
                )
                .expect("asset");
            item_ids.push(id);
        }
        for index in 0..2 {
            let id = scene.insert_note(
                format!("study note {index}"),
                Vec2::new(1_700.0 + index as f32 * 200.0, 0.0),
                Vec2::splat(120.0),
            );
            item_ids.push(id);
        }
        for index in 0..10 {
            let group = scene.allocate_group_id();
            let members: Vec<GroupChild> = item_ids[index * 20..(index + 1) * 20]
                .iter()
                .map(|id| GroupChild::Item(*id))
                .collect();
            scene.attach_group(group, members);
        }
        scene
    }

    #[test]
    fn a_fresh_save_reads_back_with_the_same_scene() {
        let dir = temp_dir("fresh");
        let path = dir.join("fresh.rizref");
        let (scene, camera) = board(&vec![7_u8; 4096], 8);
        let report = save(&path, &request(&scene, &camera, "Fresh")).expect("save");

        assert_eq!(report.mode, SaveMode::Fresh);

        let loaded = crate::load(&path).expect("load");
        assert!(!loaded.recovered);
        assert_eq!(loaded.name, "Fresh");
        assert_eq!(loaded.camera.eye, DVec2::new(10.0, 20.0));
        assert_eq!(loaded.scene.items().len(), scene.items().len());
        assert_eq!(loaded.scene.assets().len(), 1);
        assert!(loaded.thumbnail.is_some());
        assert_eq!(
            loaded
                .scene
                .assets()
                .iter()
                .next()
                .expect("asset")
                .1
                .encoded()
                .as_ref(),
            &vec![7_u8; 4096]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_members_survive_a_regenerated_document() {
        let dir = temp_dir("unknown");
        let path = dir.join("unknown.rizref");
        let (scene, camera) = board(&[5_u8; 64], 8);
        save(&path, &request(&scene, &camera, "Unknown")).expect("save");

        // Add members a future build might have written, directly to the
        // document as it would have parsed them.
        let mut document = crate::SceneDocument::from_json(
            &std::fs::read(&path)
                .ok()
                .and_then(|bytes| {
                    let mut reader = crate::BoardReader::from_bytes(bytes).ok()?;
                    reader.document().ok()?.to_json().ok()
                })
                .expect("document bytes"),
        )
        .expect("parse");
        document
            .extra
            .insert("future_top".to_owned(), serde_json::json!({ "dpi": 300 }));
        document.assets[0]
            .extra
            .insert("future_asset".to_owned(), serde_json::json!(true));
        document.items[0]
            .extra
            .insert("future_item".to_owned(), serde_json::json!("amber"));

        let mut regenerated =
            crate::SceneDocument::from_scene(&scene, "Unknown", &camera, document.assets.clone());
        super::carry_unknown_fields(&mut regenerated, &document);
        let value = serde_json::to_value(&regenerated).expect("json");

        assert_eq!(value["future_top"]["dpi"], 300);
        assert_eq!(value["assets"][0]["future_asset"], true);
        assert_eq!(value["items"][0]["future_item"], "amber");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_as_keeps_unknown_members_from_the_loaded_document() {
        let dir = temp_dir("saveas-unknown");
        let path = dir.join("one.rizref");
        let copy = dir.join("two.rizref");
        let (scene, camera) = board(&[6_u8; 64], 8);
        save(&path, &request(&scene, &camera, "One")).expect("save");

        let mut document = {
            let bytes = std::fs::read(&path).expect("read");
            let mut reader = crate::BoardReader::from_bytes(bytes).expect("open");
            reader.document().expect("document")
        };
        document
            .extra
            .insert("future_top".to_owned(), serde_json::json!({ "dpi": 600 }));

        let mut request = request(&scene, &camera, "Two");
        request.save_as = true;
        request.old_document = Some(&document);
        save(&copy, &request).expect("save as");

        let bytes = std::fs::read(&copy).expect("read");
        let mut reader = crate::BoardReader::from_bytes(bytes).expect("open");
        let written = reader.document().expect("document");
        assert_eq!(written.extra["future_top"]["dpi"], 600);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_200_item_mixed_board_round_trips() {
        let dir = temp_dir("large");
        let path = dir.join("large.rizref");
        let scene = large_board();
        let camera = Camera::new(DVec2::new(-40.0, 12.5), 0.8, -0.15);
        save(&path, &request(&scene, &camera, "Large board")).expect("save");

        let loaded = crate::load(&path).expect("load");

        assert_eq!(loaded.scene.items().len(), 200);
        assert_eq!(loaded.scene.assets().len(), 12);
        assert_eq!(loaded.scene.groups(), scene.groups());
        for (expected, actual) in scene.items().iter().zip(loaded.scene.items()) {
            assert_eq!(actual.id(), expected.id());
            assert_eq!(actual.center(), expected.center());
            assert_eq!(actual.size(), expected.size());
            assert_eq!(actual.rotation(), expected.rotation());
            assert_eq!(actual.opacity(), expected.opacity());
            assert_eq!(actual.asset(), expected.asset());
            assert_eq!(actual.text(), expected.text());
        }
        for (id, expected) in scene.assets().iter() {
            let actual = loaded.scene.asset(*id).expect("asset survived");
            assert_eq!(actual.encoded(), expected.encoded());
            assert_eq!(actual.pixel_width(), expected.pixel_width());
            assert_eq!(actual.pixel_height(), expected.pixel_height());
        }
        assert_eq!(loaded.camera.eye, camera.eye);
        assert_eq!(loaded.camera.zoom, camera.zoom);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_accepts_a_produced_file_and_catches_a_damaged_blob() {
        let dir = temp_dir("verify");
        let path = dir.join("verify.rizref");
        let (scene, camera) = board(&vec![4_u8; 2048], 8);
        save(&path, &request(&scene, &camera, "Verify")).expect("save");

        let mut reader = crate::BoardReader::open_path(&path).expect("open");
        let report = reader.verify().expect("verify");
        assert!(report.is_ok(), "unexpected problems: {:?}", report.problems);
        assert!(report.blobs_verified >= 1);

        // Find the blob payload and flip one byte.
        let original = std::fs::read(&path).expect("read");
        let mut damaged = original.clone();
        let offset = damaged
            .windows(4)
            .position(|window| window == b"BLOB")
            .expect("blob chunk")
            + 24
            + 40;
        damaged[offset] ^= 0x01;
        std::fs::write(&path, &damaged).expect("write");
        let mut reader = crate::BoardReader::open_path(&path).expect("open");
        let report = reader.verify().expect("verify");
        assert!(!report.is_ok());
        assert!(matches!(
            crate::load(&path).expect_err("damaged blob must fail"),
            crate::FormatError::IntegrityMismatch { .. } | crate::FormatError::Truncated { .. }
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_second_save_appends_without_copying_the_blob() {
        let dir = temp_dir("append");
        let path = dir.join("append.rizref");
        let (mut scene, camera) = board(&vec![9_u8; 64 * 1024], 8);
        save(&path, &request(&scene, &camera, "Append")).expect("first save");
        let first_len = std::fs::metadata(&path).expect("metadata").len();

        let note = scene
            .items()
            .iter()
            .find(|item| item.is_note())
            .expect("note")
            .id();
        scene
            .item_mut(note)
            .expect("note")
            .set_text("a changed note");
        let report = save(&path, &request(&scene, &camera, "Append")).expect("append");

        assert_eq!(report.mode, SaveMode::Appended);
        assert!(
            report.bytes_written < 64 * 1024,
            "the blob must not be copied"
        );
        assert!(report.file_len > first_len);
        let loaded = crate::load(&path).expect("load");
        assert_eq!(
            loaded.scene.item(note).expect("note").text(),
            Some("a changed note")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_1000_point_crash_matrix_never_loads_a_corrupt_board() {
        let dir = temp_dir("crash");
        let path = dir.join("crash.rizref");
        let (mut scene, camera) = board(&vec![6_u8; 8192], 8);
        save(&path, &request(&scene, &camera, "Crash")).expect("save");
        let original = std::fs::read(&path).expect("read");
        let old_index = load_existing(&path, scene.id).expect("clean file");

        let note = scene
            .items()
            .iter()
            .find(|item| item.is_note())
            .expect("note")
            .id();
        scene.item_mut(note).expect("note").set_text("the new note");
        let plan = plan_append(&old_index, &request(&scene, &camera, "Crash")).expect("plan");
        let appended = append_bytes(&plan);
        assert!(!appended.is_empty());

        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for iteration in 0..1000_usize {
            let kill = match iteration {
                0 => 0,
                1 => appended.len(),
                _ => {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    (state as usize) % (appended.len() + 1)
                }
            };
            let mut bytes = original.clone();
            bytes.extend_from_slice(&appended[..kill]);
            let mut reader = crate::BoardReader::from_bytes(bytes)
                .unwrap_or_else(|error| panic!("kill at {kill}: cannot open: {error}"));
            let loaded = reader
                .load()
                .unwrap_or_else(|error| panic!("kill at {kill}: cannot load: {error}"));
            let text = loaded.scene.item(note).expect("note").text().unwrap_or("");
            assert!(
                text == "a note" || text == "the new note",
                "kill at {kill} produced a torn board: {text:?}"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_truncated_fresh_file_never_loads_a_corrupt_board() {
        let dir = temp_dir("freshcrash");
        let path = dir.join("freshcrash.rizref");
        let (scene, camera) = board(&vec![8_u8; 4096], 8);
        save(&path, &request(&scene, &camera, "Fresh crash")).expect("save");
        let original = std::fs::read(&path).expect("read");
        let expected = scene.items().len();

        let mut kill = 0_usize;
        while kill <= original.len() {
            let partial = original[..kill].to_vec();
            if let Ok(mut reader) = crate::BoardReader::from_bytes(partial)
                && let Ok(loaded) = reader.load()
            {
                assert_eq!(
                    loaded.scene.items().len(),
                    expected,
                    "kill at {kill} loaded a different board"
                );
            }
            kill += 4096;
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore = "writes a ~500 MB board; run manually for the header-only budget"]
    fn header_only_read_of_a_large_board_is_fast() {
        let dir = temp_dir("large-read");
        let path = dir.join("large-read.rizref");
        let bytes = vec![0_u8; 500 * 1024 * 1024];
        let mut scene = Scene::new(SceneId::random());
        let asset = Asset::from_decoded(
            Arc::from(bytes),
            rizref_codecs::ImageFormat::Png,
            &ImageBuffer::filled(4, 4, [0, 0, 0, 255]),
            ImageBuffer::filled(4, 4, [0, 0, 0, 255]),
        );
        let (id, _) = scene.insert_asset(asset);
        scene
            .insert_image(id, Vec2::ZERO, Vec2::splat(100.0))
            .expect("asset");
        let camera = Camera::default();
        save(&path, &request(&scene, &camera, "Large read")).expect("save");

        // Warm the file handle and the OS cache, then measure the best of
        // several reads: the budget is about the format's own work, not about
        // a virus scanner inspecting a freshly written 500 MB file.
        let mut summary = crate::read_summary(&path).expect("summary");
        let mut best = std::time::Duration::MAX;
        for _ in 0..20 {
            let start = std::time::Instant::now();
            summary = crate::read_summary(&path).expect("summary");
            best = best.min(start.elapsed());
        }
        println!("header-only read of a 500 MB board: best of 20 = {best:?}");
        assert!(summary.thumbnail.is_some());
        assert!(
            best < std::time::Duration::from_millis(5),
            "header-only read took {best:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore = "writes target/rizref-demo.rizref for manual CLI checks"]
    fn write_demo_fixture() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("target/rizref-demo.rizref");
        let scene = large_board();
        let camera = Camera::new(DVec2::new(-40.0, 12.5), 0.8, -0.15);
        save(&path, &request(&scene, &camera, "Demo board")).expect("save");
        println!("fixture: {}", path.display());
    }

    fn append_bytes(plan: &AppendPlan) -> Vec<u8> {
        let mut bytes = Vec::new();
        for chunk in &plan.chunks {
            write_chunk(&mut bytes, chunk).expect("encode chunk");
        }
        bytes
    }

    #[test]
    fn repeated_saves_compact_and_stay_bounded() {
        let dir = temp_dir("compact");
        let path = dir.join("compact.rizref");
        let (mut scene, camera) = board(&vec![3_u8; 4096], 8);
        save(&path, &request(&scene, &camera, "Compact")).expect("first");
        let note = scene
            .items()
            .iter()
            .find(|item| item.is_note())
            .expect("note")
            .id();
        let mut modes = Vec::new();
        for index in 0..100 {
            scene
                .item_mut(note)
                .expect("note")
                .set_text(&format!("note {index}"));
            let report = save(&path, &request(&scene, &camera, "Compact")).expect("save");
            modes.push(report.mode);
        }

        assert!(
            modes.contains(&SaveMode::Compacted),
            "the threshold must eventually compact"
        );
        let len = std::fs::metadata(&path).expect("metadata").len();
        assert!(len < 512 * 1024, "the file must stay bounded, got {len}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_as_always_writes_fresh() {
        let dir = temp_dir("saveas");
        let path = dir.join("saveas.rizref");
        let (scene, camera) = board(&vec![1_u8; 1000], 8);
        save(&path, &request(&scene, &camera, "One")).expect("first");
        let mut request = request(&scene, &camera, "One");
        request.save_as = true;
        let report = save(&path, &request).expect("save as");
        assert_eq!(report.mode, SaveMode::Fresh);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_atomic_save_leaves_no_temporary_behind() {
        let dir = temp_dir("temp");
        let path = dir.join("temp.rizref");
        let (scene, camera) = board(&[2_u8; 100], 8);
        save(&path, &request(&scene, &camera, "Temp")).expect("save");

        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        assert!(!dir.join(format!("{name}.tmp")).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_recovered_file_is_compacted_on_the_next_save() {
        let dir = temp_dir("recovered");
        let path = dir.join("recovered.rizref");
        let (scene, camera) = board(&vec![5_u8; 2048], 8);
        save(&path, &request(&scene, &camera, "Recovered")).expect("save");
        let bytes = std::fs::read(&path).expect("read");
        std::fs::write(&path, &bytes[..bytes.len() - 10]).expect("truncate");
        let report = save(&path, &request(&scene, &camera, "Recovered")).expect("save");
        assert_eq!(report.mode, SaveMode::Compacted);
        crate::load(&path).expect("load");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn provenance_survives_save_append_and_load_with_and_without_fields() {
        use crate::{AssetOrigin, AssetProvenance};
        use std::collections::BTreeMap;

        let dir = temp_dir("provenance");
        let path = dir.join("provenance.rizref");
        let mut scene = Scene::new(SceneId::random());
        let mut ids = Vec::new();
        for seed in 1..=4u8 {
            let (bytes, format) = encoded(seed, false);
            let image = ImageBuffer::filled(4, 4, [seed, 0, 0, 255]);
            let asset = Asset::from_decoded(bytes, format, &image, image.clone());
            let (id, _) = scene.insert_asset(asset);
            scene
                .insert_image(
                    id,
                    Vec2::new(f32::from(seed) * 50.0, 0.0),
                    Vec2::splat(40.0),
                )
                .expect("asset");
            ids.push(id);
        }
        let local = AssetProvenance {
            origin: AssetOrigin::File,
            source_path: Some(r"D:\My Refs\a, b & c.png".to_owned()),
            source_url: None,
        };
        let web = AssetProvenance {
            origin: AssetOrigin::Clipboard,
            source_path: None,
            source_url: Some("https://example.com/img/cat.png?w=800&h=600".to_owned()),
        };
        // The format stores hints verbatim; launching validates them later.
        let hostile = AssetProvenance {
            origin: AssetOrigin::Clipboard,
            source_path: None,
            source_url: Some("javascript:alert(1)".to_owned()),
        };
        let provenance: BTreeMap<_, _> = [
            (ids[0], local.clone()),
            (ids[1], web.clone()),
            (ids[2], hostile.clone()),
        ]
        .into_iter()
        .collect();
        let camera = Camera::default();
        let mut save_request = request(&scene, &camera, "Provenance");
        save_request.provenance = &provenance;
        save(&path, &save_request).expect("fresh save");

        let entry_for = |loaded: &crate::LoadedBoard, id: rizref_core::AssetId| {
            loaded
                .document
                .assets
                .iter()
                .find(|entry| entry.hash == id.to_hex())
                .map(|entry| AssetProvenance {
                    origin: entry.origin,
                    source_path: entry.source_path.clone(),
                    source_url: entry.source_url.clone(),
                })
                .expect("asset entry")
        };
        let check = |loaded: &crate::LoadedBoard| {
            assert_eq!(loaded.document.schema_version, crate::SCHEMA_VERSION);
            assert_eq!(entry_for(loaded, ids[0]), local);
            assert_eq!(entry_for(loaded, ids[1]), web);
            assert_eq!(entry_for(loaded, ids[2]), hostile);
            assert_eq!(
                entry_for(loaded, ids[3]),
                AssetProvenance::default(),
                "no record: unknown origin, no fields"
            );
        };
        let loaded = crate::load(&path).expect("load");
        check(&loaded);
        let json = serde_json::to_string(&loaded.document).expect("json");
        assert_eq!(
            json.matches("\"source_url\"").count(),
            2,
            "absent URLs are not written, so older readers see the v1 shape"
        );

        // An appended save with no provenance map keeps the stored hints.
        scene.insert_note("later", Vec2::new(0.0, 200.0), Vec2::splat(60.0));
        let report = save(&path, &request(&scene, &camera, "Provenance")).expect("append");
        assert_eq!(report.mode, SaveMode::Appended);
        check(&crate::load(&path).expect("reload"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn note_payload_fields_survive_save_append_and_load() {
        let dir = temp_dir("note-fields");
        let path = dir.join("notes.rizref");
        let (mut scene, camera) = board(&[1_u8; 256], 2);
        let note = scene
            .items()
            .iter()
            .find(|item| item.is_note())
            .expect("note")
            .id();
        let styled = rizref_core::NoteContent {
            text: "# Study\n\nThis is <span style=\"color: red;\">red</span>".into(),
            format: rizref_core::NoteFormat::Markdown,
            background: Some([0xf5, 0xf0, 0xe1]),
            wrap_width: Some(240.0),
            spans: Vec::new(),
        };
        scene.item_mut(note).expect("note").set_note(&styled);
        save(&path, &request(&scene, &camera, "Notes")).expect("fresh save");
        let loaded = crate::load(&path).expect("load");
        assert!(loaded.warnings.is_empty());
        assert_eq!(
            loaded
                .scene
                .item(note)
                .and_then(rizref_core::Item::note_content),
            Some(styled.clone())
        );

        // An appended save that changes only the background keeps the rest.
        let mut recoloured = styled;
        recoloured.background = Some([0x10, 0x20, 0x30]);
        scene.item_mut(note).expect("note").set_note(&recoloured);
        let report = save(&path, &request(&scene, &camera, "Notes")).expect("append");
        assert_eq!(report.mode, SaveMode::Appended);
        let loaded = crate::load(&path).expect("reload");
        assert_eq!(
            loaded
                .scene
                .item(note)
                .and_then(rizref_core::Item::note_content),
            Some(recoloured)
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
