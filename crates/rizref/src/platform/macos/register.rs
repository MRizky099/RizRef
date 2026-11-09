//! `rizref register` on macOS: file types are declared by the app bundle.
//!
//! Finder learns `.rizref` from `CFBundleDocumentTypes` and the exported
//! `com.rizref.board` type in `RizRef.app/Contents/Info.plist` the moment the
//! app is copied to a folder; there is no per-user registry to write. The
//! command therefore explains that and succeeds, so scripts that call it on
//! every OS keep working.

use std::process::ExitCode;

/// `rizref register` / `rizref unregister` (any arguments are accepted and
/// ignored).
pub fn run(register: bool, _args: &[String]) -> ExitCode {
    if register {
        println!(
            "On macOS, .rizref files are associated by RizRef.app itself: \
             copy it to Applications (or any folder) and Finder picks it up."
        );
    } else {
        println!("On macOS, delete RizRef.app to remove its .rizref association.");
    }
    ExitCode::SUCCESS
}
