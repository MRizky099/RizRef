//! HEIF/HEIC through Windows Imaging Component.
//!
//! No pure-Rust HEVC-based HEIF decoder exists, so this format is not decoded by
//! `rizref-codecs`; it is bridged through the operating system instead. WIC
//! decodes HEIF when the **HEIF Image Extension** is installed — it ships with
//! Windows 11 and is a free install from the Store on Windows 10 — and WIC's
//! HEVC support is what the Photos app itself uses.
//!
//! This keeps two rules intact: `rizref-codecs` stays free of C dependencies,
//! and everything that touches the platform stays in `rizref/src/platform/`.
//! The cost is that a machine without the extension cannot open HEIC files, so
//! that case is reported as *install the extension*, not as a corrupt file.
//!
//! # Threading
//!
//! COM is initialised per calling thread on first use (the import workers are
//! plain threads, not an apartment). `CoInitializeEx` is reference counted and
//! the process never tears the apartment down, which is what wgpu and winit
//! already rely on for the UI thread.
//!
//! # Unsafe code
//!
//! This is a COM boundary: every call hands WIC a pointer whose lifetime it
//! owns. The allowance is module-wide because the decode is one linear flow;
//! the safety note on each call states the invariant that makes it sound.
#![allow(unsafe_code)]

use rizref_codecs::{CodecError, DecodeLimits, ImageBuffer, ImageFormat};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory2, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::CoInitializeEx;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::core::GUID;

/// `WINCODEC_ERR_COMPONENTNOTFOUND`: the container is known but no installed
/// codec can decode it, which for HEIF means the extension is missing.
const WINCODEC_ERR_COMPONENTNOTFOUND: i32 = 0x8898_2F50_u32 as i32;

/// `WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT`: the file decoded but could not be
/// converted to 8-bit RGBA — a 10-bit or float HEIC on an older codec.
const WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT: i32 = 0x8898_2F30_u32 as i32;

/// Message shown when the HEIF codec is not installed.
pub const MISSING_CODEC_HINT: &str = "HEIC/HEIF needs the free \"HEIF Image Extensions\" package \
     from the Microsoft Store on this machine";

/// Whether this machine can decode HEIF through WIC.
///
/// Probes by asking WIC for a HEIF decoder, which fails exactly when the
/// extension is absent. Cheap enough for a startup log line; not something to
/// call per file.
pub fn supported() -> bool {
    let Some(factory) = factory() else {
        return false;
    };
    // SAFETY: `CreateDecoder` only consults the installed codecs; a null vendor
    // GUID is the documented "any vendor" request and no stream is touched.
    unsafe { factory.CreateDecoder(&GUID_CONTAINER_FORMAT_HEIF, std::ptr::null()) }.is_ok()
}

/// Decodes a HEIF/HEIC file into the canonical RGBA buffer.
pub fn decode(bytes: &[u8], limits: DecodeLimits) -> Result<ImageBuffer, CodecError> {
    decode_inner(bytes, limits).map_err(|error| match error {
        WicError::Hresult(hresult) => {
            let code = hresult.0;
            if code == WINCODEC_ERR_COMPONENTNOTFOUND {
                CodecError::Unsupported {
                    format: ImageFormat::Heif,
                    detail: MISSING_CODEC_HINT.to_owned(),
                }
            } else if code == WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT {
                CodecError::Unsupported {
                    format: ImageFormat::Heif,
                    detail: "this HEIC uses a pixel format the installed codec cannot convert"
                        .to_owned(),
                }
            } else {
                CodecError::Decode {
                    format: ImageFormat::Heif,
                    reason: format!("WIC failed with 0x{:08x}", hresult.0),
                }
            }
        }
        WicError::Reason(reason) => CodecError::Decode {
            format: ImageFormat::Heif,
            reason,
        },
        WicError::TooLarge { width, height } => CodecError::TooLarge { width, height },
    })
}

/// Why the bridge could not produce an image.
#[derive(Debug)]
enum WicError {
    /// A COM or WIC call failed.
    Hresult(windows::core::HRESULT),
    /// A check in this module failed.
    Reason(String),
    /// The image is over the caller's cap.
    TooLarge { width: u32, height: u32 },
}

impl From<windows::core::Error> for WicError {
    fn from(error: windows::core::Error) -> Self {
        Self::Hresult(error.code())
    }
}

/// The real decode, in plain `Result` terms.
fn decode_inner(bytes: &[u8], limits: DecodeLimits) -> Result<ImageBuffer, WicError> {
    let factory = factory()
        .ok_or_else(|| WicError::Reason("cannot create the WIC imaging factory".to_owned()))?;

    // SAFETY: `SHCreateMemStream` copies the slice into a COM stream that lives
    // as long as the local binding; the decoder keeps its own reference.
    let stream = unsafe { SHCreateMemStream(Some(bytes)) }
        .ok_or_else(|| WicError::Reason("cannot wrap the file in a stream".to_owned()))?;

    // SAFETY: the stream is a live COM object; a null vendor means "any".
    let decoder = unsafe {
        factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
    }?;

    // SAFETY: index 0 of a decoder that opened successfully exists.
    let frame = unsafe { decoder.GetFrame(0) }?;

    let mut width = 0_u32;
    let mut height = 0_u32;
    // SAFETY: both out-parameters are live locals.
    unsafe { frame.GetSize(&mut width, &mut height) }?;
    if width == 0 || height == 0 {
        return Err(WicError::Reason("the image has no pixels".to_owned()));
    }
    if !limits.allows(width, height) {
        return Err(WicError::TooLarge { width, height });
    }

    let converter = unsafe { factory.CreateFormatConverter() }?;
    // SAFETY: converting to 8-bit straight-alpha RGBA is the canonical layout
    // this crate promises; a null palette is correct for a non-indexed target.
    unsafe {
        converter.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppRGBA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeMedianCut,
        )
    }?;

    let stride = width
        .checked_mul(ImageBuffer::BYTES_PER_PIXEL as u32)
        .ok_or_else(|| WicError::Reason("image width overflows the row stride".to_owned()))?;
    let len = (stride as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| WicError::Reason("image size overflows this platform".to_owned()))?;
    let mut pixels = vec![0_u8; len];

    // SAFETY: the buffer is exactly `stride * height` bytes and the converter
    // fills it in place; a null rect means "the whole image".
    unsafe { converter.CopyPixels(std::ptr::null(), stride, &mut pixels) }?;

    ImageBuffer::from_rgba(width, height, pixels).ok_or_else(|| {
        WicError::Reason("the converter returned an unexpected buffer size".to_owned())
    })
}

/// A WIC imaging factory for the calling thread.
///
/// COM is initialised on first use per thread. `RPC_E_CHANGED_MODE` means the
/// thread already has an apartment of another kind, which is fine: WIC works
/// from any apartment.
fn factory() -> Option<IWICImagingFactory> {
    // SAFETY: called from an arbitrary worker or UI thread; initialising the
    // apartment is idempotent and its reference is intentionally never released
    // because the threads outlive every decode they perform.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance::<_, IWICImagingFactory>(
            &CLSID_WICImagingFactory2,
            None,
            CLSCTX_INPROC_SERVER,
        )
        .ok()
    }
}

/// The HEIF container GUID, re-exported under a local name for readability.
const GUID_CONTAINER_FORMAT_HEIF: GUID =
    windows::Win32::Graphics::Imaging::GUID_ContainerFormatHeif;

#[cfg(test)]
mod tests {
    use super::{MISSING_CODEC_HINT, decode, decode_inner, supported};
    use rizref_codecs::{CodecError, DecodeLimits, ImageBuffer, ImageFormat};

    #[test]
    fn the_probe_answers_without_panicking() {
        // Either answer is legitimate: CI images usually lack the HEIF
        // extension, and a developer machine usually has it.
        let _ = supported();
    }

    #[test]
    fn garbage_bytes_are_reported_not_panicked_on() {
        let outcome =
            std::panic::catch_unwind(|| decode(b"not a heif file", DecodeLimits::default()));

        let result = outcome.expect("decode must not panic");
        assert!(result.is_err(), "garbage is not an image");
    }

    #[test]
    fn the_missing_codec_message_names_the_extension() {
        assert!(MISSING_CODEC_HINT.contains("HEIF Image Extensions"));
        assert!(MISSING_CODEC_HINT.to_lowercase().contains("store"));
    }

    #[test]
    fn a_heic_that_cannot_be_decoded_says_so_with_its_format() {
        let error = decode(&[], DecodeLimits::default()).expect_err("empty input");

        match error {
            CodecError::Decode { format, .. } | CodecError::Unsupported { format, .. } => {
                assert_eq!(format, ImageFormat::Heif)
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn the_wic_pipeline_decodes_a_normal_image_end_to_end() {
        // The bridge is container-agnostic: WIC sniffs the stream itself, so a
        // PNG exercises the whole COM → decoder → converter → CopyPixels path
        // even on machines without the HEIF extension. That keeps the plumbing
        // covered when HEIC itself cannot be decoded.
        let original = ImageBuffer::from_rgba(
            2,
            1,
            vec![
                255, 0, 0, 255, //
                0, 128, 255, 255,
            ],
        )
        .expect("2 x 1");
        let png = rizref_codecs::encode_png(&original).expect("encode");

        let decoded = decode_inner(&png, DecodeLimits::default()).expect("WIC decodes PNG");

        assert_eq!((decoded.width(), decoded.height()), (2, 1));
        assert_eq!(decoded.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(decoded.pixel(1, 0), Some([0, 128, 255, 255]));
    }

    #[test]
    fn the_wic_pipeline_honours_the_pixel_cap() {
        let image = ImageBuffer::filled(8, 8, [1, 2, 3, 255]);
        let png = rizref_codecs::encode_png(&image).expect("encode");

        let error = decode_inner(
            &png,
            DecodeLimits {
                max_pixels: 8 * 8 - 1,
            },
        )
        .expect_err("63 pixels is over the cap");

        assert!(matches!(error, super::WicError::TooLarge { .. }));
    }
}
