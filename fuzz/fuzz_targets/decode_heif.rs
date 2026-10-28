#![no_main]

//! HEIF/HEIC only, through the `heic-rs`-backed decoder.
//!
//! This is a young container+codec implementation, so it gets its own target
//! rather than riding along in `decode_any`: a crash here is a finding about
//! the HEVC path specifically, not about dispatch.

use libfuzzer_sys::fuzz_target;
use rizref_codecs::{DecodeLimits, ImageFormat, decode_as_with_limits};

fuzz_target!(|data: &[u8]| {
    let _ = decode_as_with_limits(data, ImageFormat::Heif, DecodeLimits::default());
});
