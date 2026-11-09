//! The clipboard's HTML through `arboard` (`text/html` on X11 and Wayland,
//! `public.html` on macOS).

/// Whether the clipboard offers HTML.
pub fn has_html() -> bool {
    read_html().is_some()
}

/// The clipboard's HTML, capped like the Windows read.
pub fn read_html() -> Option<String> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let html = clipboard.get().html().ok()?;
    (html.len() <= crate::provenance::MAX_HTML_BYTES).then_some(html)
}
