//! Why a decode or encode attempt failed.

use std::error::Error;
use std::fmt;

use crate::sniff::ImageFormat;

/// A decode or encode failure.
///
/// The variants are split so the UI can react differently: an unrecognised file
/// is a user mistake worth a hint, a damaged image is worth offering to relink,
/// and an unsupported format is worth saying "not yet" about rather than
/// implying the file is broken.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CodecError {
    /// The bytes match no signature this build knows.
    ///
    /// Also the answer for a file that is too short to hold any signature,
    /// including an empty one.
    Unrecognized,
    /// The signature is known but the image cannot be decoded.
    Decode {
        /// Format the bytes claimed to be.
        format: ImageFormat,
        /// What the decoder reported.
        reason: String,
    },
    /// The format is recognised but this build does not decode it.
    Unsupported {
        /// Format that was recognised.
        format: ImageFormat,
        /// Why it is not supported, in user-facing terms.
        detail: String,
    },
    /// Encoding failed.
    Encode {
        /// Format that was requested.
        format: ImageFormat,
        /// What the encoder reported.
        reason: String,
    },
    /// The image dimensions cannot be expressed in the target format.
    TooLarge {
        /// Pixel width that was requested.
        width: u32,
        /// Pixel height that was requested.
        height: u32,
    },
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unrecognized => formatter.write_str("not an image file this build recognises"),
            Self::Decode { format, reason } => {
                write!(formatter, "cannot decode {} image: {reason}", format.name())
            }
            Self::Unsupported { format, detail } => {
                write!(
                    formatter,
                    "{} images are recognised but not supported yet: {detail}",
                    format.name()
                )
            }
            Self::Encode { format, reason } => {
                write!(formatter, "cannot encode {} image: {reason}", format.name())
            }
            Self::TooLarge { width, height } => write!(
                formatter,
                "image of {width} x {height} pixels is too large for the target format"
            ),
        }
    }
}

impl Error for CodecError {}

#[cfg(test)]
mod tests {
    use super::CodecError;
    use crate::sniff::ImageFormat;

    #[test]
    fn messages_name_the_format_and_avoid_jargon() {
        let cases = [
            (
                CodecError::Decode {
                    format: ImageFormat::Png,
                    reason: "invalid checksum".to_owned(),
                },
                "cannot decode PNG image: invalid checksum",
            ),
            (
                CodecError::Unsupported {
                    format: ImageFormat::Heif,
                    detail: "no pure-Rust decoder in this build".to_owned(),
                },
                "HEIF images are recognised but not supported yet: no pure-Rust decoder in this build",
            ),
            (
                CodecError::Encode {
                    format: ImageFormat::Jpeg,
                    reason: "quality out of range".to_owned(),
                },
                "cannot encode JPEG image: quality out of range",
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
        }
    }

    #[test]
    fn unrecognized_and_too_large_describe_themselves() {
        assert_eq!(
            CodecError::Unrecognized.to_string(),
            "not an image file this build recognises"
        );
        assert_eq!(
            CodecError::TooLarge {
                width: 70_000,
                height: 70_000,
            }
            .to_string(),
            "image of 70000 x 70000 pixels is too large for the target format"
        );
    }
}
