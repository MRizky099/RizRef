//! BLAKE3 digests: corruption detection and content identity.
//!
//! Two uses, one hash:
//!
//! * **Corruption detection.** Every chunk's digest is stored beside it, so a
//!   damaged file is reported instead of rendered.
//! * **Content identity.** Hashes of original image bytes are the keys of the
//!   blob table, which is what lets a save copy unchanged images instead of
//!   re-encoding them (phase 06).
//!
//! BLAKE3 is built here with its `pure` feature, so the whole crate stays
//! dependency-free of C toolchains.

/// Length of a full digest in bytes.
pub const DIGEST_LEN: usize = 32;

/// Length of the short digest used where a full one would be overhead, such as
/// the fixed-size file header.
pub const CHECKSUM_LEN: usize = 8;

/// The full 32-byte BLAKE3 digest of `bytes`.
pub fn digest(bytes: &[u8]) -> [u8; DIGEST_LEN] {
    *blake3::hash(bytes).as_bytes()
}

/// The first [`CHECKSUM_LEN`] bytes of the digest of `bytes`.
pub fn checksum8(bytes: &[u8]) -> [u8; CHECKSUM_LEN] {
    let digest = digest(bytes);
    let mut short = [0_u8; CHECKSUM_LEN];
    short.copy_from_slice(&digest[..CHECKSUM_LEN]);
    short
}

/// Whether `bytes` still hashes to `expected`.
pub fn verify(bytes: &[u8], expected: &[u8; DIGEST_LEN]) -> bool {
    digest(bytes) == *expected
}

#[cfg(test)]
mod tests {
    use super::{CHECKSUM_LEN, DIGEST_LEN, checksum8, digest, verify};

    /// BLAKE3 of the empty input, from the BLAKE3 test vectors.
    const EMPTY_DIGEST: &str = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn empty_input_matches_the_blake3_test_vector() {
        assert_eq!(hex(&digest(b"")), EMPTY_DIGEST);
    }

    #[test]
    fn digests_have_the_documented_lengths() {
        assert_eq!(DIGEST_LEN, 32);
        assert_eq!(CHECKSUM_LEN, 8);
        assert_eq!(checksum8(b"board").len(), CHECKSUM_LEN);
    }

    #[test]
    fn the_short_checksum_is_a_prefix_of_the_full_digest() {
        let bytes = b"one original image, kept byte-for-byte";

        assert_eq!(&digest(bytes)[..CHECKSUM_LEN], &checksum8(bytes));
    }

    #[test]
    fn hashing_is_deterministic_and_content_sensitive() {
        let original = b"original bytes";
        let changed = b"original bytez";
        let expected = digest(original);

        assert_eq!(digest(original), expected, "same bytes, same digest");
        assert_ne!(digest(changed), expected, "one byte differs");
        assert!(verify(original, &expected));
        assert!(!verify(changed, &expected));
    }
}
