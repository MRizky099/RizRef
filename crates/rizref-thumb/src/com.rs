//! The COM surface: class factory, provider object, stream adapter, exports.
//!
//! Everything here is FFI glue; the picture itself comes from
//! [`crate::render`]. Unsafe code is confined to this module and each block
//! states the invariant it relies on.

#![allow(unsafe_code)]
// `#[implement]` expands to vtable plumbing whose generated items carry no
// docs; the trait impls below are documented by the interfaces they serve.
#![allow(missing_docs)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_POINTER, E_UNEXPECTED, S_FALSE,
    S_OK,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::System::Com::{
    IClassFactory, IClassFactory_Impl, IStream, STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET,
};
use windows::Win32::UI::Shell::PropertiesSystem::{
    IInitializeWithStream, IInitializeWithStream_Impl,
};
use windows::Win32::UI::Shell::{
    IThumbnailProvider, IThumbnailProvider_Impl, WTS_ALPHATYPE, WTSAT_ARGB, WTSAT_RGB,
};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface, Ref, Result, implement};

use crate::CLSID_U128;

/// The provider's class id.
const CLSID: GUID = GUID::from_u128(CLSID_U128);

/// Live objects plus server locks; `DllCanUnloadNow` answers from it.
static LIVE: AtomicU32 = AtomicU32::new(0);

/// Read + Seek over a shell `IStream`, so `rizref-format` reads the file in
/// place instead of copying it into memory first.
struct StreamReader(IStream);

impl Read for StreamReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let mut read = 0_u32;
        // SAFETY: `buf` is valid for `len` writable bytes and `read` for one
        // u32; the stream writes at most `len` bytes and reports the count.
        let hr = unsafe { self.0.Read(buf.as_mut_ptr().cast(), len, Some(&mut read)) };
        if hr.is_err() {
            return Err(io::Error::other(format!("IStream::Read failed: {hr:?}")));
        }
        Ok(read as usize)
    }
}

impl Seek for StreamReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let (offset, origin) = match pos {
            SeekFrom::Start(n) => (
                i64::try_from(n).map_err(|_| io::Error::other("seek past i64"))?,
                STREAM_SEEK_SET,
            ),
            SeekFrom::Current(n) => (n, STREAM_SEEK_CUR),
            SeekFrom::End(n) => (n, STREAM_SEEK_END),
        };
        let mut position = 0_u64;
        // SAFETY: `position` is a valid out-pointer for the new offset.
        unsafe { self.0.Seek(offset, origin, Some(&mut position)) }
            .map_err(|e| io::Error::other(format!("IStream::Seek failed: {e}")))?;
        Ok(position)
    }
}

/// The thumbnail provider object Explorer creates per file.
#[implement(IInitializeWithStream, IThumbnailProvider)]
struct Provider {
    stream: RefCell<Option<IStream>>,
}

impl Provider {
    fn new() -> Self {
        LIVE.fetch_add(1, Ordering::SeqCst);
        Self {
            stream: RefCell::new(None),
        }
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IInitializeWithStream_Impl for Provider_Impl {
    fn Initialize(&self, pstream: Ref<IStream>, _grfmode: u32) -> Result<()> {
        let stream = pstream.ok()?.clone();
        let mut slot = self.stream.borrow_mut();
        if slot.is_some() {
            // The contract allows one initialisation per object.
            return Err(E_UNEXPECTED.into());
        }
        *slot = Some(stream);
        Ok(())
    }
}

impl IThumbnailProvider_Impl for Provider_Impl {
    fn GetThumbnail(
        &self,
        cx: u32,
        phbmp: *mut HBITMAP,
        pdwalpha: *mut WTS_ALPHATYPE,
    ) -> Result<()> {
        if phbmp.is_null() || pdwalpha.is_null() {
            return Err(E_POINTER.into());
        }
        let stream = self.stream.borrow().clone().ok_or(E_UNEXPECTED)?;
        // Never fails for a bad board: Explorer would show a blank page.
        let thumb = crate::render_or_artwork(StreamReader(stream), cx.clamp(1, 2048));

        let header = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: thumb.width as i32,
            // Negative height: a top-down DIB, the row order `render` emits.
            biHeight: -(thumb.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO {
            bmiHeader: header,
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: `info` describes a 32-bit top-down DIB of the thumbnail's
        // size; on success `bits` points at `width * height * 4` bytes owned
        // by the bitmap, which we fill exactly once. Ownership of the HBITMAP
        // passes to the caller, as the interface specifies.
        unsafe {
            let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            if bits.is_null() {
                return Err(E_FAIL.into());
            }
            std::ptr::copy_nonoverlapping(thumb.bgra.as_ptr(), bits.cast::<u8>(), thumb.bgra.len());
            *phbmp = bitmap;
            *pdwalpha = if thumb.has_alpha {
                WTSAT_ARGB
            } else {
                WTSAT_RGB
            };
        }
        Ok(())
    }
}

/// The class factory `DllGetClassObject` hands out.
#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        if ppvobject.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: checked non-null above; the caller owns the out slot.
        unsafe { *ppvobject = std::ptr::null_mut() };
        if punkouter.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let unknown: IUnknown = Provider::new().into();
        // SAFETY: `riid` is a valid GUID pointer and `ppvobject` a valid out
        // slot; `query` AddRefs on success, handing the reference out.
        unsafe { unknown.query(riid, ppvobject).ok() }
    }

    fn LockServer(&self, flock: BOOL) -> Result<()> {
        if flock.as_bool() {
            LIVE.fetch_add(1, Ordering::SeqCst);
        } else {
            LIVE.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// COM entry point: the class factory for our one class.
///
/// # Safety
/// Called by COM with valid `rclsid`/`riid` pointers and an out slot.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    if rclsid.is_null() || riid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    // SAFETY: all three pointers were checked non-null; COM guarantees they
    // point at valid memory for the duration of the call.
    unsafe {
        *ppv = std::ptr::null_mut();
        if *rclsid != CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        factory.query(riid, ppv)
    }
}

/// COM entry point: whether the DLL may be unloaded.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if LIVE.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

#[cfg(test)]
mod tests {
    use super::{CLSID, DllCanUnloadNow, DllGetClassObject};
    use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, S_OK};
    use windows::Win32::System::Com::IClassFactory;
    use windows::Win32::UI::Shell::IThumbnailProvider;
    use windows::core::{GUID, Interface};

    #[test]
    fn the_factory_serves_only_our_class_and_creates_providers() {
        let mut out = std::ptr::null_mut();
        let other = GUID::from_u128(1);
        // SAFETY: valid pointers to locals for the duration of the calls.
        let refused = unsafe { DllGetClassObject(&other, &IClassFactory::IID, &mut out) };
        assert_eq!(refused, CLASS_E_CLASSNOTAVAILABLE);
        assert!(out.is_null());

        let hr = unsafe { DllGetClassObject(&CLSID, &IClassFactory::IID, &mut out) };
        assert_eq!(hr, S_OK);
        // SAFETY: `out` holds an AddRef'd IClassFactory on success.
        let factory = unsafe { IClassFactory::from_raw(out) };
        let provider: IThumbnailProvider =
            unsafe { factory.CreateInstance(None) }.expect("provider");
        assert_ne!(DllCanUnloadNow(), S_OK, "a live provider pins the DLL");
        drop(provider);
        drop(factory);
        assert_eq!(DllCanUnloadNow(), S_OK);
    }
}
