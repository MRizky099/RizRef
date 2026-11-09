//! CPU presentation through GDI, for the frames before the GPU is ready.
//!
//! A launch opens the window first and initialises the GPU on a background
//! thread (phase 07): on a laptop whose Vulkan/DX12 adapter is a discrete
//! GPU, waking it costs most of a second. Until the device is ready, frames —
//! the start view, the title bar, toasts, all of which the UI layer already
//! paints on the CPU with tiny-skia — are copied to the window with
//! `SetDIBitsToDevice`. Once the swapchain exists it owns the window and this
//! path is never used again.

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDC, ReleaseDC, SetDIBitsToDevice,
};
use winit::window::Window;

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
/// Returns whether GDI accepted it.
// SAFETY: the DC is obtained for this window's HWND and released before
// returning; the bitmap header describes exactly `bgra`'s dimensions
// (top-down, 32 bpp) and `bgra` outlives the call.
#[allow(unsafe_code)]
pub fn present(window: &Window, bgra: &[u8], width: u32, height: u32) -> bool {
    let Some(hwnd) = super::window_modes::hwnd(window) else {
        return false;
    };
    if bgra.len() < (width * height * 4) as usize || width == 0 || height == 0 {
        return false;
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        let dc = GetDC(Some(hwnd));
        if dc.is_invalid() {
            return false;
        }
        let lines = SetDIBitsToDevice(
            dc,
            0,
            0,
            width,
            height,
            0,
            0,
            0,
            height,
            bgra.as_ptr().cast(),
            &info,
            DIB_RGB_COLORS,
        );
        ReleaseDC(Some(hwnd), dc);
        lines > 0
    }
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
