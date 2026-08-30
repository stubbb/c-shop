//! The Lens Correction window, driven through the real interface.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_ui::commands::Action;
use cshop_ui::dialogs::Dialog;
use cshop_ui::input_harness::Harness;

fn open(h: &mut Harness, w: u32, hgt: u32) {
    h.app.open_document(Document::new("t", w, hgt, Background::Color(Rgba8::WHITE)));
    h.settle(2);
}

#[test]
fn the_window_previews_and_the_crop_shrinks_with_a_rotation() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h, 400, 300);
    h.app.push(Action::ShowLens);
    h.settle(2);

    let Dialog::Lens(d) = &mut h.app.dialog else { panic!("the window should be open") };
    assert_eq!(d.full, (400, 300));
    assert!(d.crop_estimate().is_none(), "nothing has moved, so nothing to crop");

    d.lens.rotation = 10.0;
    d.autocrop = true;
    h.settle(2);

    let Dialog::Lens(d) = &h.app.dialog else { panic!("still open") };
    let crop = d.crop_estimate().expect("a rotation should leave something to crop");
    assert!(
        crop.width() < 400 && crop.height() < 300,
        "the crop should be inside the frame, not {crop:?}"
    );
}

/// Applying runs on a worker thread, so the window stays live and can show
/// how far it has got. The test drives it the way the frame loop does.
#[test]
fn applying_reports_progress_and_lands_the_result() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    // Big enough that the pass is not over before the first frame after it
    // starts, which is what a progress bar is for.
    open(&mut h, 1600, 1200);
    h.app.push(Action::ShowLens);
    h.settle(2);

    let id = h.app.doc().unwrap().doc.active.unwrap();
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    let Dialog::Lens(d) = &mut h.app.dialog else { panic!("open") };
    d.lens.rotation = 15.0;
    d.autocrop = true;
    h.settle(1);

    h.app.push(Action::ApplyLens);
    h.settle(1);

    // Wait it out the way the frame loop would, polling each frame.
    for _ in 0..400 {
        if !matches!(h.app.dialog, Dialog::Lens(_)) {
            break;
        }
        h.settle(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        !matches!(h.app.dialog, Dialog::Lens(_)),
        "the window should close once the pass has landed"
    );

    let doc = &h.app.doc().unwrap().doc;
    assert!(
        doc.width < 1600 && doc.height < 1200,
        "autocrop should have taken the empty corners off: {}x{}",
        doc.width,
        doc.height
    );
    let after = doc.tree.get(id).unwrap().pixels().unwrap();
    assert_ne!(after, &before, "and the pixels should have been corrected");
}

/// One undo step for both halves, because putting the pixels back without the
/// canvas would leave the document a size that matches nothing.
#[test]
fn correcting_and_cropping_undo_together() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h, 300, 200);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    h.app.push(Action::ShowLens);
    h.settle(2);
    let Dialog::Lens(d) = &mut h.app.dialog else { panic!("open") };
    d.lens.rotation = 15.0;
    d.autocrop = true;
    h.settle(1);
    h.app.push(Action::ApplyLens);
    for _ in 0..400 {
        if !matches!(h.app.dialog, Dialog::Lens(_)) {
            break;
        }
        h.settle(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.app.doc().unwrap().doc.width < 300);

    h.app.push(Action::Undo);
    h.settle(2);
    let doc = &h.app.doc().unwrap().doc;
    assert_eq!((doc.width, doc.height), (300, 200), "one undo should put the canvas back");
    assert_eq!(
        doc.tree.get(id).unwrap().pixels().unwrap(),
        &before,
        "and the pixels with it"
    );
}

/// A layer with no pixels of its own has nothing to correct, and should be
/// told so rather than opening a window onto nothing.
#[test]
fn a_group_layer_is_refused() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h, 64, 64);
    h.app.push(Action::NewGroup);
    h.settle(2);
    h.app.push(Action::ShowLens);
    h.settle(1);
    assert!(
        !matches!(h.app.dialog, Dialog::Lens(_)),
        "there is nothing there to correct"
    );
}

/// The progress bar reads the counter the worker is raising, which is worth
/// checking without a race: the end-to-end test above cannot assert on a
/// number that may already have reached the end.
#[test]
fn progress_reports_how_far_the_pass_has_got() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h, 64, 64);
    h.app.push(Action::ShowLens);
    h.settle(2);
    let Dialog::Lens(d) = &mut h.app.dialog else { panic!("open") };

    assert!(!d.is_working());
    assert_eq!(d.progress(), 0.0);

    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    d.applying = Some((counter.clone(), 200));
    assert!(d.is_working());
    assert_eq!(d.progress(), 0.0);

    counter.store(50, std::sync::atomic::Ordering::Relaxed);
    assert!((d.progress() - 0.25).abs() < 1e-6);

    // A worker that overshoots its estimate must not push the bar past full.
    counter.store(400, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(d.progress(), 1.0);
}
