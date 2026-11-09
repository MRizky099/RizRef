//! Reads the raw `HTML Format` clipboard payload.
//!
//! `arboard` returns only the HTML *fragment*, which drops the `SourceURL`
//! header a browser writes next to it — and that header is what resolves a
//! relative `<img src>` and names the page when there is no image. This
//! module reads the whole `CF_HTML` block so `provenance::url_from_cf_html`
//! can parse it.
//!
//! # Unsafe code
//!
//! A clipboard read is four Win32 calls on a handle the clipboard owns; each
//! call site states its invariant. The rest of the application still denies
//! unsafe code.
#![allow(unsafe_code)]

use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::core::w;

use crate::provenance::MAX_HTML_BYTES;

/// How often to retry opening a clipboard another process holds briefly.
const OPEN_ATTEMPTS: u32 = 5;

/// The registered `HTML Format` id.
fn html_format() -> Option<u32> {
    // SAFETY: registers (or looks up) a process-wide format name.
    let id = unsafe { RegisterClipboardFormatW(w!("HTML Format")) };
    (id != 0).then_some(id)
}

/// Whether the clipboard currently offers `HTML Format`.
pub fn has_html() -> bool {
    // SAFETY: a query on a registered id; it opens nothing.
    html_format().is_some_and(|format| unsafe { IsClipboardFormatAvailable(format) }.is_ok())
}

/// The raw `CF_HTML` bytes, or `None` when absent, unreadable or larger than
/// [`MAX_HTML_BYTES`].
pub fn read_html() -> Option<Vec<u8>> {
    let format = html_format()?;
    let _open = OpenGuard::open()?;
    // SAFETY: the clipboard is open on this thread; the handle stays owned by
    // the clipboard and is only read while it is open.
    let handle = unsafe { GetClipboardData(format) }.ok()?;
    let global = HGLOBAL(handle.0);
    // SAFETY: a clipboard HGLOBAL; the size query does not lock it.
    let size = unsafe { GlobalSize(global) };
    if size == 0 || size > MAX_HTML_BYTES {
        return None;
    }
    // SAFETY: locking yields `size` readable bytes, unlocked below.
    let pointer = unsafe { GlobalLock(global) }.cast::<u8>();
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the lock guarantees `size` valid bytes until the unlock.
    let bytes = unsafe { std::slice::from_raw_parts(pointer, size) }.to_vec();
    // SAFETY: balances the lock above.
    let _ = unsafe { GlobalUnlock(global) };
    Some(bytes)
}

/// Keeps the clipboard open for one read and closes it on every path.
struct OpenGuard;

impl OpenGuard {
    fn open() -> Option<Self> {
        for attempt in 0..OPEN_ATTEMPTS {
            // SAFETY: opening without an owner window is allowed for reads.
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Some(Self);
            }
            std::thread::sleep(std::time::Duration::from_millis(5 * u64::from(attempt + 1)));
        }
        tracing::debug!("the clipboard stayed busy; no HTML source URL");
        None
    }
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        // SAFETY: balances the successful `OpenClipboard` in `open`.
        let _ = unsafe { CloseClipboard() };
    }
}
