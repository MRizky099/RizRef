#![no_main]

//! The `image`-backed families (BMP, TGA, PNM, QOI). The first byte picks the
//! backend so one target covers all four.

use libfuzzer_sys::fuzz_target;
use rizref_codecs::{ImageFormat, decode_as};

fuzz_target!(|data: &[u8]| {
    let Some(format) = data.first().map(|selector| match selector % 4 {
        0 => ImageFormat::Bmp,
        1 => ImageFormat::Tga,
        2 => ImageFormat::Pnm,
        _ => ImageFormat::Qoi,
    }) else {
        return;
    };
    let _ = decode_as(data.get(1..).unwrap_or_default(), format);
});
