//! Per-item GPU textures, with an LRU budget and preview fallback.
//!
//! The phase-02 renderer sampled eight placeholder layers from a texture array.
//! Real boards do not fit that shape: 200 items at 512 px previews already need
//! more layers than a GPU exposes, and an array forces every layer to the same
//! size. Items therefore own one 2D texture each, sampled through a per-item
//! bind group. That costs one draw call per visible item — hundreds of calls,
//! against a frame budget measured in milliseconds — and buys exact memory use:
//! a 300 px reference costs 300 px of VRAM.
//!
//! Two rules keep memory bounded and the board responsive:
//!
//! * images above [`MAX_TEXTURE_SIDE`] are box-filtered down for display; the
//!   original bytes stay in RAM for the file format and export;
//! * the cache holds at most [`DEFAULT_TEXTURE_BUDGET`] bytes and evicts the
//!   least-recently-used texture. An evicted item immediately redraws from its
//!   preview, which is at most 512 px and is re-uploaded on demand.

use std::collections::HashMap;

use rizref_codecs::ImageBuffer;
use rizref_core::{AssetId, fit_to_side};

/// Longest texture side this build uploads, regardless of the GPU limit.
///
/// 4096 covers a 4K reference at 100% zoom and keeps one board from exhausting
/// VRAM with a handful of 16K scans. Larger originals are still kept verbatim
/// and are downsampled only for display.
pub const MAX_TEXTURE_SIDE: u32 = 4096;

/// Default VRAM budget for item textures.
pub const DEFAULT_TEXTURE_BUDGET: u64 = 512 * 1024 * 1024;

/// Texture format for item pixels: sRGB, so sampling decodes to linear exactly
/// once and the swapchain re-encodes on present.
pub const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// One resident texture.
struct Entry {
    bind_group: wgpu::BindGroup,
    bytes: u64,
    last_used: u64,
    is_preview: bool,
}

/// The GPU-side texture store.
pub struct TextureCache {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    entries: HashMap<AssetId, Entry>,
    used_bytes: u64,
    budget_bytes: u64,
    max_side: u32,
    clock: u64,
}

impl TextureCache {
    /// Creates a cache using the item pipeline's bind group layout.
    pub fn new(device: &wgpu::Device, layout: wgpu::BindGroupLayout, max_side: u32) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rizref-item-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            layout,
            sampler,
            entries: HashMap::new(),
            used_bytes: 0,
            budget_bytes: DEFAULT_TEXTURE_BUDGET,
            max_side: max_side.clamp(1, MAX_TEXTURE_SIDE),
            clock: 0,
        }
    }

    /// Uploads `image` for `asset`, replacing any resident texture.
    ///
    /// A preview never replaces a full texture: previews exist to stand in
    /// while the full image is missing, not to downgrade one that is present.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        asset: AssetId,
        image: &ImageBuffer,
        is_preview: bool,
    ) {
        if let Some(existing) = self.entries.get(&asset)
            && !existing.is_preview
            && is_preview
        {
            self.touch(asset);
            return;
        }

        let fitted = fit_to_side(image, self.max_side);
        let image = fitted.image();
        let (width, height) = (image.width(), image.height());
        if width == 0 || height == 0 {
            return;
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rizref-item-texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            image.pixels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * ImageBuffer::BYTES_PER_PIXEL as u32),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rizref-item-texture-bind-group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let bytes = u64::from(width) * u64::from(height) * ImageBuffer::BYTES_PER_PIXEL as u64;
        self.clock += 1;
        if let Some(previous) = self.entries.insert(
            asset,
            Entry {
                bind_group,
                bytes,
                last_used: self.clock,
                is_preview,
            },
        ) {
            self.used_bytes = self.used_bytes.saturating_sub(previous.bytes);
        }
        self.used_bytes = self.used_bytes.saturating_add(bytes);
        self.trim(Some(asset));

        tracing::debug!(
            asset = %asset,
            width,
            height,
            is_preview,
            resized = fitted.is_resized(),
            vram_bytes = self.used_bytes,
            "item texture uploaded"
        );
    }

    /// The bind group for `asset`, if a texture is resident.
    pub fn bind_group(&self, asset: AssetId) -> Option<&wgpu::BindGroup> {
        self.entries.get(&asset).map(|entry| &entry.bind_group)
    }

    /// Marks `asset` as used this frame.
    pub fn touch(&mut self, asset: AssetId) {
        self.clock += 1;
        let clock = self.clock;
        if let Some(entry) = self.entries.get_mut(&asset) {
            entry.last_used = clock;
        }
    }

    /// Whether `asset` has a resident texture.
    pub fn contains(&self, asset: AssetId) -> bool {
        self.entries.contains_key(&asset)
    }

    /// Whether the resident texture for `asset` is only its preview.
    pub fn is_preview(&self, asset: AssetId) -> Option<bool> {
        self.entries.get(&asset).map(|entry| entry.is_preview)
    }

    /// Drops `asset`'s texture.
    pub fn remove(&mut self, asset: AssetId) -> bool {
        match self.entries.remove(&asset) {
            Some(entry) => {
                self.used_bytes = self.used_bytes.saturating_sub(entry.bytes);
                true
            }
            None => false,
        }
    }

    /// Evicts least-recently-used textures until the budget holds.
    pub fn trim(&mut self, keep: Option<AssetId>) {
        while self.used_bytes > self.budget_bytes && self.entries.len() > 1 {
            let Some(victim) = self
                .entries
                .iter()
                .filter(|(id, _)| Some(**id) != keep)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(id, _)| *id)
            else {
                break;
            };
            self.remove(victim);
        }
    }

    /// Number of resident textures.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// VRAM held by resident textures.
    pub fn used_bytes(&self) -> u64 {
        self.used_bytes
    }

    /// The configured budget.
    pub fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

    /// Longest side a texture may have.
    pub fn max_side(&self) -> u32 {
        self.max_side
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_TEXTURE_BUDGET, MAX_TEXTURE_SIDE};

    #[test]
    fn the_display_cap_is_below_the_default_budget() {
        let largest = u64::from(MAX_TEXTURE_SIDE) * u64::from(MAX_TEXTURE_SIDE) * 4;
        assert!(largest < DEFAULT_TEXTURE_BUDGET);
    }
}
