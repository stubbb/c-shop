//! Filters attached to a layer, driven through the application.
//!
//! The difference from `Filter ▸ Gaussian Blur` is that the layer's own pixels
//! are never touched, so these check both halves: that the canvas shows the
//! filter, and that the layer underneath is exactly as it was.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::filters::Filter;
use cshop_core::layer::{Layer, LayerKind};
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

fn edge(w: u32, h: u32) -> PixelBuffer {
    let mut px = PixelBuffer::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let v = if x < w as i32 / 2 { 30 } else { 220 };
            px.set(x, y, Rgba8::opaque(v, v, v));
        }
    }
    px
}

fn app_with(px: PixelBuffer) -> Option<CShopApp> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu);
    let (w, h) = (px.width(), px.height());
    app.open_document(Document::new("t", w, h, Background::Transparent));
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

/// How steep the edge is at its middle, read off the composited canvas.
fn composite_slope(app: &mut CShopApp) -> i32 {
    let gpu = app.gpu.clone();
    let px = app.render_composite(&gpu, 0);
    let m = px.width() as i32 / 2;
    px.get(m + 1, 8).r as i32 - px.get(m - 2, 8).r as i32
}

#[test]
fn an_attached_blur_shows_on_the_canvas_and_leaves_the_layer_alone() {
    let Some(mut app) = app_with(edge(64, 32)) else { return };
    let before_pixels = layer(&app).pixels().unwrap().clone();
    let before = composite_slope(&mut app);

    app.dispatch(Action::AttachFilter(Box::new(Filter::GaussianBlur { radius: 5.0 })));
    let after = composite_slope(&mut app);

    assert!(after < before, "the canvas should be softer: {before} became {after}");
    assert_eq!(
        layer(&app).pixels().unwrap().pixels(),
        before_pixels.pixels(),
        "and the layer's own pixels untouched, which is the whole difference"
    );
    assert_eq!(layer(&app).filters.slots.len(), 1);
}

#[test]
fn changing_a_setting_re_renders_rather_than_stacking_up() {
    let Some(mut app) = app_with(edge(64, 32)) else { return };
    app.dispatch(Action::AttachFilter(Box::new(Filter::GaussianBlur { radius: 8.0 })));
    let heavy = composite_slope(&mut app);

    // Change the radius on the slot that is already there.
    app.dispatch(Action::ReplaceAttachedFilter(
        0,
        Box::new(Filter::GaussianBlur { radius: 2.0 }),
    ));
    let gentle = composite_slope(&mut app);
    assert!(gentle > heavy, "less radius should be sharper: {heavy} became {gentle}");

    // And it is a gentle blur of the original, not a gentle blur of a heavy
    // one — which is what running the filter destructively would have given.
    let Some(mut fresh) = app_with(edge(64, 32)) else { return };
    fresh.dispatch(Action::AttachFilter(Box::new(Filter::GaussianBlur { radius: 2.0 })));
    assert_eq!(gentle, composite_slope(&mut fresh));
}

#[test]
fn switching_the_stack_off_puts_the_canvas_back() {
    let Some(mut app) = app_with(edge(64, 32)) else { return };
    let plain = composite_slope(&mut app);
    app.dispatch(Action::AttachFilter(Box::new(Filter::GaussianBlur { radius: 6.0 })));
    assert!(composite_slope(&mut app) < plain);

    app.dispatch(Action::ToggleAttachedFilters);
    assert_eq!(composite_slope(&mut app), plain, "off should be exactly off");
    app.dispatch(Action::ToggleAttachedFilters);
    assert!(composite_slope(&mut app) < plain, "and on again is on");
}

#[test]
fn removing_a_filter_leaves_no_trace_of_it() {
    let Some(mut app) = app_with(edge(64, 32)) else { return };
    let plain = composite_slope(&mut app);
    app.dispatch(Action::AttachFilter(Box::new(Filter::GaussianBlur { radius: 6.0 })));
    app.dispatch(Action::RemoveAttachedFilter(0));
    assert!(layer(&app).filters.slots.is_empty());
    assert_eq!(composite_slope(&mut app), plain);
}

#[test]
fn applying_the_stack_runs_it_in_and_takes_it_off() {
    let Some(mut app) = app_with(edge(64, 32)) else { return };
    app.dispatch(Action::AttachFilter(Box::new(Filter::GaussianBlur { radius: 5.0 })));
    let shown = composite_slope(&mut app);

    app.dispatch(Action::ApplyAttachedFilters);
    assert!(layer(&app).filters.slots.is_empty(), "the stack is spent");
    assert_eq!(composite_slope(&mut app), shown, "and the canvas is unchanged by that");

    // The pixels themselves now carry it.
    let m = 32;
    let px = layer(&app).pixels().unwrap();
    assert!(px.get(m + 1, 8).r as i32 - px.get(m - 2, 8).r as i32 == shown);
}

#[test]
fn each_change_to_the_stack_undoes() {
    let Some(mut app) = app_with(edge(64, 32)) else { return };
    let plain = composite_slope(&mut app);
    app.dispatch(Action::AttachFilter(Box::new(Filter::GaussianBlur { radius: 6.0 })));
    app.dispatch(Action::AttachFilter(Box::new(Filter::Mosaic { size: 4 })));
    assert_eq!(layer(&app).filters.slots.len(), 2);

    app.dispatch(Action::Undo);
    assert_eq!(layer(&app).filters.slots.len(), 1);
    app.dispatch(Action::Undo);
    assert!(layer(&app).filters.slots.is_empty());
    assert_eq!(composite_slope(&mut app), plain);
}
