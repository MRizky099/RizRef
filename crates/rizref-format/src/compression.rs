//! LZ4 compression for payload chunks.
//!
//! Chosen over a stronger codec because a payload is dominated by
//! already-compressed image bytes: what is left to compress is the scene model
//! and the metadata, which are small and want to be fast, not small. LZ4 also
//! keeps every chunk independently decompressible, which is what an incremental
//! save needs.
//!
//! The compressed form is LZ4 with the uncompressed length prepended as a
//! little-endian `u32`. The length is validated against [`MAX_UNCOMPRESSED_LEN`]
//! before anything is allocated, so a corrupt prefix turns into an error instead
//! of a multi-gigabyte allocation.

use crate::error::FormatError;

/// Length of the uncompressed-length prefix, in bytes.
pub const PREFIX_LEN: usize = 4;

/// Largest uncompressed chunk this build accepts, in bytes.
///
/// A guard against a damaged length prefix, not a policy: it sits far above any
/// single image a reference board holds, and phase 06 can lower it once the
/// blob table knows what real boards look like.
pub const MAX_UNCOMPRESSED_LEN: usize = 1 << 30;

/// Compresses `bytes`, prefixing the uncompressed length.
pub fn compress(bytes: &[u8]) -> Result<Vec<u8>, FormatError> {
    if bytes.len() > MAX_UNCOMPRESSED_LEN {
        return Err(FormatError::ChunkTooLarge {
            size: bytes.len(),
            limit: MAX_UNCOMPRESSED_LEN,
        });
    }

    let mut packed = Vec::with_capacity(bytes.len() / 2 + PREFIX_LEN);
    packed.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    packed.extend_from_slice(&lz4_flex::compress(bytes));
    Ok(packed)
}

/// Reverses [`compress`].
pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>, FormatError> {
    let Some((prefix, body)) = bytes.split_at_checked(PREFIX_LEN) else {
        return Err(FormatError::Truncated {
            needed: PREFIX_LEN,
            available: bytes.len(),
        });
    };
    let expected =
        u32::from_le_bytes(prefix.try_into().expect("4 bytes of length prefix")) as usize;
    if expected > MAX_UNCOMPRESSED_LEN {
        return Err(FormatError::ChunkTooLarge {
            size: expected,
            limit: MAX_UNCOMPRESSED_LEN,
        });
    }

    lz4_flex::decompress(body, expected).map_err(|error| FormatError::Decompression {
        reason: error.to_string(),
    })
}

/// Whether compressing `bytes` is worth it.
///
/// Incompressible input — a JPEG blob, say — grows slightly instead of
/// shrinking, and the header records per-file compression: a payload that would
/// not shrink is stored as-is rather than paying for the attempt.
pub fn is_worth_compressing(bytes: &[u8]) -> bool {
    bytes.len() > 64 && compress(bytes).is_ok_and(|packed| packed.len() < bytes.len())
}

#[cfg(test)]
mod tests {
    use super::{MAX_UNCOMPRESSED_LEN, compress, decompress, is_worth_compressing};
    use crate::error::FormatError;

    #[test]
    fn a_repetitive_payload_round_trips() {
        let original: Vec<u8> = (0..4096_u32).map(|index| (index % 7) as u8).collect();

        let packed = compress(&original).expect("small enough to compress");

        assert!(packed.len() < original.len(), "repetitive data must shrink");
        assert_eq!(decompress(&packed).expect("round trip"), original);
    }

    #[test]
    fn a_single_byte_round_trips() {
        let packed = compress(b"x").expect("compress");
        assert_eq!(decompress(&packed).expect("round trip"), b"x");
    }

    #[test]
    fn empty_input_round_trips() {
        let packed = compress(b"").expect("compress");
        assert_eq!(decompress(&packed).expect("round trip"), b"");
    }

    #[test]
    fn a_missing_length_prefix_is_reported_as_truncated() {
        let error = decompress(&[0x01, 0x02]).expect_err("must not decode");

        assert_eq!(
            error,
            FormatError::Truncated {
                needed: super::PREFIX_LEN,
                available: 2,
            }
        );
    }

    #[test]
    fn a_huge_length_prefix_is_refused_before_allocating() {
        // Claims 4 GiB of output in five bytes: the allocation must never happen.
        let error = decompress(&[0xff, 0xff, 0xff, 0xff, 0x00]).expect_err("must not decode");

        assert_eq!(
            error,
            FormatError::ChunkTooLarge {
                size: u32::MAX as usize,
                limit: MAX_UNCOMPRESSED_LEN,
            }
        );
    }

    #[test]
    fn body_that_is_not_lz4_is_an_error_not_a_panic() {
        let mut damaged = (16_u32).to_le_bytes().to_vec();
        damaged.extend_from_slice(b"definitely not lz4");

        let error = decompress(&damaged).expect_err("must not decode");

        assert!(
            matches!(error, FormatError::Decompression { .. }),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn incompressible_data_is_not_offered_for_compression() {
        // Every byte value once: nothing repeats, so LZ4 cannot win.
        let incompressible: Vec<u8> = (0..=255_u8).collect();
        let repetitive = vec![b'a'; 4096];

        assert!(!is_worth_compressing(&incompressible));
        assert!(!is_worth_compressing(b"tiny"));
        assert!(is_worth_compressing(&repetitive));
    }
}
