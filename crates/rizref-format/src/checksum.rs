//! BLAKE3 digests: content identity and corruption detection.
//!
//! Two uses, one hash:
//!
//! * **Content identity.** The digest of an original image byte-stream is the
//!   asset id, the key of the blob table and the value the scene document
//!   references. Identical pictures deduplicate without comparing pixels.
//! * **Corruption detection.** The board index stores the digest of every
//!   chunk it references, so a damaged document, thumbnail or preview is
//!   reported instead of rendered, and `rizref inspect --verify` can check the
//!   whole file without trusting it.
//!
//! BLAKE3 is built here with its `pure` feature, so the whole crate stays
//! dependency-free of C toolchains.

/// Length of a full digest in bytes.
pub const DIGEST_LEN: usize = 32;

/// The full 32-byte BLAKE3 digest of `bytes`.
pub fn digest(bytes: &[u8]) -> [u8; DIGEST_LEN] {
    *blake3::hash(bytes).as_bytes()
}

/// Whether `bytes` still hashes to `expected`.
pub fn verify(bytes: &[u8], expected: &[u8; DIGEST_LEN]) -> bool {
    digest(bytes) == *expected
}

/// The 64 lowercase hex digits of a digest.
pub fn to_hex(digest: &[u8; DIGEST_LEN]) -> String {
    let mut out = String::with_capacity(DIGEST_LEN * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

/// Parses exactly 64 hex digits, accepting upper- and lowercase.
pub fn from_hex(text: &str) -> Option<[u8; DIGEST_LEN]> {
    let digits = text.as_bytes();
    if digits.len() != DIGEST_LEN * 2 {
        return None;
    }
    let mut raw = [0_u8; DIGEST_LEN];
    for (pair, slot) in digits.as_chunks::<2>().0.iter().zip(raw.iter_mut()) {
        *slot = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Some(raw)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{DIGEST_LEN, digest, from_hex, to_hex, verify};

    /// BLAKE3 of the empty input, from the BLAKE3 test vectors.
    const EMPTY_DIGEST: &str = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

    #[test]
    fn empty_input_matches_the_blake3_test_vector() {
        assert_eq!(to_hex(&digest(b"")), EMPTY_DIGEST);
    }

    #[test]
    fn digests_have_the_documented_length() {
        assert_eq!(DIGEST_LEN, 32);
        assert_eq!(to_hex(&digest(b"board")).len(), 64);
    }

    #[test]
    fn hex_round_trips_and_accepts_uppercase() {
        let raw = digest(b"one original image, kept byte-for-byte");

        let hex = to_hex(&raw);
        assert_eq!(from_hex(&hex), Some(raw));
        assert_eq!(from_hex(&hex.to_uppercase()), Some(raw));
        assert_eq!(from_hex(""), None);
        assert_eq!(from_hex(&"0".repeat(63)), None);
        assert_eq!(from_hex(&"z".repeat(64)), None);
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
