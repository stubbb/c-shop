//! Sixteen-bit layers, from opening to export.
//!
//! The compositor works in `Rgba16Float`, which carries about eleven bits of
//! mantissa — fewer than the sixteen a deep layer holds. These ask what
//! survives the trip out, and where the extra bits have to be routed around
//! the GPU to survive at all.

use cshop_core::color::Rgba16;
use cshop_core::document::{Background, Document};
use cshop_core::layer::{LayerKind, Surface};
use cshop_core::pixels::DeepBuffer;
use cshop_ui::input_harness::Harness;

/// Sixty-four adjacent sixteen-bit reds. Nothing narrower than sixteen bits
/// can tell any two of them apart.
fn adjacent_counts() -> DeepBuffer {
    let data: Vec<Rgba16> = (0..64 * 64u32)
        .map(|i| Rgba16::new(30000 + (i % 64) as u16, 20000, 40000, 65535))
        .collect();
    DeepBuffer::from_pixels(64, 64, data).unwrap()
}

fn one_deep_layer() -> (Document, DeepBuffer) {
    let deep = adjacent_counts();
    let mut doc = Document::new("deep", 64, 64, Background::Transparent);
    let id = doc.tree.iter_all()[0];
    doc.tree.get_mut(id).unwrap().kind = LayerKind::Raster(Surface::Sixteen(deep.clone()));
    (doc, deep)
}

#[test]
fn a_single_deep_layer_reaches_the_export_intact() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    let (doc, deep) = one_deep_layer();
    h.app.open_document(doc);
    h.settle(2);

    let gpu = h.app.gpu.clone();
    let out = h.app.render_composite_deep(&gpu, 0);
    assert_eq!(out.pixels(), deep.pixels(), "nothing to composite, so nothing to lose");
}

/// The other half, and the reason the shortcut has to exist: once there is
/// something to composite, the picture goes through the GPU, and the GPU
/// cannot hold sixteen bits. This measures the cost rather than pretending
/// it is not there.
#[test]
fn compositing_costs_bits_the_gpu_cannot_hold() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    let (mut doc, deep) = one_deep_layer();
    // A second layer, fully transparent, so the picture is unchanged but the
    // compositor can no longer be skipped.
    let empty = cshop_core::pixels::PixelBuffer::new(64, 64);
    let id = doc.tree.alloc_id();
    doc.tree.push(cshop_core::layer::Layer::new(id, "clear", LayerKind::raster(empty)), None);
    h.app.open_document(doc);
    h.settle(2);

    let gpu = h.app.gpu.clone();
    let out = h.app.render_composite_deep(&gpu, 0);
    let count = |b: &DeepBuffer| {
        b.pixels().iter().map(|p| p.r).collect::<std::collections::HashSet<_>>().len()
    };
    let (before, after) = (count(&deep), count(&out));
    assert_eq!(before, 64, "the layer holds sixty-four distinct reds");
    assert!(
        after < before,
        "half-float cannot hold adjacent counts; it kept {after} of {before}"
    );
    // Still far better than eight bits, which would keep one.
    assert!(after > 1, "but it is not eight bits either: {after}");
}

/// Widening invents nothing, so narrowing again has to give back exactly what
/// was there. If it does not, the eight-to-sixteen arithmetic is wrong — the
/// usual mistake is shifting by eight rather than multiplying by 257, which
/// leaves white at 65280 instead of 65535.
#[test]
fn widening_then_narrowing_is_the_picture_it_started_as() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    let mut px = cshop_core::pixels::PixelBuffer::new(64, 64);
    for y in 0..64 {
        for x in 0..64 {
            let v = (x * 4) as u8;
            px.set(x, y, cshop_core::color::Rgba8::new(v, 255 - v, v.wrapping_mul(3), 255));
        }
    }
    let mut doc = Document::new("flat", 64, 64, Background::Transparent);
    let id = doc.tree.iter_all()[0];
    doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(px.clone());
    h.app.open_document(doc);
    h.settle(2);
    assert_eq!(h.app.doc().unwrap().doc.depth(), 8);

    h.app.dispatch(cshop_ui::commands::Action::SetDepth(16));
    h.settle(2);
    assert_eq!(h.app.doc().unwrap().doc.depth(), 16, "it should be deep now");

    h.app.dispatch(cshop_ui::commands::Action::SetDepth(8));
    h.settle(2);
    let back = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();
    assert_eq!(back.pixels(), px.pixels(), "widening loses nothing, so neither does the return");
}

/// Narrowing does lose, and undo is the only thing that has the loss on
/// record — so it has to give the whole of it back.
#[test]
fn undoing_a_narrowing_gets_every_bit_back() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    let (doc, deep) = one_deep_layer();
    let id = doc.tree.iter_all()[0];
    h.app.open_document(doc);
    h.settle(2);

    h.app.dispatch(cshop_ui::commands::Action::SetDepth(8));
    h.settle(2);
    assert_eq!(h.app.doc().unwrap().doc.depth(), 8);

    h.app.dispatch(cshop_ui::commands::Action::Undo);
    h.settle(2);
    let view = h.app.doc().unwrap();
    assert_eq!(view.doc.depth(), 16, "undo puts the depth back");
    let Some(Surface::Sixteen(back)) = view.doc.tree.get(id).unwrap().surface() else {
        panic!("and puts sixteen-bit pixels back");
    };
    assert_eq!(back.pixels(), deep.pixels(), "every count, not a narrowed copy of them");
}

/// A sixteen-bit layer turns the tools away, because they paint in eight. The
/// thing it must not say is that this is not a raster layer: it is one, and
/// someone told that goes looking for a problem they do not have.
#[test]
fn a_deep_layer_says_what_is_actually_wrong() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    let (doc, _) = one_deep_layer();
    h.app.open_document(doc);
    h.settle(2);

    h.app.begin_stroke(cshop_core::geom::Vec2::new(10.0, 10.0), cshop_core::paint::PaintMode::Paint);
    h.settle(1);
    let (msg, bad) = h.app.toast.clone().expect("it should have said something");
    assert!(bad, "and said it as a refusal");
    assert!(msg.contains("sixteen bits"), "naming the real objection: {msg}");
    assert!(msg.contains("Mode"), "and where to fix it: {msg}");
    assert!(!msg.contains("Only raster layers"), "not the wrong reason: {msg}");
}
