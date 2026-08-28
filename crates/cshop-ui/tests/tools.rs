//! The Paint Bucket, Gradient and Clone Stamp, and Edit > Fill.

use cshop_core::blend::BlendMode;
use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::fill::{Gradient, GradientKind};
use cshop_core::geom::{IRect, Vec2};
use cshop_core::layer::LayerKind;
use cshop_core::paint::PaintMode;
use cshop_core::pixels::PixelBuffer;
use cshop_core::selection::{Rectf, Selection};
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::dialogs::PickerTarget;
use cshop_ui::CShopApp;

fn app_with(w: u32, h: u32) -> Option<CShopApp> {
    let gpu = match GpuContext::headless() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping: {e}");
            return None;
        }
    };
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("t", w, h, Background::Transparent));
    Some(app)
}

fn set_pixels(app: &mut CShopApp, px: PixelBuffer) {
    let view = app.doc_mut().unwrap();
    let id = view.doc.active.unwrap();
    view.doc.tree.get_mut(id).unwrap().kind = LayerKind::Raster(px);
    view.invalidate();
}

fn pixels(app: &CShopApp) -> &PixelBuffer {
    let view = app.doc().unwrap();
    view.doc.tree.get(view.doc.active.unwrap()).unwrap().pixels().unwrap()
}

// ---------------------------------------------------------------------------
// Paint Bucket
// ---------------------------------------------------------------------------

#[test]
fn the_bucket_fills_the_region_under_the_click() {
    let Some(mut app) = app_with(64, 32) else { return };
    let mut px = PixelBuffer::filled(64, 32, Rgba8::BLACK);
    px.fill_rect(IRect::new(32, 0, 64, 32), Rgba8::WHITE);
    set_pixels(&mut app, px);

    app.foreground = Rgba8::opaque(255, 0, 0);
    app.bucket.antialias = false;
    app.bucket_fill_at(Vec2::new(8.0, 16.0));

    assert_eq!(pixels(&app).get(8, 16), Rgba8::opaque(255, 0, 0), "the clicked half");
    assert_eq!(pixels(&app).get(56, 16), Rgba8::WHITE, "the other half");
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Paint Bucket"]);

    app.dispatch(Action::Undo);
    assert_eq!(pixels(&app).get(8, 16), Rgba8::BLACK);
}

#[test]
fn the_bucket_respects_the_selection() {
    let Some(mut app) = app_with(64, 32) else { return };
    set_pixels(&mut app, PixelBuffer::filled(64, 32, Rgba8::BLACK));

    let s = Selection::from_rect(64, 32, Rectf { x0: 0.0, y0: 0.0, x1: 32.0, y1: 32.0 }, false);
    app.dispatch(Action::SetSelection(Box::new(s), "Rectangular Marquee"));

    app.foreground = Rgba8::WHITE;
    app.bucket.antialias = false;
    app.bucket_fill_at(Vec2::new(8.0, 16.0));

    assert_eq!(pixels(&app).get(8, 16), Rgba8::WHITE, "inside the selection");
    assert_eq!(pixels(&app).get(48, 16), Rgba8::BLACK, "outside it");
}

#[test]
fn the_bucket_honours_tolerance() {
    let Some(mut app) = app_with(32, 16) else { return };
    let mut px = PixelBuffer::filled(32, 16, Rgba8::opaque(100, 100, 100));
    px.fill_rect(IRect::new(16, 0, 32, 16), Rgba8::opaque(120, 100, 100));
    set_pixels(&mut app, px);

    app.foreground = Rgba8::opaque(0, 255, 0);
    app.bucket.antialias = false;
    app.bucket.tolerance = 5;
    app.bucket_fill_at(Vec2::new(4.0, 8.0));
    assert_ne!(pixels(&app).get(24, 8).g, 255, "20 levels apart, tolerance 5");

    app.dispatch(Action::Undo);
    app.bucket.tolerance = 40;
    app.bucket_fill_at(Vec2::new(4.0, 8.0));
    assert_eq!(pixels(&app).get(24, 8).g, 255, "tolerance 40 spans the difference");
}

#[test]
fn a_locked_layer_refuses_the_bucket() {
    let Some(mut app) = app_with(32, 32) else { return };
    set_pixels(&mut app, PixelBuffer::filled(32, 32, Rgba8::BLACK));
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().locks.pixels = true;
    }
    app.foreground = Rgba8::WHITE;
    app.bucket_fill_at(Vec2::new(16.0, 16.0));
    assert_eq!(pixels(&app).get(16, 16), Rgba8::BLACK, "the lock should hold");
}

// ---------------------------------------------------------------------------
// Gradient
// ---------------------------------------------------------------------------

#[test]
fn a_gradient_drag_lays_down_a_ramp() {
    let Some(mut app) = app_with(64, 16) else { return };
    set_pixels(&mut app, PixelBuffer::filled(64, 16, Rgba8::TRANSPARENT));

    app.gradient = Gradient { dither: false, ..Gradient::between(Rgba8::BLACK, Rgba8::WHITE) };
    app.gradient_drag = Some((Vec2::new(0.0, 8.0), Vec2::new(64.0, 8.0)));
    app.commit_gradient();

    assert!(pixels(&app).get(2, 8).r < 20, "dark at the start");
    assert!(pixels(&app).get(61, 8).r > 235, "light at the end");
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Gradient"]);

    app.dispatch(Action::Undo);
    assert_eq!(pixels(&app).get(32, 8).a, 0);
}

#[test]
fn a_gradient_respects_the_selection() {
    let Some(mut app) = app_with(64, 16) else { return };
    set_pixels(&mut app, PixelBuffer::filled(64, 16, Rgba8::TRANSPARENT));

    let s = Selection::from_rect(64, 16, Rectf { x0: 0.0, y0: 0.0, x1: 32.0, y1: 16.0 }, false);
    app.dispatch(Action::SetSelection(Box::new(s), "Rectangular Marquee"));

    app.gradient = Gradient { dither: false, ..Default::default() };
    app.gradient_drag = Some((Vec2::new(0.0, 8.0), Vec2::new(64.0, 8.0)));
    app.commit_gradient();

    assert!(pixels(&app).get(16, 8).a > 0, "inside the selection");
    assert_eq!(pixels(&app).get(48, 8).a, 0, "outside it");
}

#[test]
fn every_gradient_type_draws_something() {
    for kind in GradientKind::ALL {
        let Some(mut app) = app_with(48, 48) else { return };
        set_pixels(&mut app, PixelBuffer::filled(48, 48, Rgba8::TRANSPARENT));
        app.gradient = Gradient { kind, dither: false, ..Default::default() };
        app.gradient_drag = Some((Vec2::new(24.0, 24.0), Vec2::new(44.0, 24.0)));
        app.commit_gradient();
        assert!(
            pixels(&app).get(24, 24).a > 0,
            "{} left the canvas empty",
            kind.name()
        );
    }
}

#[test]
fn a_gradient_drag_of_no_length_does_nothing() {
    let Some(mut app) = app_with(32, 32) else { return };
    set_pixels(&mut app, PixelBuffer::filled(32, 32, Rgba8::TRANSPARENT));
    app.gradient_drag = Some((Vec2::new(10.0, 10.0), Vec2::new(10.2, 10.0)));
    app.commit_gradient();
    assert!(app.doc().unwrap().history.labels().is_empty());
}

// ---------------------------------------------------------------------------
// Clone Stamp
// ---------------------------------------------------------------------------

#[test]
fn the_clone_stamp_copies_from_its_anchor() {
    let Some(mut app) = app_with(64, 32) else { return };
    let mut px = PixelBuffer::filled(64, 32, Rgba8::WHITE);
    px.fill_rect(IRect::new(0, 0, 16, 32), Rgba8::opaque(255, 0, 0));
    set_pixels(&mut app, px);

    app.tool = cshop_ui::tools::Tool::CloneStamp;
    app.brush.size = 12.0;
    app.brush.hardness = 1.0;

    // Sample the red band, then paint on the white side.
    app.set_clone_anchor(Vec2::new(8.0, 16.0));
    app.begin_stroke_with(Vec2::new(40.0, 16.0), PaintMode::Paint, true);
    app.end_stroke();

    assert_eq!(
        pixels(&app).get(40, 16),
        Rgba8::opaque(255, 0, 0),
        "the red should have been copied across"
    );
}

#[test]
fn the_clone_stamp_needs_an_anchor_first() {
    let Some(mut app) = app_with(32, 32) else { return };
    set_pixels(&mut app, PixelBuffer::filled(32, 32, Rgba8::WHITE));
    app.tool = cshop_ui::tools::Tool::CloneStamp;
    app.brush.size = 12.0;

    app.begin_stroke_with(Vec2::new(16.0, 16.0), PaintMode::Paint, true);
    app.end_stroke();
    assert!(
        app.doc().unwrap().history.labels().is_empty(),
        "cloning without a source should do nothing"
    );
}

#[test]
fn the_clone_stamp_keeps_every_brush_control() {
    let Some(mut app) = app_with(64, 32) else { return };
    let mut px = PixelBuffer::filled(64, 32, Rgba8::WHITE);
    px.fill_rect(IRect::new(0, 0, 16, 32), Rgba8::BLACK);
    set_pixels(&mut app, px);

    app.tool = cshop_ui::tools::Tool::CloneStamp;
    app.brush.size = 14.0;
    app.brush.hardness = 1.0;
    app.brush.opacity = 0.5;

    app.set_clone_anchor(Vec2::new(8.0, 16.0));
    app.begin_stroke_with(Vec2::new(40.0, 16.0), PaintMode::Paint, true);
    for _ in 0..8 {
        app.continue_stroke(Vec2::new(40.0, 16.0));
    }
    app.end_stroke();

    let c = pixels(&app).get(40, 16);
    assert!(c.r > 120 && c.r < 136, "opacity should cap the clone at 50%, got {c:?}");
}

#[test]
fn an_unaligned_clone_restarts_from_the_anchor() {
    let Some(mut app) = app_with(64, 32) else { return };
    let mut px = PixelBuffer::filled(64, 32, Rgba8::WHITE);
    px.fill_rect(IRect::new(0, 0, 8, 32), Rgba8::opaque(0, 0, 255));
    set_pixels(&mut app, px);

    app.tool = cshop_ui::tools::Tool::CloneStamp;
    app.brush.size = 10.0;
    app.brush.hardness = 1.0;
    app.clone_aligned = false;
    app.set_clone_anchor(Vec2::new(4.0, 16.0));

    // Two separate strokes at different places: each should copy the blue.
    for x in [30.0f32, 50.0] {
        app.begin_stroke_with(Vec2::new(x, 16.0), PaintMode::Paint, true);
        app.end_stroke();
        assert_eq!(
            pixels(&app).get(x as i32, 16),
            Rgba8::opaque(0, 0, 255),
            "stroke at {x} should copy from the anchor"
        );
    }
}

// ---------------------------------------------------------------------------
// Edit > Fill and the colour picker
// ---------------------------------------------------------------------------

#[test]
fn fill_with_lays_down_the_chosen_colour() {
    let Some(mut app) = app_with(32, 32) else { return };
    set_pixels(&mut app, PixelBuffer::filled(32, 32, Rgba8::WHITE));

    app.dispatch(Action::FillWith {
        color: Rgba8::opaque(10, 20, 30),
        mode: BlendMode::Normal,
        opacity: 1.0,
        preserve_transparency: false,
    });
    assert_eq!(pixels(&app).get(16, 16), Rgba8::opaque(10, 20, 30));

    app.dispatch(Action::Undo);
    assert_eq!(pixels(&app).get(16, 16), Rgba8::WHITE);
}

#[test]
fn fill_opacity_and_mode_are_applied() {
    let Some(mut app) = app_with(16, 16) else { return };
    set_pixels(&mut app, PixelBuffer::filled(16, 16, Rgba8::BLACK));

    app.dispatch(Action::FillWith {
        color: Rgba8::WHITE,
        mode: BlendMode::Normal,
        opacity: 0.5,
        preserve_transparency: false,
    });
    let c = pixels(&app).get(8, 8);
    assert!(c.r > 120 && c.r < 136, "half-opacity white over black, got {c:?}");
}

#[test]
fn fill_can_preserve_transparency() {
    let Some(mut app) = app_with(32, 32) else { return };
    let mut px = PixelBuffer::new(32, 32);
    px.fill_rect(IRect::new(0, 0, 16, 32), Rgba8::opaque(30, 30, 30));
    set_pixels(&mut app, px);

    app.dispatch(Action::FillWith {
        color: Rgba8::WHITE,
        mode: BlendMode::Normal,
        opacity: 1.0,
        preserve_transparency: true,
    });
    assert_eq!(pixels(&app).get(8, 16), Rgba8::WHITE, "inside the shape");
    assert_eq!(pixels(&app).get(24, 16).a, 0, "the empty half stays empty");
}

#[test]
fn the_colour_picker_sets_the_chosen_swatch() {
    let Some(mut app) = app_with(16, 16) else { return };

    app.dispatch(Action::ShowColorPicker(PickerTarget::Foreground));
    assert!(app.dialog.is_open());
    app.dispatch(Action::CloseDialog);

    app.dispatch(Action::SetColor {
        target: PickerTarget::Foreground,
        color: Rgba8::opaque(1, 2, 3),
    });
    assert_eq!(app.foreground, Rgba8::opaque(1, 2, 3));

    app.dispatch(Action::SetColor {
        target: PickerTarget::Background,
        color: Rgba8::opaque(4, 5, 6),
    });
    assert_eq!(app.background, Rgba8::opaque(4, 5, 6));
}

#[test]
fn the_new_tools_do_not_panic_without_a_document() {
    let gpu = match GpuContext::headless() {
        Ok(g) => g,
        Err(_) => return,
    };
    let mut app = CShopApp::new(gpu);
    app.bucket_fill_at(Vec2::new(0.0, 0.0));
    app.gradient_drag = Some((Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0)));
    app.commit_gradient();
    app.set_clone_anchor(Vec2::new(1.0, 1.0));
    app.begin_stroke_with(Vec2::new(2.0, 2.0), PaintMode::Paint, true);
    app.end_stroke();
    for action in [Action::ShowFillDialog, Action::ShowColorPicker(PickerTarget::Foreground)] {
        app.dispatch(action);
    }
    assert!(app.docs.is_empty());
}

#[test]
fn the_clone_stamp_works_at_every_hardness() {
    for hardness in [0.0, 0.2, 0.5, 0.8, 1.0] {
        let Some(mut app) = app_with(64, 32) else { return };
        let mut px = PixelBuffer::filled(64, 32, Rgba8::WHITE);
        px.fill_rect(IRect::new(0, 0, 16, 32), Rgba8::opaque(255, 0, 0));
        set_pixels(&mut app, px);

        app.tool = cshop_ui::tools::Tool::CloneStamp;
        app.brush.size = 12.0;
        app.brush.hardness = hardness;

        app.set_clone_anchor(Vec2::new(8.0, 16.0));
        app.begin_stroke_with(Vec2::new(40.0, 16.0), PaintMode::Paint, true);
        app.end_stroke();

        let c = pixels(&app).get(40, 16);
        assert!(
            c.r > 200 && c.g < 80,
            "hardness {hardness} should still clone red to the centre, got {c:?}"
        );
    }
}

/// With Aligned on, the first stroke pins the offset, so painting elsewhere
/// samples from elsewhere too — and once that lands outside the image the tool
/// deposits nothing. That is the expected behaviour, but it must not look like
/// the tool has broken: no undo step is recorded and the reason is said out
/// loud.
#[test]
fn an_aligned_clone_whose_source_leaves_the_image_says_so() {
    let Some(mut app) = app_with(200, 200) else { return };
    let mut px = PixelBuffer::filled(200, 200, Rgba8::WHITE);
    px.fill_rect(IRect::new(0, 0, 60, 200), Rgba8::opaque(255, 0, 0));
    set_pixels(&mut app, px);

    app.tool = cshop_ui::tools::Tool::CloneStamp;
    app.brush.size = 20.0;
    app.brush.hardness = 0.5;
    app.clone_aligned = true;
    app.set_clone_anchor(Vec2::new(30.0, 100.0));

    // First stroke fixes the offset at (30 - 150, 0).
    app.begin_stroke_with(Vec2::new(150.0, 100.0), PaintMode::Paint, true);
    app.end_stroke();
    assert_eq!(pixels(&app).get(150, 100), Rgba8::opaque(255, 0, 0));
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Clone Stamp"]);

    // Painting near the left edge now samples from off the canvas entirely.
    app.begin_stroke_with(Vec2::new(20.0, 170.0), PaintMode::Paint, true);
    app.end_stroke();
    assert_eq!(
        app.doc().unwrap().history.labels(),
        vec!["Clone Stamp"],
        "a stroke that deposited nothing must not become an undo step"
    );
    assert!(
        app.toast.as_ref().is_some_and(|(m, _)| m.contains("outside the image")),
        "the user should be told why nothing happened, got {:?}",
        app.toast
    );
}

/// The crosshair has to follow the brush once the offset is fixed, or an
/// aligned source can wander off the canvas with nothing on screen to show it.
#[test]
fn the_clone_crosshair_tracks_the_sample_point() {
    let Some(mut app) = app_with(200, 200) else { return };
    set_pixels(&mut app, PixelBuffer::filled(200, 200, Rgba8::WHITE));
    app.tool = cshop_ui::tools::Tool::CloneStamp;
    app.brush.size = 20.0;
    app.set_clone_anchor(Vec2::new(30.0, 100.0));

    // Before any stroke the source is the anchor, wherever the pointer is.
    let at = app.clone_source_at(Some(Vec2::new(150.0, 100.0)));
    assert_eq!(at, Some(Vec2::new(30.0, 100.0)));

    app.begin_stroke_with(Vec2::new(150.0, 100.0), PaintMode::Paint, true);
    app.end_stroke();

    // Now it travels with the brush, and can leave the image.
    let at = app.clone_source_at(Some(Vec2::new(160.0, 100.0))).expect("a source");
    assert_eq!(at, Vec2::new(40.0, 100.0), "the source should move with the brush");
    let at = app.clone_source_at(Some(Vec2::new(20.0, 100.0))).expect("a source");
    assert!(at.x < 0.0, "and the crosshair should report the source being off-canvas");
}
