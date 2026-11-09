//! Reading a `.rizref` file.
//!
//! Opening a board is a small trust ladder:
//!
//! 1. The header must be intact (magic, fixed fields, supported revision).
//! 2. If the file ends with a footer, the index it points at is read and its
//!    digest checked. A clean file is resolved in a handful of seeks, no matter
//!    how large the board is — which is what makes a 500 MB board's name and
//!    thumbnail available in milliseconds.
//! 3. If there is no footer (an interrupted append), the chunk stream is
//!    scanned and the **last valid index** wins. A partially written index is
//!    rejected by its own digest, so the previous clean generation loads.
//! 4. If no index survived, the scan reconstructs one from the last complete
//!    document, name, thumbnail, blobs and previews. This is the belt-and-
//!    braces case for a file damaged by something other than an interrupted
//!    append.
//!
//! Blobs are read on demand: [`BoardReader::blob`] seeks to one chunk, checks
//! its content hash and returns the original bytes. Nothing in this module
//! assumes the whole file fits in memory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use rizref_codecs::ImageBuffer;
use rizref_core::{AssetId, Camera, Scene};

use crate::checksum;
use crate::chunk::{CHUNK_HEADER_LEN, ChunkHeader, ChunkKind};
use crate::compression;
use crate::document::{LoadedScene, SceneDocument};
use crate::error::FormatError;
use crate::header::{HEADER_LEN, Header};
use crate::index::{BoardIndex, ChunkRef, FOOTER_LEN, Footer};
use crate::preview::Preview;
use crate::thumbnail::decode_preview;

/// One chunk as seen by a scan of the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkInfo {
    /// Absolute offset of the chunk frame.
    pub offset: u64,
    /// What the chunk contains, or [`ChunkKind::Unknown`].
    pub kind: ChunkKind,
    /// Compression code.
    pub compression: u8,
    /// Payload bytes on disk.
    pub stored_len: u64,
    /// Payload bytes after decompression.
    pub decompressed_len: u64,
}

impl ChunkInfo {
    /// Total bytes this chunk occupies.
    pub const fn frame_len(&self) -> u64 {
        CHUNK_HEADER_LEN as u64 + self.stored_len
    }
}

/// The result of walking the chunk stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkScan {
    /// Every complete chunk, in file order.
    pub chunks: Vec<ChunkInfo>,
    /// Offset where scanning stopped; equals the file length for an intact
    /// stream, or the start of the first incomplete frame.
    pub end: u64,
    /// Whether the stream ended in an incomplete frame.
    pub truncated: bool,
}

/// A board as loaded from disk.
#[derive(Debug)]
pub struct LoadedBoard {
    /// The validated header.
    pub header: Header,
    /// The index the load used.
    pub index: BoardIndex,
    /// The parsed scene document.
    pub document: SceneDocument,
    /// The reconstructed scene, with every embedded asset.
    pub scene: Scene,
    /// The saved camera.
    pub camera: Camera,
    /// Display name: the `NAME` chunk, or the document's board name.
    pub name: String,
    /// The embedded thumbnail, when the board has one.
    pub thumbnail: Option<Preview>,
    /// Linked assets whose bytes were absent; they load as placeholders.
    pub missing: Vec<AssetId>,
    /// Recoverable document problems the load ignored, one line each.
    pub warnings: Vec<String>,
    /// Whether the board was recovered from an interrupted save rather than a
    /// clean footer.
    pub recovered: bool,
}

/// What a cheap header-and-thumbnail read returns.
#[derive(Debug, Clone)]
pub struct BoardSummary {
    /// The validated header.
    pub header: Header,
    /// The current index.
    pub index: BoardIndex,
    /// The display name, when stored in a `NAME` chunk.
    pub name: Option<String>,
    /// The embedded thumbnail, when present.
    pub thumbnail: Option<Preview>,
    /// Whether recovery was needed.
    pub recovered: bool,
}

/// The result of checking every digest in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// Human-readable problems; empty means every check passed.
    pub problems: Vec<String>,
    /// Chunks walked.
    pub chunks: usize,
    /// Assets listed by the index.
    pub assets: usize,
    /// Blobs whose content hash was recomputed.
    pub blobs_verified: usize,
    /// Whether recovery was needed to open the file.
    pub recovered: bool,
}

impl VerifyReport {
    /// Whether the file passed every check.
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// A reader over a seekable source.
pub struct BoardReader<S> {
    source: S,
    file_len: u64,
    header: Header,
    index: BoardIndex,
    recovered: bool,
}

impl<S> std::fmt::Debug for BoardReader<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoardReader")
            .field("file_len", &self.file_len)
            .field("header", &self.header)
            .field("generation", &self.index.generation)
            .field("recovered", &self.recovered)
            .finish_non_exhaustive()
    }
}

impl BoardReader<File> {
    /// Opens a board file.
    pub fn open_path(path: impl AsRef<Path>) -> Result<Self, FormatError> {
        let path = path.as_ref();
        let file = File::open(path)
            .map_err(|error| FormatError::io("open", path.display().to_string(), error))?;
        Self::open(file, path.display().to_string())
    }
}

impl BoardReader<Cursor<Vec<u8>>> {
    /// Reads a board already held in memory, for tests and the CLI.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, FormatError> {
        Self::open(Cursor::new(bytes), String::new())
    }
}

impl<S: Read + Seek> BoardReader<S> {
    /// Validates `source` and resolves its current index.
    pub fn open(mut source: S, label: impl Into<String>) -> Result<Self, FormatError> {
        let label = label.into();
        let file_len = source
            .seek(SeekFrom::End(0))
            .map_err(|error| FormatError::io("read", label.clone(), error))?;
        if file_len < HEADER_LEN as u64 {
            return Err(FormatError::Truncated {
                needed: HEADER_LEN,
                available: usize::try_from(file_len).unwrap_or(usize::MAX),
            });
        }
        let header_bytes = read_at(&mut source, file_len, 0, HEADER_LEN)
            .map_err(|error| with_label(error, &label))?;
        let header = Header::decode(&header_bytes)?;
        if u64::from(header.header_size) > file_len {
            return Err(FormatError::Truncated {
                needed: header.header_size as usize,
                available: usize::try_from(file_len).unwrap_or(usize::MAX),
            });
        }

        let mut reader = Self {
            source,
            file_len,
            header,
            index: empty_index(&header),
            recovered: false,
        };

        match reader.try_clean_index() {
            Ok(Some(index)) => {
                reader.index = index;
            }
            Ok(None) | Err(_) => {
                let index = reader.recover_index()?;
                reader.index = index;
                reader.recovered = true;
            }
        }
        Ok(reader)
    }

    /// The validated header.
    pub const fn header(&self) -> Header {
        self.header
    }

    /// The current index.
    pub const fn index(&self) -> &BoardIndex {
        &self.index
    }

    /// Whether the file was recovered from an interrupted save.
    pub const fn recovered(&self) -> bool {
        self.recovered
    }

    /// Length of the source in bytes.
    pub const fn file_len(&self) -> u64 {
        self.file_len
    }

    /// The parsed scene document.
    pub fn document(&mut self) -> Result<SceneDocument, FormatError> {
        self.document_with_kind(ChunkKind::Document)
    }

    /// The display name, when the file carries a `NAME` chunk.
    pub fn name(&mut self) -> Result<Option<String>, FormatError> {
        let Some(chunk) = self.index.name.clone() else {
            return Ok(None);
        };
        let bytes = self.read_chunk(&chunk, ChunkKind::Name)?;
        let name = String::from_utf8(bytes).map_err(|error| FormatError::BadChunk {
            reason: format!("board name is not UTF-8: {error}"),
        })?;
        Ok(Some(name))
    }

    /// The embedded thumbnail.
    ///
    /// The chunk payload is an 8-byte pixel size followed by encoded image
    /// bytes, so a caller can lay out a card without decoding the picture.
    pub fn thumbnail(&mut self) -> Result<Option<Preview>, FormatError> {
        let Some(chunk) = self.index.thumbnail.clone() else {
            return Ok(None);
        };
        let bytes = self.read_chunk(&chunk, ChunkKind::Thumbnail)?;
        if bytes.len() < 8 {
            return Err(FormatError::BadChunk {
                reason: "thumbnail is shorter than its 8-byte size header".to_owned(),
            });
        }
        let width = u32::from_le_bytes(bytes[0..4].try_into().expect("4 bytes"));
        let height = u32::from_le_bytes(bytes[4..8].try_into().expect("4 bytes"));
        if width == 0 || height == 0 {
            return Err(FormatError::BadChunk {
                reason: "thumbnail has a zero dimension".to_owned(),
            });
        }
        Ok(Some(Preview::new(width, height, bytes[8..].to_vec())))
    }

    /// The original bytes of asset `id`, checked against its content hash.
    pub fn blob(&mut self, id: AssetId) -> Result<Vec<u8>, FormatError> {
        let hash = id.to_hex();
        let chunk = self
            .index
            .asset(&hash)
            .and_then(|asset| asset.blob.clone())
            .ok_or_else(|| FormatError::Document {
                reason: format!("the board index has no blob for asset {hash}"),
            })?;
        let payload = self.read_chunk(&chunk, ChunkKind::Blob)?;
        let original = strip_hash_prefix(&payload, &hash)?;
        if !checksum::verify(original, id.as_bytes()) {
            return Err(FormatError::IntegrityMismatch {
                subject: "image blob",
            });
        }
        Ok(original.to_vec())
    }

    /// The encoded preview of asset `id`.
    pub fn asset_preview(&mut self, id: AssetId) -> Result<Option<Vec<u8>>, FormatError> {
        let hash = id.to_hex();
        let Some(chunk) = self
            .index
            .asset(&hash)
            .and_then(|asset| asset.preview.clone())
        else {
            return Ok(None);
        };
        let payload = self.read_chunk(&chunk, ChunkKind::Preview)?;
        Ok(Some(strip_hash_prefix(&payload, &hash)?.to_vec()))
    }

    /// Loads the document, every embedded blob, every preview and the scene.
    pub fn load(&mut self) -> Result<LoadedBoard, FormatError> {
        let document = self.document()?;
        let mut blobs: BTreeMap<AssetId, Arc<[u8]>> = BTreeMap::new();
        for entry in &document.assets {
            let Some(id) = AssetId::from_hex(&entry.hash) else {
                return Err(FormatError::Document {
                    reason: format!("asset hash {:?} is not 64 hex digits", entry.hash),
                });
            };
            if !entry.embedded || blobs.contains_key(&id) {
                continue;
            }
            let bytes = self.blob(id)?;
            blobs.insert(id, Arc::from(bytes));
        }

        let mut previews: BTreeMap<AssetId, ImageBuffer> = BTreeMap::new();
        for entry in &document.assets {
            if entry.preview.is_none() {
                continue;
            }
            let Some(id) = AssetId::from_hex(&entry.hash) else {
                continue;
            };
            if let Ok(Some(bytes)) = self.asset_preview(id)
                && let Ok(image) = decode_preview(&bytes)
            {
                previews.insert(id, image);
            }
        }

        let LoadedScene {
            scene,
            missing,
            warnings,
        } = {
            let standard = self.header.scene_id.to_hex();
            if document.scene_id != standard {
                return Err(FormatError::Document {
                    reason: format!(
                        "document scene id {:?} does not match header id {standard}",
                        document.scene_id
                    ),
                });
            }
            document.build_scene(&blobs, &previews)?
        };
        let name = self
            .name()
            .ok()
            .flatten()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| document.board.name.clone());
        let thumbnail = self.thumbnail().ok().flatten();
        let camera = document.camera();
        Ok(LoadedBoard {
            header: self.header,
            index: self.index.clone(),
            document,
            scene,
            camera,
            name,
            thumbnail,
            missing,
            warnings,
            recovered: self.recovered,
        })
    }

    /// Walks every chunk frame in the file.
    pub fn scan_chunks(&mut self) -> Result<ChunkScan, FormatError> {
        scan(&mut self.source, self.file_len, self.header.header_size)
    }

    /// Checks every chunk and every referenced digest.
    pub fn verify(&mut self) -> Result<VerifyReport, FormatError> {
        let mut problems = Vec::new();
        let scan = self.scan_chunks()?;
        if scan.truncated {
            problems.push(format!("chunk stream is truncated at offset {}", scan.end));
        }

        let mut doc_ok = false;
        let mut blobs_verified = 0_usize;
        for chunk in &scan.chunks {
            match chunk.kind {
                ChunkKind::Unknown(_) => {}
                _ => match self.read_chunk_info(chunk) {
                    Ok(payload) => match chunk.kind {
                        ChunkKind::Name => {
                            if std::str::from_utf8(&payload).is_err() {
                                problems
                                    .push(format!("name chunk at {} is not UTF-8", chunk.offset));
                            }
                        }
                        ChunkKind::Document => match SceneDocument::from_json(&payload) {
                            Ok(document) => {
                                doc_ok = true;
                                if document.scene_id != self.header.scene_id.to_hex() {
                                    problems.push(
                                        "document scene id does not match the header".to_owned(),
                                    );
                                }
                            }
                            Err(error) => {
                                problems.push(format!("document at {}: {error}", chunk.offset));
                            }
                        },
                        ChunkKind::Blob => {
                            if payload.len() < 32 {
                                problems.push(format!(
                                    "blob at {} is shorter than its 32-byte hash",
                                    chunk.offset
                                ));
                            } else if checksum::digest(&payload[32..]) != payload[..32] {
                                problems.push(format!(
                                    "blob at {} does not match its content hash",
                                    chunk.offset
                                ));
                            } else {
                                blobs_verified += 1;
                            }
                        }
                        ChunkKind::Preview => {
                            if payload.len() < 32 {
                                problems.push(format!(
                                    "preview at {} is shorter than its 32-byte hash",
                                    chunk.offset
                                ));
                            } else if decode_preview(&payload[32..]).is_err() {
                                problems
                                    .push(format!("preview at {} does not decode", chunk.offset));
                            }
                        }
                        ChunkKind::Thumbnail => {
                            if payload.len() < 8 {
                                problems.push(format!(
                                    "thumbnail at {} is shorter than its 8-byte header",
                                    chunk.offset
                                ));
                            }
                        }
                        ChunkKind::Footer => {
                            if payload.len() != FOOTER_LEN || Footer::decode(&payload).is_err() {
                                problems.push(format!(
                                    "footer at {} is not a valid footer",
                                    chunk.offset
                                ));
                            }
                        }
                        ChunkKind::Index => match BoardIndex::decode_payload(&payload) {
                            Ok(index) => {
                                if index.generation != self.index.generation && self.recovered {
                                    // Older indexes are expected during
                                    // recovery; only the live one must be valid.
                                }
                            }
                            Err(error) => {
                                problems.push(format!("index at {}: {error}", chunk.offset));
                            }
                        },
                        _ => {}
                    },
                    Err(error) => problems.push(format!(
                        "chunk {} at {} cannot be read: {error}",
                        String::from_utf8_lossy(&chunk.kind.tag()),
                        chunk.offset
                    )),
                },
            }
        }

        if !doc_ok {
            problems.push("no readable scene document".to_owned());
        }

        let mut assets = 0_usize;
        let mut seen = BTreeSet::new();
        let indexed_assets = self.index.assets.clone();
        for asset in &indexed_assets {
            assets += 1;
            if !seen.insert(asset.hash.clone()) {
                problems.push(format!("asset {} is listed twice", asset.hash));
            }
            if AssetId::from_hex(&asset.hash).is_none() {
                problems.push(format!("asset hash {:?} is not 64 hex digits", asset.hash));
            }
            match asset
                .blob
                .as_ref()
                .map(|chunk| self.read_chunk(chunk, ChunkKind::Blob))
            {
                Some(Ok(payload)) => match strip_hash_prefix(&payload, &asset.hash) {
                    Ok(original) => {
                        if let Some(id) = AssetId::from_hex(&asset.hash)
                            && !checksum::verify(original, id.as_bytes())
                        {
                            problems.push(format!(
                                "asset {} content does not match its hash",
                                asset.hash
                            ));
                        }
                    }
                    Err(error) => problems.push(format!("asset {}: {error}", asset.hash)),
                },
                Some(Err(error)) => {
                    problems.push(format!("asset {} blob: {error}", asset.hash));
                }
                None => {}
            }
            if let Some(chunk) = &asset.preview {
                match self.read_chunk(chunk, ChunkKind::Preview) {
                    Ok(payload) => match strip_hash_prefix(&payload, &asset.hash) {
                        Ok(bytes) => {
                            if decode_preview(bytes).is_err() {
                                problems
                                    .push(format!("asset {} preview does not decode", asset.hash));
                            }
                        }
                        Err(error) => {
                            problems.push(format!("asset {} preview: {error}", asset.hash));
                        }
                    },
                    Err(error) => {
                        problems.push(format!("asset {} preview: {error}", asset.hash));
                    }
                }
            }
        }

        Ok(VerifyReport {
            problems,
            chunks: scan.chunks.len(),
            assets,
            blobs_verified,
            recovered: self.recovered,
        })
    }

    /// Finds the index named by a footer at the end of the file, if there is
    /// one and it validates.
    fn try_clean_index(&mut self) -> Result<Option<BoardIndex>, FormatError> {
        let footer = self.read_footer()?;
        let Some(footer) = footer else {
            return Ok(None);
        };
        compression::check_len(footer.index_len)?;
        if footer.index_offset + CHUNK_HEADER_LEN as u64 + footer.index_len
            > self.file_len - FOOTER_LEN as u64
        {
            return Err(FormatError::Index {
                reason: "footer points past the end of the file".to_owned(),
            });
        }
        let stored = read_at(
            &mut self.source,
            self.file_len,
            footer.index_offset + CHUNK_HEADER_LEN as u64,
            usize::try_from(footer.index_len).map_err(|_| FormatError::Index {
                reason: "index is larger than this build can address".to_owned(),
            })?,
        )?;
        if checksum::digest(&stored) != footer.index_digest {
            return Err(FormatError::IntegrityMismatch {
                subject: "board footer",
            });
        }
        let index = self.read_index_chunk_at(footer.index_offset)?;
        if index.generation != footer.generation {
            return Err(FormatError::Index {
                reason: format!(
                    "footer names generation {} but the index is {}",
                    footer.generation, index.generation
                ),
            });
        }
        validate_refs(&index, self.file_len, Some(footer.index_offset))?;
        Ok(Some(index))
    }

    /// Reads the fixed footer, when the file ends with a valid one.
    fn read_footer(&mut self) -> Result<Option<Footer>, FormatError> {
        if !self.header.has_footer() || self.file_len < FOOTER_LEN as u64 {
            return Ok(None);
        }
        let bytes = read_at(
            &mut self.source,
            self.file_len,
            self.file_len - FOOTER_LEN as u64,
            FOOTER_LEN,
        )?;
        match Footer::decode(&bytes) {
            Ok(footer) => Ok(Some(footer)),
            Err(_) => Ok(None),
        }
    }

    /// Finds the last valid index by scanning, or reconstructs one.
    fn recover_index(&mut self) -> Result<BoardIndex, FormatError> {
        let scan = scan(&mut self.source, self.file_len, self.header.header_size)?;

        let mut best_index: Option<BoardIndex> = None;
        let mut best_doc: Option<(ChunkRef, Vec<u8>)> = None;
        let mut best_name: Option<ChunkRef> = None;
        let mut best_thumb: Option<ChunkRef> = None;
        let mut blobs: BTreeMap<String, ChunkRef> = BTreeMap::new();
        let mut previews: BTreeMap<String, ChunkRef> = BTreeMap::new();

        for chunk in &scan.chunks {
            let header = match chunk_header(&mut self.source, chunk.offset) {
                Ok(header) => header,
                Err(_) => continue,
            };
            match chunk.kind {
                ChunkKind::Index => {
                    if let Ok(payload) = self.read_chunk_info(chunk)
                        && let Ok(index) = BoardIndex::decode_payload(&payload)
                        && validate_refs(&index, self.file_len, Some(chunk.offset)).is_ok()
                    {
                        best_index = Some(index);
                    }
                }
                ChunkKind::Document => {
                    if let Ok(payload) = self.read_chunk_info(chunk)
                        && SceneDocument::from_json(&payload).is_ok()
                    {
                        let reference =
                            ChunkRef::new(chunk.offset, &header, checksum::digest(&payload));
                        best_doc = Some((reference, payload));
                    }
                }
                ChunkKind::Name => {
                    best_name = Some(ChunkRef::new_unverified(chunk.offset, &header));
                }
                ChunkKind::Thumbnail => {
                    best_thumb = Some(ChunkRef::new_unverified(chunk.offset, &header));
                }
                ChunkKind::Blob | ChunkKind::Preview => {
                    if let Ok(prefix) = read_at(
                        &mut self.source,
                        self.file_len,
                        chunk.offset + CHUNK_HEADER_LEN as u64,
                        32,
                    ) {
                        let hash = checksum::to_hex(&prefix.try_into().expect("32 bytes"));
                        let reference = ChunkRef::new_unverified(chunk.offset, &header);
                        if chunk.kind == ChunkKind::Blob {
                            blobs.insert(hash, reference);
                        } else {
                            previews.insert(hash, reference);
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some(index) = best_index {
            return Ok(index);
        }

        let (document_ref, document_bytes) = best_doc.ok_or(FormatError::MissingDocument)?;
        let document = SceneDocument::from_json(&document_bytes)?;

        // A name/thumbnail chunk recovered by scanning has no recorded digest.
        // Record the digest of what is actually there so `verify` can still
        // check it; the payload was not read during the scan, so compute it now.
        let name = match best_name {
            Some(reference) => Some(self.with_computed_digest(reference)?),
            None => None,
        };
        let thumbnail = match best_thumb {
            Some(reference) => Some(self.with_computed_digest(reference)?),
            None => None,
        };
        let assets = document
            .assets
            .iter()
            .map(|entry| crate::index::IndexAsset {
                hash: entry.hash.clone(),
                blob: blobs.get(&entry.hash).cloned(),
                preview: previews.get(&entry.hash).cloned(),
                extra: BTreeMap::new(),
            })
            .collect();
        Ok(BoardIndex {
            schema_version: crate::index::INDEX_SCHEMA_VERSION,
            generation: 0,
            created_ms: self.header.created_ms,
            modified_ms: self.header.modified_ms,
            appended_bytes: 0,
            writer: env!("CARGO_PKG_VERSION").to_owned(),
            name,
            document: document_ref,
            thumbnail,
            assets,
            extra: BTreeMap::new(),
        })
    }

    /// Replaces an empty digest in `reference` with the payload's real digest.
    fn with_computed_digest(&mut self, mut reference: ChunkRef) -> Result<ChunkRef, FormatError> {
        let payload = self.read_chunk_relaxed(&reference)?;
        reference.digest = checksum::to_hex(&checksum::digest(&payload));
        Ok(reference)
    }

    /// Reads and parses the index chunk at `offset`.
    fn read_index_chunk_at(&mut self, offset: u64) -> Result<BoardIndex, FormatError> {
        let header_bytes = read_at(&mut self.source, self.file_len, offset, CHUNK_HEADER_LEN)?;
        let header = ChunkHeader::decode(&header_bytes)?;
        if header.kind != ChunkKind::Index {
            return Err(FormatError::Index {
                reason: format!(
                    "footer points at {:?}, not an index",
                    String::from_utf8_lossy(&header.kind.tag())
                ),
            });
        }
        compression::check_len(header.decompressed_len)?;
        compression::check_len(header.stored_len)?;
        let stored = read_at(
            &mut self.source,
            self.file_len,
            offset + CHUNK_HEADER_LEN as u64,
            usize::try_from(header.stored_len).map_err(|_| FormatError::Index {
                reason: "index is larger than this build can address".to_owned(),
            })?,
        )?;
        let decoded = compression::decode(header.codec()?, &stored, header.decompressed_len)?;
        BoardIndex::decode_payload(&decoded)
    }

    /// Reads one referenced chunk and checks its frame, length and digest.
    fn read_chunk(
        &mut self,
        reference: &ChunkRef,
        expected: ChunkKind,
    ) -> Result<Vec<u8>, FormatError> {
        let payload = self.read_chunk_relaxed(reference)?;
        let header = chunk_header(&mut self.source, reference.offset)?;
        if header.kind != expected {
            return Err(FormatError::BadChunk {
                reason: format!(
                    "expected {:?} at {} but found {:?}",
                    String::from_utf8_lossy(&expected.tag()),
                    reference.offset,
                    String::from_utf8_lossy(&header.kind.tag())
                ),
            });
        }
        Ok(payload)
    }

    /// Reads one referenced chunk, checking its frame and digest but not its
    /// tag.
    fn read_chunk_relaxed(&mut self, reference: &ChunkRef) -> Result<Vec<u8>, FormatError> {
        if reference
            .offset
            .checked_add(reference.frame_len())
            .is_none_or(|end| end > self.file_len)
        {
            return Err(FormatError::Index {
                reason: format!(
                    "chunk at {} with {} stored bytes runs past the end of the file",
                    reference.offset, reference.stored_len
                ),
            });
        }
        let header = chunk_header(&mut self.source, reference.offset)?;
        if header.stored_len != reference.stored_len
            || header.decompressed_len != reference.decompressed_len
            || header.compression != reference.compression
        {
            return Err(FormatError::BadChunk {
                reason: format!(
                    "chunk at {} does not match the index entry",
                    reference.offset
                ),
            });
        }
        compression::check_len(header.decompressed_len)?;
        compression::check_len(header.stored_len)?;
        let stored = read_at(
            &mut self.source,
            self.file_len,
            reference.offset + CHUNK_HEADER_LEN as u64,
            usize::try_from(header.stored_len).map_err(|_| FormatError::BadChunk {
                reason: "chunk is larger than this build can address".to_owned(),
            })?,
        )?;
        let decoded = compression::decode(header.codec()?, &stored, header.decompressed_len)?;
        if !reference.digest.is_empty() {
            let expected = reference.digest_bytes()?;
            if !checksum::verify(&decoded, &expected) {
                return Err(FormatError::IntegrityMismatch { subject: "chunk" });
            }
        }
        Ok(decoded)
    }

    /// Reads a chunk found by a scan, without an index reference.
    fn read_chunk_info(&mut self, info: &ChunkInfo) -> Result<Vec<u8>, FormatError> {
        let header = chunk_header(&mut self.source, info.offset)?;
        compression::check_len(header.decompressed_len)?;
        compression::check_len(header.stored_len)?;
        let stored = read_at(
            &mut self.source,
            self.file_len,
            info.offset + CHUNK_HEADER_LEN as u64,
            usize::try_from(header.stored_len).map_err(|_| FormatError::BadChunk {
                reason: "chunk is larger than this build can address".to_owned(),
            })?,
        )?;
        compression::decode(header.codec()?, &stored, header.decompressed_len)
    }

    fn document_with_kind(&mut self, kind: ChunkKind) -> Result<SceneDocument, FormatError> {
        let reference = self.index.document.clone();
        let payload = self.read_chunk(&reference, kind)?;
        SceneDocument::from_json(&payload)
    }
}

/// Reads name and thumbnail without parsing the document.
pub fn read_summary(path: impl AsRef<Path>) -> Result<BoardSummary, FormatError> {
    let mut reader = BoardReader::open_path(path)?;
    let name = reader.name()?;
    let thumbnail = reader.thumbnail()?;
    Ok(BoardSummary {
        header: reader.header,
        index: reader.index,
        name,
        thumbnail,
        recovered: reader.recovered,
    })
}

/// Opens a board file and loads its scene.
pub fn load(path: impl AsRef<Path>) -> Result<LoadedBoard, FormatError> {
    BoardReader::open_path(path)?.load()
}

/// Reads only the fixed header of a board file.
pub fn read_header(path: impl AsRef<Path>) -> Result<Header, FormatError> {
    let path = path.as_ref();
    let mut file = File::open(path)
        .map_err(|error| FormatError::io("open", path.display().to_string(), error))?;
    let mut bytes = [0_u8; HEADER_LEN];
    file.read_exact(&mut bytes)
        .map_err(|error| FormatError::io("read", path.display().to_string(), error))?;
    Header::decode(&bytes)
}

/// The display name of a board, without opening its document.
pub fn read_name(path: impl AsRef<Path>) -> Result<Option<String>, FormatError> {
    BoardReader::open_path(path)?.name()
}

fn empty_index(header: &Header) -> BoardIndex {
    BoardIndex {
        schema_version: crate::index::INDEX_SCHEMA_VERSION,
        generation: 0,
        created_ms: header.created_ms,
        modified_ms: header.modified_ms,
        appended_bytes: 0,
        writer: String::new(),
        name: None,
        document: ChunkRef {
            offset: u64::from(header.header_size),
            tag: "DOC ".to_owned(),
            stored_len: 0,
            decompressed_len: 0,
            compression: 0,
            digest: String::new(),
            extra: BTreeMap::new(),
        },
        thumbnail: None,
        assets: Vec::new(),
        extra: BTreeMap::new(),
    }
}

fn validate_refs(
    index: &BoardIndex,
    file_len: u64,
    before: Option<u64>,
) -> Result<(), FormatError> {
    let bound = before.unwrap_or(file_len);
    let check = |reference: &ChunkRef, what: &str| -> Result<(), FormatError> {
        let end = reference
            .offset
            .checked_add(reference.frame_len())
            .ok_or_else(|| FormatError::Index {
                reason: format!("{what} offset overflows"),
            })?;
        if end > bound || end > file_len {
            return Err(FormatError::Index {
                reason: format!(
                    "{what} at {} with {} stored bytes runs past its bound {bound}",
                    reference.offset, reference.stored_len
                ),
            });
        }
        Ok(())
    };
    check(&index.document, "document")?;
    if let Some(name) = &index.name {
        check(name, "name")?;
    }
    if let Some(thumbnail) = &index.thumbnail {
        check(thumbnail, "thumbnail")?;
    }
    for asset in &index.assets {
        if let Some(blob) = &asset.blob {
            check(blob, "asset blob")?;
        }
        if let Some(preview) = &asset.preview {
            check(preview, "asset preview")?;
        }
    }
    Ok(())
}

fn scan<S: Read + Seek>(
    source: &mut S,
    file_len: u64,
    start: u32,
) -> Result<ChunkScan, FormatError> {
    let mut chunks = Vec::new();
    let mut offset = u64::from(start);
    while offset < file_len {
        if file_len - offset < CHUNK_HEADER_LEN as u64 {
            return Ok(ChunkScan {
                chunks,
                end: offset,
                truncated: true,
            });
        }
        let header_bytes = read_at(source, file_len, offset, CHUNK_HEADER_LEN)?;
        let header = match ChunkHeader::decode(&header_bytes) {
            Ok(header) => header,
            Err(_) => {
                return Ok(ChunkScan {
                    chunks,
                    end: offset,
                    truncated: true,
                });
            }
        };
        if header.stored_len > file_len - offset - CHUNK_HEADER_LEN as u64 {
            return Ok(ChunkScan {
                chunks,
                end: offset,
                truncated: true,
            });
        }
        chunks.push(ChunkInfo {
            offset,
            kind: header.kind,
            compression: header.compression,
            stored_len: header.stored_len,
            decompressed_len: header.decompressed_len,
        });
        offset += header.frame_len();
    }
    Ok(ChunkScan {
        chunks,
        end: offset,
        truncated: false,
    })
}

fn chunk_header<S: Read + Seek>(source: &mut S, offset: u64) -> Result<ChunkHeader, FormatError> {
    let bytes = read_at(source, u64::MAX, offset, CHUNK_HEADER_LEN)?;
    ChunkHeader::decode(&bytes)
}

fn read_at<S: Read + Seek>(
    source: &mut S,
    file_len: u64,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>, FormatError> {
    let end = offset
        .checked_add(len as u64)
        .ok_or(FormatError::Truncated {
            needed: 0,
            available: 0,
        })?;
    if file_len != u64::MAX && end > file_len {
        return Err(FormatError::Truncated {
            needed: usize::try_from(end).unwrap_or(usize::MAX),
            available: usize::try_from(file_len).unwrap_or(usize::MAX),
        });
    }
    source
        .seek(SeekFrom::Start(offset))
        .map_err(|error| FormatError::io("read", "", error))?;
    let mut buffer = vec![0_u8; len];
    source
        .read_exact(&mut buffer)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::UnexpectedEof => FormatError::Truncated {
                needed: len,
                available: 0,
            },
            _ => FormatError::io("read", "", error),
        })?;
    Ok(buffer)
}

fn strip_hash_prefix<'a>(payload: &'a [u8], hash: &str) -> Result<&'a [u8], FormatError> {
    if payload.len() < 32 {
        return Err(FormatError::BadChunk {
            reason: "payload is shorter than its 32-byte content hash".to_owned(),
        });
    }
    let stored = checksum::to_hex(payload[..32].try_into().expect("32 bytes"));
    if stored != hash {
        return Err(FormatError::BadChunk {
            reason: format!("payload is keyed {stored}, expected {hash}"),
        });
    }
    Ok(&payload[32..])
}

fn with_label(error: FormatError, label: &str) -> FormatError {
    match error {
        FormatError::Io {
            operation, reason, ..
        } => FormatError::Io {
            operation,
            path: label.to_owned(),
            reason,
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{BoardReader, VerifyReport, read_summary};
    use crate::error::FormatError;

    #[test]
    fn a_short_file_is_refused() {
        let error = BoardReader::from_bytes(vec![0; 10]).expect_err("must not open");

        assert!(matches!(error, FormatError::Truncated { .. }));
    }

    #[test]
    fn a_verify_report_without_problems_is_ok() {
        let report = VerifyReport {
            problems: Vec::new(),
            chunks: 1,
            assets: 0,
            blobs_verified: 0,
            recovered: false,
        };

        assert!(report.is_ok());
    }

    #[test]
    fn a_missing_path_is_an_io_error_with_the_path() {
        let error = read_summary("definitely/not/a/board.rizref").expect_err("must not open");

        assert!(error.to_string().contains("definitely/not/a/board.rizref"));
    }
}
