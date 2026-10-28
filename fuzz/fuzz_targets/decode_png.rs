#![no_main]

//! PNG only, including palette, 16-bit and interlaced variants.

use libfuzzer_sys::fuzz_target;
use rizref_codecs::{ImageFormat, decode_as};

fuzz_target!(|data: &[u8]| {
    let _ = decode_as(data, ImageFormat::Png);
});
