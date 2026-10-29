//! The GPU renderer: backend choice, the surface, and the frame.
//!
//! Backend selection is written down in one place and visible in the logs,
//! because "which backend did it pick?" is the first question in every
//! rendering bug report. The surface owns the canvas pipelines (see
//! [`crate::canvas`]); `render` executes one frame from a [`FrameParams`]:
//! background, items, HUD.

use std::sync::Arc;

use glam::DVec2;
use rizref_core::{Camera, Rect};
use winit::window::Window;

use crate::canvas::{Canvas, FrameFlags, HudUpload, ItemQuad, UiUpload};
use crate::overlay::OverlayVertex;
use crate::textures::{MAX_TEXTURE_SIDE, TextureCache};

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

/// Converts the 8-bit sRGB board background into the linear values wgpu's
/// clear takes.
///
/// The swapchain is an sRGB format: writing `byte / 255` directly would treat
/// the value as linear and re-encode it on present, washing a charcoal board
/// out to mid-grey.
fn clear_color(background: [u8; 4]) -> wgpu::Color {
    fn srgb_to_linear(byte: u8) -> f64 {
        let value = f64::from(byte) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    wgpu::Color {
        r: srgb_to_linear(background[0]),
        g: srgb_to_linear(background[1]),
        b: srgb_to_linear(background[2]),
        a: f64::from(background[3]) / 255.0,
    }
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

/// The window's GPU surface: adapter, device, queue, swapchain and the canvas
/// pipelines in one place, with a single `render` that draws a whole frame.
pub struct GpuSurface {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    canvas: Canvas,
    textures: TextureCache,
}

/// Everything one frame needs, gathered by the caller and consumed by
/// [`GpuSurface::render`].
#[derive(Debug, Clone, Copy)]
pub struct FrameParams<'a> {
    /// The view transform to draw with.
    pub camera: &'a Camera,
    /// Viewport size in logical pixels.
    pub viewport: DVec2,
    /// Device pixels per logical pixel.
    pub physical_scale: f64,
    /// The tracked board bounding box, `None` for an empty board.
    pub scene_rect: Option<Rect>,
    /// Item quads in z-order (last drawn on top), each naming its texture.
    pub items: &'a [ItemQuad],
    /// Screen-space overlay triangles (outlines, handles, guides, marquee).
    pub overlay: &'a [OverlayVertex],
    /// The CPU-raster HUD readout, if any.
    pub hud: HudUpload<'a>,
    /// The cached shell layer: keep it, replace it, or release it.
    pub ui: UiUpload<'a>,
    /// Display effects (checkerboard, grayscale).
    pub flags: FrameFlags,
    /// The clear colour.
    pub background: [u8; 4],
    /// A pixel to read back after drawing, in physical window coordinates.
    pub pick: Option<(u32, u32)>,
}

impl FrameParams<'_> {
    /// The HUD texture size the globals need, `(0, 0)` when there is no HUD.
    pub fn hud_size(&self) -> (f32, f32) {
        match self.hud {
            HudUpload::None => (0.0, 0.0),
            HudUpload::Layer { width, height, .. } => (width as f32, height as f32),
        }
    }
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
            Some(mut config) => {
                // The colour picker reads one pixel back from the presented
                // surface; that requires COPY_SRC on the swapchain texture.
                config.usage |= wgpu::TextureUsages::COPY_SRC;
                config
            }
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

        let canvas = Canvas::new(&device, config.format);
        let max_side = device
            .limits()
            .max_texture_dimension_2d
            .min(MAX_TEXTURE_SIDE);
        let textures = TextureCache::new(&device, canvas.item_texture_layout(), max_side);
        tracing::info!(
            max_texture_side = textures.max_side(),
            budget_bytes = textures.budget_bytes(),
            "item textures ready"
        );

        Some(Self {
            surface,
            device,
            queue,
            config,
            canvas,
            textures,
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

    /// Uploads full-resolution or preview pixels for `asset`.
    pub fn upload_texture(
        &mut self,
        asset: rizref_core::AssetId,
        image: &rizref_codecs::ImageBuffer,
        is_preview: bool,
    ) {
        self.textures
            .upload(&self.device, &self.queue, asset, image, is_preview);
    }

    /// Whether `asset` has a resident texture.
    pub fn has_texture(&self, asset: rizref_core::AssetId) -> bool {
        self.textures.contains(asset)
    }

    /// Whether `asset`'s resident texture is only its preview.
    pub fn texture_is_preview(&self, asset: rizref_core::AssetId) -> bool {
        self.textures.is_preview(asset).unwrap_or(false)
    }

    /// Marks `asset` as visible this frame for LRU purposes.
    pub fn touch_texture(&mut self, asset: rizref_core::AssetId) {
        self.textures.touch(asset);
    }

    /// Number of resident item textures, for diagnostics.
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// VRAM held by item textures, for diagnostics.
    pub fn texture_bytes(&self) -> u64 {
        self.textures.used_bytes()
    }

    /// Renders one frame: acquire, draw the canvas, present.
    ///
    /// The frame span and the present span carry the timings the acceptance
    /// checks read (`RIZREF_LOG=debug` shows them per frame). Occlusion and
    /// timeout are normal (a minimized window owes no frame) and return `Ok`.
    /// Outdated reconfigures and requests another redraw through the error;
    /// lost/validated surfaces are fatal and logged by the caller.
    ///
    /// When [`FrameParams::pick`] names a pixel, the same frame's colour is
    /// copied to a readback buffer and returned as straight sRGB bytes — that
    /// is the colour picker, reading exactly what the user sees.
    pub fn render(&mut self, frame: &FrameParams) -> Result<Option<[u8; 4]>, FrameError> {
        let _frame_span = tracing::debug_span!("frame").entered();
        let acquired = self.surface.get_current_texture();
        let frame_texture = match acquired {
            wgpu::CurrentSurfaceTexture::Success(texture) => texture,
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                tracing::debug!("suboptimal swapchain; presenting anyway");
                texture
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Err(FrameError::NeedsReconfigure);
            }
            wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => {
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(FrameError::Fatal("surface lost".into()));
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(FrameError::Fatal("surface validation error".into()));
            }
        };

        let pick = frame.pick.map(|(x, y)| {
            (
                x.min(self.config.width.saturating_sub(1)),
                y.min(self.config.height.saturating_sub(1)),
            )
        });
        let readback = pick.map(|_| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rizref-pick-readback"),
                size: 256,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });

        self.canvas.set_globals(
            &self.queue,
            frame.camera,
            frame.viewport,
            frame.physical_scale,
            frame.scene_rect,
            frame.hud_size(),
            frame.flags,
        );
        for item in frame.items {
            self.textures.touch(item.asset);
        }
        self.textures.trim(None);
        let bind_groups: Vec<Option<&wgpu::BindGroup>> = frame
            .items
            .iter()
            .map(|item| self.textures.bind_group(item.asset))
            .collect();
        self.canvas
            .set_items(&self.device, &self.queue, frame.items);
        self.canvas
            .set_overlay(&self.device, &self.queue, frame.overlay);
        self.canvas.set_hud(&self.device, &self.queue, frame.hud);
        self.canvas.set_ui(&self.device, &self.queue, frame.ui);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rizref-frame"),
            });
        let view = frame_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rizref-canvas-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color(frame.background)),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                ..Default::default()
            });
            self.canvas.draw_background(&mut pass);
            self.canvas.draw_items(&mut pass, &bind_groups);
            self.canvas.draw_overlay(&mut pass);
            self.canvas.draw_hud(&mut pass);
            self.canvas.draw_ui(&mut pass);
        }

        if let (Some((x, y)), Some(buffer)) = (pick, readback.as_ref()) {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &frame_texture.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        // Row pitch must be a multiple of 256 for texture→buffer
                        // copies; one pixel is copied, the padding is ignored.
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }

        {
            let _present_span = tracing::debug_span!("present").entered();
            self.queue.submit(Some(encoder.finish()));
            self.queue.present(frame_texture);
        }

        let Some(buffer) = readback else {
            return Ok(None);
        };
        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        if let Err(error) = self.device.poll(wgpu::PollType::wait_indefinitely()) {
            tracing::warn!(%error, "cannot wait for the pick readback");
            return Ok(None);
        }
        if receiver
            .recv_timeout(std::time::Duration::from_millis(250))
            .is_err()
        {
            tracing::warn!("the pick readback did not complete");
            return Ok(None);
        }
        let mapped = match slice.get_mapped_range() {
            Ok(mapped) => mapped,
            Err(error) => {
                tracing::warn!(%error, "cannot read the pick buffer");
                return Ok(None);
            }
        };
        let mut pixel = [mapped[0], mapped[1], mapped[2], mapped[3]];
        // BGRA swapchains report their bytes in display order, not RGBA.
        if matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            pixel.swap(0, 2);
        }
        drop(mapped);
        buffer.unmap();
        Ok(Some(pixel))
    }
}

#[cfg(test)]
mod tests {
    use super::{clear_color, describe_backends, preferred_backends};
    use crate::app::CANVAS_BACKGROUND;

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
        let c = clear_color(CANVAS_BACKGROUND);
        assert_eq!(c.a, 1.0, "an opaque board, not a ghost");
        assert!((c.r - 0.009_134).abs() < 1.0e-5, "sRGB 24 is linear 0.0091");
        assert!((c.g - 0.009_134).abs() < 1.0e-5);
        assert!((c.b - 0.011_612).abs() < 1.0e-5);
    }

    #[test]
    fn a_paper_theme_clears_to_its_own_colour() {
        let c = clear_color([232, 226, 214, 255]);
        assert!((c.r - 0.806_952).abs() < 1.0e-5);
        assert!((c.g - 0.760_525).abs() < 1.0e-5);
        assert!((c.b - 0.672_443).abs() < 1.0e-5);
    }
}
