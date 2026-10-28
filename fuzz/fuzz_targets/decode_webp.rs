#![no_main]

//! WebP only, lossy and lossless.

use libfuzzer_sys::fuzz_target;
use rizref_codecs::{ImageFormat, decode_as};

fuzz_target!(|data: &[u8]| {
    let _ = decode_as(data, ImageFormat::WebP);
});
