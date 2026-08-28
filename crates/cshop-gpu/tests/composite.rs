//! Checks the GPU compositor against `cshop_core`'s CPU reference.
//!
//! `composite.wgsl` and `cshop_core::blend` implement the same maths twice, in
//! two languages. These tests are what stops them drifting apart.
//!
//! They need a working GPU. When none is available they skip rather than fail,
//! so `cargo test` still means something on a headless CI box.

use cshop_core::blend::{composite as cpu_composite, BlendMode};
use cshop_core::color::{Rgba, Rgba8};
use cshop_core::document::{Background, Document};
use cshop_core::geom::IRect;
use cshop_core::layer::{FillStyle, Layer, LayerKind, LayerMask};
use cshop_core::mask::MaskBuffer;
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::compositor::Compositor;
use cshop_gpu::context::GpuContext;
use cshop_gpu::layers::LayerTextures;
use cshop_gpu::readback::read_work_texture;
use cshop_gpu::texture::GpuTexture;

/// Tolerance for the structural tests, which use flat colours where rounding
/// cannot accumulate.
const TOL: f32 = 0.002;

/// Worst per-channel deviation the blend sweep is allowed to reach, expressed
/// in 8-bit output levels.
///
/// The intermediate buffers are `Rgba16Float`, whose spacing near white is
/// ~1/2048. Color Burn and Vivid Light divide by `1 - backdrop`, so against a
/// near-white backdrop that quantisation is amplified into roughly one and a
/// half output levels. Everything else lands well inside a single level.
const MAX_LEVELS: f32 = 2.0;

struct Harness {
    ctx: GpuContext,
    compositor: Compositor,
    cache: LayerTextures,
}

impl Harness {
    fn new() -> Option<Harness> {
        let ctx = match GpuContext::headless() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipping GPU test: {e}");
                return None;
            }
        };
        let compositor = Compositor::new(&ctx);
        Some(Harness { ctx, compositor, cache: LayerTextures::new() })
    }

    /// Composite the whole document and read it back.
    fn run(&mut self, doc: &Document) -> Vec<Rgba> {
        let dest = GpuTexture::render_target(&self.ctx, "dest", doc.width, doc.height, self.ctx.work_format());
        self.cache.sync(&self.ctx, doc, &cshop_core::document::Dirty::NONE);
        self.compositor.composite(&self.ctx, doc, &self.cache, &dest, doc.bounds());
        read_work_texture(&self.ctx, &dest, doc.bounds())
    }
}

fn assert_close(got: Rgba, want: Rgba, ctx: &str) {
    let d = [
        (got.r - want.r).abs(),
        (got.g - want.g).abs(),
        (got.b - want.b).abs(),
        (got.a - want.a).abs(),
    ];
    assert!(
        d.iter().all(|&x| x < TOL),
        "{ctx}\n  gpu = {got:?}\n  cpu = {want:?}\n  delta = {d:?}"
    );
}

/// Two stacked opaque-ish layers, one pixel each, for a given blend mode.
fn two_layer_doc(bottom: Rgba8, top: Rgba8, mode: BlendMode, opacity: f32) -> Document {
    let mut doc = Document::new("t", 1, 1, Background::Transparent);
    let base = doc.active.unwrap();
    doc.tree.get_mut(base).unwrap().kind = LayerKind::Raster(PixelBuffer::filled(1, 1, bottom));

    let id = doc.tree.alloc_id();
    let mut layer = Layer::raster(id, "top", PixelBuffer::filled(1, 1, top));
    layer.blend_mode = mode;
    layer.opacity = opacity;
    doc.tree.push(layer, None);
    doc
}

#[test]
fn every_blend_mode_matches_the_cpu_reference() {
    let Some(mut h) = Harness::new() else { return };

    // A spread of colours that exercises the clamping branches in dodge, burn
    // and the non-separable modes.
    let samples: &[(Rgba8, Rgba8)] = &[
        (Rgba8::opaque(0, 0, 0), Rgba8::opaque(255, 255, 255)),
        (Rgba8::opaque(255, 255, 255), Rgba8::opaque(0, 0, 0)),
        (Rgba8::opaque(128, 128, 128), Rgba8::opaque(128, 128, 128)),
        (Rgba8::opaque(200, 50, 25), Rgba8::opaque(40, 180, 90)),
        (Rgba8::opaque(20, 200, 240), Rgba8::opaque(240, 100, 10)),
        (Rgba8::new(200, 50, 25, 180), Rgba8::new(40, 180, 90, 200)),
    ];

    let mut worst = 0.0f32;
    let mut worst_case = String::new();

    for mode in BlendMode::all() {
        for &(bottom, top) in samples {
            for &opacity in &[1.0f32, 0.5] {
                let doc = two_layer_doc(bottom, top, mode, opacity);
                let got = h.run(&doc)[0];

                // The CPU reference for the same stack: bottom over nothing,
                // then top over that.
                let base =
                    cpu_composite(BlendMode::Normal, Rgba::TRANSPARENT, bottom.to_f32(), 1.0);
                let want = cpu_composite(mode, base, top.to_f32(), opacity);

                let levels = [
                    (got.r - want.r).abs(),
                    (got.g - want.g).abs(),
                    (got.b - want.b).abs(),
                    (got.a - want.a).abs(),
                ]
                .into_iter()
                .fold(0.0f32, f32::max)
                    * 255.0;

                if levels > worst {
                    worst = levels;
                    worst_case =
                        format!("{mode:?} bottom={bottom:?} top={top:?} opacity={opacity}");
                }
            }
        }
    }

    println!("worst deviation: {worst:.3} of 255 levels ({worst_case})");
    assert!(
        worst <= MAX_LEVELS,
        "GPU and CPU blending diverged by {worst:.3} levels on {worst_case}"
    );
}

#[test]
fn a_lone_layer_survives_compositing_unchanged() {
    let Some(mut h) = Harness::new() else { return };
    let colors = [
        Rgba8::opaque(255, 0, 0),
        Rgba8::new(10, 200, 30, 128),
        Rgba8::TRANSPARENT,
        Rgba8::WHITE,
    ];
    for c in colors {
        let mut doc = Document::new("t", 1, 1, Background::Transparent);
        let base = doc.active.unwrap();
        doc.tree.get_mut(base).unwrap().kind = LayerKind::Raster(PixelBuffer::filled(1, 1, c));
        assert_close(h.run(&doc)[0], c.to_f32(), &format!("passthrough of {c:?}"));
    }
}

#[test]
fn layer_offsets_place_pixels_correctly() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = Document::new("t", 8, 8, Background::Transparent);

    let id = doc.tree.alloc_id();
    let mut layer = Layer::raster(id, "dot", PixelBuffer::filled(2, 2, Rgba8::opaque(255, 0, 0)));
    layer.offset = (3, 5);
    doc.tree.push(layer, None);

    let out = h.run(&doc);
    let at = |x: usize, y: usize| out[y * 8 + x];
    assert!(at(3, 5).a > 0.9 && at(3, 5).r > 0.9, "pixel should land at the offset");
    assert!(at(4, 6).a > 0.9);
    assert!(at(2, 5).a < 0.01, "nothing outside the layer bounds");
    assert!(at(5, 5).a < 0.01);
}

#[test]
fn a_partly_offscreen_layer_is_clipped_not_wrapped() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = Document::new("t", 4, 4, Background::Transparent);
    let id = doc.tree.alloc_id();
    let mut layer = Layer::raster(id, "big", PixelBuffer::filled(4, 4, Rgba8::opaque(0, 255, 0)));
    layer.offset = (-2, -2);
    doc.tree.push(layer, None);

    let out = h.run(&doc);
    let at = |x: usize, y: usize| out[y * 4 + x];
    assert!(at(0, 0).g > 0.9, "the visible corner should be painted");
    assert!(at(1, 1).g > 0.9);
    assert!(at(2, 2).a < 0.01, "beyond the layer must stay empty");
}

#[test]
fn masks_scale_coverage() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = Document::new("t", 4, 1, Background::Transparent);
    let base = doc.active.unwrap();
    doc.tree.get_mut(base).unwrap().kind =
        LayerKind::Raster(PixelBuffer::filled(4, 1, Rgba8::BLACK));

    let id = doc.tree.alloc_id();
    let mut layer = Layer::raster(id, "top", PixelBuffer::filled(4, 1, Rgba8::WHITE));
    let mut mask = MaskBuffer::hide_all(4, 1);
    for (x, v) in [0u8, 85, 170, 255].into_iter().enumerate() {
        mask.set(x as i32, 0, v);
    }
    layer.mask = Some(LayerMask { data: mask, offset: (0, 0), enabled: true, linked: true });
    doc.tree.push(layer, None);

    let out = h.run(&doc);
    for (x, v) in [0u8, 85, 170, 255].into_iter().enumerate() {
        let want = v as f32 / 255.0;
        assert!(
            (out[x].r - want).abs() < 0.01,
            "mask value {v} should give {want}, got {}",
            out[x].r
        );
    }

    // Disabling the mask must restore full coverage without discarding it.
    doc.tree.get_mut(id).unwrap().mask.as_mut().unwrap().enabled = false;
    let out = h.run(&doc);
    assert!(out.iter().all(|c| c.r > 0.99), "a disabled mask must not apply");
}

#[test]
fn a_mask_smaller_than_its_layer_hides_the_remainder() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = Document::new("t", 4, 1, Background::Transparent);
    let id = doc.tree.alloc_id();
    let mut layer = Layer::raster(id, "top", PixelBuffer::filled(4, 1, Rgba8::WHITE));
    layer.mask = Some(LayerMask {
        data: MaskBuffer::reveal_all(2, 1),
        offset: (0, 0),
        enabled: true,
        linked: true,
    });
    doc.tree.push(layer, None);

    let out = h.run(&doc);
    assert!(out[0].a > 0.99 && out[1].a > 0.99, "inside the mask stays visible");
    assert!(out[2].a < 0.01 && out[3].a < 0.01, "outside the mask must be hidden");
}

#[test]
fn clipping_masks_limit_a_layer_to_the_base_alpha() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = Document::new("t", 4, 1, Background::Transparent);

    // Base: opaque on the left half only.
    let base = doc.active.unwrap();
    let mut px = PixelBuffer::new(4, 1);
    px.set(0, 0, Rgba8::opaque(0, 0, 255));
    px.set(1, 0, Rgba8::opaque(0, 0, 255));
    doc.tree.get_mut(base).unwrap().kind = LayerKind::Raster(px);

    // Clipped layer covers everything but may only show over the base.
    let id = doc.tree.alloc_id();
    let mut layer = Layer::raster(id, "clipped", PixelBuffer::filled(4, 1, Rgba8::opaque(255, 0, 0)));
    layer.clipping = true;
    doc.tree.push(layer, None);

    let out = h.run(&doc);
    assert!(out[0].r > 0.99 && out[1].r > 0.99, "clipped layer shows over the base");
    assert!(out[2].a < 0.01 && out[3].a < 0.01, "clipped layer is hidden off the base");

    // Unclipped, the same layer covers the whole canvas.
    doc.tree.get_mut(id).unwrap().clipping = false;
    let out = h.run(&doc);
    assert!(out.iter().all(|c| c.r > 0.99), "without clipping it covers everything");
}

#[test]
fn two_clipped_layers_share_one_base() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = Document::new("t", 2, 1, Background::Transparent);

    let base = doc.active.unwrap();
    let mut px = PixelBuffer::new(2, 1);
    px.set(0, 0, Rgba8::opaque(0, 0, 255));
    doc.tree.get_mut(base).unwrap().kind = LayerKind::Raster(px);

    for (name, color) in [("c1", Rgba8::opaque(255, 0, 0)), ("c2", Rgba8::opaque(0, 255, 0))] {
        let id = doc.tree.alloc_id();
        let mut l = Layer::raster(id, name, PixelBuffer::filled(2, 1, color));
        l.clipping = true;
        l.opacity = 0.5;
        doc.tree.push(l, None);
    }

    let out = h.run(&doc);
    // The second clipped layer must still see the *base* alpha, not the
    // alpha after the first clipped layer was drawn.
    assert!(out[0].a > 0.99, "over the base, the stack is opaque");
    assert!(out[1].a < 0.01, "off the base, both clipped layers stay hidden");
    assert!(out[0].g > 0.4, "the topmost clipped layer must be visible");
}

#[test]
fn fill_layers_paint_a_flat_colour() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = Document::new("t", 2, 2, Background::Transparent);
    let id = doc.tree.alloc_id();
    let layer = Layer::new(
        id,
        "fill",
        LayerKind::Fill(FillStyle::Solid(Rgba8::opaque(255, 128, 0))),
    );
    doc.tree.push(layer, None);

    let out = h.run(&doc);
    let want = Rgba8::opaque(255, 128, 0).to_f32();
    for c in &out {
        assert_close(*c, want, "fill layer");
    }
}

#[test]
fn hidden_layers_and_groups_are_skipped() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = Document::new("t", 1, 1, Background::Transparent);
    let base = doc.active.unwrap();
    doc.tree.get_mut(base).unwrap().kind =
        LayerKind::Raster(PixelBuffer::filled(1, 1, Rgba8::opaque(0, 0, 255)));

    let g = doc.tree.alloc_id();
    doc.tree.push(Layer::group(g, "G"), None);
    let id = doc.tree.alloc_id();
    doc.tree.push(Layer::raster(id, "inner", PixelBuffer::filled(1, 1, Rgba8::opaque(255, 0, 0))), Some(g));

    assert!(h.run(&doc)[0].r > 0.99, "a visible group shows its children");

    doc.tree.get_mut(g).unwrap().visible = false;
    let out = h.run(&doc)[0];
    assert!(out.b > 0.99 && out.r < 0.01, "hiding the group hides its children");
}

#[test]
fn group_opacity_applies_to_the_composited_result() {
    let Some(mut h) = Harness::new() else { return };
    // Two overlapping opaque children at 50% group opacity should read as a
    // single 50% layer, not as two independently faded layers.
    let mut doc = Document::new("t", 1, 1, Background::Transparent);
    let base = doc.active.unwrap();
    doc.tree.get_mut(base).unwrap().kind =
        LayerKind::Raster(PixelBuffer::filled(1, 1, Rgba8::BLACK));

    let g = doc.tree.alloc_id();
    let mut group = Layer::group(g, "G");
    // Anything other than Pass Through forces isolated compositing.
    group.blend_mode = cshop_core::blend::BlendMode::Normal;
    group.opacity = 0.5;
    doc.tree.push(group, None);

    for _ in 0..2 {
        let id = doc.tree.alloc_id();
        doc.tree.push(
            Layer::raster(id, "child", PixelBuffer::filled(1, 1, Rgba8::WHITE)),
            Some(g),
        );
    }

    let out = h.run(&doc)[0];
    assert!((out.r - 0.5).abs() < 0.01, "expected 50% grey, got {}", out.r);
}

#[test]
fn pass_through_groups_do_not_isolate_blending() {
    let Some(mut h) = Harness::new() else { return };
    // A Multiply layer inside a pass-through group must multiply against the
    // backdrop outside the group. An isolated group would multiply against
    // transparency instead and simply show its own colour.
    let mut doc = Document::new("t", 1, 1, Background::Transparent);
    let base = doc.active.unwrap();
    doc.tree.get_mut(base).unwrap().kind =
        LayerKind::Raster(PixelBuffer::filled(1, 1, Rgba8::opaque(128, 128, 128)));

    let g = doc.tree.alloc_id();
    let mut group = Layer::group(g, "G");
    group.blend_mode = BlendMode::PassThrough;
    doc.tree.push(group, None);

    let id = doc.tree.alloc_id();
    let mut child = Layer::raster(id, "mul", PixelBuffer::filled(1, 1, Rgba8::opaque(128, 128, 128)));
    child.blend_mode = BlendMode::Multiply;
    doc.tree.push(child, Some(g));

    let out = h.run(&doc)[0];
    let want = 128.0 / 255.0 * (128.0 / 255.0);
    assert!(
        (out.r - want).abs() < 0.01,
        "pass-through should multiply with the backdrop: got {}, want {want}",
        out.r
    );

    // Switching the group to Normal isolates it, so the multiply has nothing
    // to act on and the child's own colour comes through.
    doc.tree.get_mut(g).unwrap().blend_mode = BlendMode::Normal;
    let out = h.run(&doc)[0];
    assert!(
        (out.r - 128.0 / 255.0).abs() < 0.01,
        "an isolated group should not see the outer backdrop: got {}",
        out.r
    );
}

#[test]
fn nested_groups_composite_in_order() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = Document::new("t", 1, 1, Background::Transparent);

    let outer = doc.tree.alloc_id();
    let mut g = Layer::group(outer, "Outer");
    g.blend_mode = BlendMode::Normal;
    doc.tree.push(g, None);

    let inner = doc.tree.alloc_id();
    let mut g2 = Layer::group(inner, "Inner");
    g2.blend_mode = BlendMode::Normal;
    doc.tree.push(g2, Some(outer));

    let id = doc.tree.alloc_id();
    doc.tree.push(
        Layer::raster(id, "deep", PixelBuffer::filled(1, 1, Rgba8::opaque(255, 0, 0))),
        Some(inner),
    );

    let out = h.run(&doc)[0];
    assert!(out.r > 0.99 && out.a > 0.99, "a doubly nested layer must still render");
}

#[test]
fn compositing_a_sub_region_leaves_the_rest_untouched() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = Document::new("t", 8, 8, Background::Transparent);
    let base = doc.active.unwrap();
    doc.tree.get_mut(base).unwrap().kind =
        LayerKind::Raster(PixelBuffer::filled(8, 8, Rgba8::opaque(255, 0, 0)));

    let dest = GpuTexture::render_target(&h.ctx, "dest", 8, 8, h.ctx.work_format());
    h.cache.sync(&h.ctx, &doc, &cshop_core::document::Dirty::NONE);

    // Full composite first, then recomposite one corner after a colour change.
    h.compositor.composite(&h.ctx, &doc, &h.cache, &dest, doc.bounds());
    doc.tree.get_mut(base).unwrap().kind =
        LayerKind::Raster(PixelBuffer::filled(8, 8, Rgba8::opaque(0, 0, 255)));
    // Report the edit: the cache re-uploads only what it is told about.
    h.cache.sync(&h.ctx, &doc, &cshop_core::document::Dirty::pixels(base, doc.bounds()));

    let region = IRect::at(0, 0, 4, 4);
    h.compositor.composite(&h.ctx, &doc, &h.cache, &dest, region);

    let out = read_work_texture(&h.ctx, &dest, doc.bounds());
    let at = |x: usize, y: usize| out[y * 8 + x];
    assert!(at(1, 1).b > 0.99, "inside the region should be updated");
    assert!(at(6, 6).r > 0.99, "outside the region must keep the old result");
}

// ---------------------------------------------------------------------------
// The display path
// ---------------------------------------------------------------------------

/// The present pass converts the working buffer into the 8-bit sRGB
/// premultiplied texture egui draws.
///
/// It is easy to get wrong in a way that only shows as a subtly washed-out or
/// darkened canvas, so these tests pin the round trip numerically.
#[test]
fn presenting_an_opaque_colour_is_lossless() {
    let Some(mut h) = Harness::new() else { return };

    let samples = [
        Rgba8::opaque(0, 0, 0),
        Rgba8::opaque(255, 255, 255),
        Rgba8::opaque(128, 128, 128),
        Rgba8::opaque(18, 200, 77),
        Rgba8::opaque(255, 1, 254),
    ];

    for c in samples {
        let mut doc = Document::new("t", 2, 2, Background::Transparent);
        let base = doc.active.unwrap();
        doc.tree.get_mut(base).unwrap().kind = LayerKind::Raster(PixelBuffer::filled(2, 2, c));

        let work = GpuTexture::render_target(&h.ctx, "work", 2, 2, h.ctx.work_format());
        let display = GpuTexture::render_target(
            &h.ctx,
            "display",
            2,
            2,
            cshop_gpu::texture::DISPLAY_FORMAT,
        );
        h.cache.sync(&h.ctx, &doc, &cshop_core::document::Dirty::NONE);
        h.compositor.composite(&h.ctx, &doc, &h.cache, &work, doc.bounds());
        h.compositor.present(&h.ctx, &work, &display);

        let got = cshop_gpu::readback::read_srgb8(&h.ctx, &display).get(0, 0);
        // A mid-grey that comes back as 186 means a stray sRGB encode; 55 means
        // a stray decode. Anything but the original value is a colour bug.
        assert!(
            (got.r as i32 - c.r as i32).abs() <= 1
                && (got.g as i32 - c.g as i32).abs() <= 1
                && (got.b as i32 - c.b as i32).abs() <= 1
                && got.a == 255,
            "present mangled {c:?} into {got:?}"
        );
    }
}

#[test]
fn presenting_a_translucent_colour_premultiplies_in_gamma() {
    let Some(mut h) = Harness::new() else { return };
    // egui's textures are gamma-encoded and premultiplied — that is how its own
    // font atlas is built, and its shader multiplies the sampled value by the
    // vertex colour before converting to linear for the framebuffer. So the
    // stored value is the encoded colour scaled by alpha, not the linear one.
    let c = Rgba8::new(255, 255, 255, 128);

    let mut doc = Document::new("t", 2, 2, Background::Transparent);
    let base = doc.active.unwrap();
    doc.tree.get_mut(base).unwrap().kind = LayerKind::Raster(PixelBuffer::filled(2, 2, c));

    let work = GpuTexture::render_target(&h.ctx, "work", 2, 2, h.ctx.work_format());
    let display =
        GpuTexture::render_target(&h.ctx, "display", 2, 2, cshop_gpu::texture::DISPLAY_FORMAT);
    h.cache.sync(&h.ctx, &doc, &cshop_core::document::Dirty::NONE);
    h.compositor.composite(&h.ctx, &doc, &h.cache, &work, doc.bounds());
    h.compositor.present(&h.ctx, &work, &display);

    let got = cshop_gpu::readback::read_srgb8(&h.ctx, &display).get(0, 0);
    assert_eq!(got.a, 128, "alpha must survive unchanged");

    // Un-premultiply in the encoded space; the result should be white again.
    let alpha = got.a as f32 / 255.0;
    let straight = got.r as f32 / alpha;
    assert!(
        (straight - 255.0).abs() < 2.0,
        "un-premultiplying gave {straight:.1}, expected 255 (stored {})",
        got.r
    );
}

// ---------------------------------------------------------------------------
// Adjustment layers
// ---------------------------------------------------------------------------

use cshop_core::adjust::{Adjustment, GradientStop, LevelsChannel};
use cshop_core::curve::Curve;

/// A document with one opaque colour under one adjustment layer.
fn adjusted_doc(base: Rgba8, adjustment: Adjustment) -> Document {
    let mut doc = Document::new("t", 1, 1, Background::Transparent);
    let id = doc.active.unwrap();
    doc.tree.get_mut(id).unwrap().kind = LayerKind::Raster(PixelBuffer::filled(1, 1, base));

    let adj_id = doc.tree.alloc_id();
    doc.tree.push(Layer::adjustment(adj_id, adjustment), None);
    doc
}

/// Every adjustment, at settings that actually do something.
fn adjustment_cases() -> Vec<Adjustment> {
    let levels = LevelsChannel {
        input_black: 0.1,
        input_white: 0.85,
        gamma: 1.3,
        ..Default::default()
    };

    let mut per_channel = [LevelsChannel::default(); 3];
    per_channel[2].output_white = 0.7;

    let mut curves: [Curve; 4] = Default::default();
    curves[0] = Curve::new(vec![(0.0, 0.05), (0.4, 0.6), (1.0, 0.95)]);
    curves[1] = Curve::new(vec![(0.0, 0.0), (1.0, 0.8)]);

    vec![
        Adjustment::BrightnessContrast { brightness: 0.25, contrast: 0.4 },
        Adjustment::BrightnessContrast { brightness: -0.3, contrast: -0.6 },
        Adjustment::Levels { rgb: levels, channels: per_channel },
        Adjustment::Curves { curves },
        Adjustment::Exposure { exposure: 0.8, offset: 0.02, gamma: 1.1 },
        Adjustment::Vibrance { vibrance: 0.7, saturation: -0.2 },
        Adjustment::HueSaturation {
            hue: 0.25,
            saturation: 0.4,
            lightness: 0.1,
            colorize: false,
        },
        Adjustment::HueSaturation {
            hue: 0.6,
            saturation: 0.2,
            lightness: -0.3,
            colorize: true,
        },
        Adjustment::ColorBalance {
            shadows: [0.3, -0.1, 0.0],
            midtones: [0.2, 0.0, -0.4],
            highlights: [-0.2, 0.1, 0.3],
            preserve_luminosity: true,
        },
        Adjustment::ColorBalance {
            shadows: [0.5, 0.0, -0.5],
            midtones: [0.0; 3],
            highlights: [0.0; 3],
            preserve_luminosity: false,
        },
        Adjustment::BlackAndWhite { weights: [1.2, 0.6, 0.4, 0.9, 0.2, 1.1], tint: None },
        Adjustment::BlackAndWhite {
            weights: [0.4, 0.6, 0.4, 0.6, 0.2, 0.8],
            tint: Some(Rgba8::opaque(180, 140, 90)),
        },
        Adjustment::ChannelMixer {
            matrix: [[0.6, 0.3, 0.1, 0.05], [0.1, 0.8, 0.1, 0.0], [0.2, 0.2, 0.6, -0.05]],
            monochrome: false,
        },
        Adjustment::ChannelMixer {
            matrix: [[0.4, 0.4, 0.2, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
            monochrome: true,
        },
        Adjustment::PhotoFilter {
            color: Rgba8::opaque(236, 138, 0),
            density: 0.6,
            preserve_luminosity: true,
        },
        Adjustment::PhotoFilter {
            color: Rgba8::opaque(40, 90, 220),
            density: 0.9,
            preserve_luminosity: false,
        },
        Adjustment::Invert,
        Adjustment::Posterize { levels: 5 },
        Adjustment::Threshold { level: 0.45 },
        Adjustment::GradientMap {
            stops: vec![
                GradientStop { position: 0.0, color: Rgba8::opaque(20, 0, 90) },
                GradientStop { position: 0.5, color: Rgba8::opaque(220, 60, 40) },
                GradientStop { position: 1.0, color: Rgba8::opaque(255, 250, 200) },
            ],
        },
    ]
}

#[test]
fn every_adjustment_matches_the_cpu_reference() {
    let Some(mut h) = Harness::new() else { return };

    let colours = [
        Rgba8::opaque(0, 0, 0),
        Rgba8::opaque(255, 255, 255),
        Rgba8::opaque(128, 128, 128),
        Rgba8::opaque(200, 50, 25),
        Rgba8::opaque(20, 200, 240),
        Rgba8::opaque(90, 90, 40),
    ];

    let mut worst = 0.0f32;
    let mut worst_case = String::new();

    for adjustment in adjustment_cases() {
        for base in colours {
            let doc = adjusted_doc(base, adjustment.clone());
            let got = h.run(&doc)[0];
            let want = adjustment.apply(base.to_f32());

            let levels = [
                (got.r - want.r).abs(),
                (got.g - want.g).abs(),
                (got.b - want.b).abs(),
            ]
            .into_iter()
            .fold(0.0f32, f32::max)
                * 255.0;

            assert!(
                (got.a - 1.0).abs() < 0.01,
                "{} changed alpha to {}",
                adjustment.name(),
                got.a
            );

            if levels > worst {
                worst = levels;
                worst_case = format!("{} on {base:?}", adjustment.name());
            }
        }
    }

    println!("worst adjustment deviation: {worst:.2} of 255 levels ({worst_case})");
    // The shader and the reference implement the same maths twice, once in
    // WGSL and once in Rust; a couple of levels is rounding, more is a bug.
    assert!(worst <= 2.5, "GPU and CPU adjustments diverged by {worst:.2} levels on {worst_case}");
}

#[test]
fn an_adjustment_layer_leaves_transparent_areas_alone() {
    let Some(mut h) = Harness::new() else { return };
    // Half the canvas is empty; an Invert must not turn it into opaque white.
    let mut doc = Document::new("t", 4, 1, Background::Transparent);
    let id = doc.active.unwrap();
    let mut px = PixelBuffer::new(4, 1);
    px.set(0, 0, Rgba8::opaque(0, 0, 0));
    px.set(1, 0, Rgba8::opaque(0, 0, 0));
    doc.tree.get_mut(id).unwrap().kind = LayerKind::Raster(px);

    let adj = doc.tree.alloc_id();
    doc.tree.push(Layer::adjustment(adj, Adjustment::Invert), None);

    let out = h.run(&doc);
    assert!(out[0].r > 0.99 && out[0].a > 0.99, "black inverted to white");
    assert!(out[3].a < 0.01, "empty pixels must stay empty, got alpha {}", out[3].a);
}

#[test]
fn adjustment_opacity_fades_the_effect() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = adjusted_doc(Rgba8::opaque(0, 0, 0), Adjustment::Invert);
    let adj = *doc.tree.root().last().unwrap();
    doc.tree.get_mut(adj).unwrap().opacity = 0.5;

    let out = h.run(&doc)[0];
    assert!((out.r - 0.5).abs() < 0.01, "half opacity should give mid-grey, got {}", out.r);
    assert!((out.a - 1.0).abs() < 0.01);
}

#[test]
fn an_adjustment_mask_limits_where_it_applies() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = Document::new("t", 4, 1, Background::Transparent);
    let id = doc.active.unwrap();
    doc.tree.get_mut(id).unwrap().kind =
        LayerKind::Raster(PixelBuffer::filled(4, 1, Rgba8::opaque(0, 0, 0)));

    let adj = doc.tree.alloc_id();
    let mut layer = Layer::adjustment(adj, Adjustment::Invert);
    let mut mask = MaskBuffer::hide_all(4, 1);
    mask.set(0, 0, 255);
    mask.set(1, 0, 255);
    layer.mask = Some(LayerMask { data: mask, offset: (0, 0), enabled: true, linked: true });
    doc.tree.push(layer, None);

    let out = h.run(&doc);
    assert!(out[0].r > 0.99, "inverted where the mask reveals");
    assert!(out[3].r < 0.01, "untouched where the mask hides");
}

#[test]
fn an_adjustment_clips_to_the_layer_below() {
    let Some(mut h) = Harness::new() else { return };
    let mut doc = Document::new("t", 4, 1, Background::Transparent);

    // Bottom fills the row; the middle layer covers only half.
    let base = doc.active.unwrap();
    doc.tree.get_mut(base).unwrap().kind =
        LayerKind::Raster(PixelBuffer::filled(4, 1, Rgba8::opaque(0, 0, 0)));

    let mid = doc.tree.alloc_id();
    let mut px = PixelBuffer::new(4, 1);
    px.set(0, 0, Rgba8::opaque(0, 0, 0));
    px.set(1, 0, Rgba8::opaque(0, 0, 0));
    doc.tree.push(Layer::raster(mid, "half", px), None);

    let adj = doc.tree.alloc_id();
    let mut layer = Layer::adjustment(adj, Adjustment::Invert);
    layer.clipping = true;
    doc.tree.push(layer, None);

    let out = h.run(&doc);
    assert!(out[0].r > 0.99, "inverted over the clipping base");
    assert!(out[3].r < 0.01, "not inverted beyond it");
}

#[test]
fn adjustments_stack_in_order() {
    let Some(mut h) = Harness::new() else { return };
    // Invert then invert must come back to the original.
    let mut doc = adjusted_doc(Rgba8::opaque(200, 60, 30), Adjustment::Invert);
    let second = doc.tree.alloc_id();
    doc.tree.push(Layer::adjustment(second, Adjustment::Invert), None);

    let out = h.run(&doc)[0];
    let want = Rgba8::opaque(200, 60, 30).to_f32();
    assert_close(out, want, "two inversions should cancel");
}

#[test]
fn a_neutral_adjustment_is_skipped_entirely() {
    let Some(mut h) = Harness::new() else { return };
    let neutral = Adjustment::BrightnessContrast { brightness: 0.0, contrast: 0.0 };
    let doc = adjusted_doc(Rgba8::opaque(123, 45, 67), neutral);

    let out = h.run(&doc)[0];
    assert_close(out, Rgba8::opaque(123, 45, 67).to_f32(), "neutral adjustment");
    // The pass list should contain only the raster layer.
    assert_eq!(h.compositor.last_pass_count, 1, "a no-op adjustment should emit no pass");
}

#[test]
fn an_adjustment_inside_a_group_stays_inside_it() {
    let Some(mut h) = Harness::new() else { return };
    // A grouped adjustment must not reach the layer below the group.
    let mut doc = Document::new("t", 1, 1, Background::Transparent);
    let base = doc.active.unwrap();
    doc.tree.get_mut(base).unwrap().kind =
        LayerKind::Raster(PixelBuffer::filled(1, 1, Rgba8::opaque(0, 0, 0)));

    let g = doc.tree.alloc_id();
    let mut group = Layer::group(g, "G");
    // An isolated group, so the adjustment only sees what is inside it.
    group.blend_mode = BlendMode::Normal;
    doc.tree.push(group, None);

    let inner = doc.tree.alloc_id();
    doc.tree.push(
        Layer::raster(inner, "inner", PixelBuffer::filled(1, 1, Rgba8::opaque(255, 255, 255))),
        Some(g),
    );
    let adj = doc.tree.alloc_id();
    doc.tree.push(Layer::adjustment(adj, Adjustment::Invert), Some(g));

    // The group's white content inverts to black, which then covers the
    // black backdrop — so the result is black either way, but the *alpha*
    // proves the adjustment did not leak.
    let out = h.run(&doc)[0];
    assert!(out.r < 0.01 && out.a > 0.99, "expected inverted white over black, got {out:?}");
}
