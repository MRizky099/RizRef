//! The system clipboard, for pasting reference images and text notes.
//!
//! Phase 00 checks only that the clipboard can be opened, because that is the
//! part with a failure mode a user notices: on a session without a desktop, or
//! while another application holds the clipboard open, `Clipboard::new` can
//! fail, and a board that cannot paste wants to say so rather than to ignore the
//! user's Ctrl+V silently. Reading and writing runs is phase 04 work.
//!
//! Image data goes through `rizref-codecs`: the platform layer stores PNG or BMP
//! rather than raw pixels, so the format handling stays in one crate.

/// Opens the system clipboard and immediately releases it.
///
/// Returns the platform's message on failure, which is what the startup summary
/// prints.
pub fn availability() -> Result<(), String> {
    match arboard::Clipboard::new() {
        Ok(_) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}
