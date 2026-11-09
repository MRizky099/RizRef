//! The ISOBMFF image families through Windows Imaging Component.
//!
//! HEIC is decoded in Rust (`rizref-codecs::heif`, backed by `heic-rs`), so this
//! bridge exists for the two cases the Rust path cannot serve:
//!
//! * **AVIF**, which shares the HEIF container but holds an AV1 codestream. No
//!   pure-Rust AV1 decoder builds without an external assembler — `avif-decode`
//!   pulls `rav1d` with its `asm` feature, which fails on a machine without
//!   NASM — so AVIF is read through the OS instead.
//! * **HEIC files that use a coding tool `heic-rs` does not implement yet**, for
//!   which the HEVC decoder in the HEIF Image Extension is the fallback.
//!
//! WIC opens both containers through the same HEIF container class; what makes
//! the pixels appear is the codec installed for the codestream inside — the
//! **HEIF Image Extension** for HEVC and the **AV1 Video Extension** for AVIF.
//! Both ship with Windows 11 and install free from the Store on Windows 10. When
//! the codec is missing, WIC reports `MF_E_TOPO_CODEC_NOT_FOUND`, and that is
//! turned into a message naming the package rather than into "corrupt file".
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

/// `WINCODEC_ERR_COMPONENTNOTFOUND`: the container is known but no installed
/// codec can open it at all.
const WINCODEC_ERR_COMPONENTNOTFOUND: i32 = 0x8898_2F50_u32 as i32;

/// `MF_E_TOPO_CODEC_NOT_FOUND`: the container opened, the codestream inside has
/// no installed decoder. This is what an AVIF file reports without the AV1
/// extension, and a HEIC without the HEIF extension.
const MF_E_TOPO_CODEC_NOT_FOUND: i32 = 0xC00D_5212_u32 as i32;

/// `WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT`: the file decoded but could not be
/// converted to 8-bit RGBA.
const WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT: i32 = 0x8898_2F30_u32 as i32;

/// What to install when the codec for `format` is absent.
fn missing_codec_hint(format: ImageFormat) -> String {
    match format {
        ImageFormat::Avif => "AVIF needs the free \"AV1 Video Extension\" package from the \
                              Microsoft Store on this machine"
            .to_owned(),
        _ => "HEIC/HEIF needs the free \"HEIF Image Extensions\" package from the Microsoft \
               Store on this machine"
            .to_owned(),
    }
}

/// Decodes an ISOBMFF image (`Heif` or `Avif`) into the canonical RGBA buffer.
pub fn decode(
    bytes: &[u8],
    limits: DecodeLimits,
    format: ImageFormat,
) -> Result<ImageBuffer, CodecError> {
    decode_inner(bytes, limits).map_err(|error| match error {
        WicError::Hresult(hresult) => {
            let code = hresult.0;
            if code == MF_E_TOPO_CODEC_NOT_FOUND || code == WINCODEC_ERR_COMPONENTNOTFOUND {
                CodecError::Unsupported {
                    format,
                    detail: missing_codec_hint(format),
                }
            } else if code == WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT {
                CodecError::Unsupported {
                    format,
                    detail: "this image uses a pixel format the installed codec cannot convert"
                        .to_owned(),
                }
            } else {
                CodecError::Decode {
                    format,
                    reason: format!("the OS image codec failed with 0x{:08x}", hresult.0),
                }
            }
        }
        WicError::Reason(reason) => CodecError::Decode { format, reason },
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

#[cfg(test)]
mod tests {
    use super::{decode, decode_inner, missing_codec_hint};
    use rizref_codecs::{CodecError, DecodeLimits, ImageBuffer, ImageFormat};

    #[test]
    fn the_hint_names_the_package_for_each_family() {
        assert!(missing_codec_hint(ImageFormat::Avif).contains("AV1 Video Extension"));
        assert!(missing_codec_hint(ImageFormat::Heif).contains("HEIF Image Extensions"));
    }

    #[test]
    fn garbage_bytes_are_reported_not_panicked_on() {
        let outcome = std::panic::catch_unwind(|| {
            decode(
                b"not a heif file",
                DecodeLimits::default(),
                ImageFormat::Avif,
            )
        });

        assert!(outcome.expect("decode must not panic").is_err());
    }

    #[test]
    fn a_failure_carries_the_format_that_was_requested() {
        let error =
            decode(&[], DecodeLimits::default(), ImageFormat::Avif).expect_err("empty input");

        match error {
            CodecError::Decode { format, .. } | CodecError::Unsupported { format, .. } => {
                assert_eq!(format, ImageFormat::Avif)
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn the_wic_pipeline_decodes_a_normal_image_end_to_end() {
        // The bridge is container-agnostic: WIC sniffs the stream itself, so a
        // PNG exercises the whole COM → decoder → converter → CopyPixels path
        // even on machines without the HEIF or AV1 extensions. That keeps the
        // plumbing covered when the real formats cannot be decoded in CI.
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
