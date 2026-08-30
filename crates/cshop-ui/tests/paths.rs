//! The Pen tool and the boolean operations, driven through the real interface.

use cshop_core::document::{Background, Document};
use cshop_core::shape::ShapeKind;
use cshop_ui::input_harness::Harness;
use cshop_ui::tools::Tool;

fn ready() -> Option<Harness> {
    let mut h = Harness::new((1400, 820))?;
    h.app.open_document(Document::new("t", 300, 200, Background::White));
    h.settle(3);
    Some(h)
}

fn shape_kinds(h: &Harness) -> Vec<ShapeKind> {
    let view = h.app.doc().expect("a document");
    view.doc
        .tree
        .iter_all()
        .into_iter()
        .filter_map(|id| view.doc.tree.get(id))
        .filter_map(|l| l.shape().map(|s| s.content().kind.clone()))
        .collect()
}

#[test]
fn the_pen_draws_a_closed_path() {
    let Some(mut h) = ready() else { return };
    h.app.tool = Tool::Pen;

    // Three corners, then a click back on the first to close it.
    let pts = [(40.0, 40.0), (200.0, 40.0), (200.0, 150.0)];
    for (x, y) in pts {
        let Some(at) = h.doc_to_screen(x, y) else { return };
        h.click(at);
        h.settle(1);
    }
    assert_eq!(h.app.pen.as_ref().map(|p| p.anchors.len()), Some(3), "three anchors placed");

    let Some(first) = h.doc_to_screen(pts[0].0, pts[0].1) else { return };
    h.click(first);
    h.settle(2);

    assert!(h.app.pen.is_none(), "closing should end the draft");
    let kinds = shape_kinds(&h);
    assert_eq!(kinds.len(), 1, "a path layer should have been added");
    match &kinds[0] {
        ShapeKind::Path(p) => {
            let sub = &p.parts[0].subpaths[0];
            assert!(sub.closed, "clicking the first anchor closes the path");
            assert_eq!(sub.anchors.len(), 3);
        }
        other => panic!("added a {} instead", other.name()),
    }
}

/// Enter ends a path where it is, which is how an open curve is drawn.
#[test]
fn enter_finishes_an_open_path() {
    let Some(mut h) = ready() else { return };
    h.app.tool = Tool::Pen;
    for (x, y) in [(40.0, 40.0), (150.0, 90.0), (240.0, 40.0)] {
        let Some(at) = h.doc_to_screen(x, y) else { return };
        h.click(at);
        h.settle(1);
    }
    h.press(cshop_ui::shortcuts::Chord::plain(egui::Key::Enter));
    h.settle(2);

    let kinds = shape_kinds(&h);
    assert_eq!(kinds.len(), 1);
    match &kinds[0] {
        ShapeKind::Path(p) => {
            assert!(!p.parts[0].subpaths[0].closed, "Enter leaves it open");
            assert!(kinds[0].is_open(), "and an open path is a stroke");
        }
        other => panic!("added a {} instead", other.name()),
    }
}

#[test]
fn escape_abandons_the_path_and_changes_nothing() {
    let Some(mut h) = ready() else { return };
    h.app.tool = Tool::Pen;
    for (x, y) in [(40.0, 40.0), (150.0, 90.0)] {
        let Some(at) = h.doc_to_screen(x, y) else { return };
        h.click(at);
        h.settle(1);
    }
    h.press(cshop_ui::shortcuts::Chord::plain(egui::Key::Escape));
    h.settle(2);

    assert!(h.app.pen.is_none(), "the draft should be gone");
    assert!(shape_kinds(&h).is_empty(), "and nothing should have been added");
    assert!(!h.app.doc().expect("a document").history.can_undo(), "nor recorded");
}

/// The operands become one layer, and the result is a path however they began.
#[test]
fn combining_two_shapes_leaves_one_path_layer() {
    use cshop_core::path::BoolOp;
    for op in BoolOp::all() {
        let Some(mut h) = ready() else { return };
        h.app.shape_kind = ShapeKind::Ellipse;
        h.app.tool = Tool::Shape;
        for x in [20.0f32, 90.0] {
            let Some(a) = h.doc_to_screen(x, 40.0) else { return };
            let Some(b) = h.doc_to_screen(x + 110.0, 150.0) else { return };
            h.drag(a, b, 4);
            h.settle(1);
        }
        assert_eq!(shape_kinds(&h).len(), 2, "{}: two ellipses to combine", op.name());

        let ids: Vec<_> = {
            let view = h.app.doc().expect("a document");
            view.doc.tree.iter_all()
        };
        if let Some(view) = h.app.doc_mut() {
            view.doc.selected_layers = ids.into_iter().skip(1).collect();
        }
        h.app.push(cshop_ui::commands::Action::CombineShapes(op));
        h.settle(2);

        let kinds = shape_kinds(&h);
        assert_eq!(kinds.len(), 1, "{}: the operands should be gone", op.name());
        match &kinds[0] {
            ShapeKind::Path(p) => {
                assert_eq!(p.parts.len(), 2, "{}: both operands are kept", op.name());
                assert_eq!(p.parts[1].op, op, "{}: with the chosen operation", op.name());
            }
            other => panic!("{}: came out as a {}", op.name(), other.name()),
        }

        // One history step, so undo brings both shapes back.
        let view = h.app.doc_mut().expect("a document");
        let doc_ptr = &mut view.doc;
        view.history.undo(doc_ptr).expect("should undo");
        h.settle(1);
        assert_eq!(shape_kinds(&h).len(), 2, "{}: undo restores the operands", op.name());
    }
}

// ---------------------------------------------------------------------------
// Editing a path
// ---------------------------------------------------------------------------

/// Draw a closed triangle with the Pen and leave it selected for editing.
fn with_a_path() -> Option<Harness> {
    let mut h = ready()?;
    h.app.tool = Tool::Pen;
    let pts = [(40.0, 40.0), (200.0, 40.0), (200.0, 150.0)];
    for (x, y) in pts {
        let at = h.doc_to_screen(x, y)?;
        h.click(at);
        h.settle(1);
    }
    let first = h.doc_to_screen(pts[0].0, pts[0].1)?;
    h.click(first);
    h.settle(2);
    h.app.tool = Tool::DirectSelect;
    Some(h)
}

/// Where an anchor sits in document space right now.
fn anchor_at(h: &Harness, which: usize) -> Option<cshop_core::geom::Vec2> {
    let (_, path, origin) = h.app.editable_path()?;
    let a = path.parts.first()?.subpaths.first()?.anchors.get(which)?;
    Some(cshop_core::geom::Vec2::new(origin.x + a.at.x, origin.y + a.at.y))
}

#[test]
fn clicking_an_anchor_selects_it_and_clicking_away_clears_it() {
    let Some(mut h) = with_a_path() else { return };
    let Some(a) = anchor_at(&h, 1) else { return };
    let Some(at) = h.doc_to_screen(a.x, a.y) else { return };
    h.click(at);
    h.settle(1);
    assert_eq!(h.app.path_edit.selected, vec![(0, 0, 1)], "the anchor clicked is selected");

    // Somewhere with no point under it.
    let Some(empty) = h.doc_to_screen(120.0, 130.0) else { return };
    h.click(empty);
    h.settle(1);
    assert!(h.app.path_edit.selected.is_empty(), "clicking away clears the selection");
}

#[test]
fn dragging_an_anchor_moves_it_and_is_one_undo_step() {
    let Some(mut h) = with_a_path() else { return };
    let Some(before) = anchor_at(&h, 1) else { return };
    let steps_before = h.app.doc().expect("a document").history.cursor();

    let Some(from) = h.doc_to_screen(before.x, before.y) else { return };
    let Some(to) = h.doc_to_screen(before.x + 40.0, before.y + 25.0) else { return };
    h.drag(from, to, 8);
    h.settle(2);

    let Some(after) = anchor_at(&h, 1) else { return };
    assert!(
        (after.x - (before.x + 40.0)).abs() < 2.0 && (after.y - (before.y + 25.0)).abs() < 2.0,
        "the anchor should have followed the pointer: {before:?} -> {after:?}"
    );

    let steps_after = h.app.doc().expect("a document").history.cursor();
    assert_eq!(
        steps_after - steps_before,
        1,
        "a drag of eight frames should leave one undo step, not eight"
    );

    // And undoing it puts the anchor back.
    let view = h.app.doc_mut().expect("a document");
    let doc = &mut view.doc;
    view.history.undo(doc).expect("should undo");
    h.settle(1);
    let Some(restored) = anchor_at(&h, 1) else { return };
    assert!(
        restored.distance(before) < 1.5,
        "undo should put it back: {before:?} against {restored:?}"
    );
}

/// A handle only appears once its anchor is selected, and moving it reshapes
/// the curve without moving the anchor.
#[test]
fn dragging_a_handle_reshapes_the_curve() {
    let Some(mut h) = with_a_path() else { return };

    // Pull a handle out of anchor 1 so there is one to grab.
    {
        let (id, mut path, _) = h.app.editable_path().expect("a path");
        let a = &mut path.parts[0].subpaths[0].anchors[1];
        *a = cshop_core::path::Anchor::smooth(a.at, a.at + cshop_core::geom::Vec2::new(30.0, 20.0));
        h.app.set_path_for_test(id, path);
        h.settle(1);
    }
    h.app.path_edit.selected = vec![(0, 0, 1)];

    // Compared in document space: moving a handle can grow the path's bounds,
    // and the box is renormalised to them, so local coordinates shift even
    // where nothing moved on screen.
    let (_, path, origin) = h.app.editable_path().expect("a path");
    let a = path.parts[0].subpaths[0].anchors[1];
    let doc = |o: cshop_core::geom::Vec2, p: cshop_core::geom::Vec2| {
        cshop_core::geom::Vec2::new(o.x + p.x, o.y + p.y)
    };
    let anchor_before = doc(origin, a.at);
    let handle = doc(origin, a.out_handle);

    let Some(from) = h.doc_to_screen(handle.x, handle.y) else { return };
    let Some(to) = h.doc_to_screen(handle.x + 25.0, handle.y - 15.0) else { return };
    h.drag(from, to, 6);
    h.settle(2);

    let (_, after, after_origin) = h.app.editable_path().expect("a path");
    let b = after.parts[0].subpaths[0].anchors[1];
    assert!(
        doc(after_origin, b.at).distance(anchor_before) < 1.5,
        "the anchor itself should not have moved: {anchor_before:?} -> {:?}",
        doc(after_origin, b.at)
    );
    assert!(
        doc(after_origin, b.out_handle).distance(handle) > 10.0,
        "the handle should have moved"
    );
    // Smooth anchors keep their handles mirrored.
    assert!(b.is_smooth(), "a smooth anchor stays smooth when its handle is dragged");
}

#[test]
fn delete_removes_the_selected_anchors() {
    let Some(mut h) = with_a_path() else { return };
    h.app.path_edit.selected = vec![(0, 0, 2)];
    h.press(cshop_ui::shortcuts::Chord::plain(egui::Key::Delete));
    h.settle(2);

    let (_, path, _) = h.app.editable_path().expect("the path should still be there");
    assert_eq!(path.parts[0].subpaths[0].anchors.len(), 2, "one anchor fewer");
    assert!(h.app.path_edit.selected.is_empty(), "and nothing left selected");
}

/// A path with nothing left in it would be a shape layer holding no shape.
#[test]
fn deleting_every_anchor_removes_the_layer() {
    let Some(mut h) = with_a_path() else { return };
    h.app.path_edit.selected = vec![(0, 0, 0), (0, 0, 1), (0, 0, 2)];
    h.press(cshop_ui::shortcuts::Chord::plain(egui::Key::Delete));
    h.settle(2);
    assert!(h.app.editable_path().is_none(), "the layer should be gone");
    assert!(shape_kinds(&h).is_empty());
}
