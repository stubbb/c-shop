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
