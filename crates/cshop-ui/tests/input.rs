//! Input routing, driven through the real interface.
//!
//! These click where a user would and check what happened, so a widget that
//! covers another — the class of bug that left the title bar's own menus dead
//! — fails here rather than in someone's hands.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_ui::commands::WindowCommand;
use cshop_ui::input_harness::Harness;
use cshop_ui::tools::Tool;

const SIZE: (u32, u32) = (1400, 820);

/// A harness with one open document, settled for a few frames.
fn ready(background: Background) -> Option<Harness> {
    let mut h = Harness::new(SIZE)?;
    h.app.open_document(Document::new("t", 200, 200, background));
    h.settle(3);
    Some(h)
}

#[test]
fn the_title_bar_menus_are_clickable() {
    let Some(mut h) = ready(Background::White) else { return };

    // "File" sits just right of the logo.
    h.click((48.0, 14.0));
    let open = h
        .ctx
        .layer_id_at(egui::pos2(70.0, 45.0))
        .is_some_and(|id| id.order != egui::Order::Background);
    assert!(open, "clicking File should open its menu");

    // And its first entry, New…, should do something.
    h.click((70.0, 45.0));
    assert!(h.app.dialog.is_open(), "choosing New should open its dialog");
}

#[test]
fn the_window_buttons_are_clickable() {
    let Some(mut h) = ready(Background::White) else { return };

    // Close is the rightmost button in the bar.
    h.click((SIZE.0 as f32 - 15.0, 14.0));
    assert!(h.app.quit, "the close button should ask the app to quit");

    let Some(mut h) = ready(Background::White) else { return };
    // Maximise sits immediately to its left.
    h.click((SIZE.0 as f32 - 45.0, 14.0));
    assert!(
        h.window_commands.contains(&WindowCommand::ToggleMaximize),
        "the maximise button should toggle the window"
    );

    let Some(mut h) = ready(Background::White) else { return };
    h.click((SIZE.0 as f32 - 75.0, 14.0));
    assert!(
        h.window_commands.contains(&WindowCommand::Minimize),
        "the minimise button should minimise the window"
    );
}

#[test]
fn dragging_empty_title_bar_moves_the_window() {
    let Some(mut h) = ready(Background::White) else { return };
    // A stretch of bar with no menu and no button on it.
    h.drag((600.0, 14.0), (700.0, 14.0), 4);
    assert!(
        h.window_commands.contains(&WindowCommand::StartDrag),
        "dragging the bar should start a window move"
    );
}

#[test]
fn dragging_over_a_menu_does_not_move_the_window() {
    let Some(mut h) = ready(Background::White) else { return };
    // Starting on "File" belongs to the menu, not the drag surface.
    h.drag((48.0, 14.0), (150.0, 14.0), 4);
    assert!(
        !h.window_commands.contains(&WindowCommand::StartDrag),
        "a drag beginning on a menu should not move the window"
    );
}

#[test]
fn the_brush_paints_on_the_canvas() {
    let Some(mut h) = ready(Background::Transparent) else { return };
    h.app.tool = Tool::Brush;
    h.app.foreground = Rgba8::BLACK;
    h.app.brush.size = 30.0;
    h.app.brush.hardness = 1.0;

    let from = h.doc_to_screen(60.0, 60.0).expect("a document is open");
    let to = h.doc_to_screen(140.0, 140.0).expect("a document is open");
    h.drag(from, to, 8);

    assert!(
        h.active_pixel(100, 100).is_some_and(|c| c.a > 0),
        "the stroke should have landed"
    );
}

#[test]
fn the_brush_paints_on_a_layer_added_from_the_panel() {
    // The path a user takes: click New Layer in the Layers panel, then draw.
    let Some(mut h) = ready(Background::White) else { return };

    let before = h.app.doc().map(|d| d.doc.tree.len()).unwrap_or(0);
    h.click(h.new_layer_button());
    let after = h.app.doc().map(|d| d.doc.tree.len()).unwrap_or(0);
    assert_eq!(after, before + 1, "the New Layer button should add a layer");

    h.app.tool = Tool::Brush;
    h.app.foreground = Rgba8::opaque(255, 0, 0);
    h.app.brush.size = 30.0;
    h.app.brush.hardness = 1.0;

    let from = h.doc_to_screen(60.0, 100.0).expect("open");
    let to = h.doc_to_screen(140.0, 100.0).expect("open");
    h.drag(from, to, 8);

    let px = h.active_pixel(100, 100);
    assert!(
        px.is_some_and(|c| c.a > 0 && c.r > 200),
        "the new layer should have taken the stroke, got {px:?}"
    );
}

#[test]
fn a_right_click_on_the_canvas_does_not_paint() {
    // Adding the context menu made this a real risk.
    let Some(mut h) = ready(Background::Transparent) else { return };
    h.app.tool = Tool::Brush;
    h.app.foreground = Rgba8::BLACK;
    h.app.brush.size = 40.0;

    let at = h.doc_to_screen(100.0, 100.0).expect("open");
    h.secondary_click(at);

    assert!(
        h.active_pixel(100, 100).is_none_or(|c| c.a == 0),
        "a right-click must not lay down paint"
    );
}

#[test]
fn the_resize_border_does_not_swallow_the_canvas() {
    // The border strips live in a foreground layer over everything.
    let Some(mut h) = ready(Background::Transparent) else { return };
    h.app.tool = Tool::Brush;
    h.app.foreground = Rgba8::BLACK;
    h.app.brush.size = 30.0;

    let from = h.doc_to_screen(80.0, 80.0).expect("open");
    let to = h.doc_to_screen(120.0, 120.0).expect("open");
    h.drag(from, to, 6);

    assert!(h.active_pixel(100, 100).is_some_and(|c| c.a > 0));
    assert!(
        !h.window_commands.iter().any(|c| matches!(c, WindowCommand::StartResize(_))),
        "a drag in the middle of the canvas is not a resize"
    );
}

#[test]
fn right_clicking_the_toolbox_swatches_opens_the_picker() {
    use cshop_ui::dialogs::{Dialog, PickerTarget};

    let Some(mut h) = ready(Background::White) else { return };
    let fg = h.widget_center(cshop_ui::chrome::foreground_swatch_id()).expect("fg swatch drawn");
    let bg = h.widget_center(cshop_ui::chrome::background_swatch_id()).expect("bg swatch drawn");

    h.secondary_click(fg);
    match &h.app.dialog {
        Dialog::ColorPicker(d) => assert_eq!(d.target, PickerTarget::Foreground),
        _ => panic!("right-clicking the foreground swatch opened no colour picker"),
    }

    let Some(mut h) = ready(Background::White) else { return };
    h.secondary_click(bg);
    match &h.app.dialog {
        Dialog::ColorPicker(d) => assert_eq!(d.target, PickerTarget::Background),
        _ => panic!("right-clicking the background swatch opened no colour picker"),
    }
}

#[test]
fn the_foreground_swatch_wins_the_overlap_with_the_background_one() {
    use cshop_ui::dialogs::{Dialog, PickerTarget};

    // The two squares overlap by design. The foreground is drawn on top, so
    // the corner they share must belong to it — the same last-registered-wins
    // rule that once left the title bar's own menus dead.
    let Some(mut h) = ready(Background::White) else { return };
    let fg = h.widget_center(cshop_ui::chrome::foreground_swatch_id()).expect("fg swatch drawn");
    // Down-right of centre, inside the overlap with the background swatch.
    h.secondary_click((fg.0 + 6.0, fg.1 + 6.0));
    match &h.app.dialog {
        Dialog::ColorPicker(d) => assert_eq!(d.target, PickerTarget::Foreground),
        _ => panic!("the overlap should belong to the foreground, but opened no picker"),
    }
}

#[test]
fn clicking_the_background_swatch_still_swaps() {
    let Some(mut h) = ready(Background::White) else { return };
    let before = (h.app.foreground, h.app.background);
    let bg = h.widget_center(cshop_ui::chrome::background_swatch_id()).expect("bg swatch drawn");
    // Below the foreground square, where only the background is exposed.
    h.click((bg.0 + 6.0, bg.1 + 6.0));
    assert_eq!(
        (h.app.foreground, h.app.background),
        (before.1, before.0),
        "a plain click on the background swatch should swap the two"
    );
}
