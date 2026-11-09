//! The canvas pipelines: the GPU half of the infinite board.
//!
//! Four pipelines share one uniform block of per-frame globals:
//!
//! * **background** — one full-screen triangle whose fragment shader derives
//!   the world position per pixel and draws the grid, the origin cross and the
//!   dashed scene-rect border;
//! * **items** — one instanced quad pipeline over a texture array; draw order
//!   is the instance order, which is the scene z-order;
//! * **overlay** — screen-space coloured triangles for selection outlines,
//!   handles, guides and the marquee, built on the CPU in logical pixels;
//! * **hud** — a small screen-space quad sampling the CPU-raster readout.
//!
//! The camera math is f64 on the CPU; the uniform narrows to f32 at draw time
//! *relative to the eye*, which is the whole reason the camera keeps f64: the
//! shader sees small numbers again before precision can matter.

use glam::DVec2;
use rizref_core::{AssetId, Camera, Rect};

use crate::overlay::OverlayVertex;

const SHADER: &str = include_str!("shaders/canvas.wgsl");

/// GPU capacity of the instance buffer when it is first created.
const INITIAL_INSTANCE_CAPACITY: usize = 256;

/// GPU capacity of the overlay vertex buffer when it is first created.
const INITIAL_OVERLAY_CAPACITY: usize = 4096;

/// Side length in uniform bytes of the `Globals` block.
///
/// Eighteen floats of data (72 bytes) rounded up to the 16-byte uniform
/// alignment WGSL requires.
const GLOBALS_SIZE: u64 = 80;

/// The per-frame uniform block, laid out exactly like the WGSL `Globals`
/// struct (all fields 4-byte aligned, 80 bytes total).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Globals {
    /// Camera eye in world units, narrowed to f32.
    pub eye: [f32; 2],
    /// Viewport in logical pixels.
    pub viewport: [f32; 2],
    /// (cos, sin) of the view rotation.
    pub rot_cs: [f32; 2],
    /// Logical pixels per world unit.
    pub zoom: f32,
    /// Device pixels per logical pixel.
    pub physical_scale: f32,
    /// Scene rect minimum corner, world units.
    pub scene_min: [f32; 2],
    /// Scene rect maximum corner, world units.
    pub scene_max: [f32; 2],
    /// 1.0 when the grid should draw (hidden above 400% or below 5% zoom).
    pub grid_visible: f32,
    /// 1.0 when the scene-rect border should draw.
    pub border_visible: f32,
    /// 1.0 when the checkerboard backdrop should draw.
    pub checkerboard: f32,
    /// 1.0 when items should draw in grayscale.
    pub grayscale: f32,
    /// HUD texture size in device pixels.
    pub hud_size: [f32; 2],
    /// Canvas background opacity (the WGSL `canvas_alpha`, in what was
    /// padding).
    pub canvas_alpha: f32,
}

impl Globals {
    /// Zoom above which the grid hides (400%).
    const GRID_MAX_ZOOM: f64 = 4.0;

    /// Zoom below which the grid hides (5%): further out its lines would be
    /// closer than a pixel and shimmer.
    const GRID_MIN_ZOOM: f64 = 0.05;

    /// Applies the user's view settings. Grid (with its origin axis) and
    /// border can only be switched off here: the zoom range and an empty
    /// board still hide them when the settings ask for them.
    fn apply_flags(&mut self, flags: FrameFlags) {
        if !flags.grid {
            self.grid_visible = 0.0;
        }
        if !flags.scene_border {
            self.border_visible = 0.0;
        }
        self.checkerboard = if flags.checkerboard { 1.0 } else { 0.0 };
        self.canvas_alpha = flags.canvas_alpha.clamp(0.0, 1.0);
        self.grayscale = if flags.grayscale { 1.0 } else { 0.0 };
    }

    /// Builds the block from the camera and the frame's fixed facts.
    pub fn new(
        camera: &Camera,
        viewport: DVec2,
        physical_scale: f64,
        scene_rect: Option<Rect>,
    ) -> Self {
        let scene_min = scene_rect.map_or([0.0, 0.0], |r| [r.min.x, r.min.y]);
        let scene_max = scene_rect.map_or([0.0, 0.0], |r| [r.max().x, r.max().y]);
        let (rotation_sin, rotation_cos) = camera.rotation.sin_cos();
        Self {
            eye: [camera.eye.x as f32, camera.eye.y as f32],
            viewport: [viewport.x as f32, viewport.y as f32],
            rot_cs: [rotation_cos as f32, rotation_sin as f32],
            zoom: camera.zoom as f32,
            physical_scale: physical_scale as f32,
            scene_min,
            scene_max,
            grid_visible: if camera.zoom >= Self::GRID_MIN_ZOOM
                && camera.zoom <= Self::GRID_MAX_ZOOM
            {
                1.0
            } else {
                0.0
            },
            border_visible: if scene_rect.is_some() { 1.0 } else { 0.0 },
            checkerboard: 0.0,
            grayscale: 0.0,
            hud_size: [0.0, 0.0],
            canvas_alpha: 1.0,
        }
    }

    /// The block as its 80 uniform bytes.
    pub fn bytes(&self) -> [u8; 80] {
        let floats: [f32; 20] = [
            self.eye[0],
            self.eye[1],
            self.viewport[0],
            self.viewport[1],
            self.rot_cs[0],
            self.rot_cs[1],
            self.zoom,
            self.physical_scale,
            self.scene_min[0],
            self.scene_min[1],
            self.scene_max[0],
            self.scene_max[1],
            self.grid_visible,
            self.border_visible,
            self.checkerboard,
            self.grayscale,
            self.hud_size[0],
            self.hud_size[1],
            self.canvas_alpha,
            0.0,
        ];
        pack_floats(&floats)
    }
}

/// One instanced item quad, laid out exactly like the WGSL `ItemIn` struct
/// (four 16-byte attributes, 64 bytes total).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuadInstance {
    /// Item centre in world units.
    pub center: [f32; 2],
    /// Item extent in world units.
    pub size: [f32; 2],
    /// Item rotation in radians.
    pub rotation: f32,
    /// UV rect (min.xy, max.xy) inside the item texture.
    pub uv: [f32; 4],
    /// RGBA tint, straight alpha, 1.0 = unmodified.
    pub tint: [f32; 4],
}

impl QuadInstance {
    /// An opaque full-UV untinted quad.
    pub fn new(center: [f32; 2], size: [f32; 2], rotation: f32) -> Self {
        Self {
            center,
            size,
            rotation,
            uv: [0.0, 0.0, 1.0, 1.0],
            tint: [1.0, 1.0, 1.0, 1.0],
        }
    }

    /// The instance as its 64 vertex bytes.
    pub fn bytes(&self) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        let floats = [
            self.center[0],
            self.center[1],
            self.size[0],
            self.size[1],
            self.rotation,
            0.0,
            0.0,
            0.0,
            self.uv[0],
            self.uv[1],
            self.uv[2],
            self.uv[3],
            self.tint[0],
            self.tint[1],
            self.tint[2],
            self.tint[3],
        ];
        for (slot, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(floats.iter()) {
            slot.copy_from_slice(&value.to_ne_bytes());
        }
        bytes
    }

    /// The axis-aligned world rect the rotated quad sweeps out — what the
    /// scene-rect tracker consumes.
    #[cfg(test)]
    pub fn world_rect(&self) -> Rect {
        let (sin, cos) = self.rotation.sin_cos();
        let (half_w, half_h) = (self.size[0] * 0.5, self.size[1] * 0.5);
        let extent_x = (cos * half_w).abs() + (sin * half_h).abs();
        let extent_y = (sin * half_w).abs() + (cos * half_h).abs();
        Rect::from_min_size(
            glam::Vec2::new(self.center[0] - extent_x, self.center[1] - extent_y),
            glam::Vec2::new(2.0 * extent_x, 2.0 * extent_y),
        )
    }
}

/// One drawable item: its transform and the texture it samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemQuad {
    /// Geometry and tint for the instanced vertex buffer.
    pub instance: QuadInstance,
    /// The asset whose GPU texture this item samples.
    pub asset: AssetId,
}

/// Packs 20 f32 values into the 80 bytes the uniform layout expects
/// (native-endian, 4-byte stride).
fn pack_floats(floats: &[f32; 20]) -> [u8; 80] {
    let mut bytes = [0u8; 80];
    for (slot, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(floats.iter()) {
        slot.copy_from_slice(&value.to_ne_bytes());
    }
    bytes
}

/// Everything the GPU needs to draw the board.
pub struct Canvas {
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    item_count: u32,
    overlay_buffer: wgpu::Buffer,
    overlay_capacity: usize,
    overlay_count: u32,
    hud_texture: Option<wgpu::Texture>,
    hud_bind_group: Option<wgpu::BindGroup>,
    hud_texture_size: (u32, u32),
    ui_texture: Option<wgpu::Texture>,
    ui_bind_group: Option<wgpu::BindGroup>,
    ui_texture_size: (u32, u32),
    background_pipeline: wgpu::RenderPipeline,
    item_pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    hud_pipeline: wgpu::RenderPipeline,
    ui_pipeline: wgpu::RenderPipeline,
}

impl Canvas {
    /// Format of the HUD upload texture; independent of the surface format,
    /// so the CPU pixels need no swizzling for whichever swapchain picked.
    const HUD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

    /// Creates the pipelines and the instance buffer.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rizref-canvas-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rizref-globals-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: Some(
                        wgpu::BufferSize::new(GLOBALS_SIZE).expect("uniform block is 64 bytes"),
                    ),
                },
                count: None,
            }],
        });

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rizref-item-texture-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let hud_texture_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rizref-hud-texture-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                        count: None,
                    },
                ],
            });

        let item_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: 64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 32,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 48,
                    shader_location: 3,
                },
            ],
        };

        // Nothing is culled: the screen winding of an item quad depends on the
        // item's own rotation, and each pipeline emits at most two triangles.
        let make_pipeline = |label: &'static str,
                             entry_vertex: &'static str,
                             entry_fragment: &'static str,
                             extra_layout: Option<&wgpu::BindGroupLayout>,
                             vertex_buffers: &[Option<wgpu::VertexBufferLayout>],
                             blend: wgpu::BlendState,
                             topology: wgpu::PrimitiveTopology| {
            let layouts: [Option<&wgpu::BindGroupLayout>; 2] =
                [Some(&globals_layout), extra_layout];
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &layouts,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(entry_vertex),
                    compilation_options: Default::default(),
                    buffers: vertex_buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry_fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let overlay_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: 24,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 8,
                    shader_location: 1,
                },
            ],
        };

        let background_pipeline = make_pipeline(
            "rizref-background-pipeline",
            "vs_background",
            "fs_background",
            None,
            &[],
            // Premultiplied colour over the clear, but the clear's alpha —
            // the canvas opacity — is kept: the grid and checkerboard are
            // part of the canvas, not drawn over it.
            wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            },
            wgpu::PrimitiveTopology::TriangleList,
        );
        let item_pipeline = make_pipeline(
            "rizref-item-pipeline",
            "vs_items",
            "fs_items",
            Some(&texture_layout),
            &[Some(item_vertex_layout.clone())],
            wgpu::BlendState::ALPHA_BLENDING,
            wgpu::PrimitiveTopology::TriangleStrip,
        );
        let overlay_pipeline = make_pipeline(
            "rizref-overlay-pipeline",
            "vs_overlay",
            "fs_overlay",
            None,
            &[Some(overlay_vertex_layout)],
            wgpu::BlendState::ALPHA_BLENDING,
            wgpu::PrimitiveTopology::TriangleList,
        );
        let hud_pipeline = make_pipeline(
            "rizref-hud-pipeline",
            "vs_hud",
            "fs_hud",
            Some(&hud_texture_layout),
            &[],
            wgpu::BlendState::ALPHA_BLENDING,
            wgpu::PrimitiveTopology::TriangleStrip,
        );
        let ui_pipeline = make_pipeline(
            "rizref-ui-pipeline",
            "vs_ui",
            "fs_ui",
            Some(&hud_texture_layout),
            &[],
            wgpu::BlendState::ALPHA_BLENDING,
            wgpu::PrimitiveTopology::TriangleStrip,
        );

        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rizref-globals"),
            size: GLOBALS_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rizref-globals-bind-group"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rizref-instances"),
            size: INITIAL_INSTANCE_CAPACITY as u64 * 64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let overlay_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rizref-overlay"),
            size: INITIAL_OVERLAY_CAPACITY as u64 * 24,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            globals_buffer,
            globals_bind_group,
            texture_layout,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            item_count: 0,
            overlay_buffer,
            overlay_capacity: INITIAL_OVERLAY_CAPACITY,
            overlay_count: 0,
            hud_texture: None,
            hud_bind_group: None,
            hud_texture_size: (0, 0),
            ui_texture: None,
            ui_bind_group: None,
            ui_texture_size: (0, 0),
            background_pipeline,
            item_pipeline,
            overlay_pipeline,
            hud_pipeline,
            ui_pipeline,
        }
    }

    /// The bind group layout every item texture must use.
    pub fn item_texture_layout(&self) -> wgpu::BindGroupLayout {
        self.texture_layout.clone()
    }

    /// Updates the per-frame globals from the camera and scene rect.
    ///
    /// `hud_size` rides along here because the HUD vertex shader reads it from
    /// the same uniform block.
    #[allow(clippy::too_many_arguments)]
    pub fn set_globals(
        &self,
        queue: &wgpu::Queue,
        camera: &Camera,
        viewport: DVec2,
        physical_scale: f64,
        scene_rect: Option<Rect>,
        hud_size: (f32, f32),
        flags: FrameFlags,
    ) {
        let mut globals = Globals::new(camera, viewport, physical_scale, scene_rect);
        globals.hud_size = [hud_size.0, hud_size.1];
        globals.apply_flags(flags);
        queue.write_buffer(&self.globals_buffer, 0, &globals.bytes());
    }

    /// Uploads the frame's item instances, growing the buffer if needed.
    pub fn set_items(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, items: &[ItemQuad]) {
        let bytes: Vec<u8> = items
            .iter()
            .flat_map(|item| item.instance.bytes())
            .collect();
        if bytes.len() > self.instance_capacity * 64 {
            self.instance_capacity = self.instance_capacity.max(items.len()) * 2;
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rizref-instances"),
                size: self.instance_capacity as u64 * 64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.instance_buffer, 0, &bytes);
        self.item_count = items.len() as u32;
    }

    /// Uploads the frame's overlay triangles, growing the buffer if needed.
    pub fn set_overlay(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[OverlayVertex],
    ) {
        let bytes: Vec<u8> = vertices.iter().flat_map(OverlayVertex::bytes).collect();
        if bytes.is_empty() {
            self.overlay_count = 0;
            return;
        }
        if bytes.len() > self.overlay_capacity * 24 {
            self.overlay_capacity = self.overlay_capacity.max(vertices.len()) * 2;
            self.overlay_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rizref-overlay"),
                size: self.overlay_capacity as u64 * 24,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.overlay_buffer, 0, &bytes);
        self.overlay_count = vertices.len() as u32;
    }

    /// Uploads the CPU-raster HUD readout as straight-alpha RGBA bytes, or
    /// clears the HUD when `hud` is `None`.
    ///
    /// The texture is rebuilt only when its size changes; a per-frame
    /// re-upload of a few tens of kilobytes is noise next to the present.
    pub fn set_hud(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, hud: HudUpload) {
        let (bytes, width, height) = match hud {
            HudUpload::None => {
                self.hud_bind_group = None;
                self.hud_texture = None;
                self.hud_texture_size = (0, 0);
                return;
            }
            HudUpload::Layer {
                bytes,
                width,
                height,
            } => (bytes, width, height),
        };

        if self.hud_texture_size != (width, height) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("rizref-hud"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: Self::HUD_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let layout = self.hud_pipeline.get_bind_group_layout(1);
            self.hud_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rizref-hud-bind-group"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&device.create_sampler(
                            &wgpu::SamplerDescriptor {
                                label: Some("rizref-hud-sampler"),
                                // Nearest: the HUD quad is blitted 1:1, and the
                                // non-filtering sampler binding forbids Linear.
                                mag_filter: wgpu::FilterMode::Nearest,
                                min_filter: wgpu::FilterMode::Nearest,
                                ..Default::default()
                            },
                        )),
                    },
                ],
            }));
            self.hud_texture = Some(texture);
            self.hud_texture_size = (width, height);
        }

        let texture = self
            .hud_texture
            .as_ref()
            .expect("the texture exists after the size check");
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Uploads the cached shell layer, keeps the existing one, or releases it.
    ///
    /// The texture is rebuilt only when its size changes; pixels are written on
    /// every `Layer` upload. `Keep` is what makes an idle shell free: the event
    /// loop only asks for a `Layer` when its revision moved.
    pub fn set_ui(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, ui: UiUpload) {
        let (bytes, width, height) = match ui {
            UiUpload::None => {
                self.ui_bind_group = None;
                self.ui_texture = None;
                self.ui_texture_size = (0, 0);
                return;
            }
            UiUpload::Keep => return,
            UiUpload::Layer {
                bytes,
                width,
                height,
            } => (bytes, width, height),
        };

        if self.ui_texture_size != (width, height) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("rizref-ui"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: Self::HUD_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let layout = self.ui_pipeline.get_bind_group_layout(1);
            self.ui_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rizref-ui-bind-group"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&device.create_sampler(
                            &wgpu::SamplerDescriptor {
                                label: Some("rizref-ui-sampler"),
                                mag_filter: wgpu::FilterMode::Nearest,
                                min_filter: wgpu::FilterMode::Nearest,
                                ..Default::default()
                            },
                        )),
                    },
                ],
            }));
            self.ui_texture = Some(texture);
            self.ui_texture_size = (width, height);
        }

        let texture = self
            .ui_texture
            .as_ref()
            .expect("the texture exists after the size check");
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Draws the background pass: grid, origin cross, scene-rect border.
    pub fn draw_background<'r>(&self, pass: &mut wgpu::RenderPass<'r>) {
        pass.set_pipeline(&self.background_pipeline);
        pass.set_bind_group(0, &self.globals_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Draws the item instances in scene z-order.
    ///
    /// `bind_groups` is parallel to the uploaded instances. An item without a
    /// resident texture is skipped for this frame rather than sampling an
    /// unrelated layer; the next frame uploads its preview.
    pub fn draw_items<'r>(
        &self,
        pass: &mut wgpu::RenderPass<'r>,
        bind_groups: &[Option<&wgpu::BindGroup>],
    ) {
        if self.item_count == 0 {
            return;
        }
        pass.set_pipeline(&self.item_pipeline);
        pass.set_bind_group(0, &self.globals_bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        for (index, bind_group) in bind_groups
            .iter()
            .enumerate()
            .take(self.item_count as usize)
        {
            let Some(bind_group) = bind_group else {
                continue;
            };
            pass.set_bind_group(1, *bind_group, &[]);
            pass.draw(0..4, index as u32..index as u32 + 1);
        }
    }

    /// Draws the screen-space overlay: outlines, handles, guides, marquee.
    pub fn draw_overlay<'r>(&self, pass: &mut wgpu::RenderPass<'r>) {
        if self.overlay_count == 0 {
            return;
        }
        pass.set_pipeline(&self.overlay_pipeline);
        pass.set_bind_group(0, &self.globals_bind_group, &[]);
        pass.set_vertex_buffer(0, self.overlay_buffer.slice(..));
        pass.draw(0..self.overlay_count, 0..1);
    }

    /// Draws the HUD readout quad, when one was uploaded.
    pub fn draw_hud<'r>(&self, pass: &mut wgpu::RenderPass<'r>) {
        let Some(bind_group) = self.hud_bind_group.as_ref() else {
            return;
        };
        pass.set_pipeline(&self.hud_pipeline);
        pass.set_bind_group(0, &self.globals_bind_group, &[]);
        pass.set_bind_group(1, bind_group, &[]);
        pass.draw(0..4, 0..1);
    }

    /// Draws the shell layer over the whole viewport.
    pub fn draw_ui<'r>(&self, pass: &mut wgpu::RenderPass<'r>) {
        let Some(bind_group) = self.ui_bind_group.as_ref() else {
            return;
        };
        pass.set_pipeline(&self.ui_pipeline);
        pass.set_bind_group(0, &self.globals_bind_group, &[]);
        pass.set_bind_group(1, bind_group, &[]);
        pass.draw(0..4, 0..1);
    }
}

/// Per-frame display-effect flags.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameFlags {
    /// Draw the grid and its origin axis (still hidden outside the zoom
    /// range the grid is legible in).
    pub grid: bool,
    /// Draw the dashed scene-rect border (when the board has content).
    pub scene_border: bool,
    /// Draw the checkerboard backdrop.
    pub checkerboard: bool,
    /// Draw items in grayscale.
    pub grayscale: bool,
    /// Opacity of the canvas background — fill, checkerboard, grid and
    /// boundary — on a surface that composites per-pixel alpha (phase 07:
    /// transparency is the canvas only; items, chrome and the UI stay
    /// opaque). 1.0 is opaque.
    pub canvas_alpha: f32,
}

impl Default for FrameFlags {
    fn default() -> Self {
        Self {
            grid: false,
            scene_border: false,
            checkerboard: false,
            grayscale: false,
            canvas_alpha: 1.0,
        }
    }
}

/// One frame's HUD layer, or its absence.
#[derive(Debug, Clone, Copy)]
pub enum HudUpload<'a> {
    /// No HUD this frame.
    None,
    /// Straight-alpha RGBA pixels of `width` × `height` device pixels.
    Layer {
        /// The pixels, row-major, 4 bytes per pixel.
        bytes: &'a [u8],
        /// Width in device pixels.
        width: u32,
        /// Height in device pixels.
        height: u32,
    },
}

/// One frame's shell UI layer.
#[derive(Debug, Clone, Copy)]
pub enum UiUpload<'a> {
    /// No shell UI this frame; release the texture.
    None,
    /// Reuse the texture already on the GPU (nothing in the shell changed).
    Keep,
    /// Straight-alpha RGBA pixels of `width` × `height` device pixels.
    Layer {
        /// The pixels, row-major, 4 bytes per pixel.
        bytes: &'a [u8],
        /// Width in device pixels.
        width: u32,
        /// Height in device pixels.
        height: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::{Canvas, Globals, ItemQuad, QuadInstance};
    use glam::DVec2;
    use rizref_core::{Camera, Rect};

    #[test]
    fn the_uniform_block_is_exactly_64_bytes() {
        let globals = Globals::new(
            &Camera::default(),
            DVec2::new(800.0, 600.0),
            1.5,
            Some(Rect::from_min_size(
                glam::Vec2::ZERO,
                glam::Vec2::splat(10.0),
            )),
        );
        assert_eq!(globals.bytes().len(), 80);
    }

    #[test]
    fn the_view_settings_gate_the_grid_axis_and_border() {
        let rect = Rect::from_min_size(glam::Vec2::ZERO, glam::Vec2::splat(10.0));
        let fresh = || Globals::new(&Camera::default(), DVec2::ONE, 1.0, Some(rect));

        let mut off = fresh();
        off.apply_flags(super::FrameFlags::default());
        assert_eq!(off.grid_visible, 0.0, "grid and origin axis hidden");
        assert_eq!(off.border_visible, 0.0);

        let mut on = fresh();
        on.apply_flags(super::FrameFlags {
            grid: true,
            scene_border: true,
            checkerboard: true,
            grayscale: true,
            ..super::FrameFlags::default()
        });
        assert_eq!(on.grid_visible, 1.0);
        assert_eq!(on.border_visible, 1.0);
        assert_eq!((on.checkerboard, on.grayscale), (1.0, 1.0));

        // The settings never force on what the zoom range or an empty board hides.
        let mut zoomed = Globals::new(
            &Camera::new(DVec2::ZERO, Globals::GRID_MAX_ZOOM * 2.0, 0.0),
            DVec2::ONE,
            1.0,
            None,
        );
        zoomed.apply_flags(super::FrameFlags {
            grid: true,
            scene_border: true,
            ..super::FrameFlags::default()
        });
        assert_eq!(zoomed.grid_visible, 0.0);
        assert_eq!(zoomed.border_visible, 0.0);
    }

    #[test]
    fn the_effect_flags_ride_in_the_global_block() {
        let mut globals = Globals::new(&Camera::default(), DVec2::ONE, 1.0, None);
        globals.checkerboard = 1.0;
        globals.grayscale = 1.0;
        assert_eq!(globals.bytes().len(), 80);
        assert_eq!(globals.checkerboard, 1.0);
        assert_eq!(globals.grayscale, 1.0);
    }

    #[test]
    fn the_uniform_carries_the_camera_and_flags() {
        let camera = Camera::new(DVec2::new(10.0, 20.0), 2.0, 0.5);
        let rect = Rect::from_min_size(glam::Vec2::new(1.0, 2.0), glam::Vec2::new(30.0, 40.0));
        let globals = Globals::new(&camera, DVec2::new(800.0, 600.0), 1.0, Some(rect));

        assert_eq!(globals.eye, [10.0, 20.0]);
        assert_eq!(globals.viewport, [800.0, 600.0]);
        assert_eq!(globals.zoom, 2.0);
        assert_eq!(globals.border_visible, 1.0);
        assert_eq!(globals.grid_visible, 1.0, "zoom 200% still shows the grid");
        assert_eq!(globals.scene_min, [1.0, 2.0]);
        assert_eq!(globals.scene_max, [31.0, 42.0]);
    }

    #[test]
    fn the_grid_hides_outside_the_planned_zoom_band() {
        let floor = Globals::new(
            &Camera::new(DVec2::ZERO, Globals::GRID_MIN_ZOOM, 0.0),
            DVec2::ONE,
            1.0,
            None,
        );
        assert_eq!(floor.grid_visible, 1.0, "5% is the visible floor");

        let fitted = Globals::new(
            &Camera::new(DVec2::ZERO, Camera::MIN_ZOOM, 0.0),
            DVec2::ONE,
            1.0,
            None,
        );
        assert_eq!(fitted.grid_visible, 0.0, "below 5% the grid hides");

        let zoomed_in = Globals::new(&Camera::new(DVec2::ZERO, 5.0, 0.0), DVec2::ONE, 1.0, None);
        assert_eq!(zoomed_in.grid_visible, 0.0, "above 400% the grid hides");

        let no_scene = Globals::new(&Camera::default(), DVec2::ONE, 1.0, None);
        assert_eq!(no_scene.border_visible, 0.0, "no content, no border");
    }

    #[test]
    fn an_instance_packs_to_its_64_bytes() {
        let item = QuadInstance::new([1.0, 2.0], [3.0, 4.0], 0.5);
        assert_eq!(item.bytes().len(), 64);
    }

    #[test]
    fn an_overlay_vertex_packs_to_its_24_bytes() {
        let vertex = crate::overlay::OverlayVertex {
            pos: [10.0, 20.0],
            color: [1.0, 0.5, 0.25, 0.75],
        };
        assert_eq!(vertex.bytes().len(), 24);
    }

    #[test]
    fn the_world_rect_is_the_quad_and_grows_with_rotation() {
        fn assert_corner(actual: glam::Vec2, expected: glam::Vec2) {
            assert!(
                (actual - expected).length() < 1.0e-4,
                "expected {expected:?}, got {actual:?}"
            );
        }

        let unrotated = QuadInstance::new([0.0, 0.0], [10.0, 4.0], 0.0);
        let rect = unrotated.world_rect();
        assert_corner(rect.min, glam::Vec2::new(-5.0, -2.0));
        assert_corner(rect.max(), glam::Vec2::new(5.0, 2.0));

        // A quarter turn swaps the extents (f32 pi/2 is not exact, hence the
        // tolerance rather than equality).
        let quarter = QuadInstance::new([0.0, 0.0], [10.0, 4.0], std::f32::consts::FRAC_PI_2);
        let rect = quarter.world_rect();
        assert_corner(rect.min, glam::Vec2::new(-2.0, -5.0));
        assert_corner(rect.max(), glam::Vec2::new(2.0, 5.0));
    }

    /// Renders one offscreen frame through every pipeline.
    ///
    /// This is the only test that exercises the GPU: it compiles the WGSL,
    /// validates the bind group layouts against it, uploads one real item
    /// texture, and runs all three draw paths. Machines with no usable adapter
    /// (CI runners without Vulkan/DX12) skip it rather than fail it.
    #[test]
    fn the_pipelines_render_an_offscreen_frame() {
        // A missing backend feature is a build bug, asserted by the renderer
        // tests; here it would only turn into wgpu's panic.
        assert!(crate::renderer::has_usable_backend());
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = crate::renderer::preferred_backends();
        let instance = wgpu::Instance::new(descriptor);
        // CI runners may have no GPU (or only a software one): the test then
        // skips, saying so, rather than failing.
        let adapters =
            pollster::block_on(instance.enumerate_adapters(crate::renderer::preferred_backends()));
        let adapter = adapters.into_iter().next();
        let Some(adapter) = adapter else {
            eprintln!("skipping: no GPU adapter in this environment");
            return;
        };
        let (device, queue) =
            match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("rizref-canvas-test-device"),
                ..Default::default()
            })) {
                Ok(pair) => pair,
                Err(_) => {
                    eprintln!("skipping: no usable GPU device in this environment");
                    return;
                }
            };

        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut canvas = Canvas::new(&device, format);
        canvas.set_globals(
            &queue,
            &Camera::default(),
            DVec2::new(800.0, 600.0),
            1.0,
            None,
            (0.0, 0.0),
            super::FrameFlags {
                grid: true,
                scene_border: true,
                checkerboard: true,
                grayscale: true,
                ..super::FrameFlags::default()
            },
        );

        let asset = rizref_core::AssetId::from_checksum([7; 32]);
        let mut textures = crate::textures::TextureCache::new(
            &device,
            canvas.item_texture_layout(),
            crate::textures::MAX_TEXTURE_SIDE,
        );
        textures.upload(
            &device,
            &queue,
            asset,
            &rizref_codecs::ImageBuffer::filled(16, 16, [200, 30, 40, 255]),
            false,
        );
        let items = [ItemQuad {
            instance: QuadInstance::new([0.0, 0.0], [100.0, 100.0], 0.0),
            asset,
        }];
        canvas.set_items(&device, &queue, &items);
        canvas.set_hud(&device, &queue, super::HudUpload::None);
        canvas.set_ui(
            &device,
            &queue,
            super::UiUpload::Layer {
                bytes: &[0u8; 64 * 64 * 4],
                width: 64,
                height: 64,
            },
        );
        canvas.set_overlay(
            &device,
            &queue,
            &[
                crate::overlay::OverlayVertex {
                    pos: [10.0, 10.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                },
                crate::overlay::OverlayVertex {
                    pos: [40.0, 10.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                },
                crate::overlay::OverlayVertex {
                    pos: [10.0, 40.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                },
            ],
        );

        let bind_groups = [textures.bind_group(asset)];
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rizref-canvas-test-target"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("rizref-canvas-test-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rizref-canvas-test-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                ..Default::default()
            });
            canvas.draw_background(&mut pass);
            canvas.draw_items(&mut pass, &bind_groups);
            canvas.draw_overlay(&mut pass);
            canvas.draw_hud(&mut pass);
            canvas.draw_ui(&mut pass);
        }
        queue.submit(Some(encoder.finish()));
        // Pipeline creation validates the shader and layouts synchronously;
        // submission exercises the draw calls without waiting on the GPU.
    }
}
