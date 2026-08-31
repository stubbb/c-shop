//! Windows that can be pushed aside.
//!
//! Anything whose answer shows up on the canvas has to be movable, because a
//! window sitting over the middle of the picture is no use for judging what it
//! just did — which is the whole reason these have previews.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_ui::commands::Action;
use cshop_ui::input_harness::Harness;

fn open(h: &mut Harness) {
    h.app.open_document(Document::new("t", 200, 150, Background::Color(Rgba8::opaque(90, 120, 60))));
    h.settle(2);
}

fn window_rect(h: &Harness, key: &str) -> Option<egui::Rect> {
    h.ctx.memory(|m| m.area_rect(egui::Id::new(key)))
}

/// It is a window with a title bar, and dragging that bar moves it.
#[test]
fn a_preview_window_can_be_dragged_out_of_the_way() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);
    h.app.push(Action::ShowLens);
    h.settle(3);

    let before = window_rect(&h, "window-lens").expect("the lens window is a window");
    // The title bar is the strip along the top of it.
    let grab = (before.center().x, before.top() + 8.0);
    h.drag(grab, (grab.0 + 260.0, grab.1 + 120.0), 8);
    h.settle(2);

    let after = window_rect(&h, "window-lens").expect("still there");
    assert!(
        (after.left() - before.left()).abs() > 100.0,
        "dragging the title bar should move it: {:?} then {:?}",
        before.left(),
        after.left()
    );
    assert!((after.top() - before.top()).abs() > 50.0);
}

/// Each window remembers its own place. They used to share one id, so moving
/// any of them moved where all the others would next appear.
#[test]
fn each_window_keeps_its_own_place() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);

    h.app.push(Action::ShowLens);
    h.settle(3);
    let lens_start = window_rect(&h, "window-lens").expect("lens window").left();
    let grab = {
        let r = window_rect(&h, "window-lens").unwrap();
        (r.center().x, r.top() + 8.0)
    };
    h.drag(grab, (grab.0 + 300.0, grab.1 + 60.0), 8);
    h.settle(2);
    let lens_moved = window_rect(&h, "window-lens").unwrap().left();
    assert!(lens_moved > lens_start + 100.0, "the lens window moved");
    h.app.push(Action::CloseDialog);
    h.settle(2);

    // A different window opens where it always did, not where that one went.
    h.app.push(Action::ShowUpscale);
    h.settle(3);
    let upscale = window_rect(&h, "window-upscale").expect("upscale window").left();
    assert!(
        (upscale - lens_moved).abs() > 100.0,
        "moving one window should not decide where another opens: {upscale} against {lens_moved}"
    );
}

/// The questions that have to be answered first stay modal, and so have no
/// window of their own to move.
#[test]
fn the_questions_that_must_be_answered_stay_modal() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    h.app.push(Action::RenameLayer(id));
    h.settle(3);
    assert!(h.app.dialog.is_open(), "the rename dialog should be up");
    for key in ["window-rename", "window-layer-style", "window-lens"] {
        assert!(window_rect(&h, key).is_none(), "{key} should not be a movable window");
    }
}
