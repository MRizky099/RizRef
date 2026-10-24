//! The `.rizref` board container.
//!
//! `rizref-format` is the only crate that knows how a board is laid out on
//! disk. The application never writes scene files itself; it hands the format
//! crate a scene and gets bytes back, which keeps one definition of the layout
//! instead of one per call site.
//!
//! # Layout
//!
//! ```text
//! ┌────────────────────── header, 44 bytes (see `header`) ─────────────────┐
//! │ magic "RIZREF\0\x1a" │ version │ flags │ scene id │ payload len │ hash  │
//! ├──────────────────────── payload, `payload_len` bytes ──────────────────┤
//! │ chunk │ chunk │ chunk │ …                                              │
//! └────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! The payload is a sequence of chunks ([`chunk`]): the JSON metadata, the
//! serialized scene, one chunk per original image kept byte-for-byte, and the
//! embedded preview that lets a recent-boards list show a thumbnail without
//! opening the board. The chunk framing itself lands in phase 06; what phase 00
//! fixes is the header, the checksum and the vocabulary.
//!
//! Two decisions this crate encodes:
//!
//! * **The header validates itself.** A BLAKE3-derived checksum over the fixed
//!   fields means a corrupt or truncated file is rejected before any of its
//!   contents are trusted.
//! * **Integrity is not compression.** Checksums are computed over uncompressed
//!   bytes, so a chunk can be compared, copied or skipped without being decoded
//!   first — which is what makes an incremental save (phase 06) possible.

#![deny(unsafe_code)]

pub mod checksum;
pub mod chunk;
pub mod compression;
pub mod container;
pub mod error;
pub mod header;
pub mod metadata;
pub mod preview;

pub use container::{ContainerReader, ContainerWriter};
pub use error::FormatError;
pub use header::{FORMAT_VERSION, HEADER_LEN, Header, MAGIC};
pub use metadata::SceneMetadata;
pub use preview::Preview;
