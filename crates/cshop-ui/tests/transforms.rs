//! End-to-end tests for phase 4: transforms, crop, resize, and adjustments
//! driven through the application.

use cshop_core::adjust::Adjustment;
use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::{IRect, Vec2};
use cshop_core::layer::{Layer, LayerKind, LayerMask};
use cshop_core::pixels::PixelBuffer;
use cshop_core::resample::Resampling;
use cshop_core::selection::{Rectf, Selection};
use cshop_core::transform::Handle;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::{Action, Anchor, TransformPreset};
use cshop_ui::CShopApp;

fn app_with(w: u32, h: u32, bg: Background) -> Option<CShopApp> {
    let gpu = match GpuContext::headless() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping: {e}");
            return None;
        }
    };
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("test", w, h, bg));
    Some(app)
}

fn layer(app: &CShopApp) -> &Layer {
    let view = app.doc().unwrap();
    view.doc.tree.get(view.doc.active.unwrap()).unwrap()
}

// ---------------------------------------------------------------------------
// Free Transform
// ---------------------------------------------------------------------------

#[test]
fn a_transform_scales_the_layer_and_undoes() {
    let Some(mut app) = app_with(200, 200, Background::Transparent) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().kind =
            LayerKind::raster(PixelBuffer::filled(100, 100, Rgba8::opaque(255, 0, 0)));
        view.invalidate();
    }

    app.dispatch(Action::BeginTransform);
    assert!(app.transform.is_some(), "the transform should have started");
    // The layer is hidden while its preview stands in.
    assert!(!layer(&app).visible);

    {
        let t = app.transform.as_mut().unwrap();
        t.begin_drag(Handle::BottomRight, Vec2::new(100.0, 100.0));
        t.drag_to(Vec2::new(200.0, 200.0), false, false, false);
        t.end_drag();
    }
    app.dispatch(Action::CommitTransform);

    assert!(app.transform.is_none());
    let l = layer(&app);
    assert!(l.visible, "the layer comes back");
    assert_eq!(l.pixels().unwrap().width(), 200, "scaled to twice the size");
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Free Transform"]);

    app.dispatch(Action::Undo);
    assert_eq!(layer(&app).pixels().unwrap().width(), 100);
}

#[test]
fn cancelling_a_transform_leaves_no_trace() {
    // A transparent background gives an unlocked layer; the white one is a
    // locked Background layer and would refuse to transform at all.
    let Some(mut app) = app_with(100, 100, Background::Transparent) else { return };
    app.dispatch(Action::BeginTransform);
    {
        let t = app.transform.as_mut().unwrap();
        t.begin_drag(Handle::Body, Vec2::ZERO);
        t.drag_to(Vec2::new(40.0, 40.0), false, false, false);
        t.end_drag();
    }
    app.dispatch(Action::CancelTransform);

    assert!(app.transform.is_none());
    let l = layer(&app);
    assert!(l.visible, "the layer must be shown again");
    assert_eq!(l.offset, (0, 0), "nothing moved");
    assert!(app.doc().unwrap().history.labels().is_empty(), "no history entry");
}

#[test]
fn committing_an_untouched_transform_records_nothing() {
    let Some(mut app) = app_with(100, 100, Background::Transparent) else { return };
    app.dispatch(Action::BeginTransform);
    assert!(app.transform.is_some());
    app.dispatch(Action::CommitTransform);
    assert!(layer(&app).visible);
    assert!(app.doc().unwrap().history.labels().is_empty());
}

#[test]
fn a_locked_layer_refuses_to_transform() {
    let Some(mut app) = app_with(100, 100, Background::White) else { return };
    // The background layer starts with its position locked.
    app.dispatch(Action::BeginTransform);
    assert!(app.transform.is_none(), "a locked layer should not enter transform");
}

#[test]
fn a_linked_mask_follows_the_transform() {
    let Some(mut app) = app_with(200, 200, Background::Transparent) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        let l = view.doc.tree.get_mut(id).unwrap();
        l.kind = LayerKind::raster(PixelBuffer::filled(100, 100, Rgba8::WHITE));
        l.mask = Some(LayerMask::reveal_all(100, 100));
        view.invalidate();
    }

    app.dispatch(Action::BeginTransform);
    {
        let t = app.transform.as_mut().unwrap();
        t.begin_drag(Handle::Body, Vec2::new(50.0, 50.0));
        t.drag_to(Vec2::new(90.0, 70.0), false, false, false);
        t.end_drag();
    }
    app.dispatch(Action::CommitTransform);

    let l = layer(&app);
    let mask = l.mask.as_ref().expect("the mask survives");
    // The mask must stay registered with the pixels it masks.
    assert_eq!(mask.offset, l.offset, "mask and layer should share an origin");
}

#[test]
fn rotate_ninety_swaps_the_dimensions_losslessly() {
    let Some(mut app) = app_with(200, 200, Background::Transparent) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        let mut px = PixelBuffer::filled(60, 20, Rgba8::opaque(10, 200, 30));
        px.set(0, 0, Rgba8::opaque(255, 0, 0));
        view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(px);
        view.invalidate();
    }

    app.dispatch(Action::TransformPreset(TransformPreset::Rotate90Cw));
    let l = layer(&app);
    let px = l.pixels().unwrap();
    assert_eq!((px.width(), px.height()), (20, 60), "60x20 turned is 20x60");

    // Four turns must return to exactly the original pixels.
    for _ in 0..3 {
        app.dispatch(Action::TransformPreset(TransformPreset::Rotate90Cw));
    }
    let px = layer(&app).pixels().unwrap();
    assert_eq!((px.width(), px.height()), (60, 20), "back to the original size");
    assert_eq!(px.get(0, 0), Rgba8::opaque(255, 0, 0), "and the original pixels");
}

#[test]
fn flipping_twice_returns_the_original() {
    let Some(mut app) = app_with(100, 100, Background::Transparent) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        let mut px = PixelBuffer::filled(40, 40, Rgba8::BLACK);
        px.set(0, 20, Rgba8::WHITE);
        view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(px);
        view.invalidate();
    }

    app.dispatch(Action::TransformPreset(TransformPreset::FlipHorizontal));
    assert_eq!(layer(&app).pixels().unwrap().get(39, 20), Rgba8::WHITE, "the mark moved across");

    app.dispatch(Action::TransformPreset(TransformPreset::FlipHorizontal));
    assert_eq!(layer(&app).pixels().unwrap().get(0, 20), Rgba8::WHITE, "and back again");
}

// ---------------------------------------------------------------------------
// Crop, canvas size, image size
// ---------------------------------------------------------------------------

#[test]
fn cropping_to_a_selection_resizes_the_canvas() {
    let Some(mut app) = app_with(200, 100, Background::White) else { return };
    let s = Selection::from_rect(200, 100, Rectf { x0: 40.0, y0: 20.0, x1: 140.0, y1: 80.0 }, false);
    app.dispatch(Action::SetSelection(Box::new(s), "Rectangular Marquee"));
    app.dispatch(Action::CropToSelection);

    let view = app.doc().unwrap();
    assert_eq!((view.doc.width, view.doc.height), (100, 60));
    // The layer's content should have moved with the new origin.
    assert_eq!(view.doc.tree.get(view.doc.active.unwrap()).unwrap().offset, (-40, -20));

    app.dispatch(Action::Undo);
    let view = app.doc().unwrap();
    assert_eq!((view.doc.width, view.doc.height), (200, 100));
}

#[test]
fn cropping_with_no_selection_does_nothing() {
    let Some(mut app) = app_with(100, 100, Background::White) else { return };
    app.dispatch(Action::CropToSelection);
    assert_eq!(app.doc().unwrap().doc.width, 100);
    assert!(app.doc().unwrap().history.labels().is_empty());
}

#[test]
fn canvas_size_anchors_where_asked() {
    let Some(mut app) = app_with(100, 100, Background::White) else { return };

    // Anchored top-left, the content stays put and the canvas grows away.
    app.dispatch(Action::ResizeCanvas { width: 200, height: 200, anchor: Anchor::TopLeft });
    assert_eq!(layer(&app).offset, (0, 0));
    assert_eq!(app.doc().unwrap().doc.width, 200);

    app.dispatch(Action::Undo);
    // Anchored centre, the content moves to the middle.
    app.dispatch(Action::ResizeCanvas { width: 200, height: 200, anchor: Anchor::Center });
    assert_eq!(layer(&app).offset, (50, 50));

    app.dispatch(Action::Undo);
    app.dispatch(Action::ResizeCanvas { width: 200, height: 200, anchor: Anchor::BottomRight });
    assert_eq!(layer(&app).offset, (100, 100));
}

#[test]
fn canvas_size_does_not_resample() {
    let Some(mut app) = app_with(50, 50, Background::White) else { return };
    let before = layer(&app).pixels().unwrap().clone();
    app.dispatch(Action::ResizeCanvas { width: 400, height: 400, anchor: Anchor::Center });
    assert!(layer(&app).pixels().unwrap() == &before, "the pixels must be untouched");
}

#[test]
fn image_size_resamples_every_layer() {
    let Some(mut app) = app_with(100, 100, Background::White) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.tree.alloc_id();
        view.doc.tree.push(
            Layer::raster(id, "Second", PixelBuffer::filled(100, 100, Rgba8::BLACK)),
            None,
        );
        view.invalidate();
    }

    app.dispatch(Action::ResizeImage { width: 50, height: 50, filter: Resampling::Bilinear });
    let view = app.doc().unwrap();
    assert_eq!((view.doc.width, view.doc.height), (50, 50));
    for id in view.doc.tree.iter_all() {
        let l = view.doc.tree.get(id).unwrap();
        assert_eq!(l.pixels().unwrap().width(), 50, "every layer should have been resized");
    }

    app.dispatch(Action::Undo);
    let view = app.doc().unwrap();
    assert_eq!((view.doc.width, view.doc.height), (100, 100));
    assert_eq!(
        view.doc.tree.get(view.doc.tree.root()[0]).unwrap().pixels().unwrap().width(),
        100
    );
}

#[test]
fn resizing_drops_a_selection_that_no_longer_fits() {
    let Some(mut app) = app_with(100, 100, Background::White) else { return };
    app.dispatch(Action::SelectAll);
    assert!(app.doc().unwrap().doc.has_selection());

    app.dispatch(Action::ResizeImage { width: 50, height: 50, filter: Resampling::Bilinear });
    assert!(
        !app.doc().unwrap().doc.has_selection(),
        "a selection sized to the old canvas has to go"
    );
}

// ---------------------------------------------------------------------------
// Adjustments through the application
// ---------------------------------------------------------------------------

#[test]
fn an_adjustment_layer_lands_above_the_active_layer() {
    let Some(mut app) = app_with(32, 32, Background::White) else { return };
    app.dispatch(Action::AddAdjustmentLayer(Box::new(Adjustment::Invert)));

    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.len(), 2);
    assert_eq!(view.doc.tree.root()[1], view.doc.active.unwrap(), "the new layer is on top");
    assert!(view.doc.active_layer().unwrap().kind.is_adjustment());
    // The layer below is untouched: the whole point of non-destructive.
    let base = view.doc.tree.get(view.doc.tree.root()[0]).unwrap();
    assert_eq!(base.pixels().unwrap().get(4, 4), Rgba8::WHITE);
}

#[test]
fn an_adjustment_layer_picks_up_the_selection_as_its_mask() {
    let Some(mut app) = app_with(64, 64, Background::White) else { return };
    let s = Selection::from_rect(64, 64, Rectf { x0: 8.0, y0: 8.0, x1: 40.0, y1: 40.0 }, false);
    app.dispatch(Action::SetSelection(Box::new(s), "Rectangular Marquee"));
    app.dispatch(Action::AddAdjustmentLayer(Box::new(Adjustment::Invert)));

    let mask = app.doc().unwrap().doc.active_layer().unwrap().mask.as_ref().expect("masked");
    assert_eq!(mask.data.get(20, 20), 255);
    assert_eq!(mask.data.get(50, 50), 0);
}

#[test]
fn a_destructive_adjustment_changes_the_pixels() {
    let Some(mut app) = app_with(32, 32, Background::White) else { return };
    app.dispatch(Action::ApplyAdjustment(Box::new(Adjustment::Invert)));

    assert_eq!(app.doc().unwrap().doc.tree.len(), 1, "no layer was added");
    assert_eq!(layer(&app).pixels().unwrap().get(16, 16), Rgba8::BLACK);
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Invert"]);

    app.dispatch(Action::Undo);
    assert_eq!(layer(&app).pixels().unwrap().get(16, 16), Rgba8::WHITE);
}

#[test]
fn a_destructive_adjustment_respects_the_selection() {
    let Some(mut app) = app_with(64, 64, Background::White) else { return };
    let s = Selection::from_rect(64, 64, Rectf { x0: 0.0, y0: 0.0, x1: 32.0, y1: 64.0 }, false);
    app.dispatch(Action::SetSelection(Box::new(s), "Rectangular Marquee"));
    app.dispatch(Action::ApplyAdjustment(Box::new(Adjustment::Invert)));

    assert_eq!(layer(&app).pixels().unwrap().get(16, 32), Rgba8::BLACK, "inside the selection");
    assert_eq!(layer(&app).pixels().unwrap().get(48, 32), Rgba8::WHITE, "outside is protected");
}

#[test]
fn retuning_an_adjustment_layer_collapses_into_one_entry() {
    let Some(mut app) = app_with(32, 32, Background::White) else { return };
    app.dispatch(Action::AddAdjustmentLayer(Box::new(Adjustment::BrightnessContrast {
        brightness: 0.0,
        contrast: 0.0,
    })));

    for v in [0.1f32, 0.2, 0.3] {
        app.dispatch(Action::SetAdjustment(Box::new(Adjustment::BrightnessContrast {
            brightness: v,
            contrast: 0.0,
        })));
    }
    assert_eq!(
        app.doc().unwrap().history.labels(),
        vec!["New Adjustment Layer", "Brightness/Contrast"],
        "the drag should be a single entry"
    );
}

#[test]
fn transform_actions_on_an_empty_workspace_do_not_panic() {
    let gpu = match GpuContext::headless() {
        Ok(g) => g,
        Err(_) => return,
    };
    let mut app = CShopApp::new(gpu);
    for action in [
        Action::BeginTransform,
        Action::CommitTransform,
        Action::CancelTransform,
        Action::TransformPreset(TransformPreset::Rotate180),
        Action::CommitCrop,
        Action::CancelCrop,
        Action::CropToSelection,
        Action::ShowImageSize,
        Action::ShowCanvasSize,
        Action::ResizeImage { width: 10, height: 10, filter: Resampling::Bicubic },
        Action::ResizeCanvas { width: 10, height: 10, anchor: Anchor::Center },
        Action::AddAdjustmentLayer(Box::new(Adjustment::Invert)),
        Action::ApplyAdjustment(Box::new(Adjustment::Invert)),
        Action::SetAdjustment(Box::new(Adjustment::Invert)),
    ] {
        app.dispatch(action);
    }
    assert!(app.docs.is_empty());
}

#[test]
fn the_anchor_grid_shifts_content_correctly() {
    // Pure arithmetic, but it decides where every layer lands.
    assert_eq!(Anchor::TopLeft.shift((100, 100), (200, 200)), (0, 0));
    assert_eq!(Anchor::Center.shift((100, 100), (200, 200)), (50, 50));
    assert_eq!(Anchor::BottomRight.shift((100, 100), (200, 200)), (100, 100));
    // Shrinking moves content the other way.
    assert_eq!(Anchor::Center.shift((200, 200), (100, 100)), (-50, -50));
    assert_eq!(Anchor::TopLeft.shift((200, 200), (100, 100)), (0, 0));
}

#[test]
fn a_crop_rectangle_is_clamped_to_the_canvas() {
    let Some(mut app) = app_with(100, 100, Background::White) else { return };
    let mut crop = cshop_ui::transform_tool::ActiveCrop::new(IRect::new(10, 10, 90, 90));
    crop.begin_drag(Handle::BottomRight, Vec2::new(90.0, 90.0));
    crop.drag_to(Vec2::new(400.0, 400.0), IRect::new(0, 0, 100, 100));
    app.crop = Some(crop);
    app.dispatch(Action::CommitCrop);

    let view = app.doc().unwrap();
    assert_eq!((view.doc.width, view.doc.height), (90, 90));
}

// ---------------------------------------------------------------------------
// Image > Adjustments
// ---------------------------------------------------------------------------

#[test]
fn the_adjustments_menu_opens_a_dialog_rather_than_applying_a_no_op() {
    // The bug this guards: the menu used to apply each adjustment at its
    // default settings, and most defaults are neutral — Curves defaults to the
    // identity curve — so choosing Curves appeared to do nothing at all.
    let Some(mut app) = app_with(32, 32, Background::Transparent) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().kind =
            LayerKind::raster(PixelBuffer::filled(32, 32, Rgba8::opaque(128, 128, 128)));
        view.invalidate();
    }

    for adjustment in Adjustment::all_defaults() {
        if !adjustment.has_settings() {
            continue;
        }
        app.dispatch(Action::ShowAdjustmentDialog(Box::new(adjustment.clone())));
        assert!(
            app.dialog.is_open(),
            "{} should ask for settings before applying",
            adjustment.name()
        );
        assert!(
            app.doc().unwrap().history.labels().is_empty(),
            "{} applied without asking",
            adjustment.name()
        );
        app.dispatch(Action::CloseDialog);
    }
}

#[test]
fn an_adjustment_with_no_settings_applies_straight_away() {
    let Some(mut app) = app_with(32, 32, Background::Transparent) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().kind =
            LayerKind::raster(PixelBuffer::filled(32, 32, Rgba8::WHITE));
        view.invalidate();
    }
    app.dispatch(Action::ShowAdjustmentDialog(Box::new(Adjustment::Invert)));
    assert!(!app.dialog.is_open(), "Invert has nothing to configure");
    assert_eq!(layer(&app).pixels().unwrap().get(16, 16), Rgba8::BLACK);
}

#[test]
fn curves_actually_change_the_image_once_configured() {
    let Some(mut app) = app_with(32, 32, Background::Transparent) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().kind =
            LayerKind::raster(PixelBuffer::filled(32, 32, Rgba8::opaque(128, 128, 128)));
        view.invalidate();
    }

    // What the dialog produces once the user has dragged a point.
    let mut curves: [cshop_core::curve::Curve; 4] = Default::default();
    curves[0] = cshop_core::curve::Curve::new(vec![(0.0, 0.0), (0.5, 0.85), (1.0, 1.0)]);
    app.dispatch(Action::ApplyAdjustment(Box::new(Adjustment::Curves { curves })));

    let px = layer(&app).pixels().unwrap().get(16, 16);
    assert!(px.r > 200, "mid-grey should have lifted, got {px:?}");
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Curves"]);
}
