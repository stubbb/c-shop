//! Filling a hole in with what was probably behind it.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::selection::{Rectf, Selection};
use cshop_ui::commands::Action;
use cshop_ui::input_harness::Harness;

/// A picture with structure in it, so that a fill has something to continue.
fn striped(w: u32, h: u32) -> Document {
    let mut doc = Document::new("t", w, h, Background::Color(Rgba8::opaque(40, 60, 90)));
    let id = doc.active.unwrap();
    if let Some(px) = doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                if (y / 8) % 2 == 0 {
                    px.set(x, y, Rgba8::opaque(200, 190, 170));
                }
            }
        }
    }
    doc
}

/// Wait for the layer to actually change.
///
/// Not for the history to gain an entry: setting the selection is itself an
/// entry, so that test passes before the model has even started.
fn wait_for_change(h: &mut Harness, id: cshop_core::layer::LayerId, before: &cshop_core::pixels::PixelBuffer) {
    for _ in 0..1500 {
        h.settle(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let changed = h
            .app
            .doc()
            .and_then(|v| v.doc.tree.get(id)?.pixels().map(|p| p != before))
            .unwrap_or(false);
        if changed {
            return;
        }
    }
}

#[test]
fn filling_in_needs_a_selection() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(striped(128, 128));
    h.settle(2);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();
    h.app.push(Action::FillInSelection);
    h.settle(4);
    assert_eq!(
        h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap(),
        &before,
        "with nothing selected there is no hole, so nothing should happen"
    );
}

/// The hole is filled, everything outside it is untouched to the bit, and one
/// undo puts it back.
#[test]
fn the_hole_is_filled_and_nothing_else_moves() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(striped(200, 200));
    h.settle(2);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    let sel = Selection::from_rect(200, 200, Rectf { x0: 70.0, y0: 70.0, x1: 130.0, y1: 130.0 }, false);
    h.app.push(Action::SetSelection(Box::new(sel), "Test"));
    h.settle(1);
    h.app.push(Action::FillInSelection);
    wait_for_change(&mut h, id, &before);

    let after = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();
    assert_ne!(after, before, "the hole should have been filled");

    // Outside the selection, not one pixel may differ: the model hands the
    // rest back untouched and this must not resample it on the way through.
    let mut changed_outside = 0;
    for y in 0..200i32 {
        for x in 0..200i32 {
            let inside = (70..130).contains(&x) && (70..130).contains(&y);
            if !inside && after.get(x, y) != before.get(x, y) {
                changed_outside += 1;
            }
        }
    }
    assert_eq!(changed_outside, 0, "{changed_outside} pixels outside the hole moved");

    h.app.push(Action::Undo);
    h.settle(2);
    assert_eq!(
        h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap(),
        &before,
        "one undo should put the hole back"
    );
}
