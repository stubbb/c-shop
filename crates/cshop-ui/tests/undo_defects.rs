//! Undo defects found by QA, one test each.
//!
//! Every test here **fails on purpose**. Each is a defect found while testing
//! the undo stack, reduced to the smallest thing that shows it, and written to
//! assert what *should* happen. They are `#[ignore]`d so the suite stays green,
//! and the list is run with:
//!
//! ```sh
//! cargo test -p cshop-ui --test undo_defects -- --ignored
//! ```
//!
//! Fixing one means deleting its `#[ignore]`. If a test here starts passing,
//! that is the signal.
//!
//! What *works* is in `undo_fidelity.rs`, which is green and covers filters,
//! three hundred mixed edits, the model-driven tools, the worker threads and
//! the stack's own arithmetic.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::selection::{Rectf, Selection};
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

fn app(w: u32, h: u32) -> Option<CShopApp> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("t", w, h, Background::White));
    Some(app)
}

fn layers(app: &CShopApp) -> Vec<(u64, String, (i32, i32), u64)> {
    let doc = &app.doc().unwrap().doc;
    doc.tree
        .iter_all()
        .into_iter()
        .filter_map(|id| doc.tree.get(id))
        .map(|l| {
            let sum = l.pixels().map_or(0, |p| p.as_bytes().iter().map(|b| *b as u64).sum());
            (l.id.0, l.name.clone(), l.offset, sum)
        })
        .collect()
}

fn cursor(app: &CShopApp) -> usize {
    app.doc().unwrap().history.cursor()
}

/// Undo every entry one gesture recorded.
fn undo_the_gesture(app: &mut CShopApp, back_to: usize) {
    while cursor(app) > back_to {
        app.dispatch(Action::Undo);
    }
}

fn a_second_layer(app: &mut CShopApp) {
    app.dispatch(Action::NewLayer);
    app.foreground = Rgba8::opaque(200, 40, 40);
    app.dispatch(Action::FillSwatch { background: false, preserve_transparency: false });
}

// ---------------------------------------------------------------------------

/// **Flatten Image loses a layer's position, and its name, on undo.**
///
/// `CShopApp::flatten` sets the bottom layer's `name` and `offset` directly
/// rather than through a command, so undo has nothing to put back. The pixels
/// are wrong too: the flattened image is written at a rect in *document*
/// coordinates while the layer sits at an offset, so what undo restores is
/// misaligned.
///
/// Reproduce by hand: grow the canvas with a centred anchor — which moves
/// every layer off the origin — then Image ▸ Flatten, then undo.
#[test]
#[ignore = "known defect: flatten writes name and offset outside the history"]
fn flatten_restores_the_layer_it_kept() {
    let Some(mut app) = app(80, 60) else { return };
    a_second_layer(&mut app);
    app.dispatch(Action::ResizeCanvas {
        width: 120,
        height: 100,
        anchor: cshop_ui::commands::Anchor::Center,
    });

    let before = layers(&app);
    let at = cursor(&app);
    app.dispatch(Action::FlattenImage);
    undo_the_gesture(&mut app, at);
    assert_eq!(layers(&app), before, "flatten's undo did not put the layers back");
}

/// The same defect seen through the layer's name alone, which is the half a
/// user notices first.
#[test]
#[ignore = "known defect: flatten renames the bottom layer outside the history"]
fn flatten_does_not_rename_the_layer_permanently() {
    let Some(mut app) = app(80, 60) else { return };
    {
        let view = app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        view.doc.tree.get_mut(id).unwrap().name = "My Photograph".into();
    }
    app.dispatch(Action::NewLayer);

    let at = cursor(&app);
    app.dispatch(Action::FlattenImage);
    undo_the_gesture(&mut app, at);
    let name = app.doc().unwrap().doc.tree.iter_all().first().and_then(|id| {
        app.doc().unwrap().doc.tree.get(*id).map(|l| l.name.clone())
    });
    assert_eq!(name.as_deref(), Some("My Photograph"), "the layer stayed renamed");
}

/// **Save Selection as Channel cannot be undone.**
///
/// It adds a channel to the document and records nothing, so Ctrl+Z afterwards
/// undoes whatever came *before* it while the channel stays — the confusing
/// case, because something the user did not point at disappears instead.
#[test]
#[ignore = "known defect: saving a selection as a channel records no history"]
fn saving_a_selection_as_a_channel_can_be_undone() {
    let Some(mut app) = app(100, 80) else { return };
    let sel = Selection::from_rect(
        100,
        80,
        Rectf::from_points(Vec2::new(20.0, 20.0), Vec2::new(80.0, 60.0)),
        true,
    );
    app.dispatch(Action::SetSelection(Box::new(sel), "Select"));
    let at = cursor(&app);

    app.dispatch(Action::SaveSelectionAsChannel);
    assert_eq!(app.doc().unwrap().doc.channels.len(), 1, "it should have saved one");
    assert_eq!(cursor(&app), at + 1, "and recorded exactly one history entry");

    app.dispatch(Action::Undo);
    assert_eq!(app.doc().unwrap().doc.channels.len(), 0, "which undo should take away");
}

/// **One gesture should be one Ctrl+Z.**
///
/// Flatten records a deletion per layer it absorbs, Merge Down records two
/// entries, and Separate by Content records one per layer it produces. The
/// document does come back, but only after as many undos as the gesture made
/// entries — and the first undo leaves a state the user never saw: with
/// Flatten, the picture restored and the layers still gone.
///
/// `Compound` already exists for exactly this and is what Layer via Copy uses.
#[test]
#[ignore = "known defect: flatten and merge down record several entries each"]
fn a_single_gesture_records_a_single_entry() {
    let Some(mut app) = app(80, 60) else { return };

    a_second_layer(&mut app);
    let at = cursor(&app);
    app.dispatch(Action::MergeDown);
    assert_eq!(cursor(&app) - at, 1, "Merge Down recorded more than one entry");

    for _ in 0..3 {
        a_second_layer(&mut app);
    }
    let at = cursor(&app);
    app.dispatch(Action::FlattenImage);
    assert_eq!(cursor(&app) - at, 1, "Flatten Image recorded one entry per layer");
}

/// **Crop to Selection is called "Canvas Size" in the History panel**, and its
/// undo does not bring back the selection that defined the crop.
#[test]
#[ignore = "known defect: crop is labelled as a canvas resize and drops the selection"]
fn cropping_is_named_after_itself_and_keeps_the_selection() {
    let Some(mut app) = app(160, 120) else { return };
    let sel = Selection::from_rect(
        160,
        120,
        Rectf::from_points(Vec2::new(10.0, 8.0), Vec2::new(70.0, 60.0)),
        true,
    );
    app.dispatch(Action::SetSelection(Box::new(sel), "Select"));
    let bounds = app.doc().unwrap().doc.selection.as_ref().map(|s| s.bounds());
    let at = cursor(&app);

    app.dispatch(Action::CropToSelection);
    let label = app.doc().unwrap().history.labels()[at].clone();
    assert_eq!(label, "Crop", "the History panel calls the crop {label:?}");

    app.dispatch(Action::Undo);
    assert_eq!(
        app.doc().unwrap().doc.selection.as_ref().map(|s| s.bounds()),
        bounds,
        "undoing a crop should give back the selection it cropped to"
    );
}

/// **Undo while a preview window is open leaves the history and the document
/// disagreeing.**
///
/// The preview windows write to the layer directly and put it back on Cancel,
/// which is what makes the canvas the preview. They are not modal, so Edit ▸
/// Undo is still clickable behind them — confirmed with real clicks through
/// the input harness. Undo then moves the cursor and the pixels, and Cancel
/// afterwards restores the pixels as they were when the window opened, undoing
/// the undo without moving the cursor back.
///
/// What is left: the cursor says the edit is undone, the canvas shows it
/// applied, `undo_name` is `None` so the user cannot get back, and Redo
/// advances the cursor without changing anything.
#[test]
#[ignore = "known defect: a preview window's Cancel can overwrite an undo"]
fn undo_behind_a_preview_window_agrees_with_the_document() {
    if !cshop_ui::vision::is_available() {
        eprintln!("no vision pack; skipping");
        return;
    }
    let Some(mut app) = app(180, 140) else { return };
    let untouched = layers(&app);

    app.foreground = Rgba8::opaque(255, 0, 0);
    app.dispatch(Action::FillSwatch { background: false, preserve_transparency: false });

    app.dispatch(Action::ShowRelight);
    app.dispatch(Action::Undo);
    app.dispatch(Action::RelightCancel);

    assert_eq!(cursor(&app), 0, "the fill should be undone");
    assert_eq!(
        layers(&app),
        untouched,
        "the document should show the fill undone, not the window's copy of it"
    );
}
