#![no_main]

//! The note markdown parser: math pre-pass, `<span style>` handling and the
//! pulldown-cmark fold. The application crate is a binary, so the two pure
//! modules are compiled in directly from their source files.
//!
//! Output is bounded by construction (a copy of the input plus one entry per
//! event), so the only property checked is "never panics"; the size
//! assertion keeps a pathological blow-up from passing silently.

use libfuzzer_sys::fuzz_target;

// Declared at the crate root so each `#[path]` resolves from `fuzz_targets/`
// (a path inside an inline module would go through a `markdown/` directory that
// does not exist, which Linux cannot step back out of). `parse.rs` refers to
// `super::style`, which is this crate root's `style`.
#[allow(dead_code, missing_docs, clippy::all)]
#[path = "../../crates/rizref/src/markdown/parse.rs"]
mod parse;
#[allow(dead_code, missing_docs, clippy::all)]
#[path = "../../crates/rizref/src/markdown/style.rs"]
mod style;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let document = parse::parse(source);
    assert!(document.math.len() <= parse::MAX_EQUATIONS);
    let (text, _) = parse::extract_math(source);
    assert!(text.len() <= source.len() + 4 * parse::MAX_EQUATIONS);
});
