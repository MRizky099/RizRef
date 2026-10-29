//! The embedded board thumbnail.
//!
//! A board carries its own thumbnail, which is what lets the start view — and,
//! later, the Windows Explorer thumbnail provider — show what a file looks like
//! without opening it. The thumbnail travels as its own chunk so a reader that
//! only wants the picture can skip everything else, and so an unchanged
//! thumbnail can be copied verbatim on save instead of re-rendered.
//!
//! The container keeps the encoded bytes opaque: it never learns an image
//! format. The chunk payload stores the pixel width and height in front of those
//! bytes, which is what lets a recents tile be laid out from the aspect ratio
//! alone. [`crate::thumbnail`] is the compositor that produces them.

/// An encoded preview image, with the pixel size it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// Pixel width of the encoded image.
    pub width: u32,
    /// Pixel height of the encoded image.
    pub height: u32,
    /// Encoded bytes, opaque to this crate.
    pub bytes: Vec<u8>,
}

impl Preview {
    /// A preview of an image of `width` × `height` pixels encoded in `bytes`.
    pub fn new(width: u32, height: u32, bytes: Vec<u8>) -> Self {
        Self {
            width,
            height,
            bytes,
        }
    }

    /// Whether this preview carries no image at all.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty() || self.width == 0 || self.height == 0
    }

    /// Width divided by height, or `None` for a preview with no image.
    ///
    /// A recent-boards list lays tiles out from this rather than decoding the
    /// thumbnail, which keeps a list of a hundred boards cheap to draw.
    pub fn aspect_ratio(&self) -> Option<f32> {
        if self.is_empty() {
            return None;
        }
        Some(self.width as f32 / self.height as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::Preview;

    #[test]
    fn a_described_preview_keeps_its_dimensions_and_bytes() {
        let preview = Preview::new(320, 200, vec![0xff, 0xd8, 0xff]);

        assert_eq!(preview.width, 320);
        assert_eq!(preview.height, 200);
        assert_eq!(preview.bytes.len(), 3);
        assert!(!preview.is_empty());
    }

    #[test]
    fn aspect_ratio_describes_landscape_and_portrait_alike() {
        let landscape = Preview::new(1600, 900, vec![1]);
        let portrait = Preview::new(900, 1600, vec![1]);

        assert_eq!(landscape.aspect_ratio(), Some(1600.0 / 900.0));
        assert_eq!(portrait.aspect_ratio(), Some(900.0 / 1600.0));
    }

    #[test]
    fn a_preview_without_an_image_has_no_ratio() {
        assert_eq!(Preview::new(0, 0, Vec::new()).aspect_ratio(), None);
        assert_eq!(Preview::new(320, 200, Vec::new()).aspect_ratio(), None);
        assert_eq!(Preview::new(0, 200, vec![1]).aspect_ratio(), None);
    }
}
