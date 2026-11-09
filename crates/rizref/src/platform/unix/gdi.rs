//! No CPU presenter on Linux and macOS yet (see `platform::gdi`): the frames
//! before the GPU is ready are skipped, and the window shows its first GPU
//! frame instead.

use winit::window::Window;

/// Reports that nothing was presented.
pub fn present(_window: &Window, _bgra: &[u8], _width: u32, _height: u32) -> bool {
    false
}
