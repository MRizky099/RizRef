//! The error message box through `rfd` (see `platform::message_box`).

/// Shows a blocking error dialog with `message`, and logs it.
///
/// `rfd` reaches the desktop's dialog service (the xdg-desktop-portal /
/// `zenity` on Linux, `NSAlert` on macOS); where none answers, the message is
/// still in the log and on standard error.
pub fn alert(title: &str, message: &str) {
    tracing::error!(title, message, "alert");
    eprintln!("{title}: {message}");
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(title)
        .set_description(message)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}
