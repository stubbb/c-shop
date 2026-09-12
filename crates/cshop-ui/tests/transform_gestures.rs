//! Free Transform driven through the real canvas: egui's hit-testing, the
//! cursor it asks for, and the gestures a pointer makes. Calling the tool's
//! methods directly never exercises any of that, and that is where a corner
//! grab was turning into a rotation.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_core::layer::LayerKind;
use cshop_core::pixels::PixelBuffer;
use cshop_ui::commands::Action;
use cshop_ui::input_harness::{Harness, Step};
use cshop_ui::shortcuts::keys as k;
use egui::CursorIcon;

/// A 200x200 document whose only layer is a 100x100 red square at (50, 50).
fn ready() -> Option<Harness> {
    let mut h = Harness::new((1400, 820))?;
    h.app.open_document(Document::new("t", 200, 200, Background::Transparent));
    h.settle(2);
    {
        let view = h.app.doc_mut().unwrap();
        let id = view.doc.active.unwrap();
        let layer = view.doc.tree.get_mut(id).unwrap();
        layer.kind = LayerKind::raster(PixelBuffer::filled(100, 100, Rgba8::opaque(220, 30, 30)));
        layer.offset = (50, 50);
        view.invalidate();
    }
    h.settle(2);
    Some(h)
}

fn begin(h: &mut Harness) {
    h.app.dispatch(Action::BeginTransform);
    h.settle(2);
    assert!(h.app.transform.is_some(), "the transform should have started");
}

fn screen(h: &Harness, x: f32, y: f32) -> (f32, f32) {
    h.doc_to_screen(x, y).unwrap()
}

fn hover(h: &mut Harness, at: (f32, f32)) -> CursorIcon {
    h.run(&[Step::Move(at.0, at.1), Step::Idle, Step::Idle]);
    h.cursor
}

fn corners(h: &Harness) -> [Vec2; 4] {
    h.app.transform.as_ref().unwrap().corners
}

#[test]
fn the_pointer_says_what_a_press_would_do() {
    let Some(mut h) = ready() else { return };
    begin(&mut h);
    let tl = screen(&h, 50.0, 50.0);
    let tr = screen(&h, 150.0, 50.0);
    let top = screen(&h, 100.0, 50.0);
    let right = screen(&h, 150.0, 100.0);
    let centre = screen(&h, 100.0, 100.0);
    let outside = screen(&h, 20.0, 100.0);
    assert_eq!(hover(&mut h, tl), CursorIcon::ResizeNwSe, "a corner shows the diagonal arrow");
    assert_eq!(hover(&mut h, tr), CursorIcon::ResizeNeSw, "turned the way that corner pulls");
    assert_eq!(hover(&mut h, top), CursorIcon::ResizeVertical);
    assert_eq!(hover(&mut h, right), CursorIcon::ResizeHorizontal);
    assert_eq!(hover(&mut h, centre), CursorIcon::Move, "inside, a press moves");
    assert_eq!(hover(&mut h, outside), CursorIcon::Grab, "outside, a press turns");
}

#[test]
fn dragging_inside_the_box_moves_it() {
    let Some(mut h) = ready() else { return };
    begin(&mut h);
    let zoom = h.app.doc().unwrap().zoom;
    let from = screen(&h, 100.0, 100.0);
    h.drag(from, (from.0 + 60.0, from.1 + 30.0), 12);
    let c = corners(&h);
    let (dx, dy) = (c[0].x - 50.0, c[0].y - 50.0);
    assert!(
        (dx - 60.0 / zoom).abs() < 1.5 && (dy - 30.0 / zoom).abs() < 1.5,
        "moved by ({dx}, {dy}) at zoom {zoom}"
    );
    assert!((c[2].x - c[0].x - 100.0).abs() < 0.5, "and kept its size");
}

/// Inside the box but close to a corner used to be the rotation ring, which
/// on a layer this size was most of the picture.
#[test]
fn dragging_inside_near_a_corner_still_moves() {
    let Some(mut h) = ready() else { return };
    begin(&mut h);
    let from = screen(&h, 62.0, 62.0);
    h.drag(from, (from.0 + 40.0, from.1), 8);
    let t = h.app.transform.as_ref().unwrap();
    assert!(t.rotation_degrees().abs() < 0.01, "it turned by {}°", t.rotation_degrees());
    assert!(corners(&h)[0].x > 50.0, "it moved");
}

#[test]
fn dragging_outside_the_box_rotates_it() {
    let Some(mut h) = ready() else { return };
    begin(&mut h);
    // From well to the right of the box round to below it: a quarter turn
    // about the centre.
    let from = screen(&h, 185.0, 100.0);
    let to = screen(&h, 100.0, 185.0);
    h.drag(from, to, 16);
    let t = h.app.transform.as_ref().unwrap();
    assert!((t.rotation_degrees() - 90.0).abs() < 3.0, "got {}°", t.rotation_degrees());
    let c = t.centre();
    assert!((c.x - 100.0).abs() < 0.5 && (c.y - 100.0).abs() < 0.5, "about its centre");
    assert_eq!(h.cursor, CursorIcon::Grab, "and the hand stays while the pointer is out there");
}

#[test]
fn dragging_a_corner_scales_both_dimensions_together() {
    let Some(mut h) = ready() else { return };
    begin(&mut h);
    let from = screen(&h, 150.0, 150.0);
    // Mostly sideways: a free scale would have stretched the box wide.
    h.drag(from, (from.0 + 80.0, from.1 + 20.0), 16);
    let t = h.app.transform.as_ref().unwrap();
    let (sw, sh) = t.scale_percent();
    assert!(sw > 110.0, "it grew: {sw}%");
    assert!((sw - sh).abs() < 0.5, "both dimensions alike: {sw} x {sh}");
    assert_eq!(t.corners[0], Vec2::new(50.0, 50.0), "about the opposite corner");
}

#[test]
fn undo_steps_the_transform_back_and_leaves_the_document_alone() {
    let Some(mut h) = ready() else { return };
    // One entry of real history to protect.
    h.app.dispatch(Action::LayerViaCopy);
    h.settle(1);
    let before = h.app.doc().unwrap().history.cursor();
    assert_eq!(before, 1);
    begin(&mut h);

    let from = screen(&h, 100.0, 100.0);
    h.drag(from, (from.0 + 40.0, from.1), 8);
    let moved = corners(&h);
    assert!(moved[0].x > 50.0);

    h.press(k::UNDO);
    assert_eq!(corners(&h)[0].x, 50.0, "the move steps back");
    assert!(h.app.transform.is_some(), "the transform is still live");
    assert_eq!(h.app.doc().unwrap().history.cursor(), before, "the document's history is untouched");

    h.press(k::UNDO);
    assert_eq!(h.app.doc().unwrap().history.cursor(), before, "even with nothing left to step back");
    assert_eq!(h.app.doc().unwrap().doc.tree.len(), 2);

    h.press(k::REDO);
    assert_eq!(corners(&h), moved, "and redo steps forward again");

    h.press(k::UNDO);
    h.app.dispatch(Action::CancelTransform);
    assert_eq!(h.app.doc().unwrap().history.cursor(), before);
    h.press(k::UNDO);
    assert_eq!(h.app.doc().unwrap().history.cursor(), 0, "once it is gone, undo is the document's again");
}
