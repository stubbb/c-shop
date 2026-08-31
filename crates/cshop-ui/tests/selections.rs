//! End-to-end tests for phase 3: selections, masks and Quick Mask.
//!
//! These drive `CShopApp` the way the canvas and menus do, so a regression in
//! "paint only inside the selection" — the single most important consequence of
//! having selections at all — fails here.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document, EditTarget};
use cshop_core::geom::Vec2;
use cshop_core::layer::LayerKind;
use cshop_core::paint::PaintMode;
use cshop_core::pixels::PixelBuffer;
use cshop_core::selection::{Rectf, Selection, SelectionMode};
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::{Action, ModifySelection};
use cshop_ui::CShopApp;

fn app_with_doc(w: u32, h: u32, bg: Background) -> Option<CShopApp> {
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

fn pixels(app: &CShopApp) -> &PixelBuffer {
    let view = app.doc().expect("a document is open");
    let id = view.doc.active.expect("a layer is active");
    view.doc.tree.get(id).unwrap().pixels().expect("raster layer")
}

fn select_rect(app: &mut CShopApp, x0: f32, y0: f32, x1: f32, y1: f32) {
    let (w, h) = {
        let d = app.doc().unwrap();
        (d.doc.width, d.doc.height)
    };
    let s = Selection::from_rect(w, h, Rectf { x0, y0, x1, y1 }, false);
    app.dispatch(Action::SetSelection(Box::new(s), "Rectangular Marquee"));
}

#[test]
fn a_selection_confines_painting() {
    let Some(mut app) = app_with_doc(64, 64, Background::White) else { return };
    select_rect(&mut app, 16.0, 16.0, 48.0, 48.0);

    app.foreground = Rgba8::BLACK;
    app.brush.size = 60.0;
    app.brush.hardness = 1.0;
    app.begin_stroke(Vec2::new(32.0, 32.0), PaintMode::Paint);
    app.end_stroke();

    assert_eq!(pixels(&app).get(32, 32), Rgba8::BLACK, "inside the selection");
    assert_eq!(pixels(&app).get(8, 32), Rgba8::WHITE, "outside is protected");
    assert_eq!(pixels(&app).get(56, 32), Rgba8::WHITE);
}

#[test]
fn a_selection_confines_fill_and_clear() {
    let Some(mut app) = app_with_doc(32, 32, Background::White) else { return };
    select_rect(&mut app, 8.0, 8.0, 24.0, 24.0);

    app.foreground = Rgba8::opaque(255, 0, 0);
    app.dispatch(Action::fill_foreground(false));
    assert_eq!(pixels(&app).get(16, 16), Rgba8::opaque(255, 0, 0));
    assert_eq!(pixels(&app).get(2, 2), Rgba8::WHITE, "outside the selection");

    app.dispatch(Action::ClearLayer);
    assert_eq!(pixels(&app).get(16, 16).a, 0, "cleared inside");
    assert_eq!(pixels(&app).get(2, 2).a, 255, "untouched outside");
}

#[test]
fn a_feathered_selection_fades_the_fill() {
    let Some(mut app) = app_with_doc(64, 64, Background::White) else { return };
    let s = Selection::from_rect(64, 64, Rectf { x0: 16.0, y0: 16.0, x1: 48.0, y1: 48.0 }, false);
    app.dispatch(Action::SetSelection(Box::new(s), "Rectangular Marquee"));
    app.dispatch(Action::ModifySelection(ModifySelection::Feather(6.0)));

    app.foreground = Rgba8::BLACK;
    app.dispatch(Action::fill_foreground(false));

    assert!(pixels(&app).get(32, 32).r < 20, "the middle fills solidly");
    let edge = pixels(&app).get(16, 32).r;
    assert!(edge > 20 && edge < 235, "the feathered edge should be partial, got {edge}");
    assert_eq!(pixels(&app).get(4, 32), Rgba8::WHITE, "well outside stays clean");
}

#[test]
fn select_all_deselect_and_reselect_round_trip() {
    let Some(mut app) = app_with_doc(32, 32, Background::White) else { return };
    assert!(!app.doc().unwrap().doc.has_selection());

    app.dispatch(Action::SelectAll);
    assert!(app.doc().unwrap().doc.has_selection());

    app.dispatch(Action::Deselect);
    assert!(!app.doc().unwrap().doc.has_selection());

    app.dispatch(Action::Reselect);
    assert!(app.doc().unwrap().doc.has_selection(), "Reselect brings the last one back");

    app.dispatch(Action::Undo);
    assert!(!app.doc().unwrap().doc.has_selection());
    app.dispatch(Action::Redo);
    assert!(app.doc().unwrap().doc.has_selection());
}

#[test]
fn inverse_swaps_what_is_protected() {
    let Some(mut app) = app_with_doc(32, 32, Background::White) else { return };
    select_rect(&mut app, 0.0, 0.0, 16.0, 32.0);
    app.dispatch(Action::InverseSelection);

    app.foreground = Rgba8::BLACK;
    app.dispatch(Action::fill_foreground(false));
    assert_eq!(pixels(&app).get(24, 16), Rgba8::BLACK, "the inverted half fills");
    assert_eq!(pixels(&app).get(4, 16), Rgba8::WHITE, "the original half is protected");
}

#[test]
fn inverting_with_nothing_selected_selects_everything() {
    let Some(mut app) = app_with_doc(16, 16, Background::White) else { return };
    app.dispatch(Action::InverseSelection);
    let d = app.doc().unwrap();
    assert!(d.doc.has_selection());
    assert!(d.doc.selection.as_ref().unwrap().is_everything());
}

#[test]
fn boolean_modes_accumulate_across_gestures() {
    let Some(mut app) = app_with_doc(64, 32, Background::White) else { return };
    select_rect(&mut app, 0.0, 0.0, 16.0, 32.0);

    // Add a second band.
    app.selection_mode = SelectionMode::Add;
    let s = Selection::from_rect(64, 32, Rectf { x0: 32.0, y0: 0.0, x1: 48.0, y1: 32.0 }, false);
    app.dispatch(Action::SetSelection(Box::new(s), "Rectangular Marquee"));

    let d = app.doc().unwrap();
    let sel = d.doc.selection.as_ref().unwrap();
    assert_eq!(sel.coverage(8, 16), 255);
    assert_eq!(sel.coverage(40, 16), 255);
    assert_eq!(sel.coverage(24, 16), 0, "the gap stays unselected");

    // Now subtract part of the first band.
    app.selection_mode = SelectionMode::Subtract;
    let s = Selection::from_rect(64, 32, Rectf { x0: 0.0, y0: 0.0, x1: 8.0, y1: 32.0 }, false);
    app.dispatch(Action::SetSelection(Box::new(s), "Rectangular Marquee"));

    let sel = app.doc().unwrap().doc.selection.as_ref().unwrap();
    assert_eq!(sel.coverage(4, 16), 0, "subtracted");
    assert_eq!(sel.coverage(12, 16), 255, "the rest of the band survives");
}

#[test]
fn the_magic_wand_selects_a_matching_region() {
    let Some(mut app) = app_with_doc(32, 32, Background::Transparent) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        let mut px = PixelBuffer::filled(32, 32, Rgba8::opaque(200, 30, 30));
        px.fill_rect(cshop_core::geom::IRect::new(16, 0, 32, 32), Rgba8::opaque(30, 30, 200));
        view.doc.tree.get_mut(id).unwrap().kind = LayerKind::raster(px);
        view.invalidate();
    }

    app.wand.tolerance = 20;
    app.wand.antialias = false;
    app.magic_wand_at(Vec2::new(4.0, 16.0), SelectionMode::Replace);

    let sel = app.doc().unwrap().doc.selection.as_ref().expect("the wand made a selection");
    assert_eq!(sel.coverage(8, 16), 255, "the clicked red half");
    assert_eq!(sel.coverage(24, 16), 0, "the blue half");
}

#[test]
fn modify_operations_change_the_selection_bounds() {
    let Some(mut app) = app_with_doc(64, 64, Background::White) else { return };
    select_rect(&mut app, 20.0, 20.0, 44.0, 44.0);

    app.dispatch(Action::ModifySelection(ModifySelection::Expand(6)));
    let b = app.doc().unwrap().doc.selection.as_ref().unwrap().bounds();
    assert!(b.x0 <= 14 && b.x1 >= 50, "expand should grow the bounds, got {b:?}");

    app.dispatch(Action::ModifySelection(ModifySelection::Contract(6)));
    let b = app.doc().unwrap().doc.selection.as_ref().unwrap().bounds();
    assert!(b.x0 >= 19 && b.x1 <= 45, "contract should shrink it back, got {b:?}");

    app.dispatch(Action::Undo);
    app.dispatch(Action::Undo);
    let b = app.doc().unwrap().doc.selection.as_ref().unwrap().bounds();
    assert_eq!(b, cshop_core::geom::IRect::new(20, 20, 44, 44), "undo restores the original");
}

#[test]
fn contracting_a_selection_away_deselects() {
    let Some(mut app) = app_with_doc(64, 64, Background::White) else { return };
    select_rect(&mut app, 30.0, 30.0, 34.0, 34.0);
    app.dispatch(Action::ModifySelection(ModifySelection::Contract(20)));
    assert!(!app.doc().unwrap().doc.has_selection(), "an emptied selection becomes no selection");
}

// ---------------------------------------------------------------------------
// Layer masks
// ---------------------------------------------------------------------------

#[test]
fn a_layer_mask_from_a_selection_hides_the_rest() {
    let Some(mut app) = app_with_doc(32, 32, Background::White) else { return };
    select_rect(&mut app, 8.0, 8.0, 24.0, 24.0);
    app.dispatch(Action::AddLayerMaskFromSelection { invert: false });

    let view = app.doc().unwrap();
    let layer = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
    let mask = layer.mask.as_ref().expect("a mask was added");
    assert_eq!(mask.data.get(16, 16), 255, "inside the selection is revealed");
    assert_eq!(mask.data.get(2, 2), 0, "outside is hidden");
    // Adding a mask makes it the thing you are editing.
    assert_eq!(view.doc.effective_edit_target(), EditTarget::Mask);
}

#[test]
fn painting_the_mask_target_leaves_the_pixels_alone() {
    let Some(mut app) = app_with_doc(32, 32, Background::White) else { return };
    app.dispatch(Action::AddLayerMask { hide_all: false });
    app.dispatch(Action::SetEditTarget(EditTarget::Mask));

    app.foreground = Rgba8::BLACK;
    app.brush.size = 10.0;
    app.brush.hardness = 1.0;
    app.begin_stroke(Vec2::new(16.0, 16.0), PaintMode::Paint);
    app.end_stroke();

    let view = app.doc().unwrap();
    let layer = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
    assert_eq!(layer.mask.as_ref().unwrap().data.get(16, 16), 0, "black conceals");
    assert_eq!(layer.pixels().unwrap().get(16, 16), Rgba8::WHITE, "pixels untouched");
    assert_eq!(view.history.labels(), vec!["Add Layer Mask", "Brush Tool"]);

    app.dispatch(Action::Undo);
    let view = app.doc().unwrap();
    let layer = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
    assert_eq!(layer.mask.as_ref().unwrap().data.get(16, 16), 255, "undo restores the mask");
}

#[test]
fn applying_a_mask_bakes_it_and_undoes() {
    let Some(mut app) = app_with_doc(32, 32, Background::White) else { return };
    select_rect(&mut app, 0.0, 0.0, 16.0, 32.0);
    app.dispatch(Action::AddLayerMaskFromSelection { invert: false });
    app.dispatch(Action::ApplyLayerMask);

    let view = app.doc().unwrap();
    let layer = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
    assert!(layer.mask.is_none(), "the mask is consumed");
    assert_eq!(layer.pixels().unwrap().get(4, 16).a, 255, "the revealed half stays");
    assert_eq!(layer.pixels().unwrap().get(24, 16).a, 0, "the hidden half is now transparent");

    app.dispatch(Action::Undo);
    let view = app.doc().unwrap();
    let layer = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
    assert!(layer.mask.is_some());
    assert_eq!(layer.pixels().unwrap().get(24, 16).a, 255);
}

#[test]
fn a_disabled_mask_is_kept_but_stops_applying() {
    let Some(mut app) = app_with_doc(16, 16, Background::White) else { return };
    app.dispatch(Action::AddLayerMask { hide_all: true });
    app.dispatch(Action::ToggleMaskEnabled);

    let view = app.doc().unwrap();
    let mask = view.doc.tree.get(view.doc.active.unwrap()).unwrap().mask.as_ref().unwrap();
    assert!(!mask.enabled);
    assert_eq!(mask.data.get(8, 8), 0, "the mask data survives");
}

#[test]
fn a_second_mask_is_refused() {
    let Some(mut app) = app_with_doc(16, 16, Background::White) else { return };
    app.dispatch(Action::AddLayerMask { hide_all: false });
    app.dispatch(Action::AddLayerMask { hide_all: true });
    assert_eq!(
        app.doc().unwrap().history.labels(),
        vec!["Add Layer Mask"],
        "the second request should be rejected, not stacked"
    );
}

// ---------------------------------------------------------------------------
// Quick Mask and channels
// ---------------------------------------------------------------------------

#[test]
fn quick_mask_painting_edits_the_selection() {
    let Some(mut app) = app_with_doc(64, 64, Background::White) else { return };
    app.dispatch(Action::ToggleQuickMask);
    // Entering with nothing selected starts fully selected.
    assert!(app.doc().unwrap().doc.selection.as_ref().unwrap().is_everything());

    app.foreground = Rgba8::BLACK;
    app.brush.size = 20.0;
    app.brush.hardness = 1.0;
    app.begin_stroke(Vec2::new(32.0, 32.0), PaintMode::Paint);
    app.end_stroke();

    let sel = app.doc().unwrap().doc.selection.as_ref().unwrap();
    assert_eq!(sel.coverage(32, 32), 0, "painting black protects that area");
    assert_eq!(sel.coverage(2, 2), 255, "elsewhere stays selected");

    // The pixels themselves must be untouched.
    assert_eq!(pixels(&app).get(32, 32), Rgba8::WHITE);

    app.dispatch(Action::Undo);
    let sel = app.doc().unwrap().doc.selection.as_ref().unwrap();
    assert_eq!(sel.coverage(32, 32), 255, "undo restores the selection");
}

#[test]
fn selections_save_to_and_load_from_channels() {
    let Some(mut app) = app_with_doc(32, 32, Background::White) else { return };
    select_rect(&mut app, 4.0, 4.0, 20.0, 20.0);
    app.dispatch(Action::SaveSelectionAsChannel);

    assert_eq!(app.doc().unwrap().doc.channels.len(), 1);
    assert_eq!(app.doc().unwrap().doc.channels[0].name, "Alpha 1");

    app.dispatch(Action::Deselect);
    assert!(!app.doc().unwrap().doc.has_selection());

    app.dispatch(Action::LoadChannelAsSelection(0));
    let sel = app.doc().unwrap().doc.selection.as_ref().unwrap();
    assert_eq!(sel.bounds(), cshop_core::geom::IRect::new(4, 4, 20, 20));

    app.dispatch(Action::DeleteChannel(0));
    assert!(app.doc().unwrap().doc.channels.is_empty());
}

#[test]
fn selection_actions_on_an_empty_workspace_do_not_panic() {
    let gpu = match GpuContext::headless() {
        Ok(g) => g,
        Err(_) => return,
    };
    let mut app = CShopApp::new(gpu);
    for action in [
        Action::SelectAll,
        Action::Deselect,
        Action::Reselect,
        Action::InverseSelection,
        Action::ModifySelection(ModifySelection::Feather(2.0)),
        Action::GrowSelection,
        Action::SimilarSelection,
        Action::ToggleQuickMask,
        Action::SaveSelectionAsChannel,
        Action::LoadChannelAsSelection(0),
        Action::DeleteChannel(0),
        Action::AddLayerMask { hide_all: false },
        Action::AddLayerMaskFromSelection { invert: true },
        Action::DeleteLayerMask,
        Action::ApplyLayerMask,
        Action::ToggleMaskEnabled,
        Action::CancelDrag,
        Action::CloseDrag,
    ] {
        app.dispatch(action);
    }
    assert!(app.docs.is_empty());
}

#[test]
fn modify_without_a_selection_is_a_no_op() {
    let Some(mut app) = app_with_doc(16, 16, Background::White) else { return };
    app.dispatch(Action::ModifySelection(ModifySelection::Expand(4)));
    assert!(!app.doc().unwrap().doc.has_selection());
    assert!(app.doc().unwrap().history.labels().is_empty());
}
