//! The native message box: the one channel a windowed application has for
//! reporting that it died.
//!
//! Used by the panic hook and, later, by fatal startup errors. Everything else
//! goes through `tracing` — a message box is a last resort, not a logger.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONERROR, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_YESNO, MESSAGEBOX_STYLE,
    MessageBoxW,
};
use windows::core::PCWSTR;

/// Encodes a string as a NUL-terminated UTF-16 buffer for a wide API.
///
/// Windows strings are UTF-16; this is the standard conversion, lossless for
/// any Rust `String` (invalid surrogates cannot occur in one).
fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Shows a modal native message box and returns once the user dismisses it.
///
/// Modal is the right shape for a panic or a fatal startup error: there is
/// nothing behind it to keep running.
// SAFETY: `MessageBoxW` takes pointers only to the two NUL-terminated buffers
// built above, which outlive the call, and a null owner window. It has no
// thread-affinity requirements beyond being on a GUI-capable thread, which the
// panic path satisfies (winit's event loop thread).
#[allow(unsafe_code)]
fn show(title: &str, message: &str) {
    // The error icon is the only severity the application reports today;
    // `warn` joins it when a non-fatal dialog has a caller.
    let flags: MESSAGEBOX_STYLE = MB_OK | MB_SETFOREGROUND | MB_ICONERROR;
    // Ownership of the buffers stays here; the pointers are valid for the
    // duration of the blocking call.
    let text = wide(message);
    let caption = wide(title);
    unsafe {
        MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(caption.as_ptr()), flags);
    }
}

/// Asks a yes/no question; `true` for Yes.
///
/// Used once per launch, by the crash-recovery prompt, before the board is
/// interactive — the one moment a modal native box is the right shape.
// SAFETY: as for `show`: two NUL-terminated buffers that outlive the call and
// a null owner window.
#[allow(unsafe_code)]
pub fn confirm(title: &str, message: &str) -> bool {
    let flags: MESSAGEBOX_STYLE = MB_YESNO | MB_SETFOREGROUND | MB_ICONWARNING;
    let text = wide(message);
    let caption = wide(title);
    let answer =
        unsafe { MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(caption.as_ptr()), flags) };
    answer == IDYES
}

/// Reports an error that ends the process.
pub fn alert(title: &str, message: &str) {
    show(title, message);
}

#[cfg(test)]
mod tests {
    use super::wide;

    #[test]
    fn wide_strings_are_nul_terminated_utf16() {
        let encoded = wide("RizRef");
        assert_eq!(
            &encoded[..6],
            &"RizRef".encode_utf16().collect::<Vec<u16>>()
        );
        assert_eq!(*encoded.last().expect("non-empty"), 0);
    }

    #[test]
    fn wide_handles_the_empty_string() {
        assert_eq!(wide(""), vec![0]);
    }
}
