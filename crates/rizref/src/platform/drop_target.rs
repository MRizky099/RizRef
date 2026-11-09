//! The OLE drop target: file drops from Explorer and browsers, plus virtual
//! files (Outlook attachments, zip archives opened by Explorer).
//!
//! winit registers its own `IDropTarget` for the window, but it understands
//! only `CF_HDROP` — a list of file-system paths. Dragging an attachment out of
//! Outlook, or a file out of a compressed folder, presents the contents as
//! **virtual files**: a `FileGroupDescriptorW` naming each entry and a
//! `FileContents` stream per index, with no path on disk at all. This module
//! implements the whole target so both shapes work.
//!
//! # Owning the window's drop slot
//!
//! OLE allows one target per window, so the registration here replaces winit's
//! (`RevokeDragDrop` then `RegisterDragDrop`). That means this module must keep
//! handling `CF_HDROP` itself, which it does; winit's `DroppedFile` events stop
//! firing on Windows and the app's drop path is this one. If OLE cannot be
//! initialised at all, the function returns `None` **before** revoking anything,
//! so winit's handler stays in place.
//!
//! # Threading
//!
//! COM calls arrive on the event loop's UI thread. Reading a virtual file's
//! contents has to finish before `Drop` returns (the stream belongs to the data
//! object), so that copy happens on the UI thread; decoding, hashing and
//! preview generation still happen on the import workers afterwards.
//!
//! # Unsafe code
//!
//! This is the crate's COM boundary: every pointer here belongs to OLE, and the
//! `#[implement]` macro expands to vtable plumbing whose spans are attributed to
//! the macro, so the allowance is module-wide rather than per item. The rest of
//! the application still denies unsafe code; the safety notes on each call site
//! state the invariant that makes it sound.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HGLOBAL, HWND, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, TYMED, TYMED_HGLOBAL, TYMED_ISTREAM,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
    OleInitialize, OleUninitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::core::{PCWSTR, Ref, Result as WinResult, implement, w};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::provenance::{self, DropUrlPayloads};

/// Largest `FileGroupDescriptorW` block accepted, bytes. The block is four
/// bytes plus one 592-byte descriptor per file, so this holds thousands.
const MAX_GROUP_BYTES: usize = 4 * 1024 * 1024;

/// Largest single virtual file accepted, bytes. The bytes are copied on the UI
/// thread, so the cap is what keeps a pathological drop from stalling the loop.
const MAX_VIRTUAL_FILE_BYTES: usize = 512 * 1024 * 1024;

/// `FILEDESCRIPTORW` on the wire: it is declared packed, and these offsets are
/// what the shell actually writes.
const DESCRIPTOR_LEN: usize = 592;
/// Offset of `cFileName` inside a descriptor.
const DESCRIPTOR_NAME_OFFSET: usize = 72;
/// `cFileName` capacity in UTF-16 units (`MAX_PATH`).
const DESCRIPTOR_NAME_UNITS: usize = 260;
/// `FD_FILESIZE`: the descriptor carries `nFileSizeHigh`/`nFileSizeLow`.
const FD_FILESIZE: u32 = 0x40;

/// Largest registered `PNG` payload accepted, bytes; read on the UI thread
/// like a virtual file.
const MAX_PNG_BYTES: usize = MAX_VIRTUAL_FILE_BYTES;

/// One file delivered by a drag & drop, either as a path or as bytes.
#[derive(Debug, Clone)]
pub struct DropEvent {
    /// Drop position in the window's client area, physical pixels.
    ///
    /// Physical because that is what OLE and `ScreenToClient` speak; the
    /// application divides by the window's scale factor before touching the
    /// camera.
    pub client_physical: (f32, f32),
    /// Files that exist on disk.
    pub paths: Vec<PathBuf>,
    /// Files that only exist inside the drag source.
    pub virtual_files: Vec<VirtualFile>,
    /// The web address of the dragged image, when the source exposed one
    /// and the drop carries exactly one image. A browser drag that also
    /// produced a temporary cache file reports both; the importer records
    /// the URL and not the cache path.
    pub source_url: Option<String>,
}

/// A file whose contents arrived without a path, such as an Outlook attachment.
#[derive(Debug, Clone)]
pub struct VirtualFile {
    /// Name the drag source gave the file, used as its import label.
    pub name: String,
    /// The contents, exactly as delivered.
    pub bytes: Arc<[u8]>,
}

/// A registered drop target; dropping this value unregisters it.
///
/// Held by the application for as long as the window lives. The field must be
/// dropped before the window so `RevokeDragDrop` still has a live `HWND`.
pub struct Registration {
    hwnd: HWND,
    target: Option<IDropTarget>,
}

/// Registers this process as the window's OLE drop target.
///
/// Returns `None` when the window is not a Win32 window, OLE cannot be
/// initialised, or the registration is refused; in the first two cases winit's
/// own drop handler is left untouched.
pub fn register(
    window: &Window,
    sink: impl Fn(DropEvent) + Send + Sync + 'static,
) -> Option<Registration> {
    let raw = window.window_handle().ok()?.as_raw();
    let RawWindowHandle::Win32(handle) = raw else {
        tracing::warn!("the window is not a Win32 window; drag & drop is unavailable");
        return None;
    };
    register_hwnd(HWND(handle.hwnd.get() as *mut c_void), Arc::new(sink))
}

/// The registration proper, split out so the `HWND` arrives as a plain value.
fn register_hwnd(hwnd: HWND, sink: Arc<dyn Fn(DropEvent) + Send + Sync>) -> Option<Registration> {
    // SAFETY: called on the event loop thread, which owns the window. OLE is
    // reference counted, so balancing this with `OleUninitialize` on drop is
    // correct even when another component initialised it first.
    if let Err(error) = unsafe { OleInitialize(None) } {
        tracing::warn!(
            %error,
            "OLE could not initialise; keeping winit's file drop handler"
        );
        return None;
    }

    // SAFETY: the window handle is valid and owned by this thread. Revoking a
    // slot nobody registered is an error the result discards.
    let _ = unsafe { RevokeDragDrop(hwnd) };

    let target: IDropTarget = OleDropTarget {
        sink,
        hwnd: hwnd.0 as isize,
        hovering: AtomicBool::new(false),
    }
    .into();

    // SAFETY: the target is a live COM object and the handle names this thread's
    // window.
    if let Err(error) = unsafe { RegisterDragDrop(hwnd, &target) } {
        tracing::warn!(
            %error,
            "cannot register the OLE drop target; drag & drop is unavailable until restart"
        );
        // SAFETY: balances the successful `OleInitialize` above.
        unsafe { OleUninitialize() };
        return None;
    }

    tracing::info!("OLE drop target registered");
    Some(Registration {
        hwnd,
        target: Some(target),
    })
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: this registration is the one made in `register_hwnd`, and the
        // application drops it while the window still exists.
        unsafe {
            let _ = RevokeDragDrop(self.hwnd);
            // Release our reference before tearing the apartment down.
            self.target.take();
            OleUninitialize();
        }
    }
}

// ---------------------------------------------------------------------------
// The COM object
// ---------------------------------------------------------------------------

/// The window's drop target.
#[implement(IDropTarget)]
struct OleDropTarget {
    /// Delivers a completed drop to the application.
    sink: Arc<dyn Fn(DropEvent) + Send + Sync>,
    /// Raw `HWND` value: `HWND` itself is a pointer type, and the COM object
    /// must be `Send + Sync` to cross apartments.
    hwnd: isize,
    /// Whether the drag currently in the window carries something we accept.
    hovering: AtomicBool,
}

impl IDropTarget_Impl for OleDropTarget_Impl {
    fn DragEnter(
        &self,
        pdataobj: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        let accepts = pdataobj.as_ref().is_some_and(accepts_drop);
        self.hovering.store(accepts, Ordering::Relaxed);
        // SAFETY: OLE passes a valid out-parameter for the effect.
        unsafe { *effect = effect_for(accepts) };
        if accepts {
            tracing::debug!("drag entered with an acceptable payload");
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        // SAFETY: OLE passes a valid out-parameter for the effect.
        unsafe { *effect = effect_for(self.hovering.load(Ordering::Relaxed)) };
        Ok(())
    }

    fn DragLeave(&self) -> WinResult<()> {
        self.hovering.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> WinResult<()> {
        self.hovering.store(false, Ordering::Relaxed);
        let Some(data) = pdataobj.as_ref() else {
            // SAFETY: valid out-parameter.
            unsafe { *effect = DROPEFFECT_NONE };
            return Ok(());
        };

        let paths = hdrop_paths(data);
        let mut virtual_files = if paths.is_empty() {
            virtual_files(data)
        } else {
            Vec::new()
        };
        let url = provenance::choose_drop_url(&url_payloads(data));
        if paths.is_empty()
            && virtual_files.is_empty()
            && let Some(png) = png_payload(data)
        {
            virtual_files.push(VirtualFile {
                name: provenance::png_name_from_url(url.as_deref()),
                bytes: Arc::from(png),
            });
        }
        let source_url = attach_url(paths.len() + virtual_files.len(), url);

        if paths.is_empty() && virtual_files.is_empty() {
            tracing::debug!("dropped data carried no files this build can import");
            // SAFETY: valid out-parameter.
            unsafe { *effect = DROPEFFECT_NONE };
            return Ok(());
        }

        // SAFETY: valid out-parameter.
        unsafe { *effect = DROPEFFECT_COPY };
        (self.sink)(DropEvent {
            client_physical: client_point(self.hwnd, pt),
            paths,
            virtual_files,
            source_url,
        });
        Ok(())
    }
}

/// The cursor effect for an accepted or refused drag.
fn effect_for(accepts: bool) -> DROPEFFECT {
    if accepts {
        DROPEFFECT_COPY
    } else {
        DROPEFFECT_NONE
    }
}

/// Whether the data object offers anything this build imports.
///
/// A URL-only drag (a link, or a page selection) is refused with "no drop":
/// it would need a download, which this build never does on drop.
fn accepts_drop(data: &IDataObject) -> bool {
    has_format(data, CF_HDROP.0)
        || file_group_format().is_some_and(|format| has_format(data, format))
        || register_format(w!("PNG")).is_some_and(|format| has_format(data, format))
}

/// The URL to report for a drop of `images` importable entries: only an
/// unambiguous single-image drop gets one.
fn attach_url(images: usize, url: Option<String>) -> Option<String> {
    if images == 1 { url } else { None }
}

/// The registered `PNG` payload, for a drag that offers encoded pixels but
/// neither a file nor a virtual file.
fn png_payload(data: &IDataObject) -> Option<Vec<u8>> {
    let format = register_format(w!("PNG"))?;
    let medium = MediumGuard(get_medium(data, format, TYMED_HGLOBAL, -1)?);
    // SAFETY: a TYMED_HGLOBAL medium stores its payload in `u.hGlobal`.
    global_bytes(unsafe { medium.0.u.hGlobal }, MAX_PNG_BYTES)
}

/// Reads every URL-bearing format the data object offers.
fn url_payloads(data: &IDataObject) -> DropUrlPayloads {
    let text = |name: PCWSTR, cap: usize| -> Option<Vec<u8>> {
        let format = register_format(name)?;
        let medium = MediumGuard(get_medium(data, format, TYMED_HGLOBAL, -1)?);
        // SAFETY: a TYMED_HGLOBAL medium stores its payload in `u.hGlobal`.
        global_bytes(unsafe { medium.0.u.hGlobal }, cap)
    };
    let small = provenance::MAX_URL_PAYLOAD_BYTES;
    DropUrlPayloads {
        locator_wide: text(w!("UniformResourceLocatorW"), small)
            .map(|bytes| provenance::utf16_until_nul(&bytes)),
        locator_ansi: text(w!("UniformResourceLocator"), small)
            .map(|bytes| provenance::bytes_until_nul(&bytes)),
        uri_list: text(w!("text/uri-list"), small)
            .map(|bytes| provenance::text_payload(&bytes)),
        moz_url: text(w!("text/x-moz-url"), small)
            .map(|bytes| provenance::text_payload(&bytes)),
        html: text(w!("HTML Format"), provenance::MAX_HTML_BYTES)
            .or_else(|| text(w!("text/html"), provenance::MAX_HTML_BYTES)),
    }
}

/// Whether `data` can render `format` as an HGLOBAL.
fn has_format(data: &IDataObject, format: u16) -> bool {
    let formatetc = formatetc(format, TYMED_HGLOBAL, -1);
    // SAFETY: `QueryGetData` only inspects the data object.
    unsafe { data.QueryGetData(&formatetc) }.is_ok()
}

/// Converts the OLE screen-space point into client coordinates.
fn client_point(hwnd: isize, screen: &POINTL) -> (f32, f32) {
    let mut point = POINT {
        x: screen.x,
        y: screen.y,
    };
    // SAFETY: the handle belongs to this thread's window; `ScreenToClient`
    // writes one point into a live local.
    unsafe {
        let _ = ScreenToClient(HWND(hwnd as *mut c_void), &mut point);
    }
    (point.x as f32, point.y as f32)
}

// ---------------------------------------------------------------------------
// CF_HDROP
// ---------------------------------------------------------------------------

/// The file-system paths in a `CF_HDROP` payload, empty when absent.
fn hdrop_paths(data: &IDataObject) -> Vec<PathBuf> {
    let Some(medium) = get_medium(data, CF_HDROP.0, TYMED_HGLOBAL, -1) else {
        return Vec::new();
    };
    let medium = MediumGuard(medium);

    // SAFETY: a TYMED_HGLOBAL medium stores its payload in `u.hGlobal`, and for
    // CF_HDROP that payload is an HDROP.
    let hdrop = HDROP(unsafe { medium.0.u.hGlobal }.0);
    if hdrop.0.is_null() {
        return Vec::new();
    }

    // SAFETY: the handle is a live HDROP owned by the medium.
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        // SAFETY: same handle; passing no buffer asks for the length only.
        let length = unsafe { DragQueryFileW(hdrop, index, None) } as usize;
        if length == 0 {
            continue;
        }
        let mut buffer = vec![0u16; length + 1];
        // SAFETY: the buffer holds `length + 1` units, which is what the call
        // writes for a name of `length` units plus its terminator.
        let written = unsafe { DragQueryFileW(hdrop, index, Some(&mut buffer)) } as usize;
        buffer.truncate(written.min(length));
        paths.push(PathBuf::from(String::from_utf16_lossy(&buffer)));
    }
    paths
}

// ---------------------------------------------------------------------------
// Virtual files: FileGroupDescriptorW + FileContents
// ---------------------------------------------------------------------------

/// Reads every virtual file the data object offers.
fn virtual_files(data: &IDataObject) -> Vec<VirtualFile> {
    let (Some(group_format), Some(contents_format)) = (file_group_format(), file_contents_format())
    else {
        return Vec::new();
    };

    let Some(medium) = get_medium(data, group_format, TYMED_HGLOBAL, -1) else {
        return Vec::new();
    };
    let medium = MediumGuard(medium);
    // SAFETY: the FileGroupDescriptorW medium is an HGLOBAL.
    let Some(block) = global_bytes(unsafe { medium.0.u.hGlobal }, MAX_GROUP_BYTES) else {
        return Vec::new();
    };

    let descriptors = parse_file_group(&block);
    let mut files = Vec::with_capacity(descriptors.len());
    for (index, descriptor) in descriptors.iter().enumerate() {
        let Some(bytes) = file_contents(data, contents_format, index as i32, descriptor.size)
        else {
            tracing::debug!(
                name = %descriptor.name,
                "a virtual file could not be read; skipping it"
            );
            continue;
        };
        files.push(VirtualFile {
            name: descriptor.name.clone(),
            bytes: Arc::from(bytes),
        });
    }
    files
}

/// Reads one virtual file, preferring a stream and falling back to an HGLOBAL.
fn file_contents(data: &IDataObject, format: u16, index: i32, expected: u64) -> Option<Vec<u8>> {
    stream_contents(data, format, index, expected)
        .or_else(|| global_contents(data, format, index, expected))
}

/// Reads `FileContents` when the source exposes it as an `IStream`.
fn stream_contents(data: &IDataObject, format: u16, index: i32, expected: u64) -> Option<Vec<u8>> {
    let medium = get_medium(data, format, TYMED_ISTREAM, index)?;
    let medium = MediumGuard(medium);

    // SAFETY: a TYMED_ISTREAM medium stores the stream in `u.pstm`; the stream
    // is borrowed, not moved out, so the guard still owns it.
    let stream = unsafe { (*medium.0.u.pstm).as_ref() }?;

    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let mut read = 0u32;
        // SAFETY: the buffer is valid for `chunk.len()` bytes and `read` is a
        // live local the call writes.
        let status = unsafe {
            stream.Read(
                chunk.as_mut_ptr().cast::<c_void>(),
                chunk.len() as u32,
                Some(&mut read),
            )
        };
        if status.is_err() || read == 0 {
            break;
        }
        let read = read as usize;
        if bytes.len() + read > MAX_VIRTUAL_FILE_BYTES {
            tracing::warn!(
                limit = MAX_VIRTUAL_FILE_BYTES,
                "virtual file exceeds the size cap; truncating"
            );
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }

    if bytes.is_empty() {
        return None;
    }
    truncate_to_size(&mut bytes, expected);
    Some(bytes)
}

/// Reads `FileContents` when the source exposes it as an HGLOBAL.
fn global_contents(data: &IDataObject, format: u16, index: i32, expected: u64) -> Option<Vec<u8>> {
    let medium = get_medium(data, format, TYMED_HGLOBAL, index)?;
    let medium = MediumGuard(medium);
    // SAFETY: a TYMED_HGLOBAL medium stores the payload in `u.hGlobal`.
    let handle = unsafe { medium.0.u.hGlobal };
    let mut bytes = global_bytes(handle, MAX_VIRTUAL_FILE_BYTES)?;
    truncate_to_size(&mut bytes, expected);
    Some(bytes)
}

/// Trims `bytes` to the descriptor's declared size, when it declares one.
fn truncate_to_size(bytes: &mut Vec<u8>, expected: u64) {
    if let Ok(expected) = usize::try_from(expected)
        && expected > 0
        && expected < bytes.len()
    {
        bytes.truncate(expected);
    }
}

/// One entry of a `FileGroupDescriptorW` block.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileDescriptor {
    /// File name, exactly as the drag source wrote it.
    name: String,
    /// Declared size, or `0` when the descriptor does not say.
    size: u64,
}

/// Parses a `FileGroupDescriptorW` block.
///
/// Kept pure and separate from COM so the layout — which is packed and easy to
/// get wrong — is covered by unit tests. A truncated block yields the entries
/// that fit rather than panicking.
fn parse_file_group(bytes: &[u8]) -> Vec<FileDescriptor> {
    let Some(count_bytes) = bytes.get(..4) else {
        return Vec::new();
    };
    let declared = u32::from_le_bytes(count_bytes.try_into().expect("four bytes")) as usize;
    let available = bytes.len().saturating_sub(4) / DESCRIPTOR_LEN;
    let count = declared.min(available);

    let mut descriptors = Vec::with_capacity(count);
    for index in 0..count {
        let start = 4 + index * DESCRIPTOR_LEN;
        let entry = &bytes[start..start + DESCRIPTOR_LEN];

        let flags = u32::from_le_bytes(entry[0..4].try_into().expect("four bytes"));
        let size = if flags & FD_FILESIZE != 0 {
            let high = u32::from_le_bytes(entry[64..68].try_into().expect("four bytes"));
            let low = u32::from_le_bytes(entry[68..72].try_into().expect("four bytes"));
            (u64::from(high) << 32) | u64::from(low)
        } else {
            0
        };

        let name_bytes =
            &entry[DESCRIPTOR_NAME_OFFSET..DESCRIPTOR_NAME_OFFSET + DESCRIPTOR_NAME_UNITS * 2];
        let units: Vec<u16> = name_bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .take_while(|unit| *unit != 0)
            .collect();
        let name = String::from_utf16_lossy(&units);
        if name.is_empty() {
            continue;
        }
        descriptors.push(FileDescriptor { name, size });
    }
    descriptors
}

// ---------------------------------------------------------------------------
// Data-object plumbing
// ---------------------------------------------------------------------------

/// The clipboard format id of `name`, registered on first use.
fn register_format(name: PCWSTR) -> Option<u16> {
    // SAFETY: the call registers a name and returns a process-wide id.
    let id = unsafe { windows::Win32::System::DataExchange::RegisterClipboardFormatW(name) };
    u16::try_from(id).ok().filter(|id| *id != 0)
}

/// `FileGroupDescriptorW`, the list of virtual files in a drag.
fn file_group_format() -> Option<u16> {
    register_format(w!("FileGroupDescriptorW"))
}

/// `FileContents`, the stream carrying one virtual file.
fn file_contents_format() -> Option<u16> {
    register_format(w!("FileContents"))
}

/// A `FORMATETC` for one format and transfer medium.
fn formatetc(format: u16, tymed: TYMED, index: i32) -> FORMATETC {
    FORMATETC {
        cfFormat: format,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: index,
        tymed: tymed.0 as u32,
    }
}

/// Asks the data object for `format`, or `None` when it cannot render it.
fn get_medium(data: &IDataObject, format: u16, tymed: TYMED, index: i32) -> Option<STGMEDIUM> {
    let formatetc = formatetc(format, tymed, index);
    // SAFETY: `GetData` hands back a medium the caller must release, which is
    // what `MediumGuard` does.
    unsafe { data.GetData(&formatetc) }.ok()
}

/// Releases a `STGMEDIUM` exactly once.
struct MediumGuard(STGMEDIUM);

impl Drop for MediumGuard {
    fn drop(&mut self) {
        // SAFETY: the medium came from `IDataObject::GetData` and is released
        // once, here.
        unsafe { ReleaseStgMedium(&mut self.0) };
    }
}

/// Reads an `HGLOBAL` into bytes, refusing anything larger than `cap`.
fn global_bytes(handle: HGLOBAL, cap: usize) -> Option<Vec<u8>> {
    if handle.0.is_null() {
        return None;
    }
    // SAFETY: the handle is live and was produced by the data object.
    let size = unsafe { GlobalSize(handle) };
    if size == 0 || size > cap {
        tracing::warn!(size, cap, "dropped payload is outside the accepted size");
        return None;
    }
    // SAFETY: locking returns a pointer to `size` readable bytes, unlocked
    // below on every path.
    let pointer = unsafe { GlobalLock(handle) }.cast::<u8>();
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the lock guarantees `size` valid bytes.
    let bytes = unsafe { std::slice::from_raw_parts(pointer, size) }.to_vec();
    // SAFETY: balances the lock above.
    let _ = unsafe { GlobalUnlock(handle) };
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        DESCRIPTOR_LEN, DESCRIPTOR_NAME_OFFSET, FileDescriptor, attach_url, parse_file_group,
        truncate_to_size,
    };

    #[test]
    fn a_url_is_attached_only_to_a_single_image_drop() {
        let url = || Some("https://e.com/a.png".to_owned());
        assert_eq!(attach_url(1, url()), url());
        assert_eq!(attach_url(2, url()), None, "a multi-file drop is ambiguous");
        assert_eq!(attach_url(0, url()), None, "a URL-only drag imports nothing");
        assert_eq!(attach_url(1, None), None);
    }

    /// Builds one packed `FILEDESCRIPTORW` as the shell writes it.
    fn descriptor(name: &str, size: Option<u64>) -> Vec<u8> {
        let mut entry = vec![0u8; DESCRIPTOR_LEN];
        let flags: u32 = match size {
            Some(_) => super::FD_FILESIZE,
            None => 0,
        };
        entry[0..4].copy_from_slice(&flags.to_le_bytes());
        if let Some(size) = size {
            entry[64..68].copy_from_slice(&((size >> 32) as u32).to_le_bytes());
            entry[68..72].copy_from_slice(&(size as u32).to_le_bytes());
        }
        for (index, unit) in name
            .encode_utf16()
            .take(descriptor_name_limit())
            .enumerate()
        {
            let at = DESCRIPTOR_NAME_OFFSET + index * 2;
            entry[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
        entry
    }

    /// `MAX_PATH - 1` units, leaving room for the terminator.
    const fn descriptor_name_limit() -> usize {
        super::DESCRIPTOR_NAME_UNITS - 1
    }

    fn group(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut block = (entries.len() as u32).to_le_bytes().to_vec();
        for entry in entries {
            block.extend_from_slice(entry);
        }
        block
    }

    #[test]
    fn the_descriptor_layout_matches_the_windows_declaration() {
        // A drift in either direction would silently garble every name.
        assert_eq!(
            std::mem::size_of::<windows::Win32::UI::Shell::FILEDESCRIPTORW>(),
            DESCRIPTOR_LEN
        );
        assert_eq!(
            std::mem::offset_of!(windows::Win32::UI::Shell::FILEDESCRIPTORW, cFileName),
            DESCRIPTOR_NAME_OFFSET
        );
    }

    #[test]
    fn names_and_sizes_are_read_from_the_block() {
        let block = group(&[
            descriptor("hands.png", Some(12_345)),
            descriptor("study.jpg", None),
        ]);

        let parsed = parse_file_group(&block);

        assert_eq!(
            parsed,
            vec![
                FileDescriptor {
                    name: "hands.png".to_owned(),
                    size: 12_345,
                },
                FileDescriptor {
                    name: "study.jpg".to_owned(),
                    size: 0,
                },
            ]
        );
    }

    #[test]
    fn a_size_larger_than_four_gigabytes_survives() {
        let block = group(&[descriptor("scan.tif", Some(6 * 1024 * 1024 * 1024))]);

        let parsed = parse_file_group(&block);

        assert_eq!(parsed[0].size, 6 * 1024 * 1024 * 1024);
    }

    #[test]
    fn a_truncated_block_yields_only_complete_entries() {
        let mut block = group(&[
            descriptor("first.png", Some(1)),
            descriptor("second.png", Some(2)),
        ]);
        block.truncate(4 + DESCRIPTOR_LEN + 40);

        let parsed = parse_file_group(&block);

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "first.png");
    }

    #[test]
    fn an_empty_or_hostile_header_is_not_a_panic() {
        assert!(parse_file_group(&[]).is_empty());
        assert!(parse_file_group(&[0xFF, 0xFF, 0xFF, 0xFF]).is_empty());
        assert!(parse_file_group(&[0; 3]).is_empty());
    }

    #[test]
    fn unnamed_entries_are_skipped() {
        let block = group(&[descriptor("", Some(4)), descriptor("kept.png", Some(4))]);

        let parsed = parse_file_group(&block);

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "kept.png");
    }

    #[test]
    fn the_declared_size_trims_padding_but_never_extends() {
        let mut bytes = vec![1, 2, 3, 4, 5, 6];

        truncate_to_size(&mut bytes, 4);
        assert_eq!(bytes, vec![1, 2, 3, 4]);

        truncate_to_size(&mut bytes, 99);
        assert_eq!(bytes.len(), 4, "a larger declared size must not pad");

        truncate_to_size(&mut bytes, 0);
        assert_eq!(bytes.len(), 4, "zero means the size is unknown");
    }
}
