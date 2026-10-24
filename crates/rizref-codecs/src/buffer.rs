//! The canonical pixel buffer produced by every decoder and consumed by every
//! encoder.

/// 8-bit RGBA pixels in row-major order, top row first.
///
/// One layout for the whole application. Reference images arrive as grayscale,
/// palette, RGB or RGBA, with or without alpha, and from five different
/// decoders; converting once, at the edge, means the renderer has a single
/// pixel format to upload and the thumbnail path a single stride to walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageBuffer {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl ImageBuffer {
    /// Bytes per pixel in the canonical layout.
    pub const BYTES_PER_PIXEL: usize = 4;

    /// Wraps raw RGBA8 data.
    ///
    /// Returns `None` when the length does not match `width × height × 4`, which
    /// is the one invariant the rest of the crate relies on — so it is checked
    /// at construction rather than trusted.
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let expected = Self::byte_len(width, height)?;
        (pixels.len() == expected).then_some(Self {
            width,
            height,
            pixels,
        })
    }

    /// An image of the given size filled with one colour.
    ///
    /// Used for placeholders — an item whose file has gone missing, or a
    /// thumbnail that could not be rendered — so the board keeps its layout
    /// instead of collapsing.
    ///
    /// # Panics
    ///
    /// Panics if `width × height × 4` overflows `usize`. No display on any
    /// supported platform comes close, and silently returning a smaller image
    /// would be worse than stopping.
    pub fn filled(width: u32, height: u32, rgba: [u8; Self::BYTES_PER_PIXEL]) -> Self {
        let count = Self::pixel_count(width, height).expect("image size overflows usize");
        let mut pixels = Vec::with_capacity(count * Self::BYTES_PER_PIXEL);
        for _ in 0..count {
            pixels.extend_from_slice(&rgba);
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Number of bytes `width × height × 4` needs, or `None` on overflow.
    pub fn byte_len(width: u32, height: u32) -> Option<usize> {
        Self::pixel_count(width, height)?.checked_mul(Self::BYTES_PER_PIXEL)
    }

    /// Number of pixels, or `None` when the dimensions overflow `usize`.
    pub fn pixel_count(width: u32, height: u32) -> Option<usize> {
        (width as usize).checked_mul(height as usize)
    }

    /// Pixel width.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Pixel height.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Number of pixels.
    pub fn len(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// Whether the image holds no pixels.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// All pixels, RGBA, row-major.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Consumes the image, returning its raw pixels.
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// The RGBA value at `(x, y)`, or `None` when out of bounds.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; Self::BYTES_PER_PIXEL]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y as usize * self.width as usize + x as usize) * Self::BYTES_PER_PIXEL;
        let channels = self.pixels.get(offset..offset + Self::BYTES_PER_PIXEL)?;
        Some([channels[0], channels[1], channels[2], channels[3]])
    }
}

#[cfg(test)]
mod tests {
    use super::ImageBuffer;

    /// A 2×2 image: red, green, blue, white.
    fn checker() -> ImageBuffer {
        let pixels = vec![
            255, 0, 0, 255, //
            0, 255, 0, 255, //
            0, 0, 255, 255, //
            255, 255, 255, 255,
        ];
        ImageBuffer::from_rgba(2, 2, pixels).expect("4 pixels of RGBA")
    }

    #[test]
    fn from_rgba_rejects_a_length_that_does_not_match_the_size() {
        assert!(ImageBuffer::from_rgba(2, 2, vec![0; 15]).is_none());
        assert!(ImageBuffer::from_rgba(2, 2, vec![0; 17]).is_none());
        assert!(ImageBuffer::from_rgba(2, 2, vec![0; 16]).is_some());
        assert!(ImageBuffer::from_rgba(0, 0, Vec::new()).is_some());
    }

    #[test]
    fn dimensions_that_would_overflow_are_refused() {
        assert_eq!(ImageBuffer::byte_len(u32::MAX, u32::MAX), None);
        assert!(ImageBuffer::from_rgba(u32::MAX, u32::MAX, Vec::new()).is_none());
    }

    #[test]
    fn pixels_are_addressed_row_major() {
        let image = checker();

        assert_eq!(image.width(), 2);
        assert_eq!(image.height(), 2);
        assert_eq!(image.len(), 4);
        assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(image.pixel(1, 0), Some([0, 255, 0, 255]));
        assert_eq!(image.pixel(0, 1), Some([0, 0, 255, 255]));
        assert_eq!(image.pixel(1, 1), Some([255, 255, 255, 255]));
    }

    #[test]
    fn out_of_bounds_pixels_are_none_not_a_panic() {
        let image = checker();

        assert_eq!(image.pixel(2, 0), None);
        assert_eq!(image.pixel(0, 2), None);
        assert_eq!(image.pixel(u32::MAX, u32::MAX), None);
    }

    #[test]
    fn filled_repeats_the_colour_over_every_pixel() {
        let image = ImageBuffer::filled(3, 1, [10, 20, 30, 40]);

        assert_eq!(image.len(), 3);
        assert_eq!(image.pixels().len(), 12);
        for x in 0..3 {
            assert_eq!(image.pixel(x, 0), Some([10, 20, 30, 40]));
        }
    }

    #[test]
    fn a_zero_sized_image_is_empty() {
        let image = ImageBuffer::filled(0, 4, [0, 0, 0, 0]);

        assert!(image.is_empty());
        assert!(image.pixels().is_empty());
    }
}
