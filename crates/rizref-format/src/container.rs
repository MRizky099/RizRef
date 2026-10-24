//! Reading and writing the container framing.
//!
//! Opening a board is a two-step trust decision: first the framing has to be
//! intact (magic, header checksum, and a payload length that fits in the file),
//! and only then is the payload interpreted. Phase 00 implements the first step;
//! chunk parsing is phase 06. Keeping them apart means a damaged length can
//! never turn into a read past the end of a buffer.

use rizref_core::SceneId;

use crate::error::FormatError;
use crate::header::{HEADER_LEN, Header};

/// A validated view over a `.rizref` file held in memory.
///
/// The reader borrows rather than copies: a 200 MB board is memory-mapped once
/// and every chunk is a slice of it, which is what the phase-07 load budget
/// (first interactive frame under a second) depends on.
#[derive(Debug, Clone, Copy)]
pub struct ContainerReader<'a> {
    header: Header,
    payload: &'a [u8],
}

impl<'a> ContainerReader<'a> {
    /// Validates the header of `bytes` and borrows the payload it describes.
    ///
    /// Trailing bytes beyond the declared payload are ignored rather than
    /// rejected: a board concatenated into a bigger archive should still open,
    /// and the header's own checksum already caught real corruption.
    pub fn new(bytes: &'a [u8]) -> Result<Self, FormatError> {
        let header = Header::decode(bytes)?;

        let available = bytes.len() - HEADER_LEN;
        let declared = usize::try_from(header.payload_len).unwrap_or(usize::MAX);
        if declared > available {
            return Err(FormatError::Truncated {
                needed: HEADER_LEN.saturating_add(declared),
                available: bytes.len(),
            });
        }

        Ok(Self {
            header,
            payload: &bytes[HEADER_LEN..HEADER_LEN + declared],
        })
    }

    /// The validated header.
    pub fn header(&self) -> Header {
        self.header
    }

    /// The payload bytes, exactly `header.payload_len` of them.
    pub fn payload(&self) -> &'a [u8] {
        self.payload
    }
}

/// Builds the framed bytes of a `.rizref` file.
///
/// Phase 00 writes header plus payload verbatim — which is what the format says
/// before phase 06 adds chunk framing. The type exists already because the save
/// path (write a temporary file, flush, rename over the target) is easier to get
/// right against an interface than against a bare `Vec<u8>`.
#[derive(Debug, Clone, Copy)]
pub struct ContainerWriter {
    header: Header,
}

impl ContainerWriter {
    /// A writer for a new file of the board `scene_id`.
    pub fn new(scene_id: SceneId) -> Self {
        Self {
            header: Header::new(scene_id),
        }
    }

    /// Sets a header flag; see the `FLAG_*` constants on [`Header`].
    pub fn with_flag(mut self, flag: u16) -> Self {
        self.header = self.header.with_flag(flag);
        self
    }

    /// The header that will be written, with `payload_len` still zero.
    pub fn header(&self) -> Header {
        self.header
    }

    /// Encodes the whole file: the header, then `payload` verbatim.
    pub fn encode(&self, payload: &[u8]) -> Vec<u8> {
        let mut header = self.header;
        header.payload_len = payload.len() as u64;

        let mut file = Vec::with_capacity(HEADER_LEN + payload.len());
        file.extend_from_slice(&header.encode());
        file.extend_from_slice(payload);
        file
    }
}

#[cfg(test)]
mod tests {
    use super::{ContainerReader, ContainerWriter};
    use crate::error::FormatError;
    use crate::header::{HEADER_LEN, Header};
    use rizref_core::SceneId;

    fn scene_id() -> SceneId {
        SceneId::from_hex("deadbeefdeadbeefdeadbeefdeadbeef").expect("valid hex id")
    }

    #[test]
    fn a_written_container_reads_back_identically() {
        let payload = b"a scene payload would live here";
        let writer = ContainerWriter::new(scene_id()).with_flag(Header::FLAG_HAS_PREVIEW);

        let file = writer.encode(payload);
        let reader = ContainerReader::new(&file).expect("our own output must validate");

        assert_eq!(reader.payload(), payload);
        assert_eq!(reader.header().scene_id, scene_id());
        assert_eq!(reader.header().payload_len, payload.len() as u64);
        assert!(reader.header().has_preview());
        assert!(!reader.header().is_compressed());
    }

    #[test]
    fn an_empty_payload_is_a_valid_container() {
        let file = ContainerWriter::new(scene_id()).encode(&[]);

        let reader = ContainerReader::new(&file).expect("an empty board is still a board");

        assert!(reader.payload().is_empty());
        assert_eq!(reader.header().payload_len, 0);
        assert_eq!(file.len(), HEADER_LEN);
    }

    #[test]
    fn a_payload_shorter_than_declared_is_refused() {
        let file = ContainerWriter::new(scene_id()).encode(b"twelve bytes");
        let damaged = &file[..file.len() - 4];

        let error = ContainerReader::new(damaged).expect_err("must not open");

        assert_eq!(
            error,
            FormatError::Truncated {
                needed: HEADER_LEN + 12,
                available: damaged.len(),
            }
        );
    }

    #[test]
    fn bytes_beyond_the_declared_payload_are_ignored() {
        let mut file = ContainerWriter::new(scene_id()).encode(b"twelve bytes");
        file.extend_from_slice(b"trailing junk from an archive");

        let reader = ContainerReader::new(&file).expect("must open");

        assert_eq!(reader.payload(), b"twelve bytes");
    }

    #[test]
    fn a_file_without_a_header_is_refused() {
        let error = ContainerReader::new(b"short").expect_err("must not open");

        assert!(matches!(error, FormatError::Truncated { .. }));
    }
}
