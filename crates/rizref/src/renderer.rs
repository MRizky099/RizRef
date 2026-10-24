//! The GPU renderer: backend choice, the surface, and the frame.
//!
//! Backend selection is written down in one place and visible in the logs,
//! because "which backend did it pick?" is the first question in every
//! rendering bug report. The surface, the clear pass and the frame pacing are
//! here; the item pipeline and everything that draws content arrive with the
//! canvas in phase 02.

use std::sync::Arc;

use winit::window::Window;

use crate::app::CANVAS_BACKGROUND;

/// Backends this build tries, in preference order.
///
/// Vulkan first because it is the thinnest path on Windows, DX12 second
/// because it is the one guaranteed to be present on Windows 10 and later.
///
/// GL is deliberately absent, by decision rather than omission: enabling
/// `wgpu/gles` pulls `glutin_wgl_sys`, a `-sys` crate outside the accepted
/// Windows FFI set. The pure-Rust answer for a machine with no usable
/// Vulkan/DX12 driver is the CPU raster path (`tiny-skia`), not OpenGL. See
/// the dependency audit in the repository README.
pub fn preferred_backends() -> wgpu::Backends {
    wgpu::Backends::VULKAN | wgpu::Backends::DX12
}

/// The backend order as a list, for the adapter walk.
fn backend_priority() -> Vec<wgpu::Backends> {
    vec![wgpu::Backends::VULKAN, wgpu::Backends::DX12]
}

/// Renders a backend set as the arrow-joined names a bug report should quote.
pub fn describe_backends(backends: wgpu::Backends) -> String {
    let ordered = [
        (wgpu::Backends::VULKAN, "vulkan"),
        (wgpu::Backends::DX12, "dx12"),
        (wgpu::Backends::GL, "gl"),
        (wgpu::Backends::METAL, "metal"),
        (wgpu::Backends::BROWSER_WEBGPU, "webgpu"),
    ];

    let names: Vec<&str> = ordered
        .iter()
        .filter(|(flag, _)| backends.contains(*flag))
        .map(|(_, name)| *name)
        .collect();

    if names.is_empty() {
        // wgpu treats an empty set as "everything this platform offers".
        "auto".to_owned()
    } else {
        names.join(" -> ")
    }
}

/// Converts the 8-bit board background into the linear-ish f32 wgpu clears to.
fn clear_color() -> wgpu::Color {
    let [r, g, b, a] = CANVAS_BACKGROUND.map(|c| f64::from(c) / 255.0);
    wgpu::Color { r, g, b, a }
}

/// Why a frame did not make it to the screen.
#[derive(Debug)]
pub enum FrameError {
    /// The surface needs reconfiguring before the next frame (window changed).
    NeedsReconfigure,
    /// The surface or device is gone; the caller cannot recover by retrying.
    Fatal(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsReconfigure => f.write_str("surface needs reconfiguring"),
            Self::Fatal(why) => write!(f, "surface lost: {why}"),
        }
    }
}

/// The window's GPU surface: adapter, device, queue and swapchain in one
/// place, with a single `render` that clears the board.
pub struct GpuSurface {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

impl GpuSurface {
    /// Creates the surface and picks the adapter/device for `window`.
    ///
    /// The walk is explicit rather than `request_adapter`'s, because the plan
    /// fixes an order — Vulkan before DX12 — and a set-valued option cannot
    /// express order. Hardware adapters win over software ones inside a
    /// backend; every adapter found is logged at debug for bug reports.
    pub fn new(window: Arc<Window>, size: (u32, u32)) -> Option<Self> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = preferred_backends();
        let instance = wgpu::Instance::new(descriptor);
        // Holding the window in an `Arc` (which the caller does) lets the
        // surface claim `'static` rather than borrowing the window.
        let surface = instance
            .create_surface(window)
            .inspect_err(|e| tracing::error!(%e, "cannot create the window surface"))
            .ok()?;

        let mut chosen: Option<wgpu::Adapter> = None;
        for backend in backend_priority() {
            let adapters = pollster::block_on(instance.enumerate_adapters(backend));
            for adapter in &adapters {
                let info = adapter.get_info();
                tracing::debug!(backend = %describe_backends(backend), adapter = %info.name, ?info.device_type, "adapter");
            }
            let pick = adapters
                .iter()
                .find(|a| a.get_info().device_type != wgpu::DeviceType::Cpu)
                .or_else(|| adapters.first());
            if let Some(adapter) = pick {
                let info = adapter.get_info();
                tracing::info!(
                    backend = %info.backend,
                    adapter = %info.name,
                    "GPU adapter selected"
                );
                chosen = Some(adapter.clone());
                break;
            }
        }
        let adapter = chosen?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rizref-device"),
            ..Default::default()
        }))
        .inspect_err(|e| tracing::error!(%e, "cannot acquire a GPU device"))
        .ok()?;

        let config = match surface.get_default_config(&adapter, size.0.max(1), size.1.max(1)) {
            Some(config) => config,
            None => {
                tracing::error!("the surface supports no usable configuration");
                return None;
            }
        };
        surface.configure(&device, &config);
        tracing::info!(
            format = ?config.format,
            present = ?config.present_mode,
            width = config.width,
            height = config.height,
            "surface configured"
        );

        Some(Self {
            surface,
            device,
            queue,
            config,
        })
    }

    /// Reconfigures the swapchain after a window resize or DPI change.
    ///
    /// Nothing is recreated: the adapter, device and surface survive; only the
    /// swapchain dimensions change. A size of zero is clamped to one, the
    /// minimum wgpu accepts.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// Renders one frame: acquire, clear to the board color, present.
    ///
    /// Occlusion and timeout are normal (a minimized window owes no frame) and
    /// return `Ok`. Outdated reconfigures and requests another redraw through
    /// the error; lost/validated surfaces are fatal and logged by the caller.
    pub fn render(&mut self) -> Result<(), FrameError> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                tracing::debug!("suboptimal swapchain; presenting anyway");
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Err(FrameError::NeedsReconfigure);
            }
            wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(FrameError::Fatal("surface lost".into()));
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(FrameError::Fatal("surface validation error".into()));
            }
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rizref-clear"),
            });
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        // The borrow ends before `finish` moves the encoder; the pass borrows
        // the view and the clear color only.
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rizref-clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color()),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                ..Default::default()
            });
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{clear_color, describe_backends, preferred_backends};

    #[test]
    fn this_build_prefers_vulkan_then_dx12() {
        assert_eq!(describe_backends(preferred_backends()), "vulkan -> dx12");
    }

    #[test]
    fn the_gl_fallback_is_not_compiled_in() {
        assert!(
            !preferred_backends().contains(wgpu::Backends::GL),
            "GL requires a disallowed -sys crate; see the dependency audit"
        );
    }

    #[test]
    fn an_empty_set_reads_as_auto() {
        assert_eq!(describe_backends(wgpu::Backends::empty()), "auto");
    }

    #[test]
    fn unknown_combinations_still_describe_themselves() {
        assert_eq!(
            describe_backends(wgpu::Backends::METAL),
            "metal",
            "a backend this build does not use must still be nameable"
        );
    }

    #[test]
    fn the_clear_color_is_opaque_and_matches_the_board() {
        let c = clear_color();
        assert_eq!(c.a, 1.0, "an opaque board, not a ghost");
        assert!((c.r - 24.0 / 255.0).abs() < f64::EPSILON);
        assert!((c.g - 24.0 / 255.0).abs() < f64::EPSILON);
        assert!((c.b - 28.0 / 255.0).abs() < f64::EPSILON);
    }
}
