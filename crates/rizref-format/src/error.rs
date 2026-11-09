//! Errors reported by the `.rizref` container.
//!
//! Every failure mode a reader has to distinguish is its own variant: "this is
//! not a RizRef file", "this RizRef file is damaged", "this file was written by
//! a newer build" and "the linked image this board points at is gone" call for
//! different messages and different recovery (offer the open dialog again, offer
//! the recovered board, ask to update RizRef, or run the relink flow).
//!
//! Messages are written for the person reading the title bar or a log line, not
//! for the type system: no variant name appears in its own text.

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
    /// The header is structurally invalid.
    BadHeader {
        /// What is wrong with it.
        reason: String,
    },
    /// The file was written by a container revision this build cannot read.
    UnsupportedVersion {
        /// Revision found in the header.
        found: u16,
        /// Highest revision this build understands.
        supported: u16,
    },
    /// The scene document uses a schema revision this build cannot migrate.
    UnsupportedSchema {
        /// Revision found in the document.
        found: u16,
        /// Highest revision this build understands.
        supported: u16,
    },
    /// A chunk frame is structurally invalid.
    BadChunk {
        /// What is wrong with it.
        reason: String,
    },
    /// A chunk names a compression codec this build does not know.
    UnknownCompression {
        /// The code found in the chunk header.
        code: u8,
    },
    /// A chunk is larger than this build is willing to allocate for.
    ChunkTooLarge {
        /// Size the chunk claims, in bytes.
        size: u64,
        /// Largest size this build accepts, in bytes.
        limit: u64,
    },
    /// A compressed chunk could not be decompressed.
    Decompression {
        /// What the decompressor reported.
        reason: String,
    },
    /// A digest stored beside a chunk does not match its contents.
    IntegrityMismatch {
        /// What was being checked, for example `scene document`.
        subject: &'static str,
    },
    /// The scene document is not valid JSON for the current schema.
    Document {
        /// What the JSON parser or the conversion reported.
        reason: String,
    },
    /// The board index is missing, unreadable or refers outside the file.
    Index {
        /// What the parser or the bounds check reported.
        reason: String,
    },
    /// A clean board has no scene document at all.
    MissingDocument,
    /// A document names an asset whose embedded bytes are absent.
    MissingBlob {
        /// Hex content hash of the missing asset.
        hash: String,
    },
    /// The thumbnail or preview bytes could not be encoded or decoded.
    Preview {
        /// What the codec reported.
        reason: String,
    },
    /// A file operation failed.
    Io {
        /// What was being attempted, for example `read`.
        operation: &'static str,
        /// Path involved, when there is one.
        path: String,
        /// The operating-system message.
        reason: String,
    },
    /// The target is read-only, or another process holds it locked: a save
    /// elsewhere (Save As) is the way forward, not a retry.
    Denied {
        /// What was being attempted, for example `replace`.
        operation: &'static str,
        /// Path involved.
        path: String,
        /// Why, in words.
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
            Self::BadHeader { reason } => write!(formatter, "invalid board header: {reason}"),
            Self::UnsupportedVersion { found, supported } => write!(
                formatter,
                "board uses container revision {found}, this build reads up to {supported}"
            ),
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "board uses document schema {found}, this build reads up to {supported}"
            ),
            Self::BadChunk { reason } => write!(formatter, "invalid chunk: {reason}"),
            Self::UnknownCompression { code } => {
                write!(formatter, "chunk uses unknown compression code {code}")
            }
            Self::ChunkTooLarge { size, limit } => write!(
                formatter,
                "chunk of {size} bytes exceeds the {limit} byte limit this build accepts"
            ),
            Self::Decompression { reason } => {
                write!(formatter, "cannot decompress chunk: {reason}")
            }
            Self::IntegrityMismatch { subject } => {
                write!(formatter, "{subject} does not match its checksum")
            }
            Self::Document { reason } => write!(formatter, "cannot read scene document: {reason}"),
            Self::Index { reason } => write!(formatter, "cannot read board index: {reason}"),
            Self::MissingDocument => formatter.write_str("board has no scene document"),
            Self::MissingBlob { hash } => write!(
                formatter,
                "the image with content hash {hash} is missing from the board"
            ),
            Self::Preview { reason } => write!(formatter, "cannot handle preview image: {reason}"),
            Self::Io {
                operation,
                path,
                reason,
            }
            | Self::Denied {
                operation,
                path,
                reason,
            } => {
                if path.is_empty() {
                    write!(formatter, "cannot {operation}: {reason}")
                } else {
                    write!(formatter, "cannot {operation} {path}: {reason}")
                }
            }
        }
    }
}

impl Error for FormatError {}

impl FormatError {
    /// Whether the failure was a refused write — read-only file or folder, or
    /// a lock held by another program — which Save As can route around.
    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Denied { .. })
    }
}

impl FormatError {
    /// Wraps an I/O error with the operation and path it belongs to. A
    /// permission error becomes [`FormatError::Denied`].
    pub(crate) fn io(
        operation: &'static str,
        path: impl Into<String>,
        error: std::io::Error,
    ) -> Self {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            return Self::Denied {
                operation,
                path: path.into(),
                reason: format!("{error} (read-only, or open in another program)"),
            };
        }
        Self::Io {
            operation,
            path: path.into(),
            reason: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FormatError;

    #[test]
    fn every_variant_describes_itself() {
        let errors = [
            FormatError::Truncated {
                needed: 64,
                available: 3,
            },
            FormatError::BadMagic { found: [0; 8] },
            FormatError::BadHeader {
                reason: "header_size 0".to_owned(),
            },
            FormatError::UnsupportedVersion {
                found: 9,
                supported: 1,
            },
            FormatError::UnsupportedSchema {
                found: 3,
                supported: 1,
            },
            FormatError::BadChunk {
                reason: "lengths disagree".to_owned(),
            },
            FormatError::UnknownCompression { code: 7 },
            FormatError::ChunkTooLarge {
                size: 1 << 40,
                limit: 1 << 30,
            },
            FormatError::Decompression {
                reason: "bad size prefix".to_owned(),
            },
            FormatError::IntegrityMismatch {
                subject: "scene document",
            },
            FormatError::Document {
                reason: "expected value".to_owned(),
            },
            FormatError::Index {
                reason: "not json".to_owned(),
            },
            FormatError::MissingDocument,
            FormatError::MissingBlob {
                hash: "ab".repeat(32),
            },
            FormatError::Preview {
                reason: "unknown image format".to_owned(),
            },
            FormatError::Io {
                operation: "read",
                path: "board.rizref".to_owned(),
                reason: "access denied".to_owned(),
            },
        ];

        for error in errors {
            let message = error.to_string();
            assert!(!message.is_empty());
            assert!(
                !message.contains("Truncated")
                    && !message.contains("BadMagic")
                    && !message.contains("UnsupportedSchema"),
                "messages are for users, not for the type system: {message}"
            );
        }
    }

    #[test]
    fn truncation_reports_both_lengths() {
        let error = FormatError::Truncated {
            needed: 64,
            available: 12,
        };

        assert_eq!(
            error.to_string(),
            "file is truncated: 64 bytes needed, 12 available"
        );
    }

    #[test]
    fn an_io_error_names_the_file_when_it_has_one() {
        let with_path = FormatError::Io {
            operation: "write",
            path: "C:/boards/a.rizref".to_owned(),
            reason: "disk full".to_owned(),
        };
        let without_path = FormatError::Io {
            operation: "write",
            path: String::new(),
            reason: "disk full".to_owned(),
        };

        assert_eq!(
            with_path.to_string(),
            "cannot write C:/boards/a.rizref: disk full"
        );
        assert_eq!(without_path.to_string(), "cannot write: disk full");
    }
}
