//! Errors reported by the `.rizref` container.
//!
//! Every failure mode a reader has to distinguish is its own variant: "this is
//! not a RizRef file" and "this RizRef file is damaged" call for different
//! messages and different recovery (in the first case, offer the open dialog
//! again; in the second, offer the autosave copy).

use std::error::Error;
use std::fmt;

/// What can go wrong while reading or writing a `.rizref` file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FormatError {
    /// The file ends before the bytes being read.
    Truncated {
        /// Bytes the operation needed.
        needed: usize,
        /// Bytes that were actually available.
        available: usize,
    },
    /// The magic bytes do not identify a `.rizref` file.
    BadMagic {
        /// The bytes that were found instead.
        found: [u8; 8],
    },
    /// The file was written by a container revision this build cannot read.
    UnsupportedVersion {
        /// Revision found in the header.
        found: u16,
        /// Highest revision this build understands.
        supported: u16,
    },
    /// The header's integrity check does not match its own contents.
    IntegrityMismatch,
    /// A chunk carries a kind code this build does not know.
    UnknownChunkKind {
        /// The code found in the payload.
        code: u16,
    },
    /// A chunk is larger than this build is willing to allocate for.
    ChunkTooLarge {
        /// Size the chunk claims, in bytes.
        size: usize,
        /// Largest size this build accepts, in bytes.
        limit: usize,
    },
    /// A compressed chunk could not be decompressed.
    Decompression {
        /// What the decompressor reported.
        reason: String,
    },
    /// The metadata chunk is not valid UTF-8 JSON.
    Metadata {
        /// What the JSON parser reported.
        reason: String,
    },
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { needed, available } => write!(
                formatter,
                "file is truncated: {needed} bytes needed, {available} available"
            ),
            Self::BadMagic { found } => {
                write!(formatter, "not a RizRef board (magic bytes {found:02x?})")
            }
            Self::UnsupportedVersion { found, supported } => write!(
                formatter,
                "board uses container revision {found}, this build reads up to {supported}"
            ),
            Self::IntegrityMismatch => {
                formatter.write_str("header checksum does not match the header contents")
            }
            Self::UnknownChunkKind { code } => {
                write!(formatter, "unknown payload chunk kind {code}")
            }
            Self::ChunkTooLarge { size, limit } => write!(
                formatter,
                "chunk of {size} bytes exceeds the {limit} byte limit this build accepts"
            ),
            Self::Decompression { reason } => {
                write!(formatter, "cannot decompress chunk: {reason}")
            }
            Self::Metadata { reason } => write!(formatter, "cannot read scene metadata: {reason}"),
        }
    }
}

impl Error for FormatError {}

#[cfg(test)]
mod tests {
    use super::FormatError;

    #[test]
    fn every_variant_describes_itself() {
        let errors = [
            FormatError::Truncated {
                needed: 44,
                available: 3,
            },
            FormatError::BadMagic { found: [0; 8] },
            FormatError::UnsupportedVersion {
                found: 9,
                supported: 1,
            },
            FormatError::IntegrityMismatch,
            FormatError::UnknownChunkKind { code: 999 },
            FormatError::ChunkTooLarge {
                size: 1 << 40,
                limit: 1 << 30,
            },
            FormatError::Decompression {
                reason: "bad size prefix".to_owned(),
            },
            FormatError::Metadata {
                reason: "expected value".to_owned(),
            },
        ];

        for error in errors {
            let message = error.to_string();
            assert!(!message.is_empty());
            assert!(
                !message.contains("Truncated") && !message.contains("BadMagic"),
                "messages are for users, not for the type system: {message}"
            );
        }
    }

    #[test]
    fn truncation_reports_both_lengths() {
        let error = FormatError::Truncated {
            needed: 44,
            available: 12,
        };

        assert_eq!(
            error.to_string(),
            "file is truncated: 44 bytes needed, 12 available"
        );
    }
}
