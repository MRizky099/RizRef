#![no_main]

//! The note markdown parser: math pre-pass, `<span style>` handling and the
//! pulldown-cmark fold. The application crate is a binary, so the two pure
//! modules are compiled in directly from their source files.
//!
//! Output is bounded by construction (a copy of the input plus one entry per
//! event), so the only property checked is "never panics"; the size
//! assertion keeps a pathological blow-up from passing silently.

use libfuzzer_sys::fuzz_target;

#[allow(dead_code, missing_docs, clippy::all)]
mod markdown {
    #[path = "../../../crates/rizref/src/markdown/style.rs"]
    pub mod style;
    #[path = "../../../crates/rizref/src/markdown/parse.rs"]
    pub mod parse;
}

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let document = markdown::parse::parse(source);
    assert!(document.math.len() <= markdown::parse::MAX_EQUATIONS);
    let (text, _) = markdown::parse::extract_math(source);
    assert!(text.len() <= source.len() + 4 * markdown::parse::MAX_EQUATIONS);
});
