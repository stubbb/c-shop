//! Brushes and pressure, through the application.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::layer::LayerKind;
use cshop_core::paint::PaintMode;
use cshop_core::pixels::PixelBuffer;
use cshop_core::selection::{Rectf, Selection};
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

fn app() -> Option<CShopApp> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("t", 128, 128, Background::Transparent));
    let view = app.doc_mut()?;
    let id = view.doc.active?;
    view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(PixelBuffer::new(128, 128));
    view.invalidate();
    Some(app)
}

fn pixels(app: &CShopApp) -> &PixelBuffer {
    let view = app.doc().unwrap();
    view.doc.tree.get(view.doc.active.unwrap()).unwrap().pixels().unwrap()
}

/// A wide, short selection makes a wide, short brush.
#[test]
fn a_brush_defined_from_a_selection_stamps_that_shape() {
    let Some(mut app) = app() else { return };
    {
        let view = app.doc_mut().unwrap();
        view.doc.set_selection(Some(Selection::from_rect(
            128,
            128,
            Rectf::from_points(Vec2::new(10.0, 40.0), Vec2::new(90.0, 50.0)),
            false,
        )));
    }
    app.dispatch(Action::DefineBrush);
    assert!(app.brush_tip.is_some(), "a tip should have been taken");
    app.dispatch(Action::Deselect);

    app.brush.size = 40.0;
    app.brush.opacity = 1.0;
    app.foreground = Rgba8::BLACK;
    app.begin_stroke(Vec2::new(64.0, 64.0), PaintMode::Paint);
    app.end_stroke();

    let px = pixels(&app);
    let wide = (0..128).filter(|&x| px.get(x, 64).a > 8).count();
    let tall = (0..128).filter(|&y| px.get(64, y).a > 8).count();
    assert!(wide > tall * 3, "the mark should be wide and short: {wide} by {tall}");
}

#[test]
fn going_back_to_the_round_brush_makes_a_round_mark() {
    let Some(mut app) = app() else { return };
    {
        let view = app.doc_mut().unwrap();
        view.doc.set_selection(Some(Selection::from_rect(
            128,
            128,
            Rectf::from_points(Vec2::new(10.0, 40.0), Vec2::new(90.0, 50.0)),
            false,
        )));
    }
    app.dispatch(Action::DefineBrush);
    app.dispatch(Action::ClearBrushTip);
    assert!(app.brush_tip.is_none());
    app.dispatch(Action::Deselect);

    app.brush.size = 40.0;
    app.brush.hardness = 1.0;
    app.foreground = Rgba8::BLACK;
    app.begin_stroke(Vec2::new(64.0, 64.0), PaintMode::Paint);
    app.end_stroke();

    let px = pixels(&app);
    let wide = (0..128).filter(|&x| px.get(x, 64).a > 8).count();
    let tall = (0..128).filter(|&y| px.get(64, y).a > 8).count();
    assert!((wide as i32 - tall as i32).abs() <= 2, "a round mark: {wide} by {tall}");
}

#[test]
fn a_selection_of_nothing_says_so_rather_than_making_an_empty_brush() {
    let Some(mut app) = app() else { return };
    // No selection and a layer with nothing in it.
    app.dispatch(Action::DefineBrush);
    assert!(app.brush_tip.is_none());
    let (msg, _) = app.toast.clone().expect("it should have said why");
    assert!(msg.contains("nothing"), "{msg}");
}

/// A device that cannot measure pressure presses fully, so a brush behaves
/// exactly as it always did unless something is actually reporting.
#[test]
fn a_mouse_presses_fully() {
    let Some(mut app) = app() else { return };
    app.brush.size = 24.0;
    app.brush.hardness = 1.0;
    app.brush.pressure.size = true;
    app.foreground = Rgba8::BLACK;
    app.begin_stroke(Vec2::new(64.0, 64.0), PaintMode::Paint);
    app.continue_stroke(Vec2::new(80.0, 64.0));
    app.end_stroke();

    let px = pixels(&app);
    let tall = (0..128).filter(|&y| px.get(70, y).a > 8).count();
    assert!(tall >= 22, "a mouse stroke is full width even with pressure on: {tall}");
}

#[test]
fn a_pen_that_presses_lightly_makes_a_thinner_line() {
    let stroke_at = |pressure: f32| -> Option<usize> {
        let mut app = app()?;
        app.brush.size = 30.0;
        app.brush.hardness = 1.0;
        app.brush.pressure.size = true;
        app.foreground = Rgba8::BLACK;
        // As a pen touching down would: the stroke starts at the pressure it
        // started at, rather than ramping down from full.
        app.pen_pressure = pressure;
        app.begin_stroke(Vec2::new(20.0, 64.0), PaintMode::Paint);
        app.continue_stroke_pressed(Vec2::new(60.0, 64.0), pressure);
        app.end_stroke();
        Some((0..128).filter(|&y| pixels(&app).get(50, y).a > 8).count())
    };
    let (Some(light), Some(heavy)) = (stroke_at(0.2), stroke_at(1.0)) else { return };
    assert!(heavy > light * 2, "a light press should be thinner: {light} against {heavy}");
}
