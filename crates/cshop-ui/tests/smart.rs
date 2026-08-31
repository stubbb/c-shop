//! Smart objects, driven through the application.
//!
//! The claim is that a placement is a setting rather than an edit, so the test
//! that matters is the one an ordinary raster layer fails: transform it,
//! transform it again, and again, and see whether the picture is any worse
//! than after the first.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::layer::{Layer, LayerKind};
use cshop_core::pixels::PixelBuffer;
use cshop_core::transform::Handle;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

/// A fine checker: the pattern a downscale destroys first.
fn checker(w: u32, h: u32) -> PixelBuffer {
    let mut px = PixelBuffer::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let on = (x / 2 + y / 2) % 2 == 0;
            px.set(x, y, if on { Rgba8::WHITE } else { Rgba8::BLACK });
        }
    }
    px
}

fn app_with(px: PixelBuffer) -> Option<CShopApp> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("t", 400, 400, Background::Transparent));
    let view = app.doc_mut()?;
    let id = view.doc.active?;
    view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(px);
    view.invalidate();
    Some(app)
}

fn layer(app: &CShopApp) -> &Layer {
    let view = app.doc().unwrap();
    view.doc.tree.get(view.doc.active.unwrap()).unwrap()
}

/// How much detail is left: the spread of the pixel values. A checker that has
/// been scaled down and back up is flat grey.
fn detail(px: &PixelBuffer) -> f32 {
    let v: Vec<f32> = px.pixels().iter().map(|p| p.r as f32).collect();
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    (v.iter().map(|a| (a - mean).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
}

/// Drag the bottom-right corner so the layer's box becomes `to` across.
fn scale_to(app: &mut CShopApp, from: f32, to: f32) {
    app.dispatch(Action::BeginTransform);
    let t = app.transform.as_mut().expect("the transform should have started");
    t.begin_drag(Handle::BottomRight, Vec2::new(from, from));
    t.drag_to(Vec2::new(to, to), false, false, false);
    t.end_drag();
    app.dispatch(Action::CommitTransform);
}

#[test]
fn shrinking_and_growing_a_raster_layer_wears_it_out() {
    // The behaviour a smart object exists to avoid, measured so the
    // comparison below means something.
    let Some(mut app) = app_with(checker(128, 128)) else { return };
    let before = detail(layer(&app).pixels().unwrap());
    scale_to(&mut app, 128.0, 32.0);
    scale_to(&mut app, 32.0, 128.0);
    let after = detail(layer(&app).pixels().unwrap());
    assert!(
        after < before * 0.5,
        "a raster round trip should have lost most of the detail: {after:.1} against {before:.1}"
    );
}

#[test]
fn a_smart_object_comes_back_from_the_same_trip_unharmed() {
    let Some(mut app) = app_with(checker(128, 128)) else { return };
    let before = layer(&app).pixels().unwrap().clone();

    app.dispatch(Action::ConvertToSmartObject);
    assert!(layer(&app).smart().is_some(), "it should be a smart object now");
    assert_eq!(
        layer(&app).pixels().unwrap().pixels(),
        before.pixels(),
        "and look exactly as it did"
    );

    scale_to(&mut app, 128.0, 32.0);
    assert_eq!(layer(&app).pixels().unwrap().width(), 32);
    scale_to(&mut app, 32.0, 128.0);

    let after = layer(&app).pixels().unwrap();
    assert_eq!(after.width(), 128, "back to the size it was");
    assert!(
        detail(after) > detail(&before) * 0.9,
        "and with its detail: {:.1} against {:.1}",
        detail(after),
        detail(&before)
    );
}

#[test]
fn each_placement_undoes_to_the_one_before() {
    let Some(mut app) = app_with(checker(64, 64)) else { return };
    app.dispatch(Action::ConvertToSmartObject);
    scale_to(&mut app, 64.0, 32.0);
    assert_eq!(layer(&app).pixels().unwrap().width(), 32);
    scale_to(&mut app, 32.0, 96.0);
    assert_eq!(layer(&app).pixels().unwrap().width(), 96);

    app.dispatch(Action::Undo);
    assert_eq!(layer(&app).pixels().unwrap().width(), 32, "back to the first placement");
    app.dispatch(Action::Undo);
    assert_eq!(layer(&app).pixels().unwrap().width(), 64, "and to no placement at all");
    app.dispatch(Action::Undo);
    assert!(layer(&app).smart().is_none(), "and back to plain pixels");
}

/// A placement holds nine numbers and re-renders from a source the layer is
/// already holding, so undoing one should not be charged for a picture.
#[test]
fn placements_cost_the_history_nothing() {
    let Some(mut app) = app_with(checker(256, 256)) else { return };
    app.dispatch(Action::ConvertToSmartObject);
    let after_convert = app.doc().unwrap().history.memory_bytes();
    for k in 1..=6 {
        scale_to(&mut app, 256.0 / k as f32, 256.0 / (k + 1) as f32);
    }
    let after_six = app.doc().unwrap().history.memory_bytes();
    assert_eq!(
        after_six, after_convert,
        "six placements should have added nothing to the history's weight"
    );
}

#[test]
fn a_smart_object_cannot_be_painted_on_until_it_is_rasterised() {
    let Some(mut app) = app_with(checker(64, 64)) else { return };
    app.dispatch(Action::ConvertToSmartObject);
    app.begin_stroke(Vec2::new(32.0, 32.0), cshop_core::paint::PaintMode::Paint);
    app.end_stroke();
    let (msg, bad) = app.toast.clone().expect("it should have said something");
    assert!(bad, "and refused");
    assert!(msg.contains("smart object") && msg.contains("Rasterise"), "{msg}");

    // And rasterising makes it paintable, without changing the picture.
    let before = layer(&app).pixels().unwrap().clone();
    app.dispatch(Action::RasterizeLayer);
    assert!(layer(&app).smart().is_none());
    assert_eq!(layer(&app).pixels().unwrap().pixels(), before.pixels());
    app.begin_stroke(Vec2::new(32.0, 32.0), cshop_core::paint::PaintMode::Paint);
    app.end_stroke();
    assert_ne!(layer(&app).pixels().unwrap().pixels(), before.pixels(), "now it takes paint");
}
