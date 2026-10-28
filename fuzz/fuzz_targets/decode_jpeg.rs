#![no_main]

//! JPEG only, including EXIF orientation parsing.

use libfuzzer_sys::fuzz_target;
use rizref_codecs::{ImageFormat, decode_as};

fuzz_target!(|data: &[u8]| {
    let _ = decode_as(data, ImageFormat::Jpeg);
});
