//! What differs on macOS from Linux: launching through `open`, and the
//! `register` command (associations come from the app bundle).
//!
//! The macOS-only integration — Metal, the open-document Apple Event,
//! `NSPasteboard` URLs, `⌘` shortcuts — is phase 08b.

pub mod launch;
pub mod register;
