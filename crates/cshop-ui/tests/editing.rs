//! End-to-end tests of the editing operations, driving `CShopApp` directly.
//!
//! These exercise the paths the canvas and panels invoke — painting, layer
//! management, undo — without a window or an event loop, so a regression in
//! interactive behaviour fails here rather than under someone's mouse.
//!
//! They need a GPU for the compositor, and skip when none is available.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::layer::{Layer, LayerKind};
use cshop_core::paint::PaintMode;
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

fn app() -> Option<CShopApp> {
    match GpuContext::headless() {
        Ok(gpu) => Some(CShopApp::new(gpu)),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn with_doc(w: u32, h: u32, bg: Background) -> Option<CShopApp> {
    let mut app = app()?;
    app.open_document(Document::new("test", w, h, bg));
    Some(app)
}

/// Pixels of the active layer.
fn active_pixels(app: &CShopApp) -> &PixelBuffer {
    let view = app.doc().expect("a document is open");
    let id = view.doc.active.expect("a layer is active");
    view.doc.tree.get(id).unwrap().pixels().expect("the active layer is raster")
}

#[test]
fn a_stroke_paints_and_undoes_as_one_step() {
    let Some(mut app) = with_doc(64, 64, Background::White) else { return };
    app.foreground = Rgba8::opaque(255, 0, 0);
    app.brush.size = 10.0;
    app.brush.hardness = 1.0;

    app.begin_stroke(Vec2::new(10.0, 32.0), PaintMode::Paint);
    for x in 11..54 {
        app.continue_stroke(Vec2::new(x as f32, 32.0));
    }
    app.end_stroke();

    assert_eq!(active_pixels(&app).get(32, 32), Rgba8::opaque(255, 0, 0));
    assert_eq!(active_pixels(&app).get(32, 5), Rgba8::WHITE, "outside the stroke");

    // The whole drag must collapse into a single history entry.
    let view = app.doc().unwrap();
    assert_eq!(view.history.labels(), vec!["Brush Tool"]);

    app.dispatch(Action::Undo);
    assert_eq!(active_pixels(&app).get(32, 32), Rgba8::WHITE, "undo should clear the stroke");

    app.dispatch(Action::Redo);
    assert_eq!(active_pixels(&app).get(32, 32), Rgba8::opaque(255, 0, 0));
}

#[test]
fn erasing_cuts_a_hole_in_the_layer() {
    let Some(mut app) = with_doc(64, 64, Background::White) else { return };
    app.brush.size = 12.0;
    app.brush.hardness = 1.0;

    app.begin_stroke(Vec2::new(32.0, 32.0), PaintMode::Erase);
    app.end_stroke();

    assert_eq!(active_pixels(&app).get(32, 32).a, 0);
    assert_eq!(active_pixels(&app).get(2, 2).a, 255);
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Eraser Tool"]);
}

#[test]
fn a_locked_layer_refuses_paint() {
    let Some(mut app) = with_doc(32, 32, Background::White) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().locks.pixels = true;
    }

    app.foreground = Rgba8::BLACK;
    app.begin_stroke(Vec2::new(16.0, 16.0), PaintMode::Paint);
    app.end_stroke();

    assert_eq!(active_pixels(&app).get(16, 16), Rgba8::WHITE, "the lock should hold");
    assert!(app.doc().unwrap().history.labels().is_empty());
}

#[test]
fn cancelling_a_stroke_restores_the_layer() {
    let Some(mut app) = with_doc(32, 32, Background::White) else { return };
    app.foreground = Rgba8::BLACK;
    app.brush.size = 12.0;

    app.begin_stroke(Vec2::new(16.0, 16.0), PaintMode::Paint);
    assert_ne!(active_pixels(&app).get(16, 16), Rgba8::WHITE, "the preview should be live");

    app.cancel_stroke();
    assert_eq!(active_pixels(&app).get(16, 16), Rgba8::WHITE);
    assert!(app.doc().unwrap().history.labels().is_empty(), "a cancel leaves no history");
}

#[test]
fn painting_off_canvas_records_nothing() {
    let Some(mut app) = with_doc(32, 32, Background::White) else { return };
    app.begin_stroke(Vec2::new(-500.0, -500.0), PaintMode::Paint);
    app.continue_stroke(Vec2::new(-490.0, -500.0));
    app.end_stroke();
    assert!(app.doc().unwrap().history.labels().is_empty());
}

#[test]
fn layers_can_be_added_deleted_and_restored() {
    let Some(mut app) = with_doc(32, 32, Background::White) else { return };
    assert_eq!(app.doc().unwrap().doc.tree.len(), 1);

    app.dispatch(Action::NewLayer);
    assert_eq!(app.doc().unwrap().doc.tree.len(), 2);
    // A new layer lands above the active one and becomes active itself.
    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.root().len(), 2);
    assert_eq!(view.doc.active, Some(view.doc.tree.root()[1]));

    app.dispatch(Action::DeleteLayer);
    assert_eq!(app.doc().unwrap().doc.tree.len(), 1);

    app.dispatch(Action::Undo);
    assert_eq!(app.doc().unwrap().doc.tree.len(), 2);
}

#[test]
fn the_last_layer_cannot_be_deleted() {
    let Some(mut app) = with_doc(32, 32, Background::White) else { return };
    app.dispatch(Action::DeleteLayer);
    assert_eq!(app.doc().unwrap().doc.tree.len(), 1, "a document keeps at least one layer");
}

#[test]
fn duplicating_copies_the_pixels_but_not_the_background_lock() {
    let Some(mut app) = with_doc(16, 16, Background::White) else { return };
    app.dispatch(Action::DuplicateLayer);

    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.len(), 2);
    let copy = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
    assert_eq!(copy.name, "Background copy");
    assert!(!copy.is_background, "a duplicate is an ordinary layer");
    assert!(!copy.locks.any());
    assert_eq!(copy.pixels().unwrap().get(0, 0), Rgba8::WHITE);
}

#[test]
fn fill_and_clear_act_on_the_active_layer() {
    let Some(mut app) = with_doc(16, 16, Background::Transparent) else { return };
    app.foreground = Rgba8::opaque(10, 20, 30);

    app.dispatch(Action::fill_foreground(false));
    assert_eq!(active_pixels(&app).get(8, 8), Rgba8::opaque(10, 20, 30));

    app.dispatch(Action::ClearLayer);
    // Clearing zeroes alpha but leaves the colour bytes alone. Under zero alpha
    // they are invisible, and keeping them avoids the dark fringe that zeroed
    // colour would produce if the image were later resampled.
    assert_eq!(active_pixels(&app).get(8, 8).a, 0);

    app.dispatch(Action::Undo);
    assert_eq!(active_pixels(&app).get(8, 8), Rgba8::opaque(10, 20, 30));
}

#[test]
fn nudging_moves_the_layer_and_undoes_in_one_step() {
    let Some(mut app) = with_doc(32, 32, Background::Transparent) else { return };
    let id = app.doc().unwrap().doc.active.unwrap();

    for _ in 0..5 {
        app.dispatch(Action::NudgeLayer(1, 0));
    }
    assert_eq!(app.doc().unwrap().doc.tree.get(id).unwrap().offset, (5, 0));
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Move Layer"]);

    app.dispatch(Action::Undo);
    assert_eq!(app.doc().unwrap().doc.tree.get(id).unwrap().offset, (0, 0));
}

#[test]
fn a_locked_background_will_not_be_nudged() {
    let Some(mut app) = with_doc(32, 32, Background::White) else { return };
    let id = app.doc().unwrap().doc.active.unwrap();
    app.dispatch(Action::NudgeLayer(4, 4));
    assert_eq!(app.doc().unwrap().doc.tree.get(id).unwrap().offset, (0, 0));
}

#[test]
fn flatten_collapses_the_stack_to_the_composited_image() {
    let Some(mut app) = with_doc(8, 8, Background::Transparent) else { return };
    // Bottom: opaque blue.
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().kind =
            LayerKind::raster(PixelBuffer::filled(8, 8, Rgba8::opaque(0, 0, 255)));
        view.invalidate();
    }
    // Top: half-opacity red.
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.tree.alloc_id();
        let mut l = Layer::raster(id, "Red", PixelBuffer::filled(8, 8, Rgba8::opaque(255, 0, 0)));
        l.opacity = 0.5;
        view.doc.tree.push(l, None);
        view.invalidate();
    }

    app.dispatch(Action::FlattenImage);
    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.len(), 1, "flatten leaves exactly one layer");

    let px = active_pixels(&app).get(4, 4);
    assert!(px.r > 120 && px.r < 136, "expected a 50/50 blend, got {px:?}");
    assert!(px.b > 120 && px.b < 136);
    assert_eq!(px.a, 255);
}

#[test]
fn merge_down_honours_the_upper_layer_opacity() {
    let Some(mut app) = with_doc(8, 8, Background::Transparent) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().kind =
            LayerKind::raster(PixelBuffer::filled(8, 8, Rgba8::BLACK));
        view.invalidate();
    }
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.tree.alloc_id();
        let mut l = Layer::raster(id, "White", PixelBuffer::filled(8, 8, Rgba8::WHITE));
        l.opacity = 0.5;
        view.doc.tree.push(l, None);
        view.doc.select(Some(id));
        view.invalidate();
    }

    app.dispatch(Action::MergeDown);
    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.len(), 1);
    let px = view.doc.tree.get(view.doc.tree.root()[0]).unwrap().pixels().unwrap().get(4, 4);
    assert!(px.r > 120 && px.r < 136, "expected mid-grey, got {px:?}");
}

#[test]
fn zoom_actions_stay_within_the_allowed_range() {
    let Some(mut app) = with_doc(100, 100, Background::White) else { return };
    app.canvas_viewport =
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

    for _ in 0..40 {
        app.dispatch(Action::ZoomIn);
    }
    assert!(app.doc().unwrap().zoom <= cshop_ui::doc_view::MAX_ZOOM + 1e-3);

    for _ in 0..80 {
        app.dispatch(Action::ZoomOut);
    }
    assert!(app.doc().unwrap().zoom >= cshop_ui::doc_view::MIN_ZOOM - 1e-6);

    app.dispatch(Action::ZoomActual);
    assert!((app.doc().unwrap().zoom - 1.0).abs() < 1e-6);

    // Fit never enlarges past 100%, so a small document stays actual-size.
    app.dispatch(Action::ZoomFit);
    assert!(app.doc().unwrap().zoom <= 1.0);
}

#[test]
fn closing_documents_keeps_the_active_index_valid() {
    let Some(mut app) = app() else { return };
    for i in 0..3 {
        app.open_document(Document::new(format!("d{i}"), 8, 8, Background::White));
    }
    assert_eq!(app.docs.len(), 3);
    assert_eq!(app.active, Some(2));

    app.dispatch(Action::CloseDocument(0));
    assert_eq!(app.docs.len(), 2);
    assert_eq!(app.active, Some(1));

    app.dispatch(Action::CloseDocument(1));
    app.dispatch(Action::CloseDocument(0));
    assert!(app.docs.is_empty());
    assert_eq!(app.active, None, "with no documents there is no active index");
}

#[test]
fn history_jump_reaches_an_arbitrary_state() {
    let Some(mut app) = with_doc(16, 16, Background::Transparent) else { return };
    app.foreground = Rgba8::opaque(1, 2, 3);
    app.dispatch(Action::fill_foreground(false));
    app.dispatch(Action::NewLayer);
    app.dispatch(Action::NewLayer);
    assert_eq!(app.doc().unwrap().doc.tree.len(), 3);

    app.dispatch(Action::HistoryJump(0));
    assert_eq!(app.doc().unwrap().doc.tree.len(), 1);
    assert_eq!(active_pixels(&app).get(8, 8), Rgba8::TRANSPARENT);

    app.dispatch(Action::HistoryJump(3));
    assert_eq!(app.doc().unwrap().doc.tree.len(), 3);
}

#[test]
fn operations_on_an_empty_workspace_do_not_panic() {
    let Some(mut app) = app() else { return };
    // Every action must tolerate having no document open.
    for action in [
        Action::Undo,
        Action::Redo,
        Action::Save,
        Action::NewLayer,
        Action::DeleteLayer,
        Action::DuplicateLayer,
        Action::MergeDown,
        Action::FlattenImage,
        Action::fill_foreground(false),
        Action::ClearLayer,
        Action::ZoomIn,
        Action::ZoomFit,
        Action::NudgeLayer(1, 1),
        Action::CloseDocument(0),
        Action::HistoryJump(5),
    ] {
        app.dispatch(action);
    }
    assert!(app.docs.is_empty());
}

#[test]
fn saving_writes_the_composited_stack_and_reopens() {
    let Some(mut app) = with_doc(32, 32, Background::Transparent) else { return };

    // Bottom blue, top half-opacity red: the file must contain the blend, not
    // just the active layer.
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().kind =
            LayerKind::raster(PixelBuffer::filled(32, 32, Rgba8::opaque(0, 0, 255)));
        let top = view.doc.tree.alloc_id();
        let mut l = Layer::raster(top, "Red", PixelBuffer::filled(32, 32, Rgba8::opaque(255, 0, 0)));
        l.opacity = 0.5;
        view.doc.tree.push(l, None);
        view.invalidate();
    }

    let dir = std::env::temp_dir().join("cshop-test-save");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.png");
    let _ = std::fs::remove_file(&path);

    app.dispatch(Action::SavePath { path: path.clone(), deep: false });
    assert!(path.exists(), "saving should have written the file");

    let reloaded = cshop_io::load(&path).expect("the saved file should decode");
    assert_eq!((reloaded.width(), reloaded.height()), (32, 32));
    let px = reloaded.get(16, 16);
    assert!(px.r > 120 && px.r < 136, "expected the blended result, got {px:?}");
    assert!(px.b > 120 && px.b < 136);
    assert_eq!(px.a, 255);

    // Saving clears the modified marker and adopts the new path and name.
    let view = app.doc().unwrap();
    assert!(!view.doc.modified);
    assert_eq!(view.doc.path.as_deref(), Some(path.as_path()));
    assert_eq!(view.doc.name, "out.png");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn jpeg_export_flattens_transparency_onto_white() {
    let Some(mut app) = with_doc(16, 16, Background::Transparent) else { return };
    let dir = std::env::temp_dir().join("cshop-test-save");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.jpg");
    let _ = std::fs::remove_file(&path);

    app.dispatch(Action::SavePath { path: path.clone(), deep: false });
    let reloaded = cshop_io::load(&path).expect("the JPEG should decode");
    let px = reloaded.get(8, 8);
    assert!(px.r > 240 && px.g > 240 && px.b > 240, "transparency should become white, got {px:?}");

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// The Layers panel's command row
// ---------------------------------------------------------------------------

#[test]
fn merge_down_on_the_bottom_layer_does_nothing() {
    // The panel greys the button out, but the action has to be safe anyway:
    // there is nothing beneath the bottom layer to merge into.
    let Some(mut app) = with_doc(16, 16, Background::White) else { return };
    app.dispatch(Action::MergeDown);
    assert_eq!(app.doc().unwrap().doc.tree.len(), 1);
    assert!(app.doc().unwrap().history.labels().is_empty());
}

#[test]
fn merge_all_collapses_a_document_with_groups() {
    let Some(mut app) = with_doc(16, 16, Background::Transparent) else { return };
    {
        let view = app.doc_mut().unwrap();
        let base = view.doc.active.unwrap();
        view.doc.tree.get_mut(base).unwrap().kind =
            LayerKind::raster(PixelBuffer::filled(16, 16, Rgba8::opaque(0, 0, 255)));

        // A group with a child, so flatten has to cope with nesting: deleting
        // the group takes the child with it.
        let g = view.doc.tree.alloc_id();
        view.doc.tree.push(Layer::group(g, "Group"), None);
        let child = view.doc.tree.alloc_id();
        let mut l = Layer::raster(child, "Child", PixelBuffer::filled(16, 16, Rgba8::WHITE));
        l.opacity = 0.5;
        view.doc.tree.push(l, Some(g));
        view.invalidate();
    }
    assert_eq!(app.doc().unwrap().doc.tree.len(), 3);

    app.dispatch(Action::FlattenImage);
    let view = app.doc().unwrap();
    assert_eq!(view.doc.tree.len(), 1, "everything collapses to one layer");

    // And the result is what was on screen: white at 50% over blue.
    let px = active_pixels(&app).get(8, 8);
    assert!(px.r > 110 && px.r < 145, "expected the blended result, got {px:?}");
    assert_eq!(px.a, 255);
}

#[test]
fn merge_all_on_a_single_layer_is_harmless() {
    let Some(mut app) = with_doc(16, 16, Background::White) else { return };
    app.dispatch(Action::FlattenImage);
    assert_eq!(app.doc().unwrap().doc.tree.len(), 1);
    assert_eq!(active_pixels(&app).get(8, 8), Rgba8::WHITE);
}

#[test]
fn merge_all_undoes_back_to_the_full_stack() {
    let Some(mut app) = with_doc(16, 16, Background::Transparent) else { return };
    app.dispatch(Action::NewLayer);
    app.dispatch(Action::NewLayer);
    assert_eq!(app.doc().unwrap().doc.tree.len(), 3);

    let before = app.doc().unwrap().history.cursor();
    app.dispatch(Action::FlattenImage);
    assert_eq!(app.doc().unwrap().doc.tree.len(), 1);

    // Flatten is one gesture and so one entry, however many layers it took in.
    // It used to record a deletion apiece, which meant the first Ctrl+Z gave
    // the picture back and left the layers gone — a state nobody had been in.
    assert_eq!(
        app.doc().unwrap().history.cursor() - before,
        1,
        "flatten should be a single history entry"
    );
    app.dispatch(Action::Undo);
    assert_eq!(app.doc().unwrap().doc.tree.len(), 3, "the stack comes back in one step");
}

#[test]
fn a_layer_can_be_renamed_and_undone() {
    // Until the context menu there was no way to rename a layer at all.
    let Some(mut app) = with_doc(16, 16, Background::White) else { return };
    let id = app.doc().unwrap().doc.active.unwrap();
    assert_eq!(app.doc().unwrap().doc.tree.get(id).unwrap().name, "Background");

    app.dispatch(Action::SetLayerProperty(
        id,
        cshop_core::history::LayerProperty::Name("Sky".into()),
    ));
    assert_eq!(app.doc().unwrap().doc.tree.get(id).unwrap().name, "Sky");
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Rename Layer"]);

    app.dispatch(Action::Undo);
    assert_eq!(app.doc().unwrap().doc.tree.get(id).unwrap().name, "Background");
}

#[test]
fn opening_the_rename_dialog_needs_a_layer() {
    let gpu = match cshop_gpu::context::GpuContext::headless() {
        Ok(g) => g,
        Err(_) => return,
    };
    let mut app = CShopApp::new(gpu);
    // No document, so there is nothing to rename and nothing should open.
    app.dispatch(Action::RenameLayer(cshop_core::layer::LayerId(1)));
    assert!(!app.dialog.is_open());
}

#[test]
fn blend_mode_can_be_set_per_layer_and_undone() {
    // The context menu offers all 27; this checks the command behind them.
    let Some(mut app) = with_doc(16, 16, Background::White) else { return };
    let id = app.doc().unwrap().doc.active.unwrap();

    for mode in cshop_core::blend::BlendMode::all() {
        app.dispatch(Action::SetLayerProperty(
            id,
            cshop_core::history::LayerProperty::Blend(mode),
        ));
        assert_eq!(
            app.doc().unwrap().doc.tree.get(id).unwrap().blend_mode,
            mode,
            "{mode:?} did not stick"
        );
    }

    // Every change to the same field collapses into one history entry.
    assert_eq!(app.doc().unwrap().history.labels(), vec!["Blend Mode"]);
    app.dispatch(Action::Undo);
    assert_eq!(
        app.doc().unwrap().doc.tree.get(id).unwrap().blend_mode,
        cshop_core::blend::BlendMode::Normal,
        "undo returns to the mode before the run of changes"
    );
}

#[test]
fn a_group_can_use_pass_through() {
    // Pass Through only means anything for a group, and the context menu only
    // offers it there.
    let Some(mut app) = with_doc(16, 16, Background::Transparent) else { return };
    app.dispatch(Action::NewGroup);
    let id = app.doc().unwrap().doc.active.unwrap();
    assert!(app.doc().unwrap().doc.tree.get(id).unwrap().kind.is_group());

    app.dispatch(Action::SetLayerProperty(
        id,
        cshop_core::history::LayerProperty::Blend(cshop_core::blend::BlendMode::PassThrough),
    ));
    assert_eq!(
        app.doc().unwrap().doc.tree.get(id).unwrap().blend_mode,
        cshop_core::blend::BlendMode::PassThrough
    );
}
