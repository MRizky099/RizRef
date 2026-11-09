//! The source URL of a copied browser image, read from the clipboard's HTML.
//!
//! A browser that copies an image also writes the HTML around it; its
//! `<img src>` (resolved against the page's address where the OS provides one)
//! is the image's web address, recorded as provenance and never downloaded.
//!
//! * **Windows** reads the registered `HTML Format` (`CF_HTML`, with its
//!   `SourceURL` header) directly (`platform/windows/clipboard_html.rs`);
//! * **Linux and macOS** read `text/html` / `public.html` through `arboard`,
//!   which carries no page address, so only an absolute `http(s)` `src`
//!   counts.

#[cfg(windows)]
pub use super::windows::clipboard_html::has_html;

#[cfg(unix)]
pub use super::unix::clipboard_html::has_html;

/// The web address of the copied image, when the clipboard's HTML names one.
#[cfg(windows)]
pub fn source_url() -> Option<String> {
    super::windows::clipboard_html::read_html()
        .as_deref()
        .and_then(crate::provenance::url_from_cf_html)
}

/// The web address of the copied image, when the clipboard's HTML names one.
#[cfg(unix)]
pub fn source_url() -> Option<String> {
    super::unix::clipboard_html::read_html()
        .as_deref()
        .and_then(crate::provenance::url_from_plain_html)
}
