//! Animated images: every frame of a file, already composed and timed.
//!
//! A GIF frame is a patch, not a picture: it carries its own offset inside the
//! canvas, its own palette, and a disposal rule that says what happens to the
//! canvas before the next frame is drawn. Handing the caller raw frames would
//! push that bookkeeping into the renderer, so the frames come out of here
//! **composed**: every entry is a full-canvas RGBA image that can simply be
//! drawn.
//!
//! Composition follows the parts of the format that change what a user sees —
//! offsets, transparent pixels, and `Background` disposal. `Previous` disposal
//! is approximated as "keep", which is what most decoders do in practice and is
//! visually correct for every animation whose frames cover the canvas.

use std::time::Duration;

use crate::buffer::ImageBuffer;
use crate::decode::DecodeLimits;
use crate::error::CodecError;
use crate::sniff::ImageFormat;

/// Shortest delay a frame is given, matching what browsers do with the
/// zero- and one-centisecond delays that old GIFs are full of.
///
/// Without the floor such an animation would ask for hundreds of frames a
/// second, which no display can show and no decoder should spend CPU on.
pub const MIN_FRAME_DELAY: Duration = Duration::from_millis(20);

/// Most frames kept from one file.
///
/// A 256-frame cap covers every plausible sticker or reaction GIF; the pixel
/// budget below is usually the binding constraint for large ones.
pub const MAX_FRAMES: usize = 256;

/// Most decoded pixels kept across all frames of one file (64 MP, 256 MB RGBA).
pub const MAX_TOTAL_PIXELS: u64 = 64 * 1024 * 1024;

/// One composed frame and how long it stays on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimationFrame {
    /// Full-canvas RGBA pixels.
    pub image: ImageBuffer,
    /// How long the frame is shown, already clamped to [`MIN_FRAME_DELAY`].
    pub delay: Duration,
}

impl AnimationFrame {
    /// A frame of `image` shown for `delay`.
    pub fn new(image: ImageBuffer, delay: Duration) -> Self {
        Self {
            image,
            delay: delay.max(MIN_FRAME_DELAY),
        }
    }
}

/// Decodes every frame of a GIF, composed onto one canvas.
///
/// The first frame is the image a still caller wants, so
/// [`decode_gif`](crate::decode::decode) and this function agree about what the
/// board shows before the animation starts.
///
/// Frames stop being added at [`MAX_FRAMES`] or [`MAX_TOTAL_PIXELS`]; a file
/// beyond either budget still yields a usable animation of its first frames
/// rather than failing outright.
pub fn decode_gif_animation(
    bytes: &[u8],
    limits: DecodeLimits,
) -> Result<Vec<AnimationFrame>, CodecError> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);

    let failed = |reason: String| CodecError::Decode {
        format: ImageFormat::Gif,
        reason,
    };

    let mut decoder = options
        .read_info(bytes)
        .map_err(|error| failed(error.to_string()))?;
    let canvas_width = u32::from(decoder.width());
    let canvas_height = u32::from(decoder.height());
    if !limits.allows(canvas_width, canvas_height) {
        return Err(CodecError::TooLarge {
            width: canvas_width,
            height: canvas_height,
        });
    }

    let canvas_len = ImageBuffer::byte_len(canvas_width, canvas_height)
        .ok_or_else(|| failed("image dimensions overflow this platform".to_owned()))?;
    // Transparent canvas: a GIF's first frame is usually a full redraw, but a
    // partial one must not show black where it never painted.
    let mut canvas = vec![0_u8; canvas_len];
    let mut frames: Vec<AnimationFrame> = Vec::new();
    let mut pixels_used = 0_u64;

    while let Some(frame) = decoder
        .read_next_frame()
        .map_err(|error| failed(error.to_string()))?
    {
        let frame_pixels = u64::from(frame.width) * u64::from(frame.height);
        if frames.len() >= MAX_FRAMES || pixels_used + frame_pixels > MAX_TOTAL_PIXELS {
            // The caller still gets a usable animation of the frames so far; a
            // file this large is a budget question, not a decode failure.
            break;
        }
        pixels_used += u64::from(canvas_width) * u64::from(canvas_height);

        let patch = Patch {
            rect: PatchRect {
                left: frame.left,
                top: frame.top,
                width: frame.width,
                height: frame.height,
            },
            pixels: frame.buffer.as_ref(),
        };
        blend_frame(&mut canvas, (canvas_width, canvas_height), &patch);

        let image = ImageBuffer::from_rgba(canvas_width, canvas_height, canvas.clone())
            .ok_or_else(|| failed("frame buffer does not match the canvas".to_owned()))?;
        // `delay` is in hundredths of a second; 0 means "as fast as possible".
        frames.push(AnimationFrame::new(
            image,
            Duration::from_millis(u64::from(frame.delay) * 10),
        ));

        match frame.dispose {
            gif::DisposalMethod::Background => {
                clear_rect(&mut canvas, (canvas_width, canvas_height), patch.rect);
            }
            // `Previous` would need a stack of canvases; treating it as `Keep`
            // is what the common decoders do and matches these files visually.
            gif::DisposalMethod::Any
            | gif::DisposalMethod::Keep
            | gif::DisposalMethod::Previous => {}
        }
    }

    if frames.is_empty() {
        return Err(failed("file contains no frames".to_owned()));
    }
    Ok(frames)
}

/// Where a frame sits inside the canvas, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PatchRect {
    left: u16,
    top: u16,
    width: u16,
    height: u16,
}

/// One frame's pixels and their place in the canvas.
struct Patch<'a> {
    rect: PatchRect,
    pixels: &'a [u8],
}

/// Draws one frame's RGBA patch onto the canvas, skipping transparent pixels.
fn blend_frame(canvas: &mut [u8], canvas_size: (u32, u32), patch: &Patch<'_>) {
    let (canvas_width, canvas_height) = canvas_size;
    let stride = canvas_width as usize * 4;
    let patch_stride = patch.rect.width as usize * 4;
    let x0 = usize::from(patch.rect.left).min(canvas_width as usize);
    let columns = (patch.rect.width as usize).min(canvas_width as usize - x0);

    for row in 0..usize::from(patch.rect.height) {
        let y = usize::from(patch.rect.top) + row;
        if y >= canvas_height as usize {
            break;
        }
        let Some(source) = patch
            .pixels
            .get(row * patch_stride..(row + 1) * patch_stride)
        else {
            break;
        };
        for column in 0..columns {
            let src = &source[column * 4..column * 4 + 4];
            if src[3] == 0 {
                continue;
            }
            let dst = y * stride + (x0 + column) * 4;
            if dst + 4 > canvas.len() {
                break;
            }
            // Source-over onto the composed canvas. GIF pixels are either
            // fully opaque or fully transparent, so this only has to copy.
            canvas[dst..dst + 4].copy_from_slice(src);
        }
    }
}

/// Clears the frame's rectangle to transparent, as `Background` disposal says.
fn clear_rect(canvas: &mut [u8], canvas_size: (u32, u32), rect: PatchRect) {
    let (canvas_width, canvas_height) = canvas_size;
    let stride = canvas_width as usize * 4;
    let x0 = usize::from(rect.left).min(canvas_width as usize);
    let x1 = (usize::from(rect.left) + usize::from(rect.width)).min(canvas_width as usize);
    for row in 0..usize::from(rect.height) {
        let y = usize::from(rect.top) + row;
        if y >= canvas_height as usize {
            break;
        }
        let start = y * stride + x0 * 4;
        let end = y * stride + x1 * 4;
        canvas[start..end].fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::{AnimationFrame, MAX_FRAMES, MIN_FRAME_DELAY, decode_gif_animation};
    use crate::buffer::ImageBuffer;
    use crate::decode::DecodeLimits;
    use std::time::Duration;

    /// Encodes `frames` as a GIF: each entry is `(rgba, delay_hundredths,
    /// disposal)` covering the whole canvas.
    fn gif_of(frames: &[(Vec<u8>, u16, gif::DisposalMethod)], width: u16, height: u16) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut bytes, width, height, &[]).expect("encoder");
            for (pixels, delay, dispose) in frames {
                let mut pixels = pixels.clone();
                let mut frame = gif::Frame::from_rgba_speed(width, height, &mut pixels, 10);
                frame.delay = *delay;
                frame.dispose = *dispose;
                encoder.write_frame(&frame).expect("frame");
            }
        }
        bytes
    }

    fn solid(width: u16, height: u16, rgba: [u8; 4]) -> Vec<u8> {
        let mut pixels = Vec::new();
        for _ in 0..u32::from(width) * u32::from(height) {
            pixels.extend_from_slice(&rgba);
        }
        pixels
    }

    #[test]
    fn every_frame_comes_back_composed_at_canvas_size() {
        let bytes = gif_of(
            &[
                (solid(4, 4, [255, 0, 0, 255]), 10, gif::DisposalMethod::Keep),
                (solid(4, 4, [0, 0, 255, 255]), 10, gif::DisposalMethod::Keep),
            ],
            4,
            4,
        );

        let frames = decode_gif_animation(&bytes, DecodeLimits::default()).expect("decode");

        assert_eq!(frames.len(), 2);
        for frame in &frames {
            assert_eq!((frame.image.width(), frame.image.height()), (4, 4));
        }
        assert_eq!(frames[0].delay, Duration::from_millis(100));
    }

    #[test]
    fn a_later_frame_replaces_the_pixels_of_the_earlier_one() {
        let bytes = gif_of(
            &[
                (solid(2, 2, [255, 0, 0, 255]), 10, gif::DisposalMethod::Keep),
                (solid(2, 2, [0, 255, 0, 255]), 10, gif::DisposalMethod::Keep),
            ],
            2,
            2,
        );

        let frames = decode_gif_animation(&bytes, DecodeLimits::default()).expect("decode");

        assert_eq!(frames[1].image.pixel(0, 0), Some([0, 255, 0, 255]));
    }

    #[test]
    fn a_zero_delay_is_floored_rather_than_spinning() {
        let bytes = gif_of(
            &[(solid(2, 2, [1, 2, 3, 255]), 0, gif::DisposalMethod::Keep)],
            2,
            2,
        );

        let frames = decode_gif_animation(&bytes, DecodeLimits::default()).expect("decode");

        assert_eq!(frames[0].delay, MIN_FRAME_DELAY);
    }

    #[test]
    fn a_still_gif_is_a_one_frame_animation() {
        let bytes = gif_of(
            &[(solid(3, 3, [9, 9, 9, 255]), 4, gif::DisposalMethod::Any)],
            3,
            3,
        );

        let frames = decode_gif_animation(&bytes, DecodeLimits::default()).expect("decode");

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].image.pixel(1, 1), Some([9, 9, 9, 255]));
    }

    #[test]
    fn the_frame_budget_stops_a_long_animation() {
        // Every frame is a full 2 x 2 canvas, so the pixel budget (64 MP) is not
        // the limit here; the frame count is.
        let template = solid(2, 2, [1, 2, 3, 255]);
        let bytes = {
            let mut bytes = Vec::new();
            {
                let mut encoder = gif::Encoder::new(&mut bytes, 2, 2, &[]).expect("encoder");
                for _ in 0..(MAX_FRAMES + 5) {
                    let mut pixels = template.clone();
                    let mut frame = gif::Frame::from_rgba_speed(2, 2, &mut pixels, 10);
                    frame.delay = 2;
                    encoder.write_frame(&frame).expect("frame");
                }
            }
            bytes
        };

        let frames = decode_gif_animation(&bytes, DecodeLimits::default()).expect("decode");

        assert_eq!(frames.len(), MAX_FRAMES);
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_a_panic() {
        let mut bytes = gif_of(
            &[(solid(2, 2, [1, 2, 3, 255]), 2, gif::DisposalMethod::Keep)],
            2,
            2,
        );
        bytes.truncate(bytes.len() / 2);

        let outcome =
            std::panic::catch_unwind(|| decode_gif_animation(&bytes, DecodeLimits::default()));
        assert!(outcome.is_ok(), "decoding a truncated GIF panicked");
    }

    #[test]
    fn a_canvas_over_the_cap_is_refused() {
        let bytes = gif_of(
            &[(solid(4, 4, [1, 2, 3, 255]), 2, gif::DisposalMethod::Keep)],
            4,
            4,
        );

        let error = decode_gif_animation(
            &bytes,
            DecodeLimits {
                max_pixels: 4 * 4 - 1,
            },
        )
        .expect_err("15 pixels is over the cap");

        assert!(matches!(error, crate::error::CodecError::TooLarge { .. }));
        assert_eq!(
            AnimationFrame::new(
                ImageBuffer::filled(1, 1, [0, 0, 0, 0]),
                Duration::from_millis(1)
            )
            .delay,
            MIN_FRAME_DELAY
        );
    }

    #[test]
    fn the_first_frame_matches_the_still_decoder() {
        let bytes = gif_of(
            &[
                (
                    solid(3, 2, [10, 20, 30, 255]),
                    10,
                    gif::DisposalMethod::Keep,
                ),
                (
                    solid(3, 2, [40, 50, 60, 255]),
                    10,
                    gif::DisposalMethod::Keep,
                ),
            ],
            3,
            2,
        );

        let frames = decode_gif_animation(&bytes, DecodeLimits::default()).expect("animation");
        let still = crate::decode::decode(&bytes).expect("still");

        assert_eq!(frames[0].image, still);
    }
}
