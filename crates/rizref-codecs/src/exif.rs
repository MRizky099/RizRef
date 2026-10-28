//! EXIF orientation: reading the tag and baking it into the pixels.
//!
//! A photograph stores its intended orientation as a number in its metadata and
//! leaves the pixels in sensor order. Every viewer is expected to rotate the
//! image itself, so a reference board that ignored the tag would show every
//! phone photo on its side. This module reads the tag from JPEG's APP1 segment
//! and from WebP's `EXIF` chunk, then applies the transform once, during decode,
//! so the rest of the application only ever sees an upright buffer.
//!
//! The parser is deliberately small and total: a malformed segment, a missing
//! TIFF header, or an unknown orientation value yields `1` (no transform), never
//! a panic. That is the same "damaged file decodes or fails cleanly" policy as
//! the decoders around it.

use crate::buffer::ImageBuffer;

/// Orientation tag number in IFD0.
const ORIENTATION_TAG: u16 = 0x0112;

/// TIFF type `SHORT`.
const TYPE_SHORT: u16 = 3;

/// The EXIF orientation of a JPEG, or `1` when there is none.
pub(crate) fn jpeg_orientation(bytes: &[u8]) -> u8 {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return 1;
    }

    let mut at = 2usize;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            at += 1;
            continue;
        }
        let marker = bytes[at + 1];
        // Start of scan: metadata has already passed. End of image and the
        // reserved markers carry no length field.
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        if marker == 0xFF {
            at += 1;
            continue;
        }
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            at += 2;
            continue;
        }

        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        if length < 2 {
            break;
        }
        let Some(end) = at.checked_add(2 + length) else {
            break;
        };
        if end > bytes.len() {
            break;
        }

        if marker == 0xE1 {
            let payload = &bytes[at + 4..end];
            if let Some(tiff) = payload.strip_prefix(b"Exif\0\0")
                && let Some(orientation) = tiff_orientation(tiff)
            {
                return orientation;
            }
        }
        at = end;
    }
    1
}

/// The EXIF orientation of a WebP, or `1` when there is none.
pub(crate) fn webp_orientation(bytes: &[u8]) -> u8 {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return 1;
    }

    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        let start = at + 8;
        let Some(end) = start.checked_add(size) else {
            break;
        };
        if end > bytes.len() {
            break;
        }

        if id == b"EXIF" {
            let payload = &bytes[start..end];
            // Some writers include the JPEG-style preamble, some do not.
            let tiff = payload.strip_prefix(b"Exif\0\0").unwrap_or(payload);
            if let Some(orientation) = tiff_orientation(tiff) {
                return orientation;
            }
        }

        // RIFF chunks are padded to an even number of bytes.
        at = end + (size & 1);
    }
    1
}

/// Reads the orientation tag out of a TIFF header, `None` when absent.
fn tiff_orientation(tiff: &[u8]) -> Option<u8> {
    let little_endian = match tiff.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };

    let u16_at = |at: usize| -> Option<u16> {
        let raw = tiff.get(at..at + 2)?;
        Some(if little_endian {
            u16::from_le_bytes([raw[0], raw[1]])
        } else {
            u16::from_be_bytes([raw[0], raw[1]])
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let raw = tiff.get(at..at + 4)?;
        Some(if little_endian {
            u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])
        } else {
            u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]])
        })
    };

    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;

    for index in 0..count {
        let entry = ifd + 2 + index * 12;
        let tag = u16_at(entry)?;
        if tag != ORIENTATION_TAG {
            continue;
        }
        let kind = u16_at(entry + 2)?;
        let items = u32_at(entry + 4)?;
        if kind != TYPE_SHORT || items == 0 {
            return None;
        }
        // A single SHORT sits in the first half of the value field; an array
        // stores an offset to its first element.
        let at = if items == 1 {
            entry + 8
        } else {
            u32_at(entry + 8)? as usize
        };
        return u16_at(at).and_then(|value| u8::try_from(value).ok());
    }
    None
}

/// Applies an EXIF orientation (1–8) to `image`.
///
/// Values outside the defined range are treated as "normal", because a corrupt
/// tag must not rotate a user's reference image into nonsense.
pub(crate) fn apply_orientation(image: ImageBuffer, orientation: u8) -> ImageBuffer {
    if !(2..=8).contains(&orientation) {
        return image;
    }

    let width = image.width();
    let height = image.height();
    let swaps = orientation >= 5;
    let (out_width, out_height) = if swaps {
        (height, width)
    } else {
        (width, height)
    };

    let mut pixels = Vec::with_capacity(image.pixels().len());
    for y in 0..out_height {
        for x in 0..out_width {
            let (sx, sy) = match orientation {
                2 => (width - 1 - x, y),
                3 => (width - 1 - x, height - 1 - y),
                4 => (x, height - 1 - y),
                5 => (y, x),
                6 => (y, height - 1 - x),
                7 => (width - 1 - y, height - 1 - x),
                8 => (width - 1 - y, x),
                _ => (x, y),
            };
            let pixel = image
                .pixel(sx, sy)
                .expect("the mapped source pixel is inside the image");
            pixels.extend_from_slice(&pixel);
        }
    }

    ImageBuffer::from_rgba(out_width, out_height, pixels)
        .expect("the oriented buffer has the right size")
}

#[cfg(test)]
mod tests {
    use super::{apply_orientation, jpeg_orientation, tiff_orientation, webp_orientation};
    use crate::buffer::ImageBuffer;

    /// An APP1 EXIF segment carrying `orientation`, with an optional wrong
    /// byte order marker.
    fn exif_segment(orientation: u16, little_endian: bool) -> Vec<u8> {
        let mut tiff = Vec::new();
        if little_endian {
            tiff.extend_from_slice(b"II");
            tiff.extend_from_slice(&42_u16.to_le_bytes());
            tiff.extend_from_slice(&8_u32.to_le_bytes());
            tiff.extend_from_slice(&1_u16.to_le_bytes());
            tiff.extend_from_slice(&0x0112_u16.to_le_bytes());
            tiff.extend_from_slice(&3_u16.to_le_bytes());
            tiff.extend_from_slice(&1_u32.to_le_bytes());
            tiff.extend_from_slice(&orientation.to_le_bytes());
            tiff.extend_from_slice(&[0, 0]);
            tiff.extend_from_slice(&0_u32.to_le_bytes());
        } else {
            tiff.extend_from_slice(b"MM");
            tiff.extend_from_slice(&42_u16.to_be_bytes());
            tiff.extend_from_slice(&8_u32.to_be_bytes());
            tiff.extend_from_slice(&1_u16.to_be_bytes());
            tiff.extend_from_slice(&0x0112_u16.to_be_bytes());
            tiff.extend_from_slice(&3_u16.to_be_bytes());
            tiff.extend_from_slice(&1_u32.to_be_bytes());
            tiff.extend_from_slice(&orientation.to_be_bytes());
            tiff.extend_from_slice(&[0, 0]);
            tiff.extend_from_slice(&0_u32.to_be_bytes());
        }

        let payload_len = 6 + tiff.len();
        let mut segment = vec![0xFF, 0xE1];
        segment.extend_from_slice(&((payload_len + 2) as u16).to_be_bytes());
        segment.extend_from_slice(b"Exif\0\0");
        segment.extend_from_slice(&tiff);
        segment
    }

    /// A JPEG skeleton: SOI, APP1, then an SOS marker that ends the scan.
    fn jpeg_with_exif(segment: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8];
        bytes.extend_from_slice(segment);
        bytes.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        bytes
    }

    #[test]
    fn jpeg_orientation_reads_both_byte_orders() {
        for little_endian in [true, false] {
            let bytes = jpeg_with_exif(&exif_segment(6, little_endian));
            assert_eq!(jpeg_orientation(&bytes), 6);
        }
    }

    #[test]
    fn jpeg_without_exif_is_normal() {
        assert_eq!(jpeg_orientation(&[0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02]), 1);
        assert_eq!(jpeg_orientation(b"not a jpeg"), 1);
        assert_eq!(jpeg_orientation(&[]), 1);
    }

    #[test]
    fn webp_exif_chunk_is_read_with_and_without_the_preamble() {
        for with_preamble in [true, false] {
            let tiff = {
                let mut full = Vec::new();
                full.extend_from_slice(b"II");
                full.extend_from_slice(&42_u16.to_le_bytes());
                full.extend_from_slice(&8_u32.to_le_bytes());
                full.extend_from_slice(&1_u16.to_le_bytes());
                full.extend_from_slice(&0x0112_u16.to_le_bytes());
                full.extend_from_slice(&3_u16.to_le_bytes());
                full.extend_from_slice(&1_u32.to_le_bytes());
                full.extend_from_slice(&8_u16.to_le_bytes());
                full.extend_from_slice(&[0, 0]);
                full.extend_from_slice(&0_u32.to_le_bytes());
                full
            };
            let mut payload = Vec::new();
            if with_preamble {
                payload.extend_from_slice(b"Exif\0\0");
            }
            payload.extend_from_slice(&tiff);

            let mut bytes = b"RIFF".to_vec();
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            bytes.extend_from_slice(b"WEBP");
            bytes.extend_from_slice(b"EXIF");
            bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&payload);
            if payload.len() % 2 == 1 {
                bytes.push(0);
            }

            assert_eq!(webp_orientation(&bytes), 8);
        }
    }

    #[test]
    fn truncated_metadata_is_normal_not_a_panic() {
        for length in 0..32 {
            let mut bytes = jpeg_with_exif(&exif_segment(6, true));
            bytes.truncate(length);
            assert_eq!(jpeg_orientation(&bytes), 1, "length {length}");
        }
        assert_eq!(tiff_orientation(b"II"), None);
        assert_eq!(tiff_orientation(b"II\x2a\x00\x08\x00\x00\x00"), None);
    }

    fn striped(width: u32, height: u32) -> ImageBuffer {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[
                    (x * 10) as u8,
                    (y * 10) as u8,
                    ((x + y) * 5) as u8,
                    255,
                ]);
            }
        }
        ImageBuffer::from_rgba(width, height, pixels).expect("3 x 2")
    }

    #[test]
    fn orientation_two_mirrors_horizontally() {
        let rotated = apply_orientation(striped(3, 2), 2);

        assert_eq!((rotated.width(), rotated.height()), (3, 2));
        assert_eq!(rotated.pixel(0, 0), Some([20, 0, 10, 255]));
        assert_eq!(rotated.pixel(2, 0), Some([0, 0, 0, 255]));
    }

    #[test]
    fn orientation_three_turns_the_image_around() {
        let rotated = apply_orientation(striped(3, 2), 3);

        assert_eq!((rotated.width(), rotated.height()), (3, 2));
        assert_eq!(rotated.pixel(0, 0), Some([20, 10, 15, 255]));
        assert_eq!(rotated.pixel(2, 1), Some([0, 0, 0, 255]));
    }

    #[test]
    fn the_quarter_turns_swap_the_dimensions() {
        for orientation in [5, 6, 7, 8] {
            let rotated = apply_orientation(striped(3, 2), orientation);
            assert_eq!(
                (rotated.width(), rotated.height()),
                (2, 3),
                "orientation {orientation}"
            );
        }
    }

    #[test]
    fn orientation_six_turns_clockwise() {
        // 3 x 2 in, 2 x 3 out: [a b c; d e f] becomes [d a; e b; f c].
        let rotated = apply_orientation(striped(3, 2), 6);

        assert_eq!(rotated.pixel(0, 0), Some([0, 10, 5, 255]));
        assert_eq!(rotated.pixel(1, 0), Some([0, 0, 0, 255]));
        assert_eq!(rotated.pixel(1, 2), Some([20, 0, 10, 255]));
    }

    #[test]
    fn orientation_eight_turns_counter_clockwise() {
        // [a b c; d e f] becomes [c f; b e; a d].
        let rotated = apply_orientation(striped(3, 2), 8);

        assert_eq!(rotated.pixel(0, 0), Some([20, 0, 10, 255]));
        assert_eq!(rotated.pixel(0, 2), Some([0, 0, 0, 255]));
    }

    #[test]
    fn values_outside_the_defined_range_are_ignored() {
        let original = striped(3, 2);

        assert_eq!(apply_orientation(original.clone(), 0), original);
        assert_eq!(apply_orientation(original.clone(), 1), original);
        assert_eq!(apply_orientation(original, 9), striped(3, 2));
    }
}
