//! Guides through the interface: placed, saved, snapped to.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::guides::Guide;
use cshop_ui::commands::Action;
use cshop_ui::input_harness::Harness;

fn open(h: &mut Harness) {
    h.app.open_document(Document::new("t", 800, 600, Background::Color(Rgba8::WHITE)));
    h.settle(2);
}

#[test]
fn a_guide_can_be_placed_and_cleared() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);
    h.app.push(Action::AddGuide { vertical: true, at: 400.0 });
    h.app.push(Action::AddGuide { vertical: false, at: 300.0 });
    h.settle(2);
    assert_eq!(h.app.doc().unwrap().doc.guides.len(), 2);

    h.app.push(Action::ClearGuides);
    h.settle(2);
    assert!(h.app.doc().unwrap().doc.guides.is_empty());
}

/// The rulers take a strip off two edges, and the picture gets the rest — so
/// turning them off has to give that space back.
#[test]
fn the_rulers_take_room_from_the_canvas_and_give_it_back() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);
    assert!(h.app.show_rulers, "they start on");
    let with = h.app.canvas_viewport;

    h.app.push(Action::ToggleRulers);
    h.settle(2);
    let without = h.app.canvas_viewport;

    assert!(without.width() > with.width(), "the picture should get the strip back");
    assert!(without.height() > with.height());
    assert!(
        (without.width() - with.width() - cshop_ui::rulers::RULER).abs() < 0.01,
        "and exactly the strip: {} against {}",
        without.width() - with.width(),
        cshop_ui::rulers::RULER
    );
}

/// Dragging a layer near a guide should catch on it. Driven through the real
/// pointer, because the snapping lives in the tool rather than in an action.
#[test]
fn a_dragged_layer_catches_on_a_guide() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    // A small layer on a large canvas, so there is room to drag it about.
    h.app.open_document(Document::new("t", 400, 300, Background::Transparent));
    h.settle(2);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    if let Some(view) = h.app.doc_mut() {
        if let Some(layer) = view.doc.tree.get_mut(id) {
            let mut px = cshop_core::pixels::PixelBuffer::filled(60, 40, Rgba8::WHITE);
            px.set(0, 0, Rgba8::BLACK);
            layer.kind = cshop_core::layer::LayerKind::Raster(px);
            layer.offset = (10, 10);
        }
    }
    h.settle(2);

    // A guide a little off where the drag would naturally land, so that
    // catching it and not catching it cannot give the same answer.
    let target = 103.0;
    h.app.push(Action::AddGuide { vertical: true, at: target });
    h.app.push(Action::SelectTool(cshop_ui::tools::Tool::Move));
    h.settle(2);
    assert!(h.app.snap, "snapping is on by default");

    // Drag so the left edge lands a couple of pixels short of the guide.
    let from = h.doc_to_screen(20.0, 20.0).expect("on screen");
    let to = h.doc_to_screen(108.0, 20.0).expect("on screen");
    h.drag(from, to, 6);
    h.settle(2);

    let offset = h.app.doc().unwrap().doc.tree.get(id).unwrap().offset;
    assert_eq!(
        offset.0 as f32, target,
        "the left edge should have caught the guide, but it is at {}",
        offset.0
    );
}

/// With snapping off it lands where it was dragged, guide or no guide.
#[test]
fn snapping_can_be_turned_off() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(Document::new("t", 400, 300, Background::Transparent));
    h.settle(2);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    if let Some(view) = h.app.doc_mut() {
        if let Some(layer) = view.doc.tree.get_mut(id) {
            layer.kind = cshop_core::layer::LayerKind::Raster(
                cshop_core::pixels::PixelBuffer::filled(60, 40, Rgba8::WHITE),
            );
            layer.offset = (10, 10);
        }
    }
    h.app.push(Action::AddGuide { vertical: true, at: 103.0 });
    h.app.push(Action::ToggleSnap);
    h.app.push(Action::SelectTool(cshop_ui::tools::Tool::Move));
    h.settle(2);
    assert!(!h.app.snap);

    let from = h.doc_to_screen(20.0, 20.0).expect("on screen");
    let to = h.doc_to_screen(108.0, 20.0).expect("on screen");
    h.drag(from, to, 6);
    h.settle(2);

    let offset = h.app.doc().unwrap().doc.tree.get(id).unwrap().offset;
    assert_ne!(offset.0, 103, "with snapping off it should land where it was put");
}

/// Guides are the document's, so they travel with it.
#[test]
fn guides_are_saved_with_the_document() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);
    if let Some(view) = h.app.doc_mut() {
        view.doc.guides = vec![Guide::vertical(120.0), Guide::horizontal(80.0)];
    }
    h.settle(1);

    let doc = &h.app.doc().unwrap().doc;
    let bytes = cshop_io::project::write(doc);
    let back = cshop_io::project::read(&bytes).expect("read");
    assert_eq!(back.guides, doc.guides);
}
