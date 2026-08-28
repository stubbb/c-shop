//! Pulling GPU textures back to the CPU.
//!
//! Needed for exporting a flattened image, for layer thumbnails, and for the
//! tests that check the compositor against the CPU reference implementation.

use crate::context::GpuContext;
use crate::texture::GpuTexture;
use cshop_core::color::Rgba;
use cshop_core::geom::IRect;
use cshop_core::pixels::PixelBuffer;
use half::f16;

/// `copy_texture_to_buffer` requires each row to start on a 256-byte boundary.
const COPY_ALIGN: u32 = 256;

/// Read a working-format texture back as straight-alpha, sRGB-encoded floats,
/// row-major over `rect`. Handles both `Rgba16Unorm` and `Rgba16Float`.
///
/// This stalls the GPU, so it belongs in export and test paths, never in the
/// per-frame loop.
pub fn read_work_texture(ctx: &GpuContext, tex: &GpuTexture, rect: IRect) -> Vec<Rgba> {
    let rect = rect.intersect(&IRect::from_size(tex.width, tex.height));
    if rect.is_empty() {
        return Vec::new();
    }
    let (w, h) = (rect.width(), rect.height());

    let unpadded = w * 8; // 4 channels x 2 bytes
    let padded = unpadded.div_ceil(COPY_ALIGN) * COPY_ALIGN;

    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (padded * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &tex.texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: rect.x0 as u32, y: rect.y0 as u32, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    ctx.queue.submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    ctx.wait();
    rx.recv().expect("map_async never reported").expect("buffer map failed");

    let data = slice.get_mapped_range().expect("mapped range unavailable");
    let unorm = tex.format == wgpu::TextureFormat::Rgba16Unorm;
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let row = &data[(y * padded) as usize..][..unpadded as usize];
        for px in row.chunks_exact(8) {
            let c = |i: usize| {
                let bytes = [px[i * 2], px[i * 2 + 1]];
                if unorm {
                    u16::from_le_bytes(bytes) as f32 / 65535.0
                } else {
                    f16::from_le_bytes(bytes).to_f32()
                }
            };
            out.push(Rgba::new(c(0), c(1), c(2), c(3)));
        }
    }
    drop(data);
    buffer.unmap();
    out
}

/// Read an 8-bit texture (`Rgba8Unorm` or `Rgba8UnormSrgb`) back verbatim.
///
/// The stored bytes are already in the document's encoding, so they go straight
/// into a PNG with no conversion.
pub fn read_srgb8(ctx: &GpuContext, tex: &GpuTexture) -> PixelBuffer {
    let (w, h) = (tex.width, tex.height);
    let unpadded = w * 4;
    let padded = unpadded.div_ceil(COPY_ALIGN) * COPY_ALIGN;

    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("srgb readback"),
        size: (padded * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("srgb readback") });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &tex.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    ctx.queue.submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    ctx.wait();
    rx.recv().expect("map_async never reported").expect("buffer map failed");

    let data = slice.get_mapped_range().expect("mapped range unavailable");
    let mut out = Vec::with_capacity((w * h) as usize * 4);
    for y in 0..h {
        out.extend_from_slice(&data[(y * padded) as usize..][..unpadded as usize]);
    }
    drop(data);
    buffer.unmap();

    PixelBuffer::from_rgba_bytes(w, h, &out).expect("readback returned the wrong byte count")
}

/// Read a working-format texture back as an 8-bit image, ready to encode.
pub fn read_as_pixels(ctx: &GpuContext, tex: &GpuTexture, rect: IRect) -> PixelBuffer {
    let rect = rect.intersect(&IRect::from_size(tex.width, tex.height));
    let floats = read_work_texture(ctx, tex, rect);
    let pixels = floats.into_iter().map(|c| c.to_u8()).collect();
    PixelBuffer::from_pixels(rect.width(), rect.height(), pixels)
        .unwrap_or_else(|| PixelBuffer::new(rect.width(), rect.height()))
}
