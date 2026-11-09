//! Drops on Linux and macOS arrive through winit (`WindowEvent::DroppedFile`),
//! so there is nothing to register (see `platform::drop_target`).

use winit::window::Window;

use crate::platform::drop_target::DropEvent;

/// Never constructed on Unix; exists so the application's field has a type.
pub struct Registration {
    _private: (),
}

/// Leaves winit's own drop handler in place and returns `None`.
pub fn register(
    _window: &Window,
    _sink: impl Fn(DropEvent) + Send + Sync + 'static,
) -> Option<Registration> {
    tracing::debug!("file drops arrive through winit on this OS");
    None
}
