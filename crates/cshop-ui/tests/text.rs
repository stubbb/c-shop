//! The Type tool, driven through the real interface.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_ui::commands::Action;
use cshop_ui::input_harness::Harness;
use cshop_ui::text_tool::TextInput as T;
use cshop_ui::tools::Tool;

fn ready() -> Option<Harness> {
    // Skip everywhere without fonts rather than failing; the tool genuinely
    // cannot work there.
    if cshop_core::font::FontDb::global().families().is_empty() {
        return None;
    }
    let mut h = Harness::new((1400, 820))?;
    h.app.open_document(Document::new("t", 300, 200, Background::White));
    h.settle(3);
    h.app.tool = Tool::Text;
    Some(h)
}

fn type_text(h: &mut Harness, s: &str) {
    for c in s.chars() {
        h.app.dispatch(Action::TextInput(T::Insert(c.to_string())));
    }
}

fn active_text(h: &Harness) -> Option<String> {
    let view = h.app.doc()?;
    let id = view.doc.active?;
    Some(view.doc.tree.get(id)?.text()?.content().text.clone())
}

#[test]
fn clicking_starts_a_type_layer_and_typing_fills_it() {
    let Some(mut h) = ready() else { return };
    let before = h.app.doc().unwrap().doc.tree.len();

    h.app.dispatch(Action::BeginText { at: Vec2::new(40.0, 80.0), wrap: None });
    assert!(h.app.text_edit.is_some(), "clicking should start editing");
    assert_eq!(h.app.doc().unwrap().doc.tree.len(), before + 1);

    type_text(&mut h, "Hello");
    assert_eq!(active_text(&h).as_deref(), Some("Hello"));

    // An empty layer is not an undo step; the committed one is.
    assert!(
        h.app.doc().unwrap().history.labels().is_empty(),
        "typing should not fill the history"
    );
    h.app.dispatch(Action::CommitText);
    assert!(h.app.text_edit.is_none());
    assert_eq!(h.app.doc().unwrap().history.labels(), vec!["Type Layer"]);
}

#[test]
fn a_type_layer_actually_paints_pixels() {
    let Some(mut h) = ready() else { return };
    h.app.foreground = Rgba8::new(0, 0, 0, 255);
    h.app.dispatch(Action::BeginText { at: Vec2::new(20.0, 100.0), wrap: None });
    h.app.text_style.size = 64.0;
    h.app.refresh_text_style();
    type_text(&mut h, "IIIII");
    h.app.dispatch(Action::CommitText);
    h.settle(2);

    let view = h.app.doc().unwrap();
    let id = view.doc.active.unwrap();
    let px = view.doc.tree.get(id).unwrap().pixels().expect("type has a raster");
    let ink = px.pixels().iter().filter(|p| p.a > 128).count();
    assert!(ink > 200, "the glyphs should have covered some pixels, got {ink}");
}

#[test]
fn committing_empty_type_leaves_nothing_behind() {
    let Some(mut h) = ready() else { return };
    let before = h.app.doc().unwrap().doc.tree.len();
    h.app.dispatch(Action::BeginText { at: Vec2::new(40.0, 80.0), wrap: None });
    h.app.dispatch(Action::CommitText);
    assert_eq!(
        h.app.doc().unwrap().doc.tree.len(),
        before,
        "clicking and clicking away should not leave an empty layer"
    );
    assert!(h.app.doc().unwrap().history.labels().is_empty());
}

#[test]
fn escape_abandons_a_new_layer_and_restores_an_edited_one() {
    let Some(mut h) = ready() else { return };
    let before = h.app.doc().unwrap().doc.tree.len();
    h.app.dispatch(Action::BeginText { at: Vec2::new(40.0, 80.0), wrap: None });
    type_text(&mut h, "throwaway");
    h.app.dispatch(Action::TextInput(T::Cancel));
    assert_eq!(h.app.doc().unwrap().doc.tree.len(), before, "cancelling should remove the layer");

    // Now an existing layer, edited then cancelled.
    h.app.dispatch(Action::BeginText { at: Vec2::new(40.0, 80.0), wrap: None });
    type_text(&mut h, "keep");
    h.app.dispatch(Action::CommitText);
    let id = h.app.doc().unwrap().doc.active.unwrap();

    h.app.dispatch(Action::EditTextLayer(id));
    type_text(&mut h, " and more");
    assert_eq!(active_text(&h).as_deref(), Some("keep and more"));
    h.app.dispatch(Action::TextInput(T::Cancel));
    assert_eq!(active_text(&h).as_deref(), Some("keep"), "cancelling should put the text back");
}

#[test]
fn a_whole_editing_session_is_one_undo_step() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::BeginText { at: Vec2::new(40.0, 80.0), wrap: None });
    type_text(&mut h, "first");
    h.app.dispatch(Action::CommitText);
    let id = h.app.doc().unwrap().doc.active.unwrap();

    h.app.dispatch(Action::EditTextLayer(id));
    type_text(&mut h, " second");
    h.app.dispatch(Action::CommitText);
    assert_eq!(h.app.doc().unwrap().history.labels(), vec!["Type Layer", "Edit Type"]);

    h.app.dispatch(Action::Undo);
    assert_eq!(
        active_text(&h).as_deref(),
        Some("first"),
        "one undo should take back the whole session, not one keystroke"
    );
    h.app.dispatch(Action::Redo);
    assert_eq!(active_text(&h).as_deref(), Some("first second"));
}

#[test]
fn the_caret_moves_and_edits_where_it_is() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::BeginText { at: Vec2::new(40.0, 80.0), wrap: None });
    type_text(&mut h, "abcd");
    assert_eq!(h.app.text_edit.as_ref().unwrap().caret, 4);

    h.app.dispatch(Action::TextInput(T::Left));
    h.app.dispatch(Action::TextInput(T::Left));
    assert_eq!(h.app.text_edit.as_ref().unwrap().caret, 2);
    type_text(&mut h, "X");
    assert_eq!(active_text(&h).as_deref(), Some("abXcd"));

    h.app.dispatch(Action::TextInput(T::Backspace));
    assert_eq!(active_text(&h).as_deref(), Some("abcd"));
    h.app.dispatch(Action::TextInput(T::Delete));
    assert_eq!(active_text(&h).as_deref(), Some("abd"));

    h.app.dispatch(Action::TextInput(T::Home));
    assert_eq!(h.app.text_edit.as_ref().unwrap().caret, 0);
    h.app.dispatch(Action::TextInput(T::End));
    assert_eq!(h.app.text_edit.as_ref().unwrap().caret, 3);
}

#[test]
fn enter_breaks_the_line_and_up_down_cross_it() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::BeginText { at: Vec2::new(40.0, 60.0), wrap: None });
    type_text(&mut h, "one");
    h.app.dispatch(Action::TextInput(T::Newline));
    type_text(&mut h, "two");
    assert_eq!(active_text(&h).as_deref(), Some("one\ntwo"));

    h.app.dispatch(Action::TextInput(T::Up));
    let caret = h.app.text_edit.as_ref().unwrap().caret;
    assert!(caret <= 3, "Up should land on the first line, got byte {caret}");
    h.app.dispatch(Action::TextInput(T::Down));
    assert!(h.app.text_edit.as_ref().unwrap().caret >= 4, "Down should return to the second");
}

#[test]
fn the_caret_has_a_position_to_draw() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::BeginText { at: Vec2::new(40.0, 90.0), wrap: None });
    h.app.text_style.size = 40.0;
    h.app.refresh_text_style();
    let (top, bottom) = h.app.text_caret_rect().expect("a caret while editing");
    assert!(bottom.y > top.y, "the caret should have height");
    // At the very start it sits on the anchor the click set.
    assert!((top.x - 40.0).abs() < 2.0, "caret x was {}", top.x);

    type_text(&mut h, "wide text");
    let (moved, _) = h.app.text_caret_rect().unwrap();
    assert!(moved.x > top.x, "the caret should advance as text is typed");
}

#[test]
fn dragging_makes_a_paragraph_box_that_wraps() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::BeginText { at: Vec2::new(10.0, 20.0), wrap: Some(120.0) });
    h.app.text_style.size = 16.0;
    h.app.refresh_text_style();
    type_text(&mut h, "wrapping happens inside a paragraph box of a fixed width");
    h.app.dispatch(Action::CommitText);

    let view = h.app.doc().unwrap();
    let layer = view.doc.tree.get(view.doc.active.unwrap()).unwrap();
    let content = layer.text().unwrap().content();
    assert_eq!(content.wrap_width, Some(120.0));
    // Wrapped text is taller than one line and no wider than its box.
    let b = layer.bounds();
    assert!(b.height() > 40, "wrapped text should be several lines tall, got {}", b.height());
    assert!(b.width() < 160, "and should not exceed its box, got {}", b.width());
}

#[test]
fn type_cannot_be_painted_on_until_it_is_rasterised() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::BeginText { at: Vec2::new(30.0, 90.0), wrap: None });
    type_text(&mut h, "Type");
    h.app.dispatch(Action::CommitText);
    let id = h.app.doc().unwrap().doc.active.unwrap();

    // Live type cannot be painted on until it is rasterised.
    h.app.tool = Tool::Brush;
    h.app.begin_stroke_with(Vec2::new(40.0, 90.0), cshop_core::paint::PaintMode::Paint, false);
    h.app.end_stroke();
    assert!(
        h.app.doc().unwrap().doc.tree.get(id).unwrap().text().is_some(),
        "painting must not have altered the type layer"
    );

    h.app.dispatch(Action::RasterizeLayer);
    let view = h.app.doc().unwrap();
    let layer = view.doc.tree.get(id).unwrap();
    assert!(layer.text().is_none(), "rasterising should leave a plain raster layer");
    assert!(layer.pixels().is_some());
    assert_eq!(view.history.labels().last().map(String::as_str), Some("Rasterize Type"));

    // And undo puts the type back, still editable.
    h.app.dispatch(Action::Undo);
    assert!(h.app.doc().unwrap().doc.tree.get(id).unwrap().text().is_some());
}

#[test]
fn clicking_existing_type_reopens_it() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::BeginText { at: Vec2::new(30.0, 90.0), wrap: None });
    h.app.text_style.size = 48.0;
    h.app.refresh_text_style();
    type_text(&mut h, "Reopen me");
    h.app.dispatch(Action::CommitText);
    let id = h.app.doc().unwrap().doc.active.unwrap();

    let hit = h.app.text_layer_at(Vec2::new(50.0, 80.0));
    assert_eq!(hit, Some(id), "the type should be found under a point inside it");
    assert_eq!(h.app.text_layer_at(Vec2::new(290.0, 195.0)), None, "and not far away from it");

    h.app.dispatch(Action::EditTextLayer(id));
    assert!(h.app.text_edit.is_some());
    assert_eq!(h.app.text_edit.as_ref().unwrap().caret, "Reopen me".len());
}

/// Type carries its own raster, so everything that works on a raster layer
/// should work on it untouched. If any of these needed a special case, the
/// integration would be wrong.
#[test]
fn type_behaves_like_any_other_layer() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::BeginText { at: Vec2::new(30.0, 90.0), wrap: None });
    h.app.text_style.size = 48.0;
    h.app.refresh_text_style();
    type_text(&mut h, "Layered");
    h.app.dispatch(Action::CommitText);
    let id = h.app.doc().unwrap().doc.active.unwrap();

    // Opacity and blend mode.
    h.app.dispatch(Action::SetLayerProperty(
        id,
        cshop_core::history::LayerProperty::Opacity(0.5),
    ));
    h.app.dispatch(Action::SetLayerProperty(
        id,
        cshop_core::history::LayerProperty::Blend(cshop_core::blend::BlendMode::Multiply),
    ));
    let layer = h.app.doc().unwrap().doc.tree.get(id).unwrap().clone();
    assert_eq!(layer.opacity, 0.5);
    assert_eq!(layer.blend_mode, cshop_core::blend::BlendMode::Multiply);

    // A mask.
    h.app.dispatch(Action::AddLayerMask { hide_all: false });
    assert!(
        h.app.doc().unwrap().doc.tree.get(id).unwrap().mask.is_some(),
        "type should take a layer mask"
    );

    // And it still knows it is type.
    assert!(h.app.doc().unwrap().doc.tree.get(id).unwrap().text().is_some());
}

/// Moving type and then editing it must not teleport the text back: the anchor
/// has to follow the layer.
#[test]
fn moving_type_then_editing_it_keeps_it_where_it_was_put() {
    let Some(mut h) = ready() else { return };
    h.app.dispatch(Action::BeginText { at: Vec2::new(40.0, 90.0), wrap: None });
    h.app.text_style.size = 32.0;
    h.app.refresh_text_style();
    type_text(&mut h, "moved");
    h.app.dispatch(Action::CommitText);
    let id = h.app.doc().unwrap().doc.active.unwrap();

    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().offset;
    h.app.dispatch(Action::NudgeLayer(25, -12));
    let moved = h.app.doc().unwrap().doc.tree.get(id).unwrap().offset;
    assert_eq!(moved, (before.0 + 25, before.1 - 12));

    // Re-open and type: the layer must stay where it was nudged to.
    h.app.dispatch(Action::EditTextLayer(id));
    type_text(&mut h, "!");
    let after = h.app.doc().unwrap().doc.tree.get(id).unwrap().offset;
    assert_eq!(
        after, moved,
        "editing after a move should not drag the text back to where it started"
    );
}

/// Right-aligned point text grows leftwards, so its raster's corner moves as
/// it is typed while the anchor must not.
#[test]
fn right_aligned_type_grows_away_from_its_anchor() {
    let Some(mut h) = ready() else { return };
    h.app.text_style.align = cshop_core::text::TextAlign::Right;
    h.app.text_style.size = 28.0;
    h.app.dispatch(Action::BeginText { at: Vec2::new(220.0, 100.0), wrap: None });
    type_text(&mut h, "a");
    let narrow = h.app.doc().unwrap().doc.tree.get(h.app.doc().unwrap().doc.active.unwrap()).unwrap().bounds();
    type_text(&mut h, "aaaaaaaaaaaa");
    let wide = h.app.doc().unwrap().doc.tree.get(h.app.doc().unwrap().doc.active.unwrap()).unwrap().bounds();

    assert!(wide.x0 < narrow.x0, "right-aligned text should extend to the left as it grows");
    assert!(
        (wide.x1 - narrow.x1).abs() <= 2,
        "while its right edge stays put: {} vs {}",
        narrow.x1,
        wide.x1
    );
}

#[test]
fn clicking_the_canvas_with_the_type_tool_starts_type() {
    let Some(mut h) = ready() else { return };
    let before = h.app.doc().unwrap().doc.tree.len();
    let at = h.doc_to_screen(60.0, 100.0).expect("a visible canvas");
    h.click(at);
    h.settle(2);
    assert!(h.app.text_edit.is_some(), "a click on the canvas should start editing");
    assert_eq!(h.app.doc().unwrap().doc.tree.len(), before + 1);
}
