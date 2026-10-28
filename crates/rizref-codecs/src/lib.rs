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
//! | GIF | `gif` | first frame only; animation is not a board feature |
//! | WebP | `image-webp` | lossy and lossless decode, lossless encode |
//! | BMP, TGA, PNM, QOI | `image` | trimmed to those four backends |
//! | ICO | `ico` | the largest embedded image wins |
//! | HEIF/HEIC/AVIF | none | recognised, reported as unsupported |
//!
//! The HEIF gap is deliberate and recorded in the phase-00 decisions: no mature
//! pure-Rust decoder exists today, and HEIC photography is rare on a reference
//! board compared with JPEG, PNG and WebP. A file in that family is recognised
//! by [`sniff`] and rejected with a message that says so, rather than being
//! misreported as a corrupt file.

#![deny(unsafe_code)]

pub mod animation;
pub mod buffer;
pub mod decode;
pub mod encode;
pub mod error;
mod exif;
pub mod sniff;

pub use animation::{
    AnimationFrame, MAX_FRAMES, MAX_TOTAL_PIXELS, MIN_FRAME_DELAY, decode_gif_animation,
};
pub use buffer::ImageBuffer;
pub use decode::{DecodeLimits, decode, decode_as, decode_as_with_limits, decode_with_limits};
pub use encode::{encode_jpeg, encode_png};
pub use error::CodecError;
pub use sniff::{ImageFormat, sniff};
