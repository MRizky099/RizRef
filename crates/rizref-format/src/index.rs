//! The board index and the footer that points at it.
//!
//! An incremental save appends new chunks instead of rewriting the file, so the
//! only way to know which chunks are current is an index written after them.
//! The index lists the live name, document, thumbnail, every blob and every
//! preview, with absolute offsets and digests. The reader takes the **last**
//! valid index: a crash halfway through an append leaves the previous index
//! intact, and the previous board loads.
//!
//! The footer is the cheap part: a fixed 80-byte block at the very end of a
//! clean file that says where the last index starts. Finding it is one seek to
//! `file_len - 80`; a file without it was interrupted and is recovered by
//! scanning. The footer is written last, so its presence *is* the clean-save
//! marker.
//!
//! Both structures carry a BLAKE3 digest of themselves: the index hashes its
//! JSON, the footer hashes the stored index chunk, and an index chunk stores
//! its own digest in front of its JSON. That means an index found by scanning
//! can be trusted without a footer, which is what makes recovery work.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::checksum;
use crate::chunk::ChunkHeader;
use crate::error::FormatError;
use crate::header::MAGIC;

/// Index revision this build writes.
pub const INDEX_SCHEMA_VERSION: u16 = 1;

/// Encoded footer size in bytes.
pub const FOOTER_LEN: usize = 80;

/// Footer revision this build writes.
pub const FOOTER_VERSION: u16 = 1;

/// Where one chunk lives in the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRef {
    /// Absolute offset of the chunk frame.
    pub offset: u64,
    /// The chunk's 4-byte tag, so an index cannot silently point at the wrong
    /// kind of chunk.
    pub tag: String,
    /// Payload bytes on disk.
    pub stored_len: u64,
    /// Payload bytes after decompression.
    pub decompressed_len: u64,
    /// Compression code.
    pub compression: u8,
    /// BLAKE3 of the decompressed payload, 64 hex digits.
    pub digest: String,
    /// Members from a newer build.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

impl ChunkRef {
    /// A reference to the chunk whose frame is at `offset`.
    pub fn new(offset: u64, header: &ChunkHeader, digest: [u8; 32]) -> Self {
        Self {
            offset,
            tag: String::from_utf8_lossy(&header.kind.tag()).into_owned(),
            stored_len: header.stored_len,
            decompressed_len: header.decompressed_len,
            compression: header.compression,
            digest: checksum::to_hex(&digest),
            extra: BTreeMap::new(),
        }
    }

    /// A reference whose digest has not been computed yet.
    ///
    /// Used by the recovery scan: the frame and its lengths are observed, but
    /// the payload is not read until someone asks for it.
    pub(crate) fn new_unverified(offset: u64, header: &ChunkHeader) -> Self {
        let mut reference = Self::new(offset, header, [0; 32]);
        reference.digest.clear();
        reference
    }

    /// Total bytes this chunk occupies.
    pub const fn frame_len(&self) -> u64 {
        crate::chunk::CHUNK_HEADER_LEN as u64 + self.stored_len
    }

    /// The digest as raw bytes.
    pub fn digest_bytes(&self) -> Result<[u8; 32], FormatError> {
        checksum::from_hex(&self.digest).ok_or_else(|| FormatError::Index {
            reason: format!("chunk digest {:?} is not 64 hex digits", self.digest),
        })
    }
}

/// One asset's chunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexAsset {
    /// BLAKE3 of the original bytes, 64 hex digits.
    pub hash: String,
    /// The embedded original, when this build wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<ChunkRef>,
    /// The encoded preview, when one was stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<ChunkRef>,
    /// Members from a newer build.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

/// Everything the last clean save (or append) knows about the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardIndex {
    /// Revision of this structure.
    #[serde(default = "index_schema")]
    pub schema_version: u16,
    /// Incremented on every save, so two saves can be ordered.
    #[serde(default)]
    pub generation: u64,
    /// When the board was first written, Unix milliseconds.
    #[serde(default)]
    pub created_ms: u64,
    /// When this generation was written, Unix milliseconds.
    #[serde(default)]
    pub modified_ms: u64,
    /// Bytes appended since the last full save; drives compaction.
    #[serde(default)]
    pub appended_bytes: u64,
    /// Writer version string, informational.
    #[serde(default)]
    pub writer: String,
    /// The current board name chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<ChunkRef>,
    /// The current scene document.
    pub document: ChunkRef,
    /// The current board thumbnail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<ChunkRef>,
    /// Every live asset.
    #[serde(default)]
    pub assets: Vec<IndexAsset>,
    /// Members from a newer build.
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

const fn index_schema() -> u16 {
    INDEX_SCHEMA_VERSION
}

impl BoardIndex {
    /// The asset with `id`, if the index lists it.
    pub fn asset(&self, id: &str) -> Option<&IndexAsset> {
        self.assets.iter().find(|asset| asset.hash == id)
    }

    /// The index serialised as the chunk payload: digest, then JSON.
    pub fn encode_payload(&self) -> Result<Vec<u8>, FormatError> {
        let json = serde_json::to_vec(self).map_err(|error| FormatError::Index {
            reason: error.to_string(),
        })?;
        let digest = checksum::digest(&json);
        let mut payload = Vec::with_capacity(checksum::DIGEST_LEN + json.len());
        payload.extend_from_slice(&digest);
        payload.extend_from_slice(&json);
        Ok(payload)
    }

    /// Parses a chunk payload written by [`BoardIndex::encode_payload`].
    pub fn decode_payload(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() < checksum::DIGEST_LEN {
            return Err(FormatError::Truncated {
                needed: checksum::DIGEST_LEN,
                available: bytes.len(),
            });
        }
        let (prefix, json) = bytes.split_at(checksum::DIGEST_LEN);
        let expected: [u8; checksum::DIGEST_LEN] = prefix.try_into().expect("32 bytes of digest");
        if !checksum::verify(json, &expected) {
            return Err(FormatError::IntegrityMismatch {
                subject: "board index",
            });
        }
        let index: Self = serde_json::from_slice(json).map_err(|error| FormatError::Index {
            reason: error.to_string(),
        })?;
        if index.schema_version > INDEX_SCHEMA_VERSION {
            return Err(FormatError::UnsupportedSchema {
                found: index.schema_version,
                supported: INDEX_SCHEMA_VERSION,
            });
        }
        Ok(index)
    }

    /// Bytes the chunks this index references occupy, frame included.
    ///
    /// Used for the compaction threshold: appending more than twice the live
    /// size in one save interval is a sign that a full rewrite is cheaper than
    /// carrying the dead chunks.
    pub fn live_chunk_bytes(&self) -> u64 {
        let mut bytes = self.document.frame_len();
        for chunk in self.name.iter().chain(self.thumbnail.iter()).chain(
            self.assets
                .iter()
                .flat_map(|asset| asset.blob.iter().chain(asset.preview.iter())),
        ) {
            bytes = bytes.saturating_add(chunk.frame_len());
        }
        bytes
    }
}

/// The fixed block at the end of a clean file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footer {
    /// Generation of the index this footer points at.
    pub generation: u64,
    /// Absolute offset of the index chunk frame.
    pub index_offset: u64,
    /// Stored payload length of the index chunk.
    pub index_len: u64,
    /// BLAKE3 of the index's stored payload bytes.
    pub index_digest: [u8; 32],
}

impl Footer {
    /// Encodes the fixed 80-byte block that ends a clean file.
    pub fn encode(&self) -> [u8; FOOTER_LEN] {
        let mut bytes = [0_u8; FOOTER_LEN];
        bytes[0..8].copy_from_slice(&MAGIC);
        bytes[8..10].copy_from_slice(&FOOTER_VERSION.to_le_bytes());
        // 10..12: flags, zero in v1.
        bytes[12..20].copy_from_slice(&self.generation.to_le_bytes());
        bytes[20..28].copy_from_slice(&self.index_offset.to_le_bytes());
        bytes[28..36].copy_from_slice(&self.index_len.to_le_bytes());
        bytes[36..68].copy_from_slice(&self.index_digest);
        // 68..72: reserved.
        bytes[72..80].copy_from_slice(&MAGIC);
        bytes
    }

    /// Decodes the fixed block, or reports why it is not one.
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() != FOOTER_LEN {
            return Err(FormatError::Truncated {
                needed: FOOTER_LEN,
                available: bytes.len(),
            });
        }
        if bytes[0..8] != MAGIC || bytes[72..80] != MAGIC {
            return Err(FormatError::BadMagic {
                found: bytes[0..8].try_into().expect("8 bytes"),
            });
        }
        let version = u16::from_le_bytes(bytes[8..10].try_into().expect("2 bytes"));
        if version == 0 || version > FOOTER_VERSION {
            return Err(FormatError::UnsupportedVersion {
                found: version,
                supported: FOOTER_VERSION,
            });
        }
        Ok(Self {
            generation: u64::from_le_bytes(bytes[12..20].try_into().expect("8 bytes")),
            index_offset: u64::from_le_bytes(bytes[20..28].try_into().expect("8 bytes")),
            index_len: u64::from_le_bytes(bytes[28..36].try_into().expect("8 bytes")),
            index_digest: bytes[36..68].try_into().expect("32 bytes"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{BoardIndex, ChunkRef, FOOTER_LEN, Footer, INDEX_SCHEMA_VERSION, IndexAsset};
    use crate::chunk::{ChunkHeader, ChunkKind};
    use crate::compression::Compression;
    use crate::error::FormatError;

    fn chunk_ref(offset: u64) -> ChunkRef {
        let header = ChunkHeader::new(ChunkKind::Document, Compression::Lz4, 40, 120);
        ChunkRef::new(offset, &header, [7; 32])
    }

    fn index() -> BoardIndex {
        BoardIndex {
            schema_version: INDEX_SCHEMA_VERSION,
            generation: 3,
            created_ms: 10,
            modified_ms: 20,
            appended_bytes: 512,
            writer: "rizref 0.1.0".to_owned(),
            name: Some(chunk_ref(64)),
            document: chunk_ref(200),
            thumbnail: None,
            assets: vec![IndexAsset {
                hash: "ab".repeat(32),
                blob: Some(chunk_ref(400)),
                preview: Some(chunk_ref(900)),
                extra: Default::default(),
            }],
            extra: Default::default(),
        }
    }

    #[test]
    fn an_index_round_trips_and_hashes_itself() {
        let index = index();

        let payload = index.encode_payload().expect("encode");
        let back = BoardIndex::decode_payload(&payload).expect("decode");

        assert_eq!(back, index);
        assert_eq!(back.live_chunk_bytes(), (24 + 40) * 4);
    }

    #[test]
    fn a_damaged_index_is_refused() {
        let mut payload = index().encode_payload().expect("encode");
        let last = payload.len() - 1;
        payload[last] ^= 0x01;

        let error = BoardIndex::decode_payload(&payload).expect_err("must not decode");

        assert_eq!(
            error,
            FormatError::IntegrityMismatch {
                subject: "board index"
            }
        );
    }

    #[test]
    fn an_index_from_a_newer_build_is_refused() {
        let mut index = index();
        index.schema_version = INDEX_SCHEMA_VERSION + 1;
        let payload = index.encode_payload().expect("encode");

        let error = BoardIndex::decode_payload(&payload).expect_err("must not decode");

        assert!(matches!(error, FormatError::UnsupportedSchema { .. }));
    }

    #[test]
    fn unknown_index_members_are_preserved() {
        let payload = index().encode_payload().expect("encode");
        let mut value: serde_json::Value = serde_json::from_slice(&payload[32..]).expect("json");
        value["future"] = serde_json::json!({"dpi": 300});
        let json = serde_json::to_vec(&value).expect("json");
        let digest = crate::checksum::digest(&json);
        let mut with_extra = digest.to_vec();
        with_extra.extend_from_slice(&json);

        let parsed = BoardIndex::decode_payload(&with_extra).expect("decode");

        assert_eq!(
            parsed.extra.get("future"),
            Some(&serde_json::json!({"dpi": 300}))
        );
    }

    #[test]
    fn a_footer_round_trips() {
        let footer = Footer {
            generation: 9,
            index_offset: 1234,
            index_len: 567,
            index_digest: [3; 32],
        };

        let decoded = Footer::decode(&footer.encode()).expect("round trip");

        assert_eq!(decoded, footer);
        assert_eq!(footer.encode().len(), FOOTER_LEN);
    }

    #[test]
    fn a_block_that_is_not_a_footer_is_refused() {
        let mut bytes = Footer {
            generation: 1,
            index_offset: 0,
            index_len: 0,
            index_digest: [0; 32],
        }
        .encode();
        bytes[0] ^= 0xff;

        assert!(matches!(
            Footer::decode(&bytes),
            Err(FormatError::BadMagic { .. })
        ));
        assert!(matches!(
            Footer::decode(&bytes[..10]),
            Err(FormatError::Truncated { .. })
        ));
    }

    #[test]
    fn a_footer_from_a_newer_revision_is_refused() {
        let mut bytes = Footer {
            generation: 1,
            index_offset: 0,
            index_len: 0,
            index_digest: [0; 32],
        }
        .encode();
        bytes[8..10].copy_from_slice(&(super::FOOTER_VERSION + 1).to_le_bytes());

        assert!(matches!(
            Footer::decode(&bytes),
            Err(FormatError::UnsupportedVersion { .. })
        ));
    }
}
