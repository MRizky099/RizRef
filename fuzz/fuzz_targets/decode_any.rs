#![no_main]

//! Every recognisable format, dispatched by signature.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = rizref_codecs::decode(data);
});
