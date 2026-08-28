//! Keyboard shortcuts, pressed for real.
//!
//! The table in `shortcuts.rs` proves that every named chord is dispatched;
//! these press the keys through the interface and check what actually happened
//! to the document — the half a table cannot tell you.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_ui::input_harness::Harness;
use cshop_ui::shortcuts::keys as k;
use cshop_ui::tools::Tool;

fn ready() -> Option<Harness> {
    let mut h = Harness::new((1400, 820))?;
    h.app.open_document(Document::new("t", 64, 64, Background::White));
    h.settle(3);
    Some(h)
}

#[test]
fn merge_down_and_quit_are_bound_not_just_advertised() {
    // Both were printed in a menu with nothing listening for them.
    let Some(mut h) = ready() else { return };
    h.app.dispatch(cshop_ui::commands::Action::NewLayer);
    h.settle(2);
    let before = h.app.doc().map(|v| v.doc.tree.len()).unwrap_or(0);
    h.press(k::MERGE_DOWN);
    let after = h.app.doc().map(|v| v.doc.tree.len()).unwrap_or(0);
    assert_eq!(after, before - 1, "Ctrl+E should merge the layer down");

    let Some(mut h) = ready() else { return };
    h.press(k::QUIT);
    assert!(h.app.quit, "Ctrl+Q should quit");
}

#[test]
fn the_backspace_family_fills_the_conventional_way() {
    let Some(mut h) = ready() else { return };
    h.app.foreground = Rgba8::new(255, 0, 0, 255);
    h.app.background = Rgba8::new(0, 0, 255, 255);

    // Alt+Backspace: foreground.
    h.press(k::FILL_FOREGROUND);
    assert_eq!(h.active_pixel(10, 10), Some(Rgba8::new(255, 0, 0, 255)));

    // Ctrl+Backspace: background. This one was not bound at all.
    h.press(k::FILL_BACKGROUND);
    assert_eq!(
        h.active_pixel(10, 10),
        Some(Rgba8::new(0, 0, 255, 255)),
        "Ctrl+Backspace should fill with the background colour"
    );

    // Shift+Backspace opens the Fill dialog rather than clearing the layer.
    let Some(mut h) = ready() else { return };
    h.press(k::FILL);
    assert!(h.app.dialog.is_open(), "Shift+Backspace should open the Fill dialog");
}

/// Ctrl+Shift+Backspace used to fill with the *foreground* and ignore
/// transparency, which is two mistakes: it should fill with the background
/// and keeps the layer's existing alpha.
#[test]
fn shift_backspace_variants_preserve_transparency() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(cshop_ui::commands::Action::NewLayer);
    h.settle(2);
    h.app.foreground = Rgba8::new(255, 0, 0, 255);
    h.app.background = Rgba8::new(0, 0, 255, 255);

    // The new layer is empty, so a transparency-preserving fill changes nothing.
    h.press(k::FILL_BACKGROUND_LOCKED);
    assert_eq!(
        h.active_pixel(10, 10),
        Some(Rgba8::new(0, 0, 0, 0)),
        "a locked fill must not paint where the layer is transparent"
    );

    // Whereas the plain one does.
    h.press(k::FILL_BACKGROUND);
    assert_eq!(h.active_pixel(10, 10), Some(Rgba8::new(0, 0, 255, 255)));
}

#[test]
fn the_adjustment_chords_open_their_dialogs() {
    for (chord, want) in [
        (k::LEVELS, "Levels"),
        (k::CURVES, "Curves"),
        (k::HUE_SATURATION, "Hue/Saturation"),
        (k::COLOR_BALANCE, "Color Balance"),
    ] {
        let Some(mut h) = ready() else { return };
        h.press(chord);
        match &h.app.dialog {
            cshop_ui::dialogs::Dialog::Adjustment(d) => assert_eq!(d.title(), want),
            _ => panic!("{} did not open the {want} dialog", chord.label()),
        }
    }
}

/// Ctrl+I inverts and Ctrl+Alt+I resizes. Without exact modifier matching the
/// second would fire the first on its way past.
#[test]
fn ctrl_alt_i_resizes_without_also_inverting() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(cshop_ui::commands::Action::fill_foreground(false));
    h.app.foreground = Rgba8::new(255, 255, 255, 255);
    h.app.dispatch(cshop_ui::commands::Action::fill_foreground(false));
    h.settle(2);

    h.press(k::IMAGE_SIZE);
    assert!(h.app.dialog.is_open(), "Ctrl+Alt+I should open Image Size");
    assert_eq!(
        h.active_pixel(10, 10),
        Some(Rgba8::new(255, 255, 255, 255)),
        "Ctrl+Alt+I must not also run Invert"
    );
}

/// Ctrl+D deselects and Ctrl+Shift+D reselects. If the modifier match were
/// loose, the second would deselect on its way past and then restore what it
/// had just thrown away.
#[test]
fn adding_shift_picks_the_other_command_not_both() {
    let Some(mut h) = ready() else { return };
    h.press(k::SELECT_ALL);
    assert!(h.app.doc().is_some_and(|v| v.doc.selection.is_some()), "Ctrl+A should select all");

    h.press(k::DESELECT);
    assert!(h.app.doc().is_some_and(|v| v.doc.selection.is_none()), "Ctrl+D should deselect");

    h.press(k::RESELECT);
    assert!(
        h.app.doc().is_some_and(|v| v.doc.selection.is_some()),
        "Ctrl+Shift+D should reselect, not deselect again"
    );

    // And Save As must not also save: it opens its dialog.
    let Some(mut h) = ready() else { return };
    h.press(k::SAVE_AS);
    assert!(h.app.dialog.is_open(), "Ctrl+Shift+S should open Save As");
}

#[test]
fn the_brackets_size_the_brush_and_step_back_exactly() {
    let Some(mut h) = ready() else { return };
    h.app.tool = Tool::Brush;
    let start = h.app.brush.size;
    h.press(cshop_ui::shortcuts::Chord::plain(egui::Key::CloseBracket));
    let bigger = h.app.brush.size;
    assert!(bigger > start, "] should enlarge the brush");
    h.press(cshop_ui::shortcuts::Chord::plain(egui::Key::OpenBracket));
    assert_eq!(h.app.brush.size, start, "[ should undo exactly what ] did");

    // Shift makes them hardness instead.
    h.app.brush.hardness = 0.5;
    h.press(cshop_ui::shortcuts::Chord::shift(egui::Key::CloseBracket));
    assert!(h.app.brush.hardness > 0.5, "Shift+] should harden the brush");
}

#[test]
fn a_digit_sets_the_painting_opacity() {
    let Some(mut h) = ready() else { return };
    h.app.tool = Tool::Brush;
    h.press(cshop_ui::shortcuts::Chord::plain(egui::Key::Num5));
    assert!((h.app.brush.opacity - 0.5).abs() < 1e-4, "5 should set 50% opacity");
    h.press(cshop_ui::shortcuts::Chord::plain(egui::Key::Num0));
    assert!((h.app.brush.opacity - 1.0).abs() < 1e-4, "0 should set 100% opacity");

    // With a non-painting tool the digits are left alone.
    h.app.tool = Tool::Move;
    h.press(cshop_ui::shortcuts::Chord::plain(egui::Key::Num3));
    assert!((h.app.brush.opacity - 1.0).abs() < 1e-4, "digits should not affect the Move tool");
}

#[test]
fn ctrl_bracket_restacks_the_active_layer() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(cshop_ui::commands::Action::NewLayer);
    h.settle(2);
    let id = h.app.doc().and_then(|v| v.doc.active).expect("a layer is active");
    let top = h.app.doc().map(|v| v.doc.tree.position(id).unwrap().index).unwrap();
    assert_eq!(top, 1, "the new layer starts on top");

    h.press(k::LAYER_BACKWARD);
    let now = h.app.doc().map(|v| v.doc.tree.position(id).unwrap().index).unwrap();
    assert_eq!(now, 0, "Ctrl+[ should send the layer down the stack");

    h.press(k::LAYER_TO_FRONT);
    let now = h.app.doc().map(|v| v.doc.tree.position(id).unwrap().index).unwrap();
    assert_eq!(now, 1, "Ctrl+Shift+] should bring it back to the top");
}

#[test]
fn ctrl_j_duplicates_and_ctrl_alt_g_clips() {
    let Some(mut h) = ready() else { return };
    let before = h.app.doc().map(|v| v.doc.tree.len()).unwrap_or(0);
    h.press(k::LAYER_VIA_COPY);
    assert_eq!(
        h.app.doc().map(|v| v.doc.tree.len()).unwrap_or(0),
        before + 1,
        "Ctrl+J with no selection should copy the whole layer"
    );

    h.press(k::CLIPPING_MASK);
    let clipping = h
        .app
        .doc()
        .and_then(|v| v.doc.active.and_then(|id| v.doc.tree.get(id)).map(|l| l.clipping));
    assert_eq!(clipping, Some(true), "Ctrl+Alt+G should turn on the clipping mask");
}

/// Ctrl+J is Layer via Copy, not Duplicate Layer: with a selection it lifts
/// only the selected pixels, onto a layer cropped to them.
#[test]
fn ctrl_j_copies_only_the_selection() {
    use cshop_core::selection::Selection;
    let Some(mut h) = ready() else { return };

    // Paint the whole 64x64 layer red, then select a 20x20 corner of it.
    h.app.foreground = Rgba8::new(255, 0, 0, 255);
    h.app.dispatch(cshop_ui::commands::Action::fill_foreground(false));
    let sel = Selection::from_rect(64, 64, cshop_core::selection::Rectf::from_points(cshop_core::geom::Vec2::new(10.0, 12.0), cshop_core::geom::Vec2::new(30.0, 32.0)), false);
    h.app
        .dispatch(cshop_ui::commands::Action::SetSelection(Box::new(sel), "Rectangular Marquee"));
    h.settle(2);

    h.press(k::LAYER_VIA_COPY);
    h.settle(2);

    let view = h.app.doc().expect("a document");
    let id = view.doc.active.expect("the new layer is active");
    let layer = view.doc.tree.get(id).expect("the new layer");
    assert_eq!(layer.offset, (10, 12), "the copy sits where the selection was");
    let px = layer.pixels().expect("a raster layer");
    assert_eq!((px.width(), px.height()), (20, 20), "and is cropped to the selection");
    assert_eq!(px.get(5, 5), Rgba8::new(255, 0, 0, 255), "with the selected pixels in it");

    assert_eq!(view.history.labels().last().map(String::as_str), Some("Layer via Copy"));
    assert!(view.doc.selection.is_none(), "the selection is dropped afterwards");
}

/// A feathered selection should produce a feathered layer, not a hard cut-out.
#[test]
fn a_feathered_selection_copies_a_soft_edge() {
    use cshop_core::selection::{Rectf, Selection};
    let Some(mut h) = ready() else { return };
    h.app.foreground = Rgba8::new(0, 0, 0, 255);
    h.app.dispatch(cshop_ui::commands::Action::fill_foreground(false));

    let mut sel = Selection::from_rect(64, 64, Rectf::from_points(cshop_core::geom::Vec2::new(16.0, 16.0), cshop_core::geom::Vec2::new(48.0, 48.0)), false);
    sel.feather(6.0);
    h.app.dispatch(cshop_ui::commands::Action::SetSelection(Box::new(sel), "Marquee"));
    h.settle(2);

    h.press(k::LAYER_VIA_COPY);
    h.settle(2);

    let view = h.app.doc().expect("a document");
    let id = view.doc.active.expect("a layer");
    let px = view.doc.tree.get(id).and_then(|l| l.pixels()).expect("pixels");
    let offset = view.doc.tree.get(id).unwrap().offset;
    let centre = px.get(32 - offset.0, 32 - offset.1);
    let edge = px.get(16 - offset.0, 32 - offset.1);
    assert_eq!(centre.a, 255, "the middle should be fully opaque");
    assert!(
        edge.a > 0 && edge.a < 255,
        "the feathered edge should be partly transparent, got alpha {}",
        edge.a
    );
}

/// One gesture, one undo: Ctrl+J then Ctrl+Z must take the layer away *and*
/// give the selection back.
#[test]
fn undoing_layer_via_copy_restores_the_selection_too() {
    use cshop_core::selection::{Rectf, Selection};
    let Some(mut h) = ready() else { return };
    h.app.foreground = Rgba8::new(255, 0, 0, 255);
    h.app.dispatch(cshop_ui::commands::Action::fill_foreground(false));
    let sel = Selection::from_rect(
        64,
        64,
        Rectf::from_points(cshop_core::geom::Vec2::new(8.0, 8.0), cshop_core::geom::Vec2::new(40.0, 40.0)),
        false,
    );
    h.app.dispatch(cshop_ui::commands::Action::SetSelection(Box::new(sel), "Marquee"));
    h.settle(2);
    let layers = h.app.doc().unwrap().doc.tree.len();

    h.press(k::LAYER_VIA_COPY);
    h.settle(2);
    assert_eq!(h.app.doc().unwrap().doc.tree.len(), layers + 1);
    assert!(h.app.doc().unwrap().doc.selection.is_none());

    h.press(k::UNDO);
    h.settle(2);
    assert_eq!(
        h.app.doc().unwrap().doc.tree.len(),
        layers,
        "one undo should remove the copied layer"
    );
    assert!(
        h.app.doc().unwrap().doc.selection.is_some(),
        "and the same undo should bring the selection back"
    );
}
