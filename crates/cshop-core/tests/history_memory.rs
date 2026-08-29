//! What the undo stack holds, and what it does when that is too much.
//!
//! A history bounded only by how many entries it has cannot be bounded at all:
//! an entry is a few bytes for a rename and hundreds of megabytes for a fill
//! across a large canvas. These check that it measures itself, that it drops
//! the oldest steps rather than growing without limit, and that undo still
//! restores exactly what was there.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::IRect;
use cshop_core::history::{History, ReplacePixels};
use cshop_core::layer::LayerId;
use cshop_core::pixels::PixelBuffer;

const SIDE: u32 = 256;

fn patterned(side: u32, salt: u8) -> PixelBuffer {
    let mut px = PixelBuffer::new(side, side);
    for y in 0..side as i32 {
        for x in 0..side as i32 {
            px.set(
                x,
                y,
                Rgba8::opaque(
                    (x as u8).wrapping_add(salt),
                    (y as u8).wrapping_mul(3),
                    ((x ^ y) as u8).wrapping_add(salt),
                ),
            );
        }
    }
    px
}

fn document() -> (Document, LayerId) {
    let doc = Document::new("t", SIDE, SIDE, Background::White);
    let id = doc.tree.iter_all()[0];
    (doc, id)
}

fn fill_step(id: LayerId, after: PixelBuffer) -> Box<ReplacePixels> {
    Box::new(ReplacePixels::new(id, IRect::from_size(SIDE, SIDE), after, "Edit"))
}

/// A region of one colour is most of what a large history weighs, and it is
/// exactly the case that need not be stored pixel by pixel.
#[test]
fn a_flat_edit_costs_the_history_nothing() {
    let (mut doc, id) = document();
    let mut history = History::new("New");
    history.apply(&mut doc, fill_step(id, PixelBuffer::filled(SIDE, SIDE, Rgba8::opaque(9, 9, 9))));
    assert_eq!(history.memory_bytes(), 0, "a flat fill over a flat document holds nothing");

    // Photographic content cannot be folded up, and is kept as it is.
    let (mut doc, id) = document();
    if let Some(px) = doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
        *px = patterned(SIDE, 0);
    }
    let mut history = History::new("New");
    history.apply(&mut doc, fill_step(id, PixelBuffer::filled(SIDE, SIDE, Rgba8::opaque(9, 9, 9))));
    let expected = SIDE as u64 * SIDE as u64 * 4;
    assert_eq!(
        history.memory_bytes(),
        expected,
        "the photograph underneath has to be kept; the flat fill over it does not"
    );
}

/// Whatever it does to save space, undo has to give back what was there.
#[test]
fn undo_restores_exactly_what_was_there() {
    for (label, original) in [
        ("flat", PixelBuffer::filled(SIDE, SIDE, Rgba8::opaque(30, 60, 90))),
        ("patterned", patterned(SIDE, 7)),
    ] {
        let (mut doc, id) = document();
        if let Some(px) = doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
            *px = original.clone();
        }
        let mut history = History::new("New");

        let edited = patterned(SIDE, 200);
        history.apply(&mut doc, fill_step(id, edited.clone()));
        let now = doc.tree.get(id).and_then(|l| l.pixels()).unwrap().clone();
        assert_eq!(now.pixels(), edited.pixels(), "{label}: the edit should have landed");

        history.undo(&mut doc).expect("should undo");
        let back = doc.tree.get(id).and_then(|l| l.pixels()).unwrap().clone();
        assert_eq!(back.pixels(), original.pixels(), "{label}: undo must restore the original");

        history.redo(&mut doc).expect("should redo");
        let again = doc.tree.get(id).and_then(|l| l.pixels()).unwrap().clone();
        assert_eq!(again.pixels(), edited.pixels(), "{label}: redo must put the edit back");
    }
}

/// The point of the budget: memory stops growing, and the user is told.
#[test]
fn the_oldest_steps_are_dropped_once_the_budget_is_reached() {
    let (mut doc, id) = document();
    if let Some(px) = doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
        *px = patterned(SIDE, 0);
    }
    // Room for about three of these.
    let one = SIDE as u64 * SIDE as u64 * 4;
    let mut history = History::new("New").with_budget(one * 3);

    for step in 1..=10u8 {
        history.apply(&mut doc, fill_step(id, patterned(SIDE, step.wrapping_mul(31))));
    }

    assert!(
        history.memory_bytes() <= one * 3,
        "held {} bytes against a budget of {}",
        history.memory_bytes(),
        one * 3
    );
    assert!(history.forgotten() > 0, "older steps should have been dropped");
    assert!(history.can_undo(), "what is left must still be undoable");

    // And the entries that remain still work.
    let before_undo = doc.tree.get(id).and_then(|l| l.pixels()).unwrap().clone();
    history.undo(&mut doc).expect("should undo");
    let after_undo = doc.tree.get(id).and_then(|l| l.pixels()).unwrap().clone();
    assert_ne!(before_undo.pixels(), after_undo.pixels(), "undo should have changed something");
}

/// One step is kept whatever it costs: an edit that cannot be undone at all is
/// worse than one that uses more memory than it was supposed to.
#[test]
fn a_single_entry_survives_a_budget_it_cannot_fit() {
    let (mut doc, id) = document();
    if let Some(px) = doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
        *px = patterned(SIDE, 0);
    }
    let mut history = History::new("New").with_budget(1);
    history.apply(&mut doc, fill_step(id, patterned(SIDE, 99)));
    assert!(history.can_undo(), "the only step must remain undoable");

    history.apply(&mut doc, fill_step(id, patterned(SIDE, 55)));
    assert!(history.can_undo(), "and so must the newest one");
    assert_eq!(history.cursor(), 1, "but only the newest is kept");
}
