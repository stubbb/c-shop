//! Per-document state that only the UI cares about: the GPU textures backing
//! the canvas, the view transform, and the layer thumbnails.
//!
//! One of these exists per open tab. Each owns its own [`LayerTextures`], which
//! is what keeps two documents' layer ids from colliding in the cache.

use ahash::AHashMap;
use cshop_core::document::{Dirty, Document};
use cshop_core::geom::IRect;
use cshop_core::history::History;
use cshop_core::layer::{LayerId, LayerKind};
use cshop_gpu::compositor::Compositor;
use cshop_gpu::context::GpuContext;
use cshop_gpu::layers::LayerTextures;
use cshop_gpu::texture::{GpuTexture, DISPLAY_FORMAT};

/// Zoom stops the `+`/`-` keys and the zoom tool step through, matching
/// the conventional ladder.
pub const ZOOM_STOPS: &[f32] = &[
    0.0016, 0.0033, 0.0066, 0.0125, 0.25 / 10.0, 0.05, 0.0666, 0.0833, 0.125, 0.1666, 0.25,
    0.3333, 0.5, 0.6666, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 11.0, 16.0, 22.0, 32.0,
];

pub const MIN_ZOOM: f32 = 0.0016;
pub const MAX_ZOOM: f32 = 32.0;

/// Size of a layer thumbnail in the Layers panel, in points.
const THUMB_SIZE: u32 = 36;

/// Mask thumbnails share the thumbnail cache with layer thumbnails, keyed by
/// the layer id with this bit set. Layer ids are small counters, so the bit is
/// never otherwise in use.
const MASK_THUMB_BIT: u64 = 1 << 63;

pub struct DocView {
    pub doc: Document,
    pub history: History,
    cache: LayerTextures,

    /// Full-document composite in the compositor's working format.
    composite: GpuTexture,
    /// 8-bit sRGB premultiplied copy, which is what egui can draw.
    display: GpuTexture,
    texture_id: Option<egui::TextureId>,
    filter: wgpu::FilterMode,

    /// Edits waiting to be pushed to the GPU.
    pending: Dirty,
    /// Set when the whole composite must be rebuilt, e.g. after a resize.
    needs_full: bool,
    /// Set when the composite changed outside the UI sync path, so the display
    /// copy egui draws has to be regenerated.
    needs_present: bool,

    /// Document-space point shown at the centre of the viewport.
    pub center: egui::Vec2,
    pub zoom: f32,
    /// Set once, the first time the canvas learns how big it is.
    pub zoom_initialised: bool,

    thumbnails: AHashMap<LayerId, (egui::TextureHandle, u64)>,
    thumb_epoch: AHashMap<LayerId, u64>,
    epoch: u64,
}

impl DocView {
    pub fn new(gpu: &GpuContext, doc: Document, origin_label: &str) -> Self {
        let (w, h) = (doc.width, doc.height);
        let composite =
            GpuTexture::render_target(gpu, "composite", w, h, gpu.work_format());
        let display = GpuTexture::render_target(gpu, "display", w, h, DISPLAY_FORMAT);
        let center = egui::vec2(w as f32 / 2.0, h as f32 / 2.0);

        Self {
            doc,
            history: History::new(origin_label),
            cache: LayerTextures::new(),
            composite,
            display,
            texture_id: None,
            filter: wgpu::FilterMode::Linear,
            pending: Dirty::NONE,
            needs_full: true,
            needs_present: false,
            center,
            zoom: 1.0,
            zoom_initialised: false,
            thumbnails: AHashMap::new(),
            thumb_epoch: AHashMap::new(),
            epoch: 1,
        }
    }

    /// Record an edit so the next frame re-uploads and recomposites it.
    /// The region queued for recompositing, for tests that need to see how
    /// far an edit reached.
    pub fn pending_rect(&self) -> cshop_core::geom::IRect {
        self.pending.rect
    }

    pub fn mark_dirty(&mut self, mut dirty: Dirty) {
        if dirty.is_empty() {
            return;
        }
        // A layer's effects reach outside the pixels that changed — a blurred
        // shadow spreads an edit well past its own rect — so the region to
        // recomposite has to grow with them, or the shadow is left stale
        // around whatever was just edited.
        let reach = dirty
            .layers
            .iter()
            .filter_map(|id| self.doc.tree.get(*id))
            .filter(|l| l.has_effects())
            .map(|l| cshop_core::effects::padding(&l.effects))
            .max()
            .unwrap_or(0);
        if reach > 0 {
            dirty.rect = dirty.rect.inflate(reach);
        }
        self.epoch += 1;
        for id in &dirty.layers {
            self.thumb_epoch.insert(*id, self.epoch);
        }
        if dirty.structure {
            // A structural change can move any layer, so every thumbnail is
            // suspect and the whole canvas must be rebuilt.
            self.needs_full = true;
            self.thumb_epoch.clear();
            self.thumbnails.clear();
        }
        self.pending.merge(dirty);
    }

    /// Rebuild everything on the next frame.
    pub fn invalidate(&mut self) {
        self.needs_full = true;
        self.epoch += 1;
        self.thumbnails.clear();
    }

    /// Resize the GPU targets after the document's dimensions change.
    pub fn resize_targets(&mut self, gpu: &GpuContext) {
        let (w, h) = (self.doc.width, self.doc.height);
        if self.composite.width == w && self.composite.height == h {
            return;
        }
        self.composite = GpuTexture::render_target(gpu, "composite", w, h, gpu.work_format());
        self.display = GpuTexture::render_target(gpu, "display", w, h, DISPLAY_FORMAT);
        // The old view is gone, so the registered id must be re-pointed.
        self.texture_id = None;
        self.needs_full = true;
    }

    /// The size of the target the composite is drawn into.
    ///
    /// Exposed so a test can tell the difference between "the document is the
    /// right size" and "the thing on screen is", which is exactly where
    /// undoing a resize used to go wrong.
    pub fn composite_size(&self) -> (u32, u32) {
        (self.composite.width, self.composite.height)
    }

    /// Layer texture cache, exposed so the status bar can report VRAM use.
    pub fn cache(&self) -> &LayerTextures {
        &self.cache
    }

    /// Push pending edits to the GPU and recomposite. Cheap when nothing
    /// changed, which is the common case between frames.
    pub fn sync(
        &mut self,
        gpu: &GpuContext,
        compositor: &mut Compositor,
        renderer: &mut egui_wgpu::Renderer,
    ) {
        let wanted_filter = if self.zoom >= 1.0 {
            // Above 100%, hard pixel edges read better than a blur.
            wgpu::FilterMode::Nearest
        } else {
            wgpu::FilterMode::Linear
        };

        let region = if self.needs_full {
            self.doc.bounds()
        } else {
            self.pending.rect.intersect(&self.doc.bounds())
        };

        let needs_composite = self.needs_full || !region.is_empty();
        if needs_composite {
            self.cache.sync(gpu, &self.doc, &self.pending);
            compositor.composite(gpu, &self.doc, &self.cache, &self.composite, region);
            self.pending = Dirty::NONE;
            self.needs_full = false;
        }
        if needs_composite || self.needs_present {
            compositor.present(gpu, &self.composite, &self.display);
            self.needs_present = false;
        }

        match self.texture_id {
            None => {
                self.texture_id =
                    Some(renderer.register_native_texture(&gpu.device, &self.display.view, wanted_filter));
                self.filter = wanted_filter;
            }
            Some(id) if self.filter != wanted_filter => {
                renderer.update_egui_texture_from_wgpu_texture(
                    &gpu.device,
                    &self.display.view,
                    wanted_filter,
                    id,
                );
                self.filter = wanted_filter;
            }
            Some(_) => {}
        }
    }

    pub fn texture_id(&self) -> Option<egui::TextureId> {
        self.texture_id
    }

    /// Recomposite without touching egui, for export and colour picking.
    pub fn sync_composite_only(&mut self, gpu: &GpuContext, compositor: &mut Compositor) {
        let region = if self.needs_full {
            self.doc.bounds()
        } else {
            self.pending.rect.intersect(&self.doc.bounds())
        };
        if !self.needs_full && region.is_empty() {
            return;
        }
        self.cache.sync(gpu, &self.doc, &self.pending);
        compositor.composite(gpu, &self.doc, &self.cache, &self.composite, region);
        self.pending = Dirty::NONE;
        self.needs_full = false;
        // The display copy is now stale; rebuild it on the next UI sync.
        self.needs_present = true;
    }

    /// Read the flattened document back from the GPU.
    ///
    /// Stalls the pipeline, so this is for saving and exporting only.
    pub fn read_composite(&self, gpu: &GpuContext) -> cshop_core::pixels::PixelBuffer {
        cshop_gpu::readback::read_as_pixels(gpu, &self.composite, self.doc.bounds())
    }

    /// The same, at sixteen bits a channel.
    ///
    /// The compositor works in `Rgba16Float`, so this is not a widened
    /// eight-bit picture — it is the precision that was there all along and
    /// was being thrown away on the last step out.
    ///
    /// Half-float carries about eleven bits of mantissa, though, which is
    /// fewer than the sixteen a deep layer holds. A document that is one
    /// deep layer and nothing else therefore skips the compositor entirely
    /// and hands back the layer: there is nothing to composite, and passing
    /// through the GPU would cost bits it cannot carry. That is the shape a
    /// photograph has from opening to export, so it is the common case, not
    /// a corner of one.
    pub fn read_composite_deep(&self, gpu: &GpuContext) -> cshop_core::pixels::DeepBuffer {
        if let Some(deep) = self.doc.single_deep_layer() {
            return deep.clone();
        }
        cshop_gpu::readback::read_as_deep(gpu, &self.composite, self.doc.bounds())
    }

    /// Composited colour at a document pixel, for the eyedropper.
    pub fn sample_composite(
        &mut self,
        gpu: &GpuContext,
        x: i32,
        y: i32,
    ) -> Option<cshop_core::color::Rgba8> {
        let rect = IRect::at(x, y, 1, 1).intersect(&self.doc.bounds());
        if rect.is_empty() {
            return None;
        }
        let px = cshop_gpu::readback::read_as_pixels(gpu, &self.composite, rect);
        Some(px.get(0, 0))
    }

    /// Approximate VRAM used by this document.
    pub fn vram_bytes(&self) -> u64 {
        let px = |t: &GpuTexture, bpp: u64| t.width as u64 * t.height as u64 * bpp;
        self.cache.memory_bytes() + px(&self.composite, 8) + px(&self.display, 4)
    }

    // --- view transform ----------------------------------------------------

    /// Screen rectangle the document occupies inside `viewport`.
    pub fn canvas_rect(&self, viewport: egui::Rect) -> egui::Rect {
        let size = egui::vec2(self.doc.width as f32, self.doc.height as f32) * self.zoom;
        let centre = viewport.center() - self.center * self.zoom
            + egui::vec2(self.doc.width as f32, self.doc.height as f32) * self.zoom / 2.0;
        egui::Rect::from_min_size(centre - size / 2.0, size)
    }

    /// Screen point to document pixel coordinates.
    pub fn screen_to_doc(&self, viewport: egui::Rect, p: egui::Pos2) -> egui::Vec2 {
        let rect = self.canvas_rect(viewport);
        (p - rect.min) / self.zoom
    }

    /// Document pixel coordinates to a screen point.
    pub fn doc_to_screen(&self, viewport: egui::Rect, p: egui::Vec2) -> egui::Pos2 {
        self.canvas_rect(viewport).min + p * self.zoom
    }

    /// Zoom about a fixed screen point, so the pixel under the cursor stays
    /// under the cursor.
    pub fn zoom_to(&mut self, viewport: egui::Rect, zoom: f32, anchor: egui::Pos2) {
        let before = self.screen_to_doc(viewport, anchor);
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let after = self.screen_to_doc(viewport, anchor);
        self.center += before - after;
    }

    /// Next zoom stop above or below the current level.
    pub fn stepped_zoom(&self, up: bool) -> f32 {
        if up {
            ZOOM_STOPS
                .iter()
                .copied()
                .find(|&z| z > self.zoom * 1.001)
                .unwrap_or(MAX_ZOOM)
        } else {
            ZOOM_STOPS
                .iter()
                .rev()
                .copied()
                .find(|&z| z < self.zoom * 0.999)
                .unwrap_or(MIN_ZOOM)
        }
    }

    /// Scale the document to fit inside `viewport` with a small margin, and
    /// recentre it.
    pub fn fit_to(&mut self, viewport: egui::Rect) {
        let margin = 32.0;
        let avail = viewport.size() - egui::vec2(margin, margin);
        if avail.x <= 0.0 || avail.y <= 0.0 {
            return;
        }
        let scale =
            (avail.x / self.doc.width as f32).min(avail.y / self.doc.height as f32);
        // Never zoom past 100% just to fill the window.
        self.zoom = scale.clamp(MIN_ZOOM, 1.0);
        self.centre_document();
    }

    pub fn centre_document(&mut self) {
        self.center = egui::vec2(self.doc.width as f32 / 2.0, self.doc.height as f32 / 2.0);
    }

    // --- thumbnails --------------------------------------------------------

    /// Thumbnail for a layer, generated on demand and cached until the layer
    /// changes.
    pub fn thumbnail(&mut self, ctx: &egui::Context, id: LayerId) -> Option<egui::TextureHandle> {
        let want = self.thumb_epoch.get(&id).copied().unwrap_or(0);
        if let Some((handle, epoch)) = self.thumbnails.get(&id) {
            if *epoch == want {
                return Some(handle.clone());
            }
        }

        let layer = self.doc.tree.get(id)?;
        let image = match &layer.kind {
            // Type and shapes show their own rendering, which is more use
            // than a badge when several of them are stacked.
            LayerKind::Raster(_) | LayerKind::Text(_) | LayerKind::Shape(_) => {
                let px = layer.pixels()?;
                // Keep the aspect ratio inside a square cell.
                let (w, h) = (px.width().max(1), px.height().max(1));
                let scale = (THUMB_SIZE as f32 / w as f32).min(THUMB_SIZE as f32 / h as f32);
                let tw = ((w as f32 * scale).round() as u32).max(1);
                let th = ((h as f32 * scale).round() as u32).max(1);
                px.downscale(tw, th)
            }
            // Fill layers get a flat swatch.
            LayerKind::Fill(cshop_core::layer::FillStyle::Solid(c)) => {
                cshop_core::pixels::PixelBuffer::filled(THUMB_SIZE, THUMB_SIZE, *c)
            }
            // An adjustment's thumbnail is its own effect applied to a grey
            // ramp, so the panel shows at a glance what it does.
            LayerKind::Adjustment(adj) => {
                let mut px = cshop_core::pixels::PixelBuffer::new(THUMB_SIZE, THUMB_SIZE);
                let adj = adj.prepare();
                for y in 0..THUMB_SIZE {
                    for x in 0..THUMB_SIZE {
                        let t = x as f32 / (THUMB_SIZE - 1).max(1) as f32;
                        let out = adj.apply_rgb([t, t, t]);
                        px.set(
                            x as i32,
                            y as i32,
                            cshop_core::color::Rgba::new(out[0], out[1], out[2], 1.0).to_u8(),
                        );
                    }
                }
                px
            }
            LayerKind::Group { .. } => return None,
        };

        let pixels: Vec<egui::Color32> = image
            .pixels()
            .iter()
            .map(|p| egui::Color32::from_rgba_unmultiplied(p.r, p.g, p.b, p.a))
            .collect();
        let color_image = egui::ColorImage {
            size: [image.width() as usize, image.height() as usize],
            source_size: egui::vec2(image.width() as f32, image.height() as f32),
            pixels,
        };
        let handle = ctx.load_texture(
            format!("thumb-{}-{}", self.doc.id.0, id.0),
            color_image,
            egui::TextureOptions::LINEAR,
        );
        self.thumbnails.insert(id, (handle.clone(), want));
        Some(handle)
    }

    /// Thumbnail of a layer's mask, or `None` when it has none.
    pub fn mask_thumbnail(
        &mut self,
        ctx: &egui::Context,
        id: LayerId,
    ) -> Option<egui::TextureHandle> {
        let want = self.thumb_epoch.get(&id).copied().unwrap_or(0);
        let key = LayerId(id.0 | MASK_THUMB_BIT);
        if let Some((handle, epoch)) = self.thumbnails.get(&key) {
            if *epoch == want {
                return Some(handle.clone());
            }
        }

        let mask = self.doc.tree.get(id)?.mask.as_ref()?;
        let (w, h) = (mask.data.width().max(1), mask.data.height().max(1));
        let scale = (THUMB_SIZE as f32 / w as f32).min(THUMB_SIZE as f32 / h as f32);
        let (tw, th) = (
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
        );

        // Box-filter the coverage down, then show it as opaque grey — a mask
        // reads as a greyscale plate, not as transparency.
        //
        // Sampled at a stride for the same reason the pixel thumbnail is: the
        // cost has to belong to the thumbnail, not to the canvas, or editing a
        // large mask stalls on redrawing a picture of it.
        let mut pixels = Vec::with_capacity((tw * th) as usize);
        for ty in 0..th {
            for tx in 0..tw {
                let sx0 = tx as u64 * w as u64 / tw as u64;
                let sx1 = (((tx + 1) as u64 * w as u64 / tw as u64) as u32).max(sx0 as u32 + 1);
                let sy0 = ty as u64 * h as u64 / th as u64;
                let sy1 = (((ty + 1) as u64 * h as u64 / th as u64) as u32).max(sy0 as u32 + 1);
                let step_x = cshop_core::pixels::sample_step(sx1.min(w).saturating_sub(sx0 as u32));
                let step_y = cshop_core::pixels::sample_step(sy1.min(h).saturating_sub(sy0 as u32));
                let (mut sum, mut n) = (0u32, 0u32);
                for sy in (sy0 as u32..sy1.min(h)).step_by(step_y as usize) {
                    for sx in (sx0 as u32..sx1.min(w)).step_by(step_x as usize) {
                        sum += mask.data.get(sx as i32, sy as i32) as u32;
                        n += 1;
                    }
                }
                let v = sum.checked_div(n).unwrap_or(0) as u8;
                pixels.push(egui::Color32::from_rgb(v, v, v));
            }
        }

        let handle = ctx.load_texture(
            format!("maskthumb-{}-{}", self.doc.id.0, id.0),
            egui::ColorImage {
                size: [tw as usize, th as usize],
                source_size: egui::vec2(tw as f32, th as f32),
                pixels,
            },
            egui::TextureOptions::LINEAR,
        );
        self.thumbnails.insert(key, (handle.clone(), want));
        Some(handle)
    }

    /// Document region a screen rect covers, for partial recomposites.
    pub fn visible_doc_rect(&self, viewport: egui::Rect) -> IRect {
        let tl = self.screen_to_doc(viewport, viewport.min);
        let br = self.screen_to_doc(viewport, viewport.max);
        IRect::from_points(
            tl.x.floor() as i32,
            tl.y.floor() as i32,
            br.x.ceil() as i32,
            br.y.ceil() as i32,
        )
        .intersect(&self.doc.bounds())
    }
}
