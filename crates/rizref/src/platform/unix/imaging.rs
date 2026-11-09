//! No OS image codec bridge on Linux and macOS (see `platform::imaging`).

use rizref_codecs::{CodecError, DecodeLimits, ImageBuffer, ImageFormat};

/// Reports `format` as unsupported on this OS.
pub fn decode(
    _bytes: &[u8],
    _limits: DecodeLimits,
    format: ImageFormat,
) -> Result<ImageBuffer, CodecError> {
    Err(CodecError::Unsupported {
        format,
        detail: unsupported_detail(format),
    })
}

/// The message for a format that only the Windows codec bridge decodes.
fn unsupported_detail(format: ImageFormat) -> String {
    match format {
        ImageFormat::Avif => "AVIF images can only be opened on Windows in this version".to_owned(),
        _ => "this HEIC/HEIF image uses a coding feature RizRef can only decode on Windows \
              (through the HEIF Image Extensions)"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use rizref_codecs::{CodecError, DecodeLimits, ImageFormat};

    #[test]
    fn avif_is_reported_as_unsupported_not_corrupt() {
        let error = super::decode(b"", DecodeLimits::default(), ImageFormat::Avif)
            .expect_err("no AVIF decoder");
        assert!(matches!(
            error,
            CodecError::Unsupported {
                format: ImageFormat::Avif,
                ..
            }
        ));
    }
}
