//! Downscaling: previews for the board and fitted copies for the GPU.
//!
//! Both jobs use the same box filter. It is the right filter here for two
//! reasons: downscaling by a large factor averages every source pixel (so a
//! one-pixel highlight cannot disappear from a thumbnail), and it is cheap
//! enough to run on an import worker while the user keeps panning.
//!
//! Neither function ever upscales. A 200 px reference stays 200 px; blowing it
//! up would only cost memory and make the preview look softer than the original.

use rizref_codecs::ImageBuffer;

/// Longest side of a generated preview, in pixels.
pub const PREVIEW_SIDE: u32 = 512;

/// A downscaled image, or the original when it already fits.
#[derive(Debug)]
pub enum Fitted<'a> {
    /// The input was already small enough; no pixels were copied.
    Original(&'a ImageBuffer),
    /// A box-filtered copy at the target size.
    Resized(ImageBuffer),
}

impl Fitted<'_> {
    /// The pixels to upload.
    pub fn image(&self) -> &ImageBuffer {
        match self {
            Self::Original(image) => image,
            Self::Resized(image) => image,
        }
    }

    /// Whether a copy was made.
    pub fn is_resized(&self) -> bool {
        matches!(self, Self::Resized(_))
    }
}

/// Generates the embedded preview for an imported image.
///
/// Images already within [`PREVIEW_SIDE`] are cloned unchanged: an asset always
/// carries a preview, and the item renderer assumes it is at least one pixel.
pub fn generate_preview(image: &ImageBuffer) -> ImageBuffer {
    match fit_to_side(image, PREVIEW_SIDE) {
        Fitted::Original(original) => original.clone(),
        Fitted::Resized(resized) => resized,
    }
}

/// Downscales `image` so its longest side is at most `max_side`.
///
/// Returns [`Fitted::Original`] when no downscale is needed — including for an
/// empty image — so callers can avoid a pointless copy on the upload path.
pub fn fit_to_side(image: &ImageBuffer, max_side: u32) -> Fitted<'_> {
    let width = image.width();
    let height = image.height();
    let longest = width.max(height);
    if max_side == 0 || longest == 0 || longest <= max_side {
        return Fitted::Original(image);
    }

    let scale = f64::from(max_side) / f64::from(longest);
    let target_width = ((f64::from(width) * scale).round() as u32).max(1);
    let target_height = ((f64::from(height) * scale).round() as u32).max(1);

    Fitted::Resized(box_downscale(image, target_width, target_height))
}

/// Box-filter downscale to an exact target size.
///
/// Each destination pixel averages the source rectangle that maps onto it.
/// Source rectangles are half-open and cover the input exactly, so no pixel is
/// counted twice or skipped.
fn box_downscale(image: &ImageBuffer, target_width: u32, target_height: u32) -> ImageBuffer {
    let source_width = image.width();
    let source_height = image.height();
    let source = image.pixels();
    let mut out = Vec::with_capacity((target_width * target_height * 4) as usize);

    for dy in 0..target_height {
        let y0 = (u64::from(dy) * u64::from(source_height) / u64::from(target_height)) as u32;
        let y1 = (((u64::from(dy) + 1) * u64::from(source_height) + u64::from(target_height) - 1)
            / u64::from(target_height)) as u32;
        let y1 = y1.max(y0 + 1).min(source_height);

        for dx in 0..target_width {
            let x0 = (u64::from(dx) * u64::from(source_width) / u64::from(target_width)) as u32;
            let x1 = (((u64::from(dx) + 1) * u64::from(source_width) + u64::from(target_width) - 1)
                / u64::from(target_width)) as u32;
            let x1 = x1.max(x0 + 1).min(source_width);

            let mut sums = [0_u64; 4];
            let mut count = 0_u64;
            for y in y0..y1 {
                let row = y as usize * source_width as usize;
                for x in x0..x1 {
                    let offset = (row + x as usize) * 4;
                    for (channel, sum) in sums.iter_mut().enumerate() {
                        *sum += u64::from(source[offset + channel]);
                    }
                    count += 1;
                }
            }

            for sum in sums {
                let average = (sum + count / 2) / count;
                out.push(average as u8);
            }
        }
    }

    ImageBuffer::from_rgba(target_width, target_height, out)
        .expect("the box filter writes exactly one RGBA pixel per destination")
}

#[cfg(test)]
mod tests {
    use super::{Fitted, PREVIEW_SIDE, box_downscale, fit_to_side, generate_preview};
    use rizref_codecs::ImageBuffer;

    fn gradient(width: u32, height: u32) -> ImageBuffer {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[
                    (x * 255 / width.max(1)) as u8,
                    (y * 255 / height.max(1)) as u8,
                    ((x + y) * 255 / (width + height).max(1)) as u8,
                    255,
                ]);
            }
        }
        ImageBuffer::from_rgba(width, height, pixels).expect("gradient size")
    }

    #[test]
    fn a_three_by_three_block_averages_to_one_pixel() {
        let mut pixels = Vec::new();
        for value in 0..9_u8 {
            pixels.extend_from_slice(&[value * 10, 100, 50, 255]);
        }
        let image = ImageBuffer::from_rgba(3, 3, pixels).expect("3 x 3");

        let small = box_downscale(&image, 1, 1);

        // Red values 0, 10, .. 80 average to 40; rounding is symmetric.
        assert_eq!(small.pixel(0, 0), Some([40, 100, 50, 255]));
    }

    #[test]
    fn the_box_filter_keeps_the_aspect_ratio() {
        let source = gradient(4000, 1000);
        let fitted = fit_to_side(&source, 512);

        let Fitted::Resized(image) = fitted else {
            panic!("a 4000 px image must be resized");
        };
        assert_eq!((image.width(), image.height()), (512, 128));
    }

    #[test]
    fn a_small_image_is_not_upscaled() {
        let image = gradient(64, 32);

        let fitted = fit_to_side(&image, PREVIEW_SIDE);
        assert!(!fitted.is_resized());
        assert_eq!(fitted.image(), &image);

        let preview = generate_preview(&image);
        assert_eq!((preview.width(), preview.height()), (64, 32));
        assert_eq!(preview, image);
    }

    #[test]
    fn a_preview_fits_the_side_and_preserves_the_aspect() {
        let preview = generate_preview(&gradient(8000, 6000));

        assert_eq!((preview.width(), preview.height()), (512, 384));
    }

    #[test]
    fn odd_sizes_round_to_at_least_one_pixel() {
        let source = gradient(1000, 1);
        let fitted = fit_to_side(&source, 512);

        let image = fitted.image();
        assert_eq!(image.width(), 512);
        assert_eq!(image.height(), 1, "a one-pixel row survives");
    }

    #[test]
    fn zero_sized_and_zero_limits_are_left_alone() {
        let empty = ImageBuffer::filled(0, 0, [0, 0, 0, 0]);
        assert!(!fit_to_side(&empty, 512).is_resized());

        let image = gradient(4, 4);
        assert!(!fit_to_side(&image, 0).is_resized());
    }
}
