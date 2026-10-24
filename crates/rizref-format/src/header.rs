//! The fixed-size header every `.rizref` file starts with.
//!
//! The header is deliberately small and fully self-describing, so opening a
//! board is one `read` of 44 bytes plus a checksum, without parsing anything
//! else. It holds the board identity — not a file path — because a board is
//! expected to be moved, copied and renamed freely.

use rizref_core::SceneId;

use crate::checksum;
use crate::error::FormatError;

/// Magic bytes opening every `.rizref` file: `RIZREF` followed by the NUL byte
/// and the DOS end-of-file marker, so an obviously-wrong file fails this check
/// long before anything is interpreted.
pub const MAGIC: [u8; 8] = *b"RIZREF\x00\x1a";

/// Container revision this build writes.
pub const FORMAT_VERSION: u16 = 1;

/// Encoded size of the header in bytes.
pub const HEADER_LEN: usize = 44;

/// Number of leading header bytes covered by the integrity check — everything
/// except the checksum itself.
const CHECKED_LEN: usize = HEADER_LEN - checksum::CHECKSUM_LEN;

/// The header of a `.rizref` file.
///
/// All multi-byte fields are little-endian. The layout is fixed for a revision:
/// new information goes into a payload chunk, never into the header, so a
/// reader can always trust these offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Container revision; readers must reject anything above their own.
    pub version: u16,
    /// Bit set of the `FLAG_*` constants below.
    pub flags: u16,
    /// Identity of the board stored in this file.
    pub scene_id: SceneId,
    /// Length of the payload that follows the header, in bytes.
    pub payload_len: u64,
}

impl Header {
    /// Payload chunks are LZ4-compressed.
    pub const FLAG_COMPRESSED: u16 = 1 << 0;

    /// The payload starts with an embedded board preview chunk.
    pub const FLAG_HAS_PREVIEW: u16 = 1 << 1;

    /// An empty header for `scene_id`: current revision, no flags, no payload.
    pub fn new(scene_id: SceneId) -> Self {
        Self {
            version: FORMAT_VERSION,
            flags: 0,
            scene_id,
            payload_len: 0,
        }
    }

    /// Whether [`Header::FLAG_COMPRESSED`] is set.
    pub const fn is_compressed(self) -> bool {
        self.flags & Self::FLAG_COMPRESSED != 0
    }

    /// Whether [`Header::FLAG_HAS_PREVIEW`] is set.
    pub const fn has_preview(self) -> bool {
        self.flags & Self::FLAG_HAS_PREVIEW != 0
    }

    /// Returns the header with `flag` set.
    pub const fn with_flag(mut self, flag: u16) -> Self {
        self.flags |= flag;
        self
    }

    /// Encodes the header into the fixed 44-byte block that opens a file.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut block = [0_u8; HEADER_LEN];
        block[0..8].copy_from_slice(&MAGIC);
        block[8..10].copy_from_slice(&self.version.to_le_bytes());
        block[10..12].copy_from_slice(&self.flags.to_le_bytes());
        block[12..28].copy_from_slice(self.scene_id.as_bytes());
        block[28..36].copy_from_slice(&self.payload_len.to_le_bytes());
        let short = checksum::checksum8(&block[..CHECKED_LEN]);
        block[CHECKED_LEN..HEADER_LEN].copy_from_slice(&short);
        block
    }

    /// Decodes and validates the header at the start of `bytes`.
    ///
    /// The order of the checks matters for the message a user ends up reading:
    /// length, then magic, then integrity, and only then the revision. A file
    /// whose version byte was flipped by a bad sector should be reported as
    /// damaged, not as "written by a newer RizRef".
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

        let mut stored = [0_u8; checksum::CHECKSUM_LEN];
        stored.copy_from_slice(&block[CHECKED_LEN..HEADER_LEN]);
        if checksum::checksum8(&block[..CHECKED_LEN]) != stored {
            return Err(FormatError::IntegrityMismatch);
        }

        let version = u16::from_le_bytes([block[8], block[9]]);
        if version == 0 || version > FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion {
                found: version,
                supported: FORMAT_VERSION,
            });
        }

        let scene_id = SceneId::from_bytes(block[12..28].try_into().expect("16 bytes of scene id"));

        Ok(Self {
            version,
            flags: u16::from_le_bytes([block[10], block[11]]),
            scene_id,
            payload_len: u64::from_le_bytes(
                block[28..36].try_into().expect("8 bytes of payload length"),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{FORMAT_VERSION, HEADER_LEN, Header, MAGIC};
    use crate::error::FormatError;
    use rizref_core::SceneId;

    fn scene_id() -> SceneId {
        SceneId::from_hex("0123456789abcdeffedcba9876543210").expect("valid hex id")
    }

    #[test]
    fn the_header_fits_its_documented_size() {
        assert_eq!(HEADER_LEN, 44);
        assert_eq!(Header::new(scene_id()).encode().len(), HEADER_LEN);
        assert_eq!(&Header::new(scene_id()).encode()[..8], &MAGIC);
    }

    #[test]
    fn a_decoded_header_equals_the_one_that_was_encoded() {
        let header = Header::new(scene_id())
            .with_flag(Header::FLAG_COMPRESSED)
            .with_flag(Header::FLAG_HAS_PREVIEW);
        let mut expected = header;
        expected.payload_len = 4096;

        let decoded = Header::decode(&expected.encode()).expect("round trip");

        assert_eq!(decoded, expected);
        assert_eq!(decoded.version, FORMAT_VERSION);
        assert!(decoded.is_compressed() && decoded.has_preview());
        assert_eq!(decoded.scene_id, scene_id());
    }

    #[test]
    fn a_short_file_is_reported_as_truncated() {
        let truncated = Header::new(scene_id()).encode();

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
        let mut block = Header::new(scene_id()).encode();
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
    fn a_flipped_bit_is_caught_by_the_checksum() {
        let valid = Header::new(scene_id()).encode();

        // Every byte of the checked region must be able to invalidate the header.
        for offset in 0..40 {
            let mut damaged = valid;
            damaged[offset] ^= 0x01;
            assert!(
                Header::decode(&damaged).is_err(),
                "a flipped bit at offset {offset} must not decode"
            );
        }
    }

    #[test]
    fn a_newer_revision_is_refused_even_though_it_is_intact() {
        let mut newer = Header::new(scene_id());
        newer.version = FORMAT_VERSION + 1;

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
        let mut origin = Header::new(scene_id());
        origin.version = 0;

        assert!(matches!(
            Header::decode(&origin.encode()),
            Err(FormatError::UnsupportedVersion { found: 0, .. })
        ));
    }

    #[test]
    fn extra_trailing_bytes_belong_to_the_payload() {
        let header = Header::new(scene_id());
        let mut file = header.encode().to_vec();
        file.extend_from_slice(b"payload follows");

        assert_eq!(Header::decode(&file).expect("header decodes"), header);
    }
}
