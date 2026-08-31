//! Thin wrapper bundling a texture with its view and size.

use crate::context::GpuContext;

/// The compositor's working format: straight alpha holding sRGB-encoded
/// components at 16 bits per channel, which is what keeps stacked blend and
/// adjustment layers from banding.
///
/// Read it through [`crate::context::GpuContext::work_format`] rather than
/// directly, so the choice can vary per device later.
pub const WORK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Format handed to egui for display: 8-bit, premultiplied alpha, holding
/// **sRGB-encoded** components.
///
/// Deliberately **not** `*Srgb`. egui's renderer says so itself — "we expect
/// normal textures that are NOT sRGB-aware" — and its fragment shader treats
/// whatever it samples as gamma-encoded before converting to linear for the
/// framebuffer. An `*Srgb` texture would be linearised once by the hardware on
/// sample and then again by that shader, which showed the whole canvas two
/// stops dark while the saved file stayed correct.
pub const DISPLAY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Layer pixel format. Deliberately **not** `*Srgb`: the shader wants the raw
/// encoded values, not hardware-linearised ones.
pub const LAYER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Format for a layer that holds sixteen bits a channel.
///
/// Half-float rather than sixteen-bit unorm, because unorm at that width needs
/// a device feature that is not everywhere and this is core: it is also the
/// format the compositor already works in, so a deep layer arrives in the
/// space the blending happens in rather than being converted on the way.
pub const DEEP_LAYER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Mask coverage format.
pub const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

pub struct GpuTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
}

impl std::fmt::Debug for GpuTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuTexture")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .finish()
    }
}

impl GpuTexture {
    pub fn new(
        ctx: &GpuContext,
        label: &str,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view, width, height, format }
    }

    /// A render target that can also be sampled and copied — what every
    /// compositor scratch buffer needs.
    pub fn render_target(
        ctx: &GpuContext,
        label: &str,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self::new(
            ctx,
            label,
            width,
            height,
            format,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
        )
    }

    /// A sampled-only texture uploaded from the CPU.
    pub fn sampled(
        ctx: &GpuContext,
        label: &str,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self::new(
            ctx,
            label,
            width,
            height,
            format,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
        )
    }

    pub fn size(&self) -> wgpu::Extent3d {
        wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 }
    }

    /// Upload a full-size image. `bytes_per_pixel` must match `self.format`.
    pub fn write(&self, ctx: &GpuContext, data: &[u8], bytes_per_pixel: u32) {
        self.write_region(ctx, data, bytes_per_pixel, 0, 0, self.width, self.height);
    }

    /// Upload a sub-rectangle. `data` must be tightly packed and exactly
    /// `w * h * bytes_per_pixel` long.
    #[allow(clippy::too_many_arguments)]
    pub fn write_region(
        &self,
        ctx: &GpuContext,
        data: &[u8],
        bytes_per_pixel: u32,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        debug_assert_eq!(data.len(), (w * h * bytes_per_pixel) as usize);
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * bytes_per_pixel),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }
}

/// A colour transform on the card: `size³` samples with a linear filter over
/// them.
///
/// Built on the processor by the colour engine and uploaded when the profiles
/// change, which is rarely — opening a document, choosing a display profile,
/// switching soft proofing on. Between those it is read once per pixel per
/// frame and costs nothing worth measuring.
pub struct ColourTable {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub size: u32,
}

impl ColourTable {
    /// Upload a table. `data` is `size³` RGBA bytes, blue slowest.
    pub fn new(ctx: &crate::context::GpuContext, size: u32, data: &[u8]) -> ColourTable {
        let size = size.clamp(2, 64);
        let extent = wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: size,
        };
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("colour table"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let wanted = (size * size * size * 4) as usize;
        let mut bytes = data.to_vec();
        bytes.resize(wanted, 255);
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            extent,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = ctx.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("colour table"),
            // Linear between the grid points, clamped at the ends: a colour
            // outside the cube does not exist, and wrapping one round to the
            // other side would be spectacular.
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        ColourTable { texture, view, sampler, size }
    }

    /// The table that changes nothing, for a document already in the display's
    /// own space.
    pub fn identity(ctx: &crate::context::GpuContext, size: u32) -> ColourTable {
        let size = size.clamp(2, 64);
        let n = size as usize;
        let step = |i: usize| ((i as f32 / (n - 1) as f32) * 255.0).round() as u8;
        let mut data = Vec::with_capacity(n * n * n * 4);
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    data.extend_from_slice(&[step(r), step(g), step(b), 255]);
                }
            }
        }
        ColourTable::new(ctx, size, &data)
    }
}
