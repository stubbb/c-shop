//! GPU-side mirror of the document's layer pixels.
//!
//! Every raster layer and every layer mask gets a texture that is kept in step
//! with the CPU buffer. Uploads are restricted to the dirty rectangle, so a
//! brush dab costs its own bounding box rather than the whole layer.

use crate::context::GpuContext;
use crate::texture::{GpuTexture, LAYER_FORMAT, MASK_FORMAT};
use ahash::{AHashMap, AHashSet};
use cshop_core::document::{Dirty, Document, DocumentId};
use cshop_core::geom::IRect;
use cshop_core::layer::{Layer, LayerId, LayerKind};

struct Entry {
    pixels: Option<GpuTexture>,
    mask: Option<GpuTexture>,
    /// Whether what is in `pixels` is a composition — the layer with its
    /// filters and effects run over it — rather than the layer's own pixels.
    /// Switching a stack off has to reach the GPU even when no pixel changed.
    composed: bool,
}

/// Texture cache keyed by layer id.
#[derive(Default)]
pub struct LayerTextures {
    entries: AHashMap<LayerId, Entry>,
    /// Which document these entries belong to. Layer ids restart at 1 in each
    /// document, so without this a cache reused across documents would hand
    /// back the wrong pixels.
    doc: Option<DocumentId>,
    /// Scratch buffer reused for packing sub-rectangles before upload.
    staging: Vec<u8>,
    /// Set when the document needs more texture memory than the GPU budget
    /// allows, so the UI can say so instead of showing a half-rendered image.
    over_budget: Option<(u64, u64)>,
}

impl LayerTextures {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pixels(&self, id: LayerId) -> Option<&GpuTexture> {
        self.entries.get(&id)?.pixels.as_ref()
    }

    pub fn mask(&self, id: LayerId) -> Option<&GpuTexture> {
        self.entries.get(&id)?.mask.as_ref()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `Some((needed, budget))` when the document does not fit in the GPU
    /// texture budget. While this is set, the composite is incomplete.
    pub fn over_budget(&self) -> Option<(u64, u64)> {
        self.over_budget
    }

    /// Approximate VRAM held by the cache, for the status bar.
    pub fn memory_bytes(&self) -> u64 {
        self.entries
            .values()
            .map(|e| {
                let p = e.pixels.as_ref().map_or(0, |t| t.width as u64 * t.height as u64 * 4);
                let m = e.mask.as_ref().map_or(0, |t| t.width as u64 * t.height as u64);
                p + m
            })
            .sum()
    }

    /// Drop everything.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.doc = None;
        self.over_budget = None;
    }

    /// Texture bytes `doc` would need if every layer were resident.
    pub fn required_bytes(doc: &Document) -> u64 {
        doc.tree
            .iter_all()
            .into_iter()
            .filter_map(|id| doc.tree.get(id))
            .map(|l| {
                let px = match &l.kind {
                    LayerKind::Raster(p) => p.width() as u64 * p.height() as u64 * 4,
                    // A lookup table, which is negligible but not nothing.
                    LayerKind::Adjustment(_) => 1024,
                    _ => 0,
                };
                let mask =
                    l.mask.as_ref().map_or(0, |m| m.data.width() as u64 * m.data.height() as u64);
                px + mask
            })
            .sum()
    }

    /// Bring the cache in line with `doc`.
    ///
    /// `dirty` narrows the work: layers it names are re-uploaded over
    /// `dirty.rect` only. A layer that is new, resized, or absent from the
    /// cache is uploaded in full regardless.
    ///
    /// Callers own the contract that edited pixels appear in `dirty.layers`;
    /// an unreported edit will keep showing the previous contents.
    pub fn sync(&mut self, ctx: &GpuContext, doc: &Document, dirty: &Dirty) {
        if self.doc != Some(doc.id) {
            self.entries.clear();
            self.doc = Some(doc.id);
            self.over_budget = None;
        }

        // Pre-flight the allocation. Asking the driver for more than it has
        // yields textures that silently fail to bind, which shows up as layers
        // vanishing from the canvas with no explanation.
        let needed = Self::required_bytes(doc);
        let budget = ctx.texture_budget();
        if needed > budget {
            if self.over_budget.is_none() {
                log::error!(
                    "document needs {needed} bytes of layer textures, over the {budget} byte \
                     budget for this GPU; the canvas will be incomplete"
                );
            }
            self.over_budget = Some((needed, budget));
            return;
        }
        self.over_budget = None;

        let mut live: AHashSet<LayerId> = AHashSet::new();

        for id in doc.tree.iter_all() {
            let Some(layer) = doc.tree.get(id) else { continue };
            live.insert(id);
            self.sync_layer(ctx, layer, dirty);
        }

        // Textures for layers that no longer exist are freed here rather than
        // at delete time, so undo can re-add a layer without a special case.
        self.entries.retain(|id, _| live.contains(id));
    }

    /// Put a whole eight-bit picture into a layer's texture, making it if the
    /// size has changed.
    fn upload_whole(ctx: &GpuContext, entry: &mut Entry, id: u64, px: &cshop_core::pixels::PixelBuffer) {
        let fresh = entry
            .pixels
            .as_ref()
            .is_none_or(|t| t.width != px.width() || t.height != px.height());
        if fresh {
            entry.pixels = Some(GpuTexture::sampled(
                ctx,
                &format!("layer {id} pixels"),
                px.width(),
                px.height(),
                LAYER_FORMAT,
            ));
        }
        if let Some(tex) = &entry.pixels {
            tex.write(ctx, px.as_bytes(), 4);
        }
    }

    /// The same for a layer that holds sixteen bits a channel.
    ///
    /// The samples are unsigned integers filling 0..65535 and the texture is
    /// half-float, so they are scaled on the way. Half-float carries about
    /// eleven bits of mantissa — less than sixteen, and a great deal more than
    /// eight — and it is what the compositor blends in, so nothing is lost a
    /// second time on the way through.
    fn upload_deep(
        ctx: &GpuContext,
        entry: &mut Entry,
        id: u64,
        deep: &cshop_core::pixels::DeepBuffer,
    ) {
        use crate::texture::DEEP_LAYER_FORMAT;
        let (w, h) = (deep.width(), deep.height());
        let fresh = entry
            .pixels
            .as_ref()
            .is_none_or(|t| t.width != w || t.height != h || t.format != DEEP_LAYER_FORMAT);
        if fresh {
            entry.pixels = Some(GpuTexture::sampled(
                ctx,
                &format!("layer {id} deep pixels"),
                w,
                h,
                DEEP_LAYER_FORMAT,
            ));
        }
        let mut bytes = Vec::with_capacity(deep.pixels().len() * 8);
        for p in deep.pixels() {
            for c in [p.r, p.g, p.b, p.a] {
                bytes.extend_from_slice(
                    &half::f16::from_f32(c as f32 / 65535.0).to_le_bytes(),
                );
            }
        }
        if let Some(tex) = &entry.pixels {
            tex.write(ctx, &bytes, 8);
        }
    }

    fn sync_layer(&mut self, ctx: &GpuContext, layer: &Layer, dirty: &Dirty) {
        let entry = self.entries.entry(layer.id).or_insert(Entry { pixels: None, mask: None, composed: false });

        // --- pixels --------------------------------------------------------
        match &layer.kind {
            // Type and shapes are uploaded from their cached raster, so the
            // GPU never needs to know they are anything but pixels.
            LayerKind::Raster(_)
            | LayerKind::Text(_)
            | LayerKind::Shape(_)
            | LayerKind::Smart(_) => 'pixels: {
                // A layer with effects or filters uploads the composition of
                // them with its pixels, sized to `render_bounds`. Everything
                // from here on treats that as the layer's texture, so the
                // compositor needs no knowledge of either.
                //
                // Composing is only worth doing when something is going to be
                // uploaded. Nothing below writes to the GPU unless the layer
                // is named in `dirty` or has no texture yet, and a filter
                // stack is not cheap — a full-layer blur is a fifth of a
                // second — so working one out to throw it away would cost that
                // on every frame the document was dirty anywhere at all.
                //
                // `structure` is in the condition because a size change does
                // not name its layer — `Dirty::structural` names none at all —
                // and the full-upload branch below used to catch those by
                // comparing the texture's size against the pixels it had just
                // composed. It cannot compare what it has not computed.
                let wants = layer.has_effects() || layer.has_filters();
                let upload = dirty.structure
                    || dirty.layers.contains(&layer.id)
                    || entry.pixels.is_none()
                    || entry.composed != wants;
                if !upload {
                    // Only the pixel work is skipped; the mask below still
                    // has to be brought up to date.
                    break 'pixels;
                }
                entry.composed = wants;
                let composed = layer.render_with_effects().map(|(px, _)| px);

                // A deep layer with nothing composited over it goes up at its
                // own depth, into the format the compositor already blends in.
                // With effects it does not: those are worked out at eight bits,
                // and a layer that has been through them has already been
                // narrowed.
                if composed.is_none() {
                    if let LayerKind::Raster(cshop_core::layer::Surface::Sixteen(deep)) =
                        &layer.kind
                    {
                        Self::upload_deep(ctx, entry, layer.id.0, deep);
                        break 'pixels;
                    }
                }

                let px = match &composed {
                    Some(p) => p,
                    None => match layer.pixels() {
                        Some(p) => p,
                        // A deep layer with effects: narrowed for them, which
                        // is what the effects themselves did.
                        None => {
                            let narrowed = match &layer.kind {
                                LayerKind::Raster(s) => s.to_eight(),
                                _ => break 'pixels,
                            };
                            Self::upload_whole(ctx, entry, layer.id.0, &narrowed);
                            break 'pixels;
                        }
                    },
                };
                let needs_full = match &entry.pixels {
                    Some(t) => t.width != px.width() || t.height != px.height(),
                    None => true,
                };
                if needs_full {
                    let tex = GpuTexture::sampled(
                        ctx,
                        &format!("layer {} pixels", layer.id.0),
                        px.width(),
                        px.height(),
                        LAYER_FORMAT,
                    );
                    tex.write(ctx, px.as_bytes(), 4);
                    entry.pixels = Some(tex);
                } else if composed.is_some() {
                    // Effects and filters are re-rendered whole — a blur
                    // spreads a change well beyond the dirty rect — so the
                    // partial upload below would leave stale pixels around the
                    // edit.
                    let tex = entry.pixels.as_ref().expect("checked above");
                    tex.write(ctx, px.as_bytes(), 4);
                } else if dirty.layers.contains(&layer.id) {
                    let tex = entry.pixels.as_ref().expect("checked above");
                    // dirty.rect is document space; the texture is layer space.
                    let local = dirty
                        .rect
                        .translate(-layer.offset.0, -layer.offset.1)
                        .intersect(&px.bounds());
                    if !local.is_empty() {
                        Self::pack_rgba(&mut self.staging, px, local);
                        tex.write_region(
                            ctx,
                            &self.staging,
                            4,
                            local.x0 as u32,
                            local.y0 as u32,
                            local.width(),
                            local.height(),
                        );
                    }
                }
            }
            // An adjustment layer's "pixels" are its baked lookup table, bound
            // to the same slot: the shader knows from its flags whether to read
            // the texture as an image or as a table, so no extra binding is
            // needed.
            LayerKind::Adjustment(adj) => {
                let needs_texture = entry.pixels.as_ref().is_none_or(|t| t.width != 256);
                if needs_texture || dirty.layers.contains(&layer.id) {
                    let tex = entry.pixels.take().filter(|t| t.width == 256).unwrap_or_else(|| {
                        GpuTexture::sampled(
                            ctx,
                            &format!("layer {} lut", layer.id.0),
                            256,
                            1,
                            LAYER_FORMAT,
                        )
                    });
                    // A kilobyte, so it is always re-uploaded whole.
                    tex.write(ctx, &adj.bake_lut(), 4);
                    entry.pixels = Some(tex);
                }
            }
            // Groups and fill layers have no pixel texture.
            LayerKind::Group { .. } | LayerKind::Fill(_) => entry.pixels = None,
        }

        // --- mask ----------------------------------------------------------
        match &layer.mask {
            Some(m) => {
                let (w, h) = (m.data.width(), m.data.height());
                let needs_full = match &entry.mask {
                    Some(t) => t.width != w || t.height != h,
                    None => true,
                };
                if needs_full {
                    let tex = GpuTexture::sampled(
                        ctx,
                        &format!("layer {} mask", layer.id.0),
                        w,
                        h,
                        MASK_FORMAT,
                    );
                    tex.write(ctx, m.data.as_bytes(), 1);
                    entry.mask = Some(tex);
                } else if dirty.layers.contains(&layer.id) {
                    let tex = entry.mask.as_ref().expect("checked above");
                    let local =
                        dirty.rect.translate(-m.offset.0, -m.offset.1).intersect(&m.data.bounds());
                    if !local.is_empty() {
                        Self::pack_mask(&mut self.staging, &m.data, local);
                        tex.write_region(
                            ctx,
                            &self.staging,
                            1,
                            local.x0 as u32,
                            local.y0 as u32,
                            local.width(),
                            local.height(),
                        );
                    }
                }
            }
            None => entry.mask = None,
        }
    }

    /// Copy `rect` out of `px` into `out` as tightly packed RGBA, because
    /// `write_texture` wants contiguous rows.
    fn pack_rgba(out: &mut Vec<u8>, px: &cshop_core::pixels::PixelBuffer, rect: IRect) {
        out.clear();
        out.reserve(rect.area() as usize * 4);
        for y in rect.y0..rect.y1 {
            let row = &px.row(y as u32)[rect.x0 as usize..rect.x1 as usize];
            out.extend_from_slice(bytemuck::cast_slice(row));
        }
    }

    fn pack_mask(out: &mut Vec<u8>, m: &cshop_core::mask::MaskBuffer, rect: IRect) {
        out.clear();
        out.reserve(rect.area() as usize);
        let stride = m.width() as usize;
        for y in rect.y0..rect.y1 {
            let s = y as usize * stride;
            out.extend_from_slice(&m.as_bytes()[s + rect.x0 as usize..s + rect.x1 as usize]);
        }
    }
}
