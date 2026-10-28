//! Image decode and encode dispatch, backed only by pure-Rust codecs.
//!
//! Every decoder in this crate produces the same thing — an [`ImageBuffer`] of
//! 8-bit RGBA pixels — and every encoder consumes one. Normalising here, at the
//! edge, is what lets the rest of the application ignore which format a picture
//! arrived in: the canvas gets one pixel layout, one stride and one alpha
//! convention.
//!
//! # Choosing a backend
//!
//! Mature C libraries exist for all of these formats, which is exactly why the
//! reference application ended up statically linking ImageMagick, libpng,
//! FreeType and friends. RizRef's pure-Rust rule forces the narrower set below
//! instead:
//!
//! | Format | Backend | Notes |
//! |---|---|---|
//! | PNG | `png` | alpha, bit depths and interlacing preserved |
//! | JPEG | `zune-jpeg` / `jpeg-encoder` | decode and encode |
//! | GIF | `gif` | every frame, composed and timed ([`decode_gif_animation`]) |
//! | WebP | `image-webp` | lossy and lossless decode, lossless encode |
//! | BMP, TGA, PNM, QOI | `image` | trimmed to those four backends |
//! | ICO | `ico` | the largest embedded image wins |
//! | HEIF/HEIC | `heic-rs` | grid tiles, 10-bit, transforms; no C, no `unsafe` |
//! | AVIF | none here | recognised; the application bridges it to the OS codec |
//!
//! HEIC used to be this crate's one deliberate gap, because the only decoders
//! were a C library and an AGPL crate. `heic-rs` closed it: it implements the
//! container and the HEVC still-picture decoder in Rust, so HEIC is decoded here
//! like every other format and works on every platform this application builds
//! for. AVIF is the remaining gap, and it is a tooling problem rather than a
//! licensing one — the pure-Rust AV1 decoders need an external assembler to
//! build, so the application reads AVIF through the operating system instead.

#![deny(unsafe_code)]

pub mod animation;
pub mod buffer;
pub mod decode;
pub mod encode;
pub mod error;
mod exif;
pub mod heif;
pub mod sniff;

pub use animation::{
    AnimationFrame, MAX_FRAMES, MAX_TOTAL_PIXELS, MIN_FRAME_DELAY, decode_gif_animation,
};
pub use buffer::ImageBuffer;
pub use decode::{DecodeLimits, decode, decode_as, decode_as_with_limits, decode_with_limits};
pub use encode::{encode_jpeg, encode_png};
pub use error::CodecError;
pub use sniff::{ImageFormat, sniff};
