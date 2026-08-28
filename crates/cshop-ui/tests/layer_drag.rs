//! Dragging layers in the panel to reorder them.

use cshop_core::document::{Background, Document};
use cshop_ui::commands::Action;
use cshop_ui::input_harness::Harness;

/// A document with four named layers, bottom-first: Background, A, B, C.
fn stack() -> Option<Harness> {
    let mut h = Harness::new((1400, 820))?;
    h.app.open_document(Document::new("t", 64, 64, Background::White));
    h.settle(2);
    for name in ["A", "B", "C"] {
        h.app.dispatch(Action::NewLayer);
        let id = h.app.doc().unwrap().doc.active.unwrap();
        h.app.dispatch(Action::SetLayerProperty(
            id,
            cshop_core::history::LayerProperty::Name(name.into()),
        ));
    }
    h.settle(3);
    Some(h)
}

/// Layer names bottom-first, which is how the tree stores them.
fn order(h: &Harness) -> Vec<String> {
    let view = h.app.doc().unwrap();
    view.doc
        .tree
        .root()
        .iter()
        .map(|id| view.doc.tree.get(*id).unwrap().name.clone())
        .collect()
}

fn row(h: &Harness, name: &str) -> (f32, f32) {
    let view = h.app.doc().unwrap();
    let id = view
        .doc
        .tree
        .iter_all()
        .into_iter()
        .find(|id| view.doc.tree.get(*id).unwrap().name == name)
        .unwrap_or_else(|| panic!("no layer called {name}"));
    h.widget_center(cshop_ui::panels::layer_row_id(id))
        .unwrap_or_else(|| panic!("{name}'s row was not drawn"))
}

/// Row height, so a drag can aim at a half of a row rather than its middle.
const HALF: f32 = 13.0;

#[test]
fn the_stack_starts_in_the_order_it_was_built() {
    let Some(h) = stack() else { return };
    assert_eq!(order(&h), vec!["Background", "A", "B", "C"]);
}

/// The bug this replaces: only a drop on the *first* row ever registered,
/// because whichever row was drawn first cleared the drag state before the row
/// under the pointer had a chance to look at it.
#[test]
fn a_layer_can_be_dropped_on_a_row_other_than_the_first() {
    let Some(mut h) = stack() else { return };
    // Rows top-first are C, B, A, Background. Drag C down to just above
    // Background — the last gap, which the old code could never reach.
    let from = row(&h, "C");
    let to = row(&h, "Background");
    h.drag(from, (to.0, to.1 - HALF), 8);
    h.settle(2);
    assert_eq!(order(&h), vec!["Background", "C", "A", "B"]);
}

#[test]
fn dropping_on_the_upper_half_puts_the_layer_above_that_row() {
    let Some(mut h) = stack() else { return };
    let from = row(&h, "A");
    let to = row(&h, "C");
    h.drag(from, (to.0, to.1 - HALF), 8);
    h.settle(2);
    assert_eq!(order(&h), vec!["Background", "B", "C", "A"], "A should be at the top");
}

#[test]
fn dropping_on_the_lower_half_puts_the_layer_below_that_row() {
    let Some(mut h) = stack() else { return };
    let from = row(&h, "A");
    let to = row(&h, "C");
    h.drag(from, (to.0, to.1 + HALF), 8);
    h.settle(2);
    assert_eq!(order(&h), vec!["Background", "B", "A", "C"], "A should land under C");
}

#[test]
fn a_reorder_is_one_undo_step() {
    let Some(mut h) = stack() else { return };
    let before = order(&h);
    let from = row(&h, "A");
    let to = row(&h, "C");
    h.drag(from, (to.0, to.1 - HALF), 8);
    h.settle(2);
    assert_ne!(order(&h), before);
    assert_eq!(h.app.doc().unwrap().history.labels().last().map(String::as_str), Some("Reorder Layer"));

    h.app.dispatch(Action::Undo);
    assert_eq!(order(&h), before, "one undo should put the stack back");
}

/// The Background is pinned at the bottom. Dragging it, or dropping
/// something beneath it, must do nothing.
#[test]
fn the_background_stays_at_the_bottom() {
    let Some(mut h) = stack() else { return };
    let before = order(&h);

    // Try to drag the Background up to the top.
    let from = row(&h, "Background");
    let to = row(&h, "C");
    h.drag(from, (to.0, to.1 - HALF), 8);
    h.settle(2);
    assert_eq!(order(&h), before, "the Background should not be draggable");

    // And try to drop C underneath it.
    let from = row(&h, "C");
    let to = row(&h, "Background");
    h.drag(from, (to.0, to.1 + HALF), 8);
    h.settle(2);
    assert_eq!(order(&h), before, "nothing should pass below the Background");
}

#[test]
fn dropping_a_layer_back_where_it_started_changes_nothing() {
    let Some(mut h) = stack() else { return };
    let before = order(&h);
    let at = row(&h, "B");
    h.drag(at, (at.0, at.1 - HALF), 6);
    h.settle(2);
    assert_eq!(order(&h), before);
    assert!(
        !h.app.doc().unwrap().history.labels().iter().any(|l| l == "Reorder Layer"),
        "a drop that changes nothing should not be an undo step"
    );
}

/// A group can be dragged as a unit, and cannot be dropped inside itself.
#[test]
fn a_group_moves_with_its_children_and_cannot_swallow_itself() {
    let Some(mut h) = stack() else { return };
    h.app.dispatch(Action::NewGroup);
    let group = h.app.doc().unwrap().doc.active.unwrap();
    h.app.dispatch(Action::SetLayerProperty(
        group,
        cshop_core::history::LayerProperty::Name("G".into()),
    ));
    // Put A inside it.
    let view = h.app.doc().unwrap();
    let a = view
        .doc
        .tree
        .iter_all()
        .into_iter()
        .find(|id| view.doc.tree.get(*id).unwrap().name == "A")
        .unwrap();
    h.app.dispatch(Action::MoveLayer(
        a,
        cshop_core::tree::LayerPos { parent: Some(group), index: 0 },
    ));
    h.settle(3);
    assert_eq!(
        h.app.doc().unwrap().doc.tree.children(Some(group)).len(),
        1,
        "A should be inside the group"
    );

    // Dragging the group onto its own child must be refused.
    let from = row(&h, "G");
    let to = row(&h, "A");
    h.drag(from, (to.0, to.1 - HALF), 8);
    h.settle(2);
    assert_eq!(
        h.app.doc().unwrap().doc.tree.children(Some(group)).len(),
        1,
        "the group should not have been moved into itself"
    );
    assert!(
        h.app.doc().unwrap().doc.tree.position(group).is_some_and(|p| p.parent.is_none()),
        "and should still be at the root"
    );
}
