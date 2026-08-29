//! Copy, cut and paste.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::IRect;
use cshop_core::selection::{Rectf, Selection};
use cshop_ui::commands::Action;
use cshop_ui::input_harness::Harness;

/// A document whose active layer is a red square on transparent, so what was
/// copied is recognisable wherever it lands.
fn ready() -> Option<(Harness, cshop_core::layer::LayerId)> {
    let mut h = Harness::new((1400, 820))?;
    h.app.open_document(Document::new("t", 100, 100, Background::Transparent));
    h.settle(2);
    // Detached by default: these tests run as threads of one process and
    // would otherwise all share the single X selection, overwriting each
    // other's copies. One test below covers the system path on purpose.
    h.app.clipboard = cshop_ui::clipboard::Clipboard::detached();
    let id = h.app.doc().unwrap().doc.active.unwrap();
    if let Some(view) = h.app.doc_mut() {
        if let Some(px) = view.doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
            px.fill_rect(IRect::at(20, 20, 40, 40), Rgba8::opaque(220, 30, 30));
        }
        // Editing pixels behind the commands' back means saying so, or the
        // layer is never re-uploaded and the composite stays empty.
        let bounds = view.doc.bounds();
        view.mark_dirty(cshop_core::document::Dirty::pixels(id, bounds));
        view.invalidate();
    }
    h.settle(2);
    Some((h, id))
}

fn select(h: &mut Harness, x0: f32, y0: f32, x1: f32, y1: f32) {
    let s = Selection::from_rect(
        100,
        100,
        Rectf::from_points(
            cshop_core::geom::Vec2::new(x0, y0),
            cshop_core::geom::Vec2::new(x1, y1),
        ),
        false,
    );
    h.app.dispatch(Action::SetSelection(Box::new(s), "Marquee"));
}

fn layer_count(h: &Harness) -> usize {
    h.app.doc().unwrap().doc.tree.len()
}

#[test]
fn copy_and_paste_makes_a_new_layer_holding_what_was_copied() {
    let Some((mut h, _)) = ready() else { return };
    select(&mut h, 20.0, 20.0, 60.0, 60.0);
    h.app.dispatch(Action::Copy);
    assert!(h.app.clipboard.has_content(), "the copy should have landed somewhere");

    let before = layer_count(&h);
    h.app.dispatch(Action::Paste);
    assert_eq!(layer_count(&h), before + 1, "paste should add a layer");
    assert_eq!(
        h.app.doc().unwrap().history.labels().last().map(String::as_str),
        Some("Paste")
    );

    let view = h.app.doc().unwrap();
    let layer = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
    let px = layer.pixels().unwrap();
    assert_eq!((px.width(), px.height()), (40, 40), "the copied region's size");
    assert_eq!(px.get(20, 20), Rgba8::opaque(220, 30, 30), "and its pixels");
}

#[test]
fn paste_centres_and_paste_in_place_does_not() {
    let Some((mut h, _)) = ready() else { return };
    select(&mut h, 20.0, 20.0, 60.0, 60.0);
    h.app.dispatch(Action::Copy);

    h.app.dispatch(Action::Paste);
    let centred = {
        let v = h.app.doc().unwrap();
        v.doc.tree.get(v.doc.active.unwrap()).unwrap().offset
    };
    // A 40x40 clipping on a 100x100 canvas lands at (30, 30).
    assert_eq!(centred, (30, 30), "a plain paste is centred on the canvas");

    h.app.dispatch(Action::PasteInPlace);
    let in_place = {
        let v = h.app.doc().unwrap();
        v.doc.tree.get(v.doc.active.unwrap()).unwrap().offset
    };
    assert_eq!(in_place, (20, 20), "paste in place goes back where it came from");
}

#[test]
fn cut_copies_and_then_clears() {
    let Some((mut h, id)) = ready() else { return };
    select(&mut h, 20.0, 20.0, 40.0, 40.0);
    h.app.dispatch(Action::Cut);

    let view = h.app.doc().unwrap();
    let px = view.doc.tree.get(id).unwrap().pixels().unwrap();
    // Clearing takes the alpha away; the colour under it is left alone, which
    // is what every other clear here does.
    assert_eq!(px.get(25, 25).a, 0, "the cut region should be gone");
    assert_eq!(
        px.get(50, 50),
        Rgba8::opaque(220, 30, 30),
        "and the rest of the layer left alone"
    );

    // What was cut is still on the clipboard.
    h.app.dispatch(Action::Paste);
    let v = h.app.doc().unwrap();
    let pasted = v.doc.tree.get(v.doc.active.unwrap()).unwrap().pixels().unwrap();
    assert_eq!(pasted.get(5, 5), Rgba8::opaque(220, 30, 30));
}

/// With no selection, copying takes the whole layer rather than nothing.
#[test]
fn copying_without_a_selection_takes_the_whole_layer() {
    let Some((mut h, _)) = ready() else { return };
    h.app.dispatch(Action::Copy);
    h.app.dispatch(Action::Paste);
    let v = h.app.doc().unwrap();
    let px = v.doc.tree.get(v.doc.active.unwrap()).unwrap().pixels().unwrap();
    assert_eq!((px.width(), px.height()), (100, 100));
}

/// A feathered selection should copy a feathered edge, not a square one.
#[test]
fn a_feathered_selection_copies_a_soft_edge() {
    let Some((mut h, _)) = ready() else { return };
    let mut s = Selection::from_rect(
        100,
        100,
        Rectf::from_points(
            cshop_core::geom::Vec2::new(20.0, 20.0),
            cshop_core::geom::Vec2::new(60.0, 60.0),
        ),
        false,
    );
    s.feather(5.0);
    h.app.dispatch(Action::SetSelection(Box::new(s), "Marquee"));
    h.app.dispatch(Action::Copy);
    h.app.dispatch(Action::Paste);

    let v = h.app.doc().unwrap();
    let px = v.doc.tree.get(v.doc.active.unwrap()).unwrap().pixels().unwrap();
    let partial = px.pixels().iter().filter(|p| p.a > 0 && p.a < 255).count();
    assert!(partial > 30, "the feathered edge should carry partial alpha, got {partial}");
}

#[test]
fn copy_merged_takes_every_visible_layer() {
    let Some((mut h, _)) = ready() else { return };
    // A second layer, offset from the first, so a merged copy differs from
    // copying either one alone.
    h.app.dispatch(Action::NewLayer);
    let top = h.app.doc().unwrap().doc.active.unwrap();
    if let Some(view) = h.app.doc_mut() {
        if let Some(px) = view.doc.tree.get_mut(top).and_then(|l| l.pixels_mut()) {
            px.fill_rect(IRect::at(60, 20, 20, 20), Rgba8::opaque(30, 30, 220));
        }
        let bounds = view.doc.bounds();
        view.mark_dirty(cshop_core::document::Dirty::pixels(top, bounds));
        view.invalidate();
    }
    h.settle(3);

    select(&mut h, 0.0, 0.0, 100.0, 100.0);
    h.app.dispatch(Action::CopyMerged);
    h.app.dispatch(Action::Paste);

    let v = h.app.doc().unwrap();
    let px = v.doc.tree.get(v.doc.active.unwrap()).unwrap().pixels().unwrap();
    assert_eq!(px.get(30, 30), Rgba8::opaque(220, 30, 30), "the lower layer is in it");
    assert_eq!(px.get(65, 25), Rgba8::opaque(30, 30, 220), "and so is the upper one");
}

#[test]
fn pasting_with_nothing_copied_says_so_and_adds_no_layer() {
    let Some((mut h, _)) = ready() else { return };
    let before = layer_count(&h);
    h.app.dispatch(Action::Paste);
    assert_eq!(layer_count(&h), before, "nothing should have been added");
    assert!(
        h.app.toast.as_ref().is_some_and(|(m, _)| m.contains("nothing to paste")),
        "got {:?}",
        h.app.toast
    );
}

/// A selection that misses the layer entirely should say so rather than
/// copying an empty rectangle.
#[test]
fn copying_outside_the_layer_is_refused() {
    let Some((mut h, id)) = ready() else { return };
    if let Some(view) = h.app.doc_mut() {
        if let Some(l) = view.doc.tree.get_mut(id) {
            l.offset = (0, 0);
        }
    }
    select(&mut h, 0.0, 0.0, 1.0, 1.0);
    // Shift the layer out from under the selection.
    if let Some(view) = h.app.doc_mut() {
        if let Some(l) = view.doc.tree.get_mut(id) {
            l.offset = (500, 500);
        }
    }
    h.app.dispatch(Action::Copy);
    assert!(
        h.app.toast.as_ref().is_some_and(|(m, _)| m.contains("does not overlap")),
        "got {:?}",
        h.app.toast
    );
}

/// The chords the rest of the world uses, pressed for real.
#[test]
fn the_clipboard_chords_are_bound() {
    use cshop_ui::shortcuts::keys as k;
    let Some((mut h, _)) = ready() else { return };
    select(&mut h, 20.0, 20.0, 60.0, 60.0);

    h.press(k::COPY);
    assert!(h.app.clipboard.has_content(), "Ctrl+C should copy");

    let before = layer_count(&h);
    h.press(k::PASTE);
    h.settle(2);
    assert_eq!(layer_count(&h), before + 1, "Ctrl+V should paste");
}

/// The other half of the feature: what is copied here should be readable by
/// everything else on the desktop, and what they copy readable here.
///
/// The only test that touches the real clipboard, so it cannot race the
/// others — and it is skipped where there is no desktop to talk to.
#[test]
fn images_travel_through_the_system_clipboard() {
    let Some((mut h, _)) = ready() else { return };
    h.app.clipboard = Default::default();
    select(&mut h, 20.0, 20.0, 60.0, 60.0);
    h.app.dispatch(Action::Copy);

    // Read it back the way another application would.
    let Ok(mut outside) = arboard::Clipboard::new() else { return };
    let Ok(image) = outside.get_image() else { return };
    assert_eq!((image.width, image.height), (40, 40), "the size another app sees");
    assert_eq!(&image.bytes[..4], &[220, 30, 30, 255], "and the pixels");

    // And something copied elsewhere pastes in here.
    let bytes: Vec<u8> = (0..8 * 8).flat_map(|_| [9u8, 200, 40, 255]).collect();
    if outside
        .set_image(arboard::ImageData { width: 8, height: 8, bytes: bytes.into() })
        .is_err()
    {
        return;
    }
    let before = layer_count(&h);
    h.app.dispatch(Action::Paste);
    assert_eq!(layer_count(&h), before + 1);
    let v = h.app.doc().unwrap();
    let px = v.doc.tree.get(v.doc.active.unwrap()).unwrap().pixels().unwrap();
    assert_eq!((px.width(), px.height()), (8, 8), "the foreign image pastes at its own size");
    assert_eq!(px.get(0, 0), Rgba8::opaque(9, 200, 40));
}
