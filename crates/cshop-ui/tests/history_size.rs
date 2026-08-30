//! Undoing something that changed the size of the picture.
//!
//! The document going back is only half of it. What is on screen is drawn from
//! a texture sized to the document, and if that texture is not resized too the
//! old picture stays around the restored one — which looks like the before and
//! after superimposed, because that is what it is.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::resample::Resampling;
use cshop_ui::commands::Action;
use cshop_ui::input_harness::Harness;

fn open(h: &mut Harness) {
    h.app.open_document(Document::new("t", 200, 150, Background::Color(Rgba8::opaque(200, 90, 60))));
    h.settle(3);
}

/// Both halves have to come back: the document *and* the thing it is drawn
/// into.
#[test]
fn undoing_a_resize_puts_the_canvas_back_too() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);
    assert_eq!(h.app.doc().unwrap().composite_size(), (200, 150));

    h.app.push(Action::ResizeImage { width: 400, height: 300, filter: Resampling::Bilinear });
    h.settle(3);
    assert_eq!(h.app.doc().unwrap().composite_size(), (400, 300));

    h.app.push(Action::Undo);
    h.settle(3);
    let view = h.app.doc().unwrap();
    assert_eq!((view.doc.width, view.doc.height), (200, 150));
    assert_eq!(
        view.composite_size(),
        (200, 150),
        "the target is still the size it was, so the old picture is still on screen around the new one"
    );

    h.app.push(Action::Redo);
    h.settle(3);
    let view = h.app.doc().unwrap();
    assert_eq!((view.doc.width, view.doc.height), (400, 300));
    assert_eq!(view.composite_size(), (400, 300), "and redo has to follow as well");
}

/// A crop shrinks the canvas, which is the same problem from the other side.
#[test]
fn undoing_a_canvas_change_puts_the_canvas_back_too() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);
    h.app.push(Action::ResizeCanvas {
        width: 90,
        height: 70,
        anchor: cshop_ui::commands::Anchor::Center,
    });
    h.settle(3);
    assert_eq!(h.app.doc().unwrap().composite_size(), (90, 70));

    h.app.push(Action::Undo);
    h.settle(3);
    assert_eq!(h.app.doc().unwrap().composite_size(), (200, 150));
}

/// And jumping about in the History panel, which can cross several of them at
/// once.
#[test]
fn jumping_through_the_history_keeps_the_canvas_in_step() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);
    h.app.push(Action::ResizeImage { width: 300, height: 225, filter: Resampling::Bilinear });
    h.settle(2);
    h.app.push(Action::ResizeImage { width: 500, height: 375, filter: Resampling::Bilinear });
    h.settle(2);
    assert_eq!(h.app.doc().unwrap().composite_size(), (500, 375));

    // All the way back to where it started, in one move.
    h.app.push(Action::HistoryJump(0));
    h.settle(3);
    let view = h.app.doc().unwrap();
    assert_eq!((view.doc.width, view.doc.height), (200, 150));
    assert_eq!(view.composite_size(), (200, 150));
}
