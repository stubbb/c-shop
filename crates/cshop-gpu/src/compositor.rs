//! GPU layer compositor.
//!
//! # Why ping-pong
//!
//! Fixed-function blending cannot express Multiply, Overlay or Luminosity
//! together with correct alpha compositing, so each layer is drawn as a
//! full-region pass that *samples* the backdrop instead of blending into it.
//! Two scratch textures alternate roles: read from one, write the other, swap.
//!
//! # Why the scratch is region-sized
//!
//! Ping-pong would normally destroy dirty-rectangle rendering, because every
//! pass rewrites its whole target. Confining the scratch buffers to the dirty
//! region restores it: the stack runs for that region only, and the result is
//! copied back into the persistent full-document composite.
//!
//! # Tiling
//!
//! Scratch buffers are capped at [`MAX_TILE`] square. Without that cap they
//! scale with the document: a 24 MP canvas would need three 6000x4000 16-bit
//! targets *per nesting level*, which is most of a gigabyte before a single
//! layer is uploaded. A region larger than the cap is composited tile by tile
//! instead, so scratch memory is a constant no matter how big the image is.
//!
//! # Groups
//!
//! A *pass-through* group with no mask and full opacity is inlined into its
//! parent's pass sequence, which is exactly what pass-through means. Any other
//! group is composited into its own scratch pair one level deeper, then drawn
//! into the parent as if it were a single layer.

use crate::context::GpuContext;
use crate::layers::LayerTextures;
use crate::texture::{GpuTexture, DISPLAY_FORMAT, LAYER_FORMAT, MASK_FORMAT};
use bytemuck::{Pod, Zeroable};
use cshop_core::blend::BlendMode;
use cshop_core::document::Document;
use cshop_core::geom::IRect;
use cshop_core::layer::{FillStyle, Layer, LayerId, LayerKind};

/// Uniform block for one layer pass. Must match `Params` in `composite.wgsl`
/// byte for byte.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct LayerParams {
    solid_color: [f32; 4],
    region_origin: [i32; 2],
    layer_origin: [i32; 2],
    layer_size: [u32; 2],
    mask_origin: [i32; 2],
    mask_size: [u32; 2],
    blend_mode: u32,
    opacity: f32,
    flags: u32,
    adjust_kind: u32,
    _pad: [u32; 2],
    adjust_params: [[f32; 4]; 4],
}

const FLAG_HAS_MASK: u32 = 1;
const FLAG_CLIPPING: u32 = 2;
const FLAG_SOLID: u32 = 4;
const FLAG_ADJUSTMENT: u32 = 8;

/// Uniform buffer offsets must be a multiple of this, so each pass's params
/// occupy a padded slot.
const PARAM_STRIDE: u64 = 256;

/// Scratch allocations are rounded up to this, so panning and resizing do not
/// reallocate on every frame.
const SCRATCH_GRANULARITY: u32 = 256;

/// Largest region composited in one go, per side.
///
/// Chosen to bound scratch memory (three 2048x2048 16-bit targets is ~100 MB
/// per nesting level) while keeping the number of tiles — and so the number of
/// render passes — low enough that per-pass overhead stays in the noise.
pub const MAX_TILE: u32 = 2048;

/// Ping-pong pair plus the clipping-base capture, for one nesting depth.
struct Scratch {
    a: GpuTexture,
    b: GpuTexture,
    clip: GpuTexture,
    /// `false` = `a` holds the current result, `true` = `b` does.
    front_is_b: bool,
}

impl Scratch {
    fn new(ctx: &GpuContext, depth: usize, w: u32, h: u32) -> Self {
        let f = ctx.work_format();
        Self {
            a: GpuTexture::render_target(ctx, &format!("scratch {depth}a"), w, h, f),
            b: GpuTexture::render_target(ctx, &format!("scratch {depth}b"), w, h, f),
            clip: GpuTexture::render_target(ctx, &format!("clip {depth}"), w, h, f),
            front_is_b: false,
        }
    }

    fn fits(&self, w: u32, h: u32) -> bool {
        self.a.width >= w && self.a.height >= h
    }

    fn front(&self) -> &GpuTexture {
        if self.front_is_b {
            &self.b
        } else {
            &self.a
        }
    }

    fn back(&self) -> &GpuTexture {
        if self.front_is_b {
            &self.a
        } else {
            &self.b
        }
    }
}

/// Owns the compositing pipelines and the scratch pool.
pub struct Compositor {
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    present_pipeline: wgpu::RenderPipeline,
    present_layout: wgpu::BindGroupLayout,

    scratch: Vec<Scratch>,
    params: wgpu::Buffer,
    params_capacity: u64,

    /// Stand-ins so every binding slot is always filled.
    dummy_layer: GpuTexture,
    dummy_mask: GpuTexture,
    dummy_work: GpuTexture,

    /// Passes issued during the last composite, for the status bar.
    pub last_pass_count: u32,
}

impl Compositor {
    pub fn new(ctx: &GpuContext) -> Self {
        let device = &ctx.device;

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite bind layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<LayerParams>() as u64
                        ),
                    },
                    count: None,
                },
                // backdrop, layer, mask, clip — all read with textureLoad, so
                // no sampler is needed and filtering is irrelevant.
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                texture_entry(4),
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/composite.wgsl").into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite pipeline layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: ctx.work_format(),
                    // The shader does the blending itself and writes the final
                    // value, so the blend unit must stay out of the way.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // --- present pipeline ----------------------------------------------
        let present_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("present bind layout"),
            entries: &[texture_entry(0)],
        });
        let present_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("present.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/present.wgsl").into()),
        });
        let ppl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("present pipeline layout"),
            bind_group_layouts: &[Some(&present_layout)],
            immediate_size: 0,
        });
        let present_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("present"),
            layout: Some(&ppl),
            vertex: wgpu::VertexState {
                module: &present_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &present_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: DISPLAY_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("composite params"),
            size: PARAM_STRIDE * 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let dummy_layer = GpuTexture::sampled(ctx, "dummy layer", 1, 1, LAYER_FORMAT);
        dummy_layer.write(ctx, &[0, 0, 0, 0], 4);
        let dummy_mask = GpuTexture::sampled(ctx, "dummy mask", 1, 1, MASK_FORMAT);
        dummy_mask.write(ctx, &[255], 1);
        // Bound as the backdrop for clip-base passes, so its contents matter:
        // a freshly created render target is undefined until something writes
        // it.
        let dummy_work = GpuTexture::render_target(ctx, "dummy work", 1, 1, ctx.work_format());
        {
            let mut encoder = ctx.device.create_command_encoder(
                &wgpu::CommandEncoderDescriptor { label: Some("clear dummy") },
            );
            clear_pass(&mut encoder, &dummy_work, "clear dummy");
            ctx.queue.submit(Some(encoder.finish()));
        }

        Self {
            pipeline,
            bind_layout,
            present_pipeline,
            present_layout,
            scratch: Vec::new(),
            params,
            params_capacity: 64,
            dummy_layer,
            dummy_mask,
            dummy_work,
            last_pass_count: 0,
        }
    }

    /// Composite `region` of `doc` into `dest`, a full-document texture in
    /// [`GpuContext::work_format`].
    ///
    /// `region` is clamped to the document, so callers can pass a generous
    /// dirty rect without bounds-checking it first.
    pub fn composite(
        &mut self,
        ctx: &GpuContext,
        doc: &Document,
        cache: &LayerTextures,
        dest: &GpuTexture,
        region: IRect,
    ) {
        let region = region.intersect(&doc.bounds());
        if region.is_empty() {
            self.last_pass_count = 0;
            return;
        }

        self.last_pass_count = 0;
        // Walk the region in tiles so scratch memory stays bounded. Small
        // regions — a brush dab, say — are a single tile and take the same path.
        let mut y = region.y0;
        while y < region.y1 {
            let y1 = (y + MAX_TILE as i32).min(region.y1);
            let mut x = region.x0;
            while x < region.x1 {
                let x1 = (x + MAX_TILE as i32).min(region.x1);
                self.composite_tile(ctx, doc, cache, dest, IRect::new(x, y, x1, y1));
                x = x1;
            }
            y = y1;
        }
    }

    /// Composite one tile, which is guaranteed to be at most [`MAX_TILE`] on a
    /// side.
    fn composite_tile(
        &mut self,
        ctx: &GpuContext,
        doc: &Document,
        cache: &LayerTextures,
        dest: &GpuTexture,
        region: IRect,
    ) {
        let plan = Plan::build(doc, region);
        self.ensure_scratch(ctx, plan.max_depth + 1, region.width(), region.height());
        self.ensure_params(ctx, plan.passes.len() as u64);

        // Upload every pass's uniforms in one go.
        let mut bytes = vec![0u8; plan.passes.len() * PARAM_STRIDE as usize];
        for (i, pass) in plan.passes.iter().enumerate() {
            let start = i * PARAM_STRIDE as usize;
            let size = std::mem::size_of::<LayerParams>();
            bytes[start..start + size].copy_from_slice(bytemuck::bytes_of(&pass.params));
        }
        ctx.queue.write_buffer(&self.params, 0, &bytes);

        let mut encoder =
            ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("composite"),
            });

        for depth in 0..=plan.max_depth {
            self.scratch[depth].front_is_b = false;
        }
        // Clear the base of every depth that will be used.
        for depth in 0..=plan.max_depth {
            clear_pass(&mut encoder, &self.scratch[depth].a, "clear scratch");
        }

        for (i, pass) in plan.passes.iter().enumerate() {
            match pass.op {
                Op::Draw { source, depth } => {
                    self.draw_pass(ctx, &mut encoder, cache, i, pass, source, depth, region, false);
                }
                Op::DrawClipBase { source, depth } => {
                    self.draw_pass(ctx, &mut encoder, cache, i, pass, source, depth, region, true);
                }
            }
        }

        // The finished region lives in depth 0's front buffer.
        let front = self.scratch[0].front();
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &front.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &dest.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: region.x0.max(0) as u32,
                    y: region.y0.max(0) as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: region.width(),
                height: region.height(),
                depth_or_array_layers: 1,
            },
        );

        ctx.queue.submit(Some(encoder.finish()));
        self.last_pass_count += plan.passes.len() as u32;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_pass(
        &self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        cache: &LayerTextures,
        index: usize,
        pass: &Pass,
        source: Source,
        depth: usize,
        region: IRect,
        into_clip: bool,
    ) {
        let s = &self.scratch[depth];

        // Resolve the source texture. A group's result is the front buffer of
        // the depth below, which by now holds that group's finished composite.
        let layer_tex = match source {
            Source::Solid => &self.dummy_layer,
            // Adjustments bind their baked table into the same slot.
            Source::Pixels(id) | Source::Adjustment(id) => {
                cache.pixels(id).unwrap_or(&self.dummy_layer)
            }
            Source::Group { depth: child } => self.scratch[child].front(),
        };
        let mask_tex = match pass.mask_of {
            Some(id) => cache.mask(id).unwrap_or(&self.dummy_mask),
            None => &self.dummy_mask,
        };
        let clip_tex =
            if pass.params.flags & FLAG_CLIPPING != 0 { &s.clip } else { &self.dummy_work };

        // A clip-base pass renders the layer alone over transparency, so its
        // backdrop is the empty dummy and its target is the clip buffer.
        let backdrop = if into_clip { &self.dummy_work } else { s.front() };
        let target = if into_clip { &s.clip } else { s.back() };

        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite pass"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.params,
                        offset: index as u64 * PARAM_STRIDE,
                        size: wgpu::BufferSize::new(std::mem::size_of::<LayerParams>() as u64),
                    }),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&backdrop.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&layer_tex.view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&mask_tex.view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&clip_tex.view) },
            ],
        });

        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("layer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // Scratch may be larger than the region; restrict rasterisation so
            // the full-screen triangle covers exactly the region.
            rp.set_viewport(0.0, 0.0, region.width() as f32, region.height() as f32, 0.0, 1.0);
            rp.set_pipeline(&self.pipeline);
            rp.set_bind_group(0, &bind, &[]);
            rp.draw(0..3, 0..1);
        }

        // Swap: what we just wrote becomes the backdrop for the next pass. A
        // clip-base pass wrote to the clip buffer instead, so the ping-pong
        // stays where it is.
        if !into_clip {
            self.flip(depth);
        }
    }

    /// Flip the ping-pong role at `depth`.
    ///
    /// Interior mutability via a raw pointer is deliberate: the render pass
    /// holds shared borrows of both scratch textures for its whole lifetime,
    /// and only this one `bool` needs to change.
    fn flip(&self, depth: usize) {
        let s = &self.scratch[depth];
        let p = std::ptr::addr_of!(s.front_is_b) as *mut bool;
        // SAFETY: `front_is_b` is a plain `bool` that nothing else aliases
        // during a composite, and compositing is single-threaded.
        unsafe { *p = !*p };
    }

    fn ensure_scratch(&mut self, ctx: &GpuContext, depths: usize, w: u32, h: u32) {
        let w = round_up(w, SCRATCH_GRANULARITY).min(MAX_TILE);
        let h = round_up(h, SCRATCH_GRANULARITY).min(MAX_TILE);
        while self.scratch.len() < depths {
            let d = self.scratch.len();
            self.scratch.push(Scratch::new(ctx, d, w, h));
        }
        for (d, s) in self.scratch.iter_mut().enumerate().take(depths) {
            if !s.fits(w, h) {
                *s = Scratch::new(ctx, d, w.max(s.a.width), h.max(s.a.height));
            }
        }
    }

    fn ensure_params(&mut self, ctx: &GpuContext, passes: u64) {
        let want = passes.max(1);
        if want <= self.params_capacity {
            return;
        }
        let cap = want.next_power_of_two();
        self.params = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("composite params"),
            size: PARAM_STRIDE * cap,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.params_capacity = cap;
    }

    /// Convert a working-format composite into the 8-bit sRGB premultiplied
    /// texture egui draws.
    pub fn present(&self, ctx: &GpuContext, src: &GpuTexture, dest: &GpuTexture) {
        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present"),
            layout: &self.present_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&src.view),
            }],
        });
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("present") });
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dest.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&self.present_pipeline);
            rp.set_bind_group(0, &bind, &[]);
            rp.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn clear_pass(encoder: &mut wgpu::CommandEncoder, target: &GpuTexture, label: &str) {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &target.view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
}

fn round_up(v: u32, to: u32) -> u32 {
    v.div_ceil(to) * to
}

// ---------------------------------------------------------------------------
// Pass planning
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Source {
    Pixels(LayerId),
    Solid,
    /// Recolours the backdrop; the layer's texture holds its lookup table.
    Adjustment(LayerId),
    /// Result already composited at this scratch depth.
    Group { depth: usize },
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Draw { source: Source, depth: usize },
    /// Draw the clipping base on its own, over transparency, into the clip
    /// buffer.
    ///
    /// Snapshotting the running composite instead would clip to the
    /// *accumulated* alpha, so an opaque layer anywhere below would stop the
    /// clip restricting anything at all. A clipping group clips to the base layer's
    /// own alpha, and so does this.
    DrawClipBase { source: Source, depth: usize },
}

struct Pass {
    op: Op,
    params: LayerParams,
    mask_of: Option<LayerId>,
}

impl Pass {
    /// The same layer drawn alone into the clip buffer: its own blend mode and
    /// clipping flag are irrelevant there, only its alpha matters.
    fn as_clip_base(&self, source: Source, depth: usize) -> Pass {
        let mut params = self.params;
        params.blend_mode = BlendMode::Normal as u32;
        params.flags &= !FLAG_CLIPPING;
        Pass { op: Op::DrawClipBase { source, depth }, params, mask_of: self.mask_of }
    }
}

/// Flattens the layer tree into an ordered list of GPU passes.
///
/// Doing this before touching the encoder keeps the recursion, the group rules
/// and the clipping bookkeeping in one readable place, and makes the pass list
/// straightforward to assert on in tests.
struct Plan {
    passes: Vec<Pass>,
    max_depth: usize,
}

impl Plan {
    fn build(doc: &Document, region: IRect) -> Plan {
        let mut plan = Plan { passes: Vec::new(), max_depth: 0 };
        plan.walk(doc, None, 0, region);
        plan
    }

    /// Emit passes for the children of `parent` into scratch level `depth`.
    fn walk(&mut self, doc: &Document, parent: Option<LayerId>, depth: usize, region: IRect) {
        self.max_depth = self.max_depth.max(depth);
        let children = doc.tree.children(parent).to_vec();

        for (i, &id) in children.iter().enumerate() {
            let Some(layer) = doc.tree.get(id) else { continue };
            if !layer.contributes() {
                continue;
            }

            // A clipping run needs its base layer's alpha captured first.
            let next_clips = children
                .get(i + 1)
                .and_then(|&n| doc.tree.get(n))
                .is_some_and(|n| n.clipping && n.contributes());

            // Where the layer's own pass will land, so a clip-base copy of it
            // can be appended straight after.
            let first_pass = self.passes.len();

            match &layer.kind {
                LayerKind::Group { .. } if is_inlined(layer) => {
                    // Pass-through: children composite straight onto this
                    // backdrop, so no isolation and no extra depth.
                    self.walk(doc, Some(id), depth, region);
                }
                LayerKind::Group { .. } => {
                    let child_depth = depth + 1;
                    self.walk(doc, Some(id), child_depth, region);
                    self.emit(
                        doc,
                        layer,
                        Source::Group { depth: child_depth },
                        depth,
                        region,
                        // The group's result is region-aligned, so it covers
                        // the whole region.
                        region,
                    );
                }
                // Type and shapes carry their own raster, so they draw
                // exactly like one.
                LayerKind::Raster(_) | LayerKind::Text(_) | LayerKind::Shape(_) => {
                    let bounds = layer.bounds();
                    // Skip layers that fall entirely outside the dirty region.
                    if !bounds.intersects(&region) && !next_clips {
                        continue;
                    }
                    self.emit(doc, layer, Source::Pixels(id), depth, region, bounds);
                }
                LayerKind::Fill(_) => {
                    self.emit(doc, layer, Source::Solid, depth, region, region);
                }
                LayerKind::Adjustment(adj) => {
                    // A neutral adjustment costs a full-region pass for
                    // nothing, so skip it.
                    if adj.is_identity() {
                        continue;
                    }
                    self.emit(doc, layer, Source::Adjustment(id), depth, region, region);
                }
            }

            if next_clips && !layer.clipping {
                // Re-draw this layer alone into the clip buffer. Without a pass
                // of its own there is nothing to copy — a pass-through group,
                // for instance, emitted several — so fall back to the last one.
                if let Some(base) = self.passes.get(first_pass).or_else(|| self.passes.last()) {
                    if let Op::Draw { source, depth } = base.op {
                        let clip_pass = base.as_clip_base(source, depth);
                        self.passes.push(clip_pass);
                    }
                }
            }
        }
    }

    fn emit(
        &mut self,
        _doc: &Document,
        layer: &Layer,
        source: Source,
        depth: usize,
        region: IRect,
        bounds: IRect,
    ) {
        let mut flags = 0u32;
        let mut mask_of = None;
        let mut mask_origin = [0i32; 2];
        let mut mask_size = [0u32; 2];

        if let Some(m) = &layer.mask {
            if m.enabled {
                flags |= FLAG_HAS_MASK;
                mask_of = Some(layer.id);
                mask_origin = [m.offset.0, m.offset.1];
                mask_size = [m.data.width(), m.data.height()];
            }
        }
        if layer.clipping {
            flags |= FLAG_CLIPPING;
        }

        let solid_color = match (&layer.kind, source) {
            (LayerKind::Fill(FillStyle::Solid(c)), _) => {
                flags |= FLAG_SOLID;
                c.to_f32().to_array()
            }
            _ => [0.0; 4],
        };

        let (adjust_kind, adjust_params) = match &layer.kind {
            LayerKind::Adjustment(adj) => {
                flags |= FLAG_ADJUSTMENT;
                (adj.kind() as u32, adj.gpu_params())
            }
            _ => (0, [[0.0f32; 4]; 4]),
        };

        // A group applies its own opacity to an already-composited result, so
        // fill opacity does not double up on its children.
        let opacity = match source {
            Source::Group { .. } => layer.opacity.clamp(0.0, 1.0),
            _ => layer.effective_alpha(),
        };

        // Pass-through only means anything for groups; as a layer blend mode it
        // behaves like Normal.
        let mode = if layer.blend_mode == BlendMode::PassThrough {
            BlendMode::Normal
        } else {
            layer.blend_mode
        };

        self.passes.push(Pass {
            op: Op::Draw { source, depth },
            params: LayerParams {
                solid_color,
                region_origin: [region.x0, region.y0],
                layer_origin: [bounds.x0, bounds.y0],
                layer_size: [bounds.width(), bounds.height()],
                mask_origin,
                mask_size,
                blend_mode: mode as u32,
                opacity,
                flags,
                adjust_kind,
                _pad: [0; 2],
                adjust_params,
            },
            mask_of,
        });
    }
}

/// A pass-through group with nothing of its own to apply can be inlined.
fn is_inlined(layer: &Layer) -> bool {
    layer.blend_mode == BlendMode::PassThrough
        && layer.opacity >= 1.0
        && layer.mask.is_none()
        && !layer.clipping
}
