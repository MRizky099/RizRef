//! Per-chunk compression.
//!
//! Chosen over a stronger codec because a board is dominated by
//! already-compressed image bytes: what is left to compress is the scene
//! document, the board index and the name, which are small and want to be fast,
//! not small. LZ4 also keeps every chunk independently decompressible, which is
//! what an incremental save needs.
//!
//! The uncompressed length travels in the chunk frame, so nothing is stored
//! twice and a damaged length can be checked against the file size before a
//! single byte is allocated.

use crate::error::FormatError;

/// Compression code stored in a chunk header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// The payload is stored verbatim.
    Raw = 0,
    /// The payload is one LZ4 block.
    Lz4 = 1,
}

impl Compression {
    /// Resolves a code from a chunk header.
    pub const fn from_code(code: u8) -> Result<Self, FormatError> {
        match code {
            0 => Ok(Self::Raw),
            1 => Ok(Self::Lz4),
            other => Err(FormatError::UnknownCompression { code: other }),
        }
    }

    /// The code stored in a chunk header.
    pub const fn code(self) -> u8 {
        self as u8
    }
}

/// Largest uncompressed chunk this build accepts, in bytes.
///
/// A guard against a damaged length, not a policy: it sits far above any single
/// image a reference board holds. Blobs are read lazily, one chunk at a time,
/// so this is also a ceiling on a single allocation.
pub const MAX_UNCOMPRESSED_LEN: u64 = 1 << 30;

/// Compresses `bytes` as one LZ4 block.
pub fn compress_lz4(bytes: &[u8]) -> Result<Vec<u8>, FormatError> {
    check_len(bytes.len() as u64)?;
    Ok(lz4_flex::block::compress(bytes))
}

/// Decompresses one LZ4 block to exactly `decompressed_len` bytes.
pub fn decompress_lz4(bytes: &[u8], decompressed_len: u64) -> Result<Vec<u8>, FormatError> {
    check_len(decompressed_len)?;
    let expected = usize::try_from(decompressed_len).map_err(|_| FormatError::ChunkTooLarge {
        size: decompressed_len,
        limit: MAX_UNCOMPRESSED_LEN,
    })?;
    lz4_flex::block::decompress(bytes, expected).map_err(|error| FormatError::Decompression {
        reason: error.to_string(),
    })
}

/// Chooses the smaller of raw and LZ4 for `bytes`.
///
/// Returns the codec and its payload. Text-like chunks (the document, the
/// index) compress several times over; image blobs do not, and the caller can
/// skip the attempt with [`store_raw`] where codec work would be wasted.
pub fn choose(bytes: &[u8]) -> Result<(Compression, Vec<u8>), FormatError> {
    if bytes.len() < 64 {
        return Ok((Compression::Raw, bytes.to_vec()));
    }
    let packed = compress_lz4(bytes)?;
    if packed.len() < bytes.len() {
        Ok((Compression::Lz4, packed))
    } else {
        Ok((Compression::Raw, bytes.to_vec()))
    }
}

/// Stores `bytes` without attempting compression.
pub fn store_raw(bytes: &[u8]) -> Result<(Compression, Vec<u8>), FormatError> {
    check_len(bytes.len() as u64)?;
    Ok((Compression::Raw, bytes.to_vec()))
}

/// Reverses [`choose`] or [`store_raw`].
pub fn decode(
    compression: Compression,
    stored: &[u8],
    decompressed_len: u64,
) -> Result<Vec<u8>, FormatError> {
    match compression {
        Compression::Raw => {
            if stored.len() as u64 != decompressed_len {
                return Err(FormatError::BadChunk {
                    reason: format!(
                        "raw chunk stores {} bytes but declares {decompressed_len}",
                        stored.len()
                    ),
                });
            }
            check_len(decompressed_len)?;
            Ok(stored.to_vec())
        }
        Compression::Lz4 => decompress_lz4(stored, decompressed_len),
    }
}

/// Refuses a length above [`MAX_UNCOMPRESSED_LEN`].
pub fn check_len(len: u64) -> Result<(), FormatError> {
    if len > MAX_UNCOMPRESSED_LEN {
        return Err(FormatError::ChunkTooLarge {
            size: len,
            limit: MAX_UNCOMPRESSED_LEN,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Compression, MAX_UNCOMPRESSED_LEN, choose, compress_lz4, decode, decompress_lz4, store_raw,
    };
    use crate::error::FormatError;

    #[test]
    fn compression_codes_round_trip_and_reserve_the_documented_values() {
        assert_eq!(Compression::Raw.code(), 0);
        assert_eq!(Compression::Lz4.code(), 1);
        assert_eq!(Compression::from_code(0), Ok(Compression::Raw));
        assert_eq!(Compression::from_code(1), Ok(Compression::Lz4));
        assert_eq!(
            Compression::from_code(2),
            Err(FormatError::UnknownCompression { code: 2 })
        );
    }

    #[test]
    fn a_repetitive_payload_round_trips_through_lz4() {
        let original: Vec<u8> = (0..4096_u32).map(|index| (index % 7) as u8).collect();

        let packed = compress_lz4(&original).expect("small enough to compress");
        let back = decompress_lz4(&packed, original.len() as u64).expect("round trip");

        assert!(packed.len() < original.len(), "repetitive data must shrink");
        assert_eq!(back, original);
    }

    #[test]
    fn the_chooser_prefers_the_smaller_form() {
        let repetitive = vec![b'a'; 4096];
        let incompressible: Vec<u8> = (0..=255_u8).collect();

        let (repetitive_codec, repetitive_packed) = choose(&repetitive).expect("choose");
        let (small_codec, small_packed) = choose(b"tiny").expect("choose");
        let (random_codec, random_packed) = choose(&incompressible).expect("choose");

        assert_eq!(repetitive_codec, Compression::Lz4);
        assert!(repetitive_packed.len() < repetitive.len());
        assert_eq!(
            small_codec,
            Compression::Raw,
            "tiny inputs are not worth it"
        );
        assert_eq!(small_packed, b"tiny");
        assert_eq!(random_codec, Compression::Raw);
        assert_eq!(random_packed, incompressible);
    }

    #[test]
    fn raw_chunks_refuse_a_length_that_disagrees_with_the_payload() {
        let error = decode(Compression::Raw, b"short", 10).expect_err("must not decode");

        assert!(matches!(error, FormatError::BadChunk { .. }));
    }

    #[test]
    fn a_huge_declared_length_is_refused_before_allocating() {
        let error =
            decompress_lz4(&[0x00, 0x01], MAX_UNCOMPRESSED_LEN + 1).expect_err("must not decode");

        assert_eq!(
            error,
            FormatError::ChunkTooLarge {
                size: MAX_UNCOMPRESSED_LEN + 1,
                limit: MAX_UNCOMPRESSED_LEN,
            }
        );
    }

    #[test]
    fn body_that_is_not_lz4_is_an_error_not_a_panic() {
        let error = decompress_lz4(b"definitely not lz4", 16).expect_err("must not decode");

        assert!(
            matches!(error, FormatError::Decompression { .. }),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn store_raw_round_trips() {
        let (codec, stored) = store_raw(b"original image bytes").expect("store");

        assert_eq!(codec, Compression::Raw);
        assert_eq!(
            decode(codec, &stored, 20).expect("decode"),
            b"original image bytes"
        );
    }
}
