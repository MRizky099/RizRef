//! Recognising an image format from the bytes themselves.
//!
//! Format detection reads signatures, not file names: a board can hold files the
//! user renamed, downloaded without an extension, or dragged straight out of a
//! browser. The extension is only a hint, used when a signature is absent.
//!
//! # Formats without a signature
//!
//! TGA has no reliable leading magic — only an optional 26-byte footer, which
//! some writers omit — so [`sniff`] never reports it. TGA is decoded when the
//! caller already knows the format, which in practice means
//! [`ImageFormat::from_extension`] on the file name.
//!
//! HEIF/HEIC/AVIF are recognised through their ISO-BMFF `ftyp` brand so that the
//! answer to "why does this HEIC not open?" is a specific message instead of
//! "unrecognised file".

/// An image format RizRef can recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImageFormat {
    /// Portable Network Graphics.
    Png,
    /// JPEG, including the JFIF and Exif variants.
    Jpeg,
    /// Graphics Interchange Format; only the first frame is used.
    Gif,
    /// WebP, lossy or lossless.
    WebP,
    /// Windows bitmap.
    Bmp,
    /// Truevision TGA; detected by extension, never by signature.
    Tga,
    /// Portable anymap: PBM, PGM, PPM and PAM.
    Pnm,
    /// Quite OK Image.
    Qoi,
    /// Windows icon; the largest embedded image is used.
    Ico,
    /// The HEIF/HEIC/AVIF family. Recognised, not decoded: see the crate docs.
    Heif,
}

impl ImageFormat {
    /// Every format this build recognises.
    pub const ALL: [Self; 10] = [
        Self::Png,
        Self::Jpeg,
        Self::Gif,
        Self::WebP,
        Self::Bmp,
        Self::Tga,
        Self::Pnm,
        Self::Qoi,
        Self::Ico,
        Self::Heif,
    ];

    /// Human-readable name, for messages.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Gif => "GIF",
            Self::WebP => "WebP",
            Self::Bmp => "BMP",
            Self::Tga => "TGA",
            Self::Pnm => "PNM",
            Self::Qoi => "QOI",
            Self::Ico => "ICO",
            Self::Heif => "HEIF",
        }
    }

    /// Canonical file extension, without the dot.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::WebP => "webp",
            Self::Bmp => "bmp",
            Self::Tga => "tga",
            Self::Pnm => "pnm",
            Self::Qoi => "qoi",
            Self::Ico => "ico",
            Self::Heif => "heic",
        }
    }

    /// Whether this build can decode the format.
    pub const fn is_decodable(self) -> bool {
        !matches!(self, Self::Heif)
    }

    /// The format a file extension names.
    ///
    /// Accepts the usual aliases (`jpg`/`jpeg`, `tif`/`tiff`, `heic`/`heif`) and
    /// ignores case and a leading dot, because that is what file names look like.
    pub fn from_extension(extension: &str) -> Option<Self> {
        let extension = extension
            .strip_prefix('.')
            .unwrap_or(extension)
            .to_ascii_lowercase();
        Self::ALL.into_iter().find(|format| {
            format.extension() == extension || format.aliases().contains(&extension.as_str())
        })
    }

    /// Alternative extensions that also name this format.
    const fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Jpeg => &["jpeg", "jfif"],
            Self::Pnm => &["pbm", "pgm", "ppm", "pam"],
            Self::Tga => &["vda", "icb", "vst"],
            Self::Heif => &["heif", "hif"],
            _ => &[],
        }
    }
}

/// The format of `bytes`, judged by its signature alone.
///
/// Returns `None` for anything unrecognised, including input too short to hold a
/// signature. Every check is a bounds-checked slice comparison, so a truncated
/// or hostile file cannot read past its end.
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    let starts_with = |signature: &[u8]| bytes.starts_with(signature);

    if starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(ImageFormat::Png);
    }
    if starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(ImageFormat::Jpeg);
    }
    if starts_with(b"GIF87a") || starts_with(b"GIF89a") {
        return Some(ImageFormat::Gif);
    }
    if bytes.len() >= 12 && starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(ImageFormat::WebP);
    }
    if starts_with(b"BM") {
        return Some(ImageFormat::Bmp);
    }
    if starts_with(b"qoif") {
        return Some(ImageFormat::Qoi);
    }
    if starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        return Some(ImageFormat::Ico);
    }
    if is_pnm(bytes) {
        return Some(ImageFormat::Pnm);
    }
    if is_iso_bmff_image(bytes) {
        return Some(ImageFormat::Heif);
    }

    None
}

/// Whether `bytes` opens with a PNM magic number followed by whitespace.
///
/// The whitespace check matters: a PNG signature check cannot false-positive,
/// but `P1` alone does, and PNM's ASCII variants are the only ones where the
/// magic is not followed by binary data.
fn is_pnm(bytes: &[u8]) -> bool {
    matches!(bytes, [b'P', b'1'..=b'7', b' ' | b'\t' | b'\r' | b'\n', ..])
}

/// Whether `bytes` is an ISO base media file whose brand names an image format
/// this build recognises but does not decode (HEIC, HEIF, AVIF).
///
/// The brand lives in the `ftyp` box: bytes 4..8 are the box type and 8..12 the
/// major brand.
fn is_iso_bmff_image(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    matches!(
        &bytes[8..12],
        b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1" | b"avif"
    )
}

#[cfg(test)]
mod tests {
    use super::{ImageFormat, sniff};

    /// A minimal but genuinely well-formed file header per format.
    fn png() -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D', b'R']);
        bytes
    }

    fn webp() -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&32_u32.to_le_bytes());
        bytes.extend_from_slice(b"WEBPVP8L");
        bytes
    }

    fn heic() -> Vec<u8> {
        let mut bytes = 24_u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"ftypheic");
        bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        bytes
    }

    fn ico() -> Vec<u8> {
        vec![0x00, 0x00, 0x01, 0x00, 0x01, 0x00]
    }

    #[test]
    fn signatures_are_recognised() {
        let cases: [(&[u8], ImageFormat); 8] = [
            (&png(), ImageFormat::Png),
            (&[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10], ImageFormat::Jpeg),
            (b"GIF89a\x01\x00\x01\x00", ImageFormat::Gif),
            (b"GIF87a\x01\x00\x01\x00", ImageFormat::Gif),
            (&webp(), ImageFormat::WebP),
            (b"BM\x36\x00\x00\x00", ImageFormat::Bmp),
            (b"qoif\x00\x00\x00\x01", ImageFormat::Qoi),
            (&ico()[..], ImageFormat::Ico),
        ];

        for (bytes, expected) in cases {
            assert_eq!(sniff(bytes), Some(expected), "sniffing {expected:?}");
        }
    }

    #[test]
    fn pnm_is_recognised_by_magic_and_whitespace() {
        for header in [
            &b"P1\n"[..],
            &b"P2\t"[..],
            &b"P3 "[..],
            &b"P6\n"[..],
            &b"P7\r\n"[..],
        ] {
            assert_eq!(sniff(header), Some(ImageFormat::Pnm), "{header:?}");
        }

        // `P1` without following whitespace is a coincidence, not a header.
        assert_eq!(sniff(b"P1x"), None);
        assert_eq!(sniff(b"P8\n"), None);
    }

    #[test]
    fn heif_family_is_recognised_so_its_gap_can_be_explained() {
        assert_eq!(sniff(&heic()), Some(ImageFormat::Heif));

        let mut avif = 20_u32.to_be_bytes().to_vec();
        avif.extend_from_slice(b"ftypavif");
        avif.extend_from_slice(b"mif1");
        assert_eq!(sniff(&avif), Some(ImageFormat::Heif));

        // An MP4 shares the container but is not an image format we claim.
        let mut mp4 = 24_u32.to_be_bytes().to_vec();
        mp4.extend_from_slice(b"ftypisom");
        mp4.extend_from_slice(&[0; 8]);
        assert_eq!(sniff(&mp4), None);
    }

    #[test]
    fn tga_is_never_sniffed_because_it_has_no_signature() {
        // A plausible TGA header: no magic to find anywhere in it.
        let tga_header = [
            0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a,
        ];

        assert_eq!(sniff(&tga_header), None);
        assert_eq!(
            ImageFormat::from_extension("tga"),
            Some(ImageFormat::Tga),
            "TGA is found by extension instead"
        );
    }

    #[test]
    fn short_and_empty_input_is_unrecognised_not_a_panic() {
        for length in 0..4 {
            let bytes = vec![0x89; length];
            assert_eq!(sniff(&bytes), None, "input of {length} bytes");
        }
        assert_eq!(sniff(b"RIFF"), None, "RIFF without the WebP tag");
    }

    #[test]
    fn text_and_other_files_are_not_images() {
        assert_eq!(sniff(b"# notes about this board\n"), None);
        assert_eq!(sniff(b"{\"json\": true}"), None);
        assert_eq!(sniff(b"RIZREF\x00\x1a"), None, "a board is not an image");
    }

    #[test]
    fn extensions_map_to_formats_including_aliases_and_case() {
        let cases: [(&str, ImageFormat); 14] = [
            ("png", ImageFormat::Png),
            (".png", ImageFormat::Png),
            ("JPG", ImageFormat::Jpeg),
            ("jpeg", ImageFormat::Jpeg),
            ("jfif", ImageFormat::Jpeg),
            ("gif", ImageFormat::Gif),
            ("webp", ImageFormat::WebP),
            ("BMP", ImageFormat::Bmp),
            ("tga", ImageFormat::Tga),
            ("ppm", ImageFormat::Pnm),
            ("PAM", ImageFormat::Pnm),
            ("qoi", ImageFormat::Qoi),
            ("ico", ImageFormat::Ico),
            ("heic", ImageFormat::Heif),
        ];

        for (extension, expected) in cases {
            assert_eq!(
                ImageFormat::from_extension(extension),
                Some(expected),
                "extension {extension:?}"
            );
        }

        assert_eq!(ImageFormat::from_extension("rizref"), None);
        assert_eq!(ImageFormat::from_extension(""), None);
    }

    #[test]
    fn every_decodable_format_has_a_distinct_extension() {
        for (index, format) in ImageFormat::ALL.iter().enumerate() {
            assert!(!format.extension().is_empty());
            assert!(!format.name().is_empty());
            for other in &ImageFormat::ALL[index + 1..] {
                assert_ne!(format.extension(), other.extension());
                assert_ne!(format.name(), other.name());
            }
        }
    }

    #[test]
    fn only_heif_is_known_but_undecodable() {
        for format in ImageFormat::ALL {
            assert_eq!(
                format.is_decodable(),
                format != ImageFormat::Heif,
                "{format:?}"
            );
        }
    }

    #[test]
    fn the_extension_of_a_sniffed_file_round_trips() {
        let format = sniff(&png()).expect("PNG is recognised");

        assert_eq!(sniff(&[]), None);
        assert_eq!(
            ImageFormat::from_extension(format.extension()),
            Some(format)
        );
    }
}
