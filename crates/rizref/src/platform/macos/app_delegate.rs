//! Finder's open-document requests and Dock/menu quits, on winit's
//! `NSApplicationDelegate`.
//!
//! A double-clicked `.rizref` reaches a Mac app as an Apple Event, not as a
//! command-line argument — both when the app is launched for it and when it
//! is already running — and AppKit delivers it as
//! `-[NSApplicationDelegate application:openURLs:]`. winit 0.30 does not
//! implement that method, so this module adds it (and
//! `applicationShouldTerminate:`) to winit's delegate class,
//! `WinitApplicationDelegate`, with the Objective-C runtime's
//! `class_addMethod`. That happens right after the event loop is built —
//! the delegate class exists from then — and before `-[NSApp run]`, which
//! is when AppKit delivers the documents a launch was asked to open.
//!
//! `applicationShouldTerminate:` answers *cancel* and asks the event loop to
//! close instead, so Quit from the Dock or ⌘Q in a system menu goes through
//! the same unsaved-changes path as RizRef's own Quit.
//!
//! The handlers allocate nothing on the Objective-C side; they turn the URLs
//! into file paths and hand them to the sinks installed here, which post an
//! event to the winit loop.

use std::ffi::{CStr, c_char};
use std::sync::OnceLock;

use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{ffi, sel};
use objc2_foundation::{NSArray, NSURL};

/// The class winit registers for its application delegate.
const WINIT_DELEGATE: &CStr = c"WinitApplicationDelegate";

/// `NSTerminateCancel`.
const TERMINATE_CANCEL: usize = 0;

/// `-application:openURLs:` as a C function.
type OpenUrlsFn =
    unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSURL>);
/// `-applicationShouldTerminate:` as a C function.
type ShouldTerminateFn = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize;

type PathSink = Box<dyn Fn(Vec<String>) + Send + Sync>;
type QuitSink = Box<dyn Fn() + Send + Sync>;

static OPEN: OnceLock<PathSink> = OnceLock::new();
static QUIT: OnceLock<QuitSink> = OnceLock::new();

/// Installs the handlers. Call once, on the main thread, after the winit
/// event loop is built and before it runs; later calls keep the first sinks.
pub fn install(
    on_open: impl Fn(Vec<String>) + Send + Sync + 'static,
    on_quit: impl Fn() + Send + Sync + 'static,
) {
    let _ = OPEN.set(Box::new(on_open));
    let _ = QUIT.set(Box::new(on_quit));
    let Some(class) = AnyClass::get(WINIT_DELEGATE) else {
        tracing::warn!("winit's application delegate class is missing; Finder opens are off");
        return;
    };
    // SAFETY: each function has the C ABI and exactly the signature its
    // type encoding states (`v@:@@`: void, self, _cmd, NSApplication*,
    // NSArray*; `Q@:@`: NSUInteger, self, _cmd, NSApplication*); the
    // transmute only erases that signature to `Imp`, as `class_addMethod`
    // requires. Adding a method to a registered class is allowed by the
    // runtime; it fails (and is logged) if the class already has one.
    #[allow(unsafe_code)]
    unsafe {
        let open: OpenUrlsFn = open_urls;
        let quit: ShouldTerminateFn = should_terminate;
        add(
            class,
            sel!(application:openURLs:),
            std::mem::transmute::<OpenUrlsFn, Imp>(open),
            c"v@:@@",
        );
        add(
            class,
            sel!(applicationShouldTerminate:),
            std::mem::transmute::<ShouldTerminateFn, Imp>(quit),
            c"Q@:@",
        );
    }
}

/// `class_addMethod`, logged.
#[allow(unsafe_code)]
unsafe fn add(class: &AnyClass, name: Sel, imp: Imp, types: &CStr) {
    let class = std::ptr::from_ref(class).cast_mut();
    let types: *const c_char = types.as_ptr();
    // SAFETY: forwarded from `install`.
    let added = unsafe { ffi::class_addMethod(class, name, imp, types) };
    if added.as_bool() {
        tracing::debug!(selector = %name.name().to_string_lossy(), "app delegate method added");
    } else {
        tracing::warn!(selector = %name.name().to_string_lossy(), "app delegate already answers");
    }
}

/// `-application:openURLs:`.
#[allow(unsafe_code)]
unsafe extern "C-unwind" fn open_urls(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
    urls: *mut NSArray<NSURL>,
) {
    // SAFETY: AppKit passes a valid `NSArray<NSURL *>` that lives for the
    // duration of the call; `as_ref` maps a null pointer to `None`.
    let Some(urls) = (unsafe { urls.as_ref() }) else {
        return;
    };
    let paths = file_paths(urls);
    tracing::info!(count = paths.len(), "Finder asked to open documents");
    if !paths.is_empty()
        && let Some(sink) = OPEN.get()
    {
        sink(paths);
    }
}

/// `-applicationShouldTerminate:`: cancel, and let RizRef close itself.
#[allow(unsafe_code)]
unsafe extern "C-unwind" fn should_terminate(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
) -> usize {
    tracing::info!("the system asked RizRef to quit");
    if let Some(sink) = QUIT.get() {
        sink();
    }
    TERMINATE_CANCEL
}

/// The file-system paths among `urls`; other schemes are ignored.
pub(crate) fn file_paths(urls: &NSArray<NSURL>) -> Vec<String> {
    urls.iter()
        .filter(|url| url.isFileURL())
        .filter_map(|url| url.path())
        .map(|path| path.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use objc2_foundation::{NSArray, NSString, NSURL};

    #[test]
    fn only_file_urls_become_paths() {
        let file = NSURL::fileURLWithPath(&NSString::from_str("/Users/u/My Refs/a b.rizref"));
        let web = NSURL::URLWithString(&NSString::from_str("https://example.com/x.rizref"))
            .expect("a valid URL");
        let urls = NSArray::from_retained_slice(&[file, web]);
        assert_eq!(
            super::file_paths(&urls),
            vec!["/Users/u/My Refs/a b.rizref".to_owned()]
        );
    }
}
