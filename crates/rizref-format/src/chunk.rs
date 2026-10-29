//! Chunk framing and the chunk vocabulary.
//!
//! A board is a header followed by a stream of chunks, each introduced by a
//! 4-byte tag and a fixed 24-byte frame. Tags rather than numeric codes because
//! they survive a hexdump: when a board has to be inspected by hand, `NAME`,
//! `DOC `, `BLOB` and `END!` say what they are, and a reader that meets a tag
//! from a newer build can skip it by its length without guessing.
//!
//! The frame carries both lengths — stored and decompressed — so a reader can
//! skip a chunk without decompressing it and can check a claimed allocation
//! against the bytes actually present. Compression is per chunk: the document
//! is LZ4, an image blob is usually raw because its format already compressed it.
//!
//! # Chunk tags
//!
//! | Tag | Constant | Contents |
//! |---|---|---|
//! | `NAME` | [`ChunkKind::Name`] | board display name, UTF-8 |
//! | `DOC ` | [`ChunkKind::Document`] | scene document, JSON |
//! | `THMB` | [`ChunkKind::Thumbnail`] | board thumbnail (u32 width, u32 height, encoded image) |
//! | `BLOB` | [`ChunkKind::Blob`] | 32-byte content hash followed by the original image bytes |
//! | `PREV` | [`ChunkKind::Preview`] | 32-byte content hash followed by an encoded asset preview |
//! | `INDX` | [`ChunkKind::Index`] | board index, JSON with a 32-byte digest prefix |
//! | `END!` | [`ChunkKind::Footer`] | fixed 80-byte footer pointing at the index |

use crate::compression::Compression;
use crate::error::FormatError;

/// Length of a chunk tag in bytes.
pub const TAG_LEN: usize = 4;

/// Encoded size of a chunk frame, before the payload.
pub const CHUNK_HEADER_LEN: usize = 24;

/// What a payload chunk contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChunkKind {
    /// Board display name, UTF-8. Multiple allowed; the last one wins.
    Name,
    /// The scene document: items, groups, assets and the saved view.
    Document,
    /// The embedded board thumbnail.
    Thumbnail,
    /// One original image, content-addressed by the 32-byte hash that prefixes
    /// its payload.
    ///
    /// Keeping the original bytes is a product decision, not an implementation
    /// detail: it is what preserves embedded metadata (EXIF, C2PA content
    /// credentials) that re-encoding would destroy.
    Blob,
    /// A small encoded preview of one asset, keyed by the same content hash.
    Preview,
    /// The board index: where the current document, name, thumbnail, blobs and
    /// previews live. The last valid index wins.
    Index,
    /// The footer that marks a clean save and points at the last index.
    Footer,
    /// A tag this build does not know. Skipped by its stored length.
    Unknown([u8; TAG_LEN]),
}

impl ChunkKind {
    /// Every kind this build knows, in the order a fresh save writes them.
    pub const ALL: [Self; 7] = [
        Self::Name,
        Self::Thumbnail,
        Self::Document,
        Self::Blob,
        Self::Preview,
        Self::Index,
        Self::Footer,
    ];

    /// The tag stored in the container.
    pub const fn tag(self) -> [u8; TAG_LEN] {
        match self {
            Self::Name => *b"NAME",
            Self::Document => *b"DOC ",
            Self::Thumbnail => *b"THMB",
            Self::Blob => *b"BLOB",
            Self::Preview => *b"PREV",
            Self::Index => *b"INDX",
            Self::Footer => *b"END!",
            Self::Unknown(tag) => tag,
        }
    }

    /// The kind for `tag`, or [`ChunkKind::Unknown`] for a tag from a newer
    /// build.
    ///
    /// Unknown is not an error: such a chunk is skipped by its length, never
    /// interpreted.
    pub fn from_tag(tag: [u8; TAG_LEN]) -> Self {
        match &tag {
            b"NAME" => Self::Name,
            b"DOC " => Self::Document,
            b"THMB" => Self::Thumbnail,
            b"BLOB" => Self::Blob,
            b"PREV" => Self::Preview,
            b"INDX" => Self::Index,
            b"END!" => Self::Footer,
            _ => Self::Unknown(tag),
        }
    }

    /// Whether this build knows the tag.
    pub const fn is_known(self) -> bool {
        !matches!(self, Self::Unknown(_))
    }
}

/// The fixed frame in front of every chunk payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkHeader {
    /// What the chunk contains.
    pub kind: ChunkKind,
    /// Compression code; see [`Compression`].
    pub compression: u8,
    /// Payload bytes actually stored after the frame.
    pub stored_len: u64,
    /// Payload bytes after decompression.
    pub decompressed_len: u64,
}

impl ChunkHeader {
    /// A frame for a payload that was already encoded.
    pub fn new(
        kind: ChunkKind,
        compression: Compression,
        stored_len: u64,
        decompressed_len: u64,
    ) -> Self {
        Self {
            kind,
            compression: compression.code(),
            stored_len,
            decompressed_len,
        }
    }

    /// A frame for `payload`, which is stored as given.
    pub fn raw(kind: ChunkKind, payload_len: u64) -> Self {
        Self::new(kind, Compression::Raw, payload_len, payload_len)
    }

    /// The compression codec, or an error for a code from a newer build.
    pub fn codec(self) -> Result<Compression, FormatError> {
        Compression::from_code(self.compression)
    }

    /// Encodes the fixed 24-byte frame.
    pub fn encode(&self) -> [u8; CHUNK_HEADER_LEN] {
        let mut bytes = [0_u8; CHUNK_HEADER_LEN];
        bytes[0..4].copy_from_slice(&self.kind.tag());
        bytes[4] = self.compression;
        // bytes[5..8] stay zero: reserved for a future frame revision.
        bytes[8..16].copy_from_slice(&self.stored_len.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.decompressed_len.to_le_bytes());
        bytes
    }

    /// Decodes one frame.
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() < CHUNK_HEADER_LEN {
            return Err(FormatError::Truncated {
                needed: CHUNK_HEADER_LEN,
                available: bytes.len(),
            });
        }
        let mut tag = [0_u8; TAG_LEN];
        tag.copy_from_slice(&bytes[0..4]);
        let header = Self {
            kind: ChunkKind::from_tag(tag),
            compression: bytes[4],
            stored_len: u64::from_le_bytes(bytes[8..16].try_into().expect("8 bytes")),
            decompressed_len: u64::from_le_bytes(bytes[16..24].try_into().expect("8 bytes")),
        };
        if header.compression == Compression::Raw.code()
            && header.stored_len != header.decompressed_len
        {
            return Err(FormatError::BadChunk {
                reason: format!(
                    "raw chunk stores {} bytes but declares {}",
                    header.stored_len, header.decompressed_len
                ),
            });
        }
        Ok(header)
    }

    /// Total bytes this chunk occupies, frame included.
    pub const fn frame_len(&self) -> u64 {
        CHUNK_HEADER_LEN as u64 + self.stored_len
    }
}

#[cfg(test)]
mod tests {
    use super::{CHUNK_HEADER_LEN, ChunkHeader, ChunkKind, TAG_LEN};
    use crate::compression::Compression;
    use crate::error::FormatError;

    #[test]
    fn every_known_kind_round_trips_through_its_tag() {
        for kind in ChunkKind::ALL {
            assert_eq!(ChunkKind::from_tag(kind.tag()), kind);
            assert!(kind.is_known());
        }
    }

    #[test]
    fn tags_are_four_bytes_and_distinct() {
        for (index, kind) in ChunkKind::ALL.iter().enumerate() {
            assert_eq!(kind.tag().len(), TAG_LEN);
            for other in &ChunkKind::ALL[index + 1..] {
                assert_ne!(
                    kind.tag(),
                    other.tag(),
                    "{kind:?} and {other:?} share a tag"
                );
            }
        }
    }

    #[test]
    fn unknown_tags_are_skippable_not_errors() {
        let kind = ChunkKind::from_tag(*b"ZZZZ");

        assert_eq!(kind, ChunkKind::Unknown(*b"ZZZZ"));
        assert!(!kind.is_known());
        assert_eq!(kind.tag(), *b"ZZZZ");
        assert_eq!(
            ChunkKind::from_tag(*b"doc "),
            ChunkKind::Unknown(*b"doc "),
            "tags are case sensitive"
        );
        assert_eq!(ChunkKind::from_tag(*b"DOC "), ChunkKind::Document);
    }

    #[test]
    fn tags_are_frozen_by_the_file_format() {
        assert_eq!(ChunkKind::Name.tag(), *b"NAME");
        assert_eq!(ChunkKind::Document.tag(), *b"DOC ");
        assert_eq!(ChunkKind::Thumbnail.tag(), *b"THMB");
        assert_eq!(ChunkKind::Blob.tag(), *b"BLOB");
        assert_eq!(ChunkKind::Preview.tag(), *b"PREV");
        assert_eq!(ChunkKind::Index.tag(), *b"INDX");
        assert_eq!(ChunkKind::Footer.tag(), *b"END!");
        assert_eq!(TAG_LEN, 4);
    }

    #[test]
    fn a_chunk_header_round_trips() {
        let header = ChunkHeader::new(ChunkKind::Document, Compression::Lz4, 100, 400);

        let decoded = ChunkHeader::decode(&header.encode()).expect("round trip");

        assert_eq!(decoded, header);
        assert_eq!(decoded.codec(), Ok(Compression::Lz4));
        assert_eq!(decoded.frame_len(), CHUNK_HEADER_LEN as u64 + 100);
    }

    #[test]
    fn a_raw_chunk_must_declare_equal_lengths() {
        let mut bytes = ChunkHeader::raw(ChunkKind::Name, 4).encode();
        bytes[16..24].copy_from_slice(&9_u64.to_le_bytes());

        let error = ChunkHeader::decode(&bytes).expect_err("must not decode");

        assert!(matches!(error, FormatError::BadChunk { .. }));
    }

    #[test]
    fn a_short_frame_is_reported_as_truncated() {
        let error = ChunkHeader::decode(&[0; 5]).expect_err("must not decode");

        assert_eq!(
            error,
            FormatError::Truncated {
                needed: CHUNK_HEADER_LEN,
                available: 5,
            }
        );
    }

    #[test]
    fn an_unknown_compression_code_is_reported_when_the_payload_is_needed() {
        let mut bytes = ChunkHeader::raw(ChunkKind::Document, 4).encode();
        bytes[4] = 9;

        let header = ChunkHeader::decode(&bytes).expect("frame decodes");
        assert_eq!(
            header.codec(),
            Err(FormatError::UnknownCompression { code: 9 })
        );
    }
}
