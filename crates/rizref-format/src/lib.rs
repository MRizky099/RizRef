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
//! ┌──────────────────── header, 64 bytes (see `header`) ───────────────────┐
//! │ magic "RIZREF\x01\x00" │ header_size │ revision │ writer │ scene id … │
//! ├─────────────────────── chunk stream (see `chunk`) ─────────────────────┤
//! │ NAME │ THMB │ DOC  │ BLOB* │ PREV* │ INDX │ END!                      │
//! └────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! * The **header** identifies the board (UUID v4), the container revision and
//!   the file's creation/modification times. It is fixed for a revision; new
//!   information goes into a chunk, never into the header.
//! * The **chunk stream** carries the display name, the board thumbnail, the
//!   scene document (JSON), one chunk per original image kept byte-for-byte,
//!   one small preview per asset, the board index and the footer.
//! * The **index** lists where the live chunks are and a digest of each. The
//!   **footer** is a fixed 80-byte block at the end of a clean file pointing at
//!   the last index; its presence is the clean-save marker.
//! * An **incremental save appends** new chunks, a new index and a new footer.
//!   The last valid index wins, so a crash halfway through an append leaves the
//!   previous generation loadable.
//!
//! # Reading
//!
//! [`BoardReader`] validates the header, resolves the footer or recovers the
//! last valid index, and then reads chunks on demand. [`read_summary`] returns
//! only the name and thumbnail (what a recents list or an Explorer thumbnail
//! provider needs), without parsing the document or touching image blobs.
//! [`load`] returns the complete scene.
//!
//! # Writing
//!
//! [`save`] writes a fresh file through a temporary and an atomic rename, or
//! appends to a clean file of the same board. It compacts automatically when
//! appended bytes exceed twice the live bytes. See [`writer`].
//!
//! # Integrity
//!
//! Digests are BLAKE3. BLAKE3 is built with its `pure` feature, so the crate
//! stays free of a C toolchain. [`checksum`] also provides the content
//! addressing used by asset blobs: an asset's id *is* the digest of its
//! original bytes, so identical images deduplicate without comparing pixels.

#![deny(unsafe_code)]

pub mod checksum;
pub mod chunk;
pub mod compression;
pub mod document;
pub mod error;
pub mod header;
pub mod index;
pub mod preview;
pub mod reader;
pub mod thumbnail;
pub mod writer;

pub use chunk::{CHUNK_HEADER_LEN, ChunkHeader, ChunkKind};
pub use document::{AssetOrigin, SCHEMA_VERSION, SceneDocument};
pub use error::FormatError;
pub use header::{FORMAT_VERSION, HEADER_LEN, Header, MAGIC};
pub use index::{BoardIndex, ChunkRef, Footer, INDEX_SCHEMA_VERSION};
pub use preview::Preview;
pub use reader::{
    BoardReader, BoardSummary, ChunkInfo, ChunkScan, LoadedBoard, VerifyReport, load, read_header,
    read_name, read_summary,
};
pub use writer::{AssetProvenance, SaveMode, SaveReport, SaveRequest, save};
