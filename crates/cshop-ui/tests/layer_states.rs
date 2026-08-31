//! Layer states, driven through the application.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::layer::{Layer, LayerKind};
use cshop_core::pixels::PixelBuffer;
use cshop_gpu::context::GpuContext;
use cshop_ui::commands::Action;
use cshop_ui::CShopApp;

/// Two layers, one red and one blue, each covering the canvas.
fn app() -> Option<(CShopApp, cshop_core::layer::LayerId, cshop_core::layer::LayerId)> {
    let gpu = GpuContext::headless().ok()?;
    let mut app = CShopApp::new(gpu);
    app.open_document(Document::new("t", 32, 32, Background::Transparent));
    let view = app.doc_mut()?;
    let a = view.doc.tree.alloc_id();
    view.doc.tree.push(
        Layer::new(a, "Red", LayerKind::raster(PixelBuffer::filled(32, 32, Rgba8::opaque(220, 30, 30)))),
        None,
    );
    let b = view.doc.tree.alloc_id();
    view.doc.tree.push(
        Layer::new(b, "Blue", LayerKind::raster(PixelBuffer::filled(32, 32, Rgba8::opaque(30, 30, 220)))),
        None,
    );
    view.doc.active = Some(a);
    view.invalidate();
    Some((app, a, b))
}

fn set_visible(app: &mut CShopApp, id: cshop_core::layer::LayerId, on: bool) {
    let view = app.doc_mut().unwrap();
    view.doc.tree.get_mut(id).unwrap().visible = on;
    view.invalidate();
}

fn visible(app: &CShopApp, id: cshop_core::layer::LayerId) -> bool {
    app.doc().unwrap().doc.tree.get(id).unwrap().visible
}

/// What the canvas actually shows, which is the thing a state is for.
fn canvas(app: &mut CShopApp) -> Rgba8 {
    let gpu = app.gpu.clone();
    app.render_composite(&gpu, 0).get(16, 16)
}

#[test]
fn two_versions_of_one_document_can_be_switched_between() {
    let Some((mut app, a, b)) = app() else { return };

    set_visible(&mut app, a, true);
    set_visible(&mut app, b, false);
    app.dispatch(Action::SaveLayerState("Red".into()));

    set_visible(&mut app, a, false);
    set_visible(&mut app, b, true);
    app.dispatch(Action::SaveLayerState("Blue".into()));
    assert_eq!(app.doc().unwrap().doc.states.len(), 2);

    app.dispatch(Action::ApplyLayerState(0));
    assert!(visible(&app, a) && !visible(&app, b));
    assert_eq!(canvas(&mut app).r, 220, "the canvas shows the red version");

    app.dispatch(Action::ApplyLayerState(1));
    assert!(!visible(&app, a) && visible(&app, b));
    assert_eq!(canvas(&mut app).b, 220, "and now the blue one");
}

/// A state remembers settings, not pixels, so an edit made after it was saved
/// is still there when it comes back. That is the reason to have states rather
/// than two documents.
#[test]
fn an_edit_made_afterwards_survives_the_state_coming_back() {
    let Some((mut app, a, b)) = app() else { return };
    set_visible(&mut app, b, false);
    app.dispatch(Action::SaveLayerState("Red".into()));

    // Repaint the red layer green, then switch away and back.
    {
        let view = app.doc_mut().unwrap();
        view.doc.tree.get_mut(a).unwrap().kind =
            LayerKind::raster(PixelBuffer::filled(32, 32, Rgba8::opaque(30, 200, 30)));
        view.invalidate();
    }
    set_visible(&mut app, b, true);
    app.dispatch(Action::ApplyLayerState(0));

    assert!(!visible(&app, b), "the state came back");
    assert_eq!(canvas(&mut app).g, 200, "and the repaint came with it");
}

#[test]
fn applying_a_state_is_one_undo_step() {
    let Some((mut app, a, b)) = app() else { return };
    set_visible(&mut app, a, true);
    set_visible(&mut app, b, false);
    app.dispatch(Action::SaveLayerState("Red".into()));
    set_visible(&mut app, a, false);
    set_visible(&mut app, b, true);

    app.dispatch(Action::ApplyLayerState(0));
    assert!(visible(&app, a) && !visible(&app, b));
    app.dispatch(Action::Undo);
    assert!(!visible(&app, a) && visible(&app, b), "undo puts the layers back");
}

#[test]
fn a_state_can_be_replaced_and_forgotten() {
    let Some((mut app, a, _b)) = app() else { return };
    app.dispatch(Action::SaveLayerState("One".into()));
    set_visible(&mut app, a, false);
    app.dispatch(Action::UpdateLayerState(0));

    // The saved one now says hidden, so re-applying keeps it hidden.
    set_visible(&mut app, a, true);
    app.dispatch(Action::ApplyLayerState(0));
    assert!(!visible(&app, a));

    app.dispatch(Action::DeleteLayerState(0));
    assert!(app.doc().unwrap().doc.states.is_empty());
}

#[test]
fn an_unnamed_state_gets_a_name() {
    let Some((mut app, _a, _b)) = app() else { return };
    app.dispatch(Action::SaveLayerState(String::new()));
    app.dispatch(Action::SaveLayerState("  ".into()));
    let names: Vec<String> =
        app.doc().unwrap().doc.states.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names, vec!["State 1", "State 2"]);
}
