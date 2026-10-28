#![no_main]

//! ICO only, including the multi-size directory walk.

use libfuzzer_sys::fuzz_target;
use rizref_codecs::{ImageFormat, decode_as};

fuzz_target!(|data: &[u8]| {
    let _ = decode_as(data, ImageFormat::Ico);
});
