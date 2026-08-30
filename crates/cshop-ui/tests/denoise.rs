//! The Remove Noise window, driven through the real interface.
//!
//! These need the vision pack; without it they check that the window says so
//! rather than failing, and stop there.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_ui::commands::Action;
use cshop_ui::dialogs::Dialog;
use cshop_ui::input_harness::Harness;

/// A small field of noise, which is what the model is for.
fn noisy(w: u32, h: u32) -> Document {
    let mut doc = Document::new("t", w, h, Background::Color(Rgba8::opaque(128, 120, 110)));
    let id = doc.active.unwrap();
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    if let Some(px) = doc.tree.get_mut(id).and_then(|l| l.pixels_mut()) {
        for p in px.pixels_mut() {
            let mut next = || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                ((seed >> 33) % 81) as i32 - 40
            };
            let jitter = |v: u8, d: i32| (v as i32 + d).clamp(0, 255) as u8;
            *p = Rgba8::new(jitter(p.r, next()), jitter(p.g, next()), jitter(p.b, next()), 255);
        }
    }
    doc
}

fn wait_for_model(h: &mut Harness) {
    for _ in 0..3000 {
        let done = matches!(&h.app.dialog, Dialog::Denoise(d) if !d.is_working());
        if done {
            return;
        }
        h.settle(1);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn the_window_says_when_the_pack_is_missing() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(Document::new("t", 64, 64, Background::White));
    h.settle(2);
    h.app.push(Action::ShowDenoise);
    h.settle(1);
    let Dialog::Denoise(d) = &h.app.dialog else { panic!("the window should be open") };
    if cshop_ui::vision::is_available() {
        assert!(!d.unavailable);
    } else {
        assert!(d.unavailable);
        assert!(d.status.contains("setup.sh") || d.status.contains("not installed"));
    }
}

/// With nothing selected the whole layer is the region; with a selection, only
/// that. Which one it is decides whether this takes seconds or minutes, so the
/// window has to get it right before anything is run.
#[test]
fn a_selection_narrows_what_will_be_cleaned() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(noisy(200, 160));
    h.settle(2);

    h.app.push(Action::ShowDenoise);
    h.settle(1);
    let Dialog::Denoise(d) = &h.app.dialog else { panic!("open") };
    assert_eq!((d.region.width(), d.region.height()), (200, 160));
    h.app.push(Action::DenoiseCancel);
    h.settle(1);

    let rect = cshop_core::selection::Selection::from_rect(
        200,
        160,
        cshop_core::selection::Rectf { x0: 40.0, y0: 30.0, x1: 120.0, y1: 94.0 },
        false,
    );
    h.app.push(Action::SetSelection(Box::new(rect), "Test"));
    h.settle(1);
    h.app.push(Action::ShowDenoise);
    h.settle(1);
    let Dialog::Denoise(d) = &h.app.dialog else { panic!("open") };
    assert_eq!((d.region.width(), d.region.height()), (80, 64));
    assert_eq!((d.region.x0, d.region.y0), (40, 30));
}

/// The model runs once and the strength is free afterwards, which is the whole
/// shape of the window.
#[test]
fn the_model_runs_once_and_strength_moves_afterwards() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(noisy(128, 128));
    h.settle(2);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    h.app.push(Action::ShowDenoise);
    h.settle(1);
    h.app.push(Action::RunDenoise);
    h.settle(1);
    wait_for_model(&mut h);

    let Dialog::Denoise(d) = &h.app.dialog else { panic!("the window should still be open") };
    assert!(d.cleaned.is_some(), "the model should have answered: {}", d.status);
    assert!(d.showing, "and its answer should be on the canvas");

    // On the canvas now, before anything has been committed.
    let shown = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();
    assert_ne!(shown, before, "the preview should be visible");
    assert!(
        !h.app.doc().unwrap().history.can_undo(),
        "and should not have made a history entry yet"
    );

    // Half strength is a different picture again, without running the model.
    if let Dialog::Denoise(d) = &mut h.app.dialog {
        d.strength = 0.5;
    }
    h.app.push(Action::DenoiseRestrength);
    h.settle(1);
    let half = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();
    assert_ne!(half, shown, "strength should change what is shown");
    assert_ne!(half, before);

    h.app.push(Action::DenoiseKeep);
    h.settle(2);
    assert!(!matches!(h.app.dialog, Dialog::Denoise(_)), "Keep closes the window");

    // One entry, and undoing it gives the original back exactly.
    h.app.push(Action::Undo);
    h.settle(2);
    assert_eq!(
        h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap(),
        &before,
        "one undo should put the noise back"
    );
}

/// Cancelling after a run has to leave the layer as it was found, since the
/// preview was pasted straight in.
#[test]
fn cancelling_puts_the_original_back() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(noisy(128, 128));
    h.settle(2);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    h.app.push(Action::ShowDenoise);
    h.settle(1);
    h.app.push(Action::RunDenoise);
    h.settle(1);
    wait_for_model(&mut h);
    h.app.push(Action::DenoiseCancel);
    h.settle(2);

    assert!(!matches!(h.app.dialog, Dialog::Denoise(_)));
    assert_eq!(
        h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap(),
        &before,
        "cancel should leave no trace"
    );
}
