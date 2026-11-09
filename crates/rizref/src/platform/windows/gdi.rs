//! CPU presentation through GDI, for the frames before the GPU is ready
//! (the Windows half of `platform::gdi`).
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
