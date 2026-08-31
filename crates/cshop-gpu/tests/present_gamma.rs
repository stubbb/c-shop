//! What the present pass hands to egui.
//!
//! The compositor works in sRGB-encoded values; egui is handed a texture in an
//! `*Srgb` format, so the hardware's encode on write and decode on sample have
//! to cancel exactly. If they do not, everything on the canvas is displayed at
//! the wrong brightness while the saved file stays correct — which is easy to
//! miss and hard to diagnose from a screenshot.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::IRect;
use cshop_gpu::compositor::Compositor;
use cshop_gpu::context::GpuContext;
use cshop_gpu::layers::LayerTextures;
use cshop_gpu::readback::{read_srgb8, read_work_texture};
use cshop_gpu::texture::{GpuTexture, DISPLAY_FORMAT};

/// egui's renderer states the contract in its own shader: "we expect normal
/// textures that are NOT sRGB-aware", and it linearises whatever it samples.
/// Handing it an `*Srgb` texture makes the hardware linearise it first, and the
/// canvas is displayed two stops dark while the file it saves stays correct —
/// a difference no test that only reads the texture back can see, because the
/// stored bytes are the same either way. Only the format tells them apart.
#[test]
fn the_texture_egui_draws_is_not_srgb_aware() {
    assert!(
        !DISPLAY_FORMAT.is_srgb(),
        "{DISPLAY_FORMAT:?} would be linearised twice before it reached the screen"
    );
}

#[test]
fn the_displayed_canvas_keeps_the_document_s_brightness() {
    let Some(ctx) = GpuContext::headless().ok() else { return };
    let mut compositor = Compositor::new(&ctx);
    let mut textures = LayerTextures::new();

    // A flat mid grey: the value most obviously wrong if a transfer function
    // is applied one time too many or too few.
    let grey = Rgba8::opaque(128, 128, 128);
    let doc = Document::new("t", 8, 8, Background::Color(grey));
    textures.sync(&ctx, &doc, &Default::default());

    let work = GpuTexture::render_target(&ctx, "work", 8, 8, ctx.work_format());
    compositor.composite(&ctx, &doc, &textures, &work, doc.bounds());

    // The working buffer holds sRGB-encoded values, so it should read back as
    // the document's own numbers.
    let composited = read_work_texture(&ctx, &work, IRect::from_size(8, 8));
    let c = composited[0];
    let as_u8 = |v: f32| (v * 255.0).round() as i32;
    assert!(
        (as_u8(c.r) - 128).abs() <= 1,
        "the composite should still be mid grey, got {}",
        as_u8(c.r)
    );

    // And the texture egui draws must round-trip to the same number.
    let display = GpuTexture::render_target(&ctx, "display", 8, 8, DISPLAY_FORMAT);
    compositor.present(&ctx, &work, &display, &identity_table(&ctx));
    // The display texture is not sRGB-aware, so what egui samples is exactly
    // what is stored: the document's own number.
    let shown = read_srgb8(&ctx, &display);
    let p = shown.get(4, 4);
    assert!(
        (p.r as i32 - 128).abs() <= 2,
        "the displayed canvas should be mid grey too, got {p:?}"
    );

    // Transparency is premultiplied on the way out, which is what egui blends
    // with; a half-covered mid grey must not come back as mid grey.
    let half = Rgba8::new(128, 128, 128, 128);
    let doc = Document::new("t", 8, 8, Background::Transparent);
    let mut doc = doc;
    let id = doc.tree.alloc_id();
    let px = cshop_core::pixels::PixelBuffer::filled(8, 8, half);
    doc.tree.push(cshop_core::layer::Layer::raster(id, "l", px), None);
    textures.sync(&ctx, &doc, &Default::default());
    compositor.composite(&ctx, &doc, &textures, &work, doc.bounds());
    compositor.present(&ctx, &work, &display, &identity_table(&ctx));
    let shown = read_srgb8(&ctx, &display);
    let p = shown.get(4, 4);
    assert_eq!(p.a, 128, "alpha should survive, got {p:?}");
    assert!(
        (p.r as i32 - 64).abs() <= 2,
        "premultiplied half-covered mid grey should be about 64, got {p:?}"
    );
}

/// A colour transform that changes nothing, which is what the present pass
/// gets for a document already in the display's own space.
fn identity_table(ctx: &cshop_gpu::context::GpuContext) -> cshop_gpu::texture::ColourTable {
    cshop_gpu::texture::ColourTable::identity(ctx, 33)
}
