//! The ISOBMFF image families (AVIF, and HEIC files `heic-rs` cannot decode)
//! through the OS's image codecs.
//!
//! On Windows that is Windows Imaging Component (`platform/windows/imaging.rs`).
//! Linux and macOS 10.13 have no system AVIF decoder RizRef can reach without C
//! (ImageIO's AVIF arrived in macOS 13), so there `decode` reports the format
//! as unsupported on this OS, with the message phase 03 defined. Whether AVIF
//! gets a pure-Rust decoder on every OS is an open phase-08 decision.

#[cfg(windows)]
pub use super::windows::imaging::decode;

#[cfg(unix)]
pub use super::unix::imaging::decode;
