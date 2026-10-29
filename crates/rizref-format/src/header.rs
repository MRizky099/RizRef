//! The fixed-size header every `.rizref` file starts with.
//!
//! The header is deliberately small and fully self-describing, so opening a
//! board is one `read` of 64 bytes, without parsing anything else. It holds the
//! board identity — not a file path — because a board is expected to be moved,
//! copied and renamed freely.
//!
//! # Why there is no header checksum
//!
//! An earlier skeleton gave the header its own checksum. The final design does
//! not, for one reason: an incremental save appends to an existing file and
//! must never rewrite a byte that a previous clean save depended on. The only
//! header field that changes over a board's life is `modified_ms`; without a
//! checksum a torn 8-byte timestamp write costs a timestamp, not the board.
//! Structural damage is still caught where it matters — by the magic bytes,
//! the fixed field sizes, the chunk framing and the footer's index digest.

use rizref_core::SceneId;

use crate::error::FormatError;

/// Magic bytes opening every `.rizref` file.
///
/// The trailing bytes are the container revision (`\x01\x00`, little-endian
/// `u16` 1). A future incompatible revision gets its own magic instead of a
/// file that older builds would try to interpret.
pub const MAGIC: [u8; 8] = *b"RIZREF\x01\x00";

/// Container revision this build writes.
pub const FORMAT_VERSION: u16 = 1;

/// Encoded size of the v1 header in bytes.
pub const HEADER_LEN: usize = 64;

/// Upper bound accepted for a future `header_size`.
///
/// The reader only needs the first 64 bytes; the field exists so a later
/// revision can grow the header without moving the chunk stream. A damaged
/// length must not make the reader skip an arbitrary amount of a file, so it
/// is capped well above any plausible future header.
pub const MAX_HEADER_SIZE: u32 = 1 << 20;

/// The header of a `.rizref` file.
///
/// All multi-byte fields are little-endian. `header_size` is the offset of the
/// first chunk, which is 64 for every file this build writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Offset of the first chunk; at least [`HEADER_LEN`].
    pub header_size: u32,
    /// Container revision; readers reject anything above their own.
    pub format_version: u16,
    /// Writer version, packed as `major << 8 | minor`; informational.
    pub writer_version: u16,
    /// Identity of the board stored in this file (UUID v4).
    pub scene_id: SceneId,
    /// When the board was first written, Unix milliseconds.
    pub created_ms: u64,
    /// When the last full save happened, Unix milliseconds.
    ///
    /// An incremental append does not rewrite the header, so the board index
    /// carries the authoritative last-modified time. This field stays the
    /// answer for a reader that only has the header.
    pub modified_ms: u64,
    /// Bit set of the `FLAG_*` constants below.
    pub flags: u64,
}

impl Header {
    /// A clean save ended with a footer and an index at the end of the file.
    pub const FLAG_HAS_FOOTER: u64 = 1 << 0;

    /// A header for `scene_id`, written now and not yet closed by a footer.
    pub fn new(scene_id: SceneId, created_ms: u64) -> Self {
        Self {
            header_size: HEADER_LEN as u32,
            format_version: FORMAT_VERSION,
            writer_version: packed_writer_version(),
            scene_id,
            created_ms,
            modified_ms: created_ms,
            flags: 0,
        }
    }

    /// Whether [`Header::FLAG_HAS_FOOTER`] is set.
    pub const fn has_footer(self) -> bool {
        self.flags & Self::FLAG_HAS_FOOTER != 0
    }

    /// The header with `flag` set.
    pub const fn with_flag(mut self, flag: u64) -> Self {
        self.flags |= flag;
        self
    }

    /// Encodes the fixed 64-byte block that opens a file.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut block = [0_u8; HEADER_LEN];
        block[0..8].copy_from_slice(&MAGIC);
        block[8..12].copy_from_slice(&self.header_size.to_le_bytes());
        block[12..14].copy_from_slice(&self.format_version.to_le_bytes());
        block[14..16].copy_from_slice(&self.writer_version.to_le_bytes());
        block[16..32].copy_from_slice(self.scene_id.as_bytes());
        block[32..40].copy_from_slice(&self.created_ms.to_le_bytes());
        block[40..48].copy_from_slice(&self.modified_ms.to_le_bytes());
        block[48..56].copy_from_slice(&self.flags.to_le_bytes());
        // 56..64 stays zero: reserved for a revision that grows the header.
        block
    }

    /// Decodes and validates the header at the start of `bytes`.
    ///
    /// The order of the checks matters for the message a user ends up reading:
    /// length, magic, header size, and only then the revision. A file whose
    /// version byte was flipped by a bad sector should be reported as damaged,
    /// not as "written by a newer RizRef".
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() < HEADER_LEN {
            return Err(FormatError::Truncated {
                needed: HEADER_LEN,
                available: bytes.len(),
            });
        }
        let block = &bytes[..HEADER_LEN];

        let mut magic = [0_u8; 8];
        magic.copy_from_slice(&block[0..8]);
        if magic != MAGIC {
            return Err(FormatError::BadMagic { found: magic });
        }

        let header_size = u32::from_le_bytes(block[8..12].try_into().expect("4 bytes"));
        if !(HEADER_LEN as u32..=MAX_HEADER_SIZE).contains(&header_size) {
            return Err(FormatError::BadHeader {
                reason: format!("header_size {header_size} is outside 64..={MAX_HEADER_SIZE}"),
            });
        }

        let format_version = u16::from_le_bytes(block[12..14].try_into().expect("2 bytes"));
        if format_version == 0 || format_version > FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion {
                found: format_version,
                supported: FORMAT_VERSION,
            });
        }

        Ok(Self {
            header_size,
            format_version,
            writer_version: u16::from_le_bytes(block[14..16].try_into().expect("2 bytes")),
            scene_id: SceneId::from_bytes(block[16..32].try_into().expect("16 bytes")),
            created_ms: u64::from_le_bytes(block[32..40].try_into().expect("8 bytes")),
            modified_ms: u64::from_le_bytes(block[40..48].try_into().expect("8 bytes")),
            flags: u64::from_le_bytes(block[48..56].try_into().expect("8 bytes")),
        })
    }
}

/// The crate version packed as `major << 8 | minor`.
pub fn packed_writer_version() -> u16 {
    packed_version(env!("CARGO_PKG_VERSION"))
}

/// Packs `major.minor.patch` into the 16-bit informational field.
fn packed_version(version: &str) -> u16 {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u16>().unwrap_or(0).min(u16::MAX >> 8));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    (major << 8) | (minor & 0xff)
}

/// Milliseconds since the Unix epoch, saturating at zero before 1970.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::{FORMAT_VERSION, HEADER_LEN, Header, MAGIC, now_ms, packed_version};
    use crate::error::FormatError;
    use rizref_core::SceneId;

    fn scene_id() -> SceneId {
        SceneId::from_hex("0123456789abcdeffedcba9876543210").expect("valid hex id")
    }

    #[test]
    fn the_header_fits_its_documented_size() {
        assert_eq!(HEADER_LEN, 64);
        assert_eq!(Header::new(scene_id(), 7).encode().len(), HEADER_LEN);
        assert_eq!(&Header::new(scene_id(), 7).encode()[..8], &MAGIC);
    }

    #[test]
    fn a_decoded_header_equals_the_one_that_was_encoded() {
        let header = Header::new(scene_id(), 1_700_000_000_000).with_flag(Header::FLAG_HAS_FOOTER);
        let mut expected = header;
        expected.modified_ms = 1_700_000_123_456;

        let decoded = Header::decode(&expected.encode()).expect("round trip");

        assert_eq!(decoded, expected);
        assert_eq!(decoded.format_version, FORMAT_VERSION);
        assert!(decoded.has_footer());
        assert_eq!(decoded.scene_id, scene_id());
        assert_eq!(decoded.header_size, HEADER_LEN as u32);
    }

    #[test]
    fn a_short_file_is_reported_as_truncated() {
        let truncated = Header::new(scene_id(), 1).encode();

        let error = Header::decode(&truncated[..HEADER_LEN - 1]).expect_err("must not decode");

        assert_eq!(
            error,
            FormatError::Truncated {
                needed: HEADER_LEN,
                available: HEADER_LEN - 1,
            }
        );
        assert_eq!(
            Header::decode(&[]).expect_err("must not decode"),
            FormatError::Truncated {
                needed: HEADER_LEN,
                available: 0,
            }
        );
    }

    #[test]
    fn another_file_type_is_rejected_by_the_magic_bytes() {
        let mut block = Header::new(scene_id(), 1).encode();
        block[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");

        let error = Header::decode(&block).expect_err("must not decode");

        assert_eq!(
            error,
            FormatError::BadMagic {
                found: *b"\x89PNG\r\n\x1a\n"
            }
        );
    }

    #[test]
    fn a_bad_header_size_is_refused() {
        let mut block = Header::new(scene_id(), 1).encode();
        block[8..12].copy_from_slice(&63_u32.to_le_bytes());
        assert!(matches!(
            Header::decode(&block),
            Err(FormatError::BadHeader { .. })
        ));

        block[8..12].copy_from_slice(&(super::MAX_HEADER_SIZE + 1).to_le_bytes());
        assert!(matches!(
            Header::decode(&block),
            Err(FormatError::BadHeader { .. })
        ));
    }

    #[test]
    fn a_newer_revision_is_refused_even_though_it_is_intact() {
        let mut newer = Header::new(scene_id(), 1);
        newer.format_version = FORMAT_VERSION + 1;

        let error = Header::decode(&newer.encode()).expect_err("must not decode");

        assert_eq!(
            error,
            FormatError::UnsupportedVersion {
                found: FORMAT_VERSION + 1,
                supported: FORMAT_VERSION,
            }
        );
    }

    #[test]
    fn a_zero_revision_is_refused() {
        let mut origin = Header::new(scene_id(), 1);
        origin.format_version = 0;

        assert!(matches!(
            Header::decode(&origin.encode()),
            Err(FormatError::UnsupportedVersion { found: 0, .. })
        ));
    }

    #[test]
    fn extra_trailing_bytes_belong_to_the_chunk_stream() {
        let header = Header::new(scene_id(), 1);
        let mut file = header.encode().to_vec();
        file.extend_from_slice(b"chunks follow");

        assert_eq!(Header::decode(&file).expect("header decodes"), header);
    }

    #[test]
    fn writer_versions_pack_to_major_and_minor() {
        assert_eq!(packed_version("1.2.3"), 0x0102);
        assert_eq!(packed_version("0.1.0"), 0x0001);
        assert_eq!(packed_version("garbage"), 0);
        assert_eq!(packed_version("1"), 0x0100);
    }

    #[test]
    fn now_ms_is_after_the_epoch() {
        assert!(now_ms() > 1_600_000_000_000);
    }
}
