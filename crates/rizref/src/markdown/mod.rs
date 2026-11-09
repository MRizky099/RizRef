//! Markdown notes: parsing, styling, math and layout.
//!
//! The pipeline is `parse` (source → [`parse::Document`]) → `layout`
//! (document + wrap width → [`layout::NoteLayout`] in world units) → paint
//! (in `note.rs`, layout + palette + pixel density → pixels). Each stage is
//! pure and testable without a window; only the paint stage knows about
//! textures.

pub mod layout;
pub mod math;
pub mod parse;
pub mod style;
