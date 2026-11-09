//! CPU presentation for the frames before the GPU is ready.
//!
//! A launch opens the window first and initialises the GPU on a background
//! thread (phase 07). Until the device is ready, the CPU-painted shell layer
//! is composed into a BGRA frame here ([`compose_bgra`], pure) and handed to
//! the OS half:
//!
//! * **Windows** copies it with GDI's `SetDIBitsToDevice`;
//! * **Linux and macOS** have no CPU presenter yet (`platform/unix/gdi.rs`
//!   reports `false`): the window shows its first frame once the GPU is up,
//!   a few hundred milliseconds later. A real CPU path is the phase-08
//!   "CPU raster fallback" item.

use winit::window::Window;

#[cfg(windows)]
use super::windows::gdi as os;

#[cfg(unix)]
use super::unix::gdi as os;

/// Composites straight-alpha RGBA `layer` over an opaque `background` into a
/// `width` × `height` BGRA buffer (top-down rows), the layout a 32-bit DIB
/// wants. The layer is clipped to the buffer.
pub fn compose_bgra(
    width: u32,
    height: u32,
    background: [u8; 4],
    layer: Option<(&[u8], u32, u32)>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..width * height {
        out.extend_from_slice(&[background[2], background[1], background[0], 255]);
    }
    if let Some((rgba, lw, lh)) = layer {
        for y in 0..lh.min(height) {
            for x in 0..lw.min(width) {
                let s = ((y * lw + x) * 4) as usize;
                let d = ((y * width + x) * 4) as usize;
                let a = u16::from(rgba[s + 3]);
                if a == 0 {
                    continue;
                }
                for (channel, source) in [(0, 2), (1, 1), (2, 0)] {
                    let under = u16::from(out[d + channel]);
                    let over = u16::from(rgba[s + source]);
                    out[d + channel] = ((over * a + under * (255 - a) + 127) / 255) as u8;
                }
            }
        }
    }
    out
}

/// Copies a BGRA buffer of the window's client size onto the window.
/// Returns whether the OS accepted it.
pub fn present(window: &Window, bgra: &[u8], width: u32, height: u32) -> bool {
    os::present(window, bgra, width, height)
}

/// Clears the window surface to fully transparent: once the GPU presents
/// through a composition visual above it, the last CPU frame would otherwise
/// show through a see-through canvas. Returns whether the OS accepted it.
pub fn clear(window: &Window, width: u32, height: u32) -> bool {
    let (width, height) = (width.max(1), height.max(1));
    let zeros = vec![0_u8; (width * height * 4) as usize];
    present(window, &zeros, width, height)
}

#[cfg(test)]
mod tests {
    use super::compose_bgra;

    #[test]
    fn the_background_fills_and_the_layer_blends_straight_alpha() {
        // 2 x 1 layer over a 3 x 1 frame: opaque red, then half-transparent
        // white; the third pixel is background only.
        let layer = [255, 0, 0, 255, 255, 255, 255, 128];

        let out = compose_bgra(3, 1, [24, 24, 28, 255], Some((&layer, 2, 1)));

        assert_eq!(&out[0..4], &[0, 0, 255, 255], "red as BGRA");
        assert_eq!(
            &out[4..8],
            &[142, 140, 140, 255],
            "half white over the background"
        );
        assert_eq!(&out[8..12], &[28, 24, 24, 255], "background as BGRA");
    }

    #[test]
    fn an_oversized_layer_is_clipped() {
        let layer = vec![9_u8; 4 * 4 * 4];

        let out = compose_bgra(2, 2, [0, 0, 0, 255], Some((&layer, 4, 4)));

        assert_eq!(out.len(), 2 * 2 * 4);
    }
}
