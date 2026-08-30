//! The Segment Object window, driven through the real interface.
//!
//! These need the vision pack; without it they check that the window says so
//! rather than failing, and stop there.

use cshop_core::document::{Background, Document};
use cshop_core::geom::Vec2;
use cshop_ui::commands::Action;
use cshop_ui::dialogs::Dialog;
use cshop_ui::input_harness::Harness;

fn open_sample(h: &mut Harness, name: &str) -> bool {
    let path = std::path::PathBuf::from(std::env::var("HOME").unwrap())
        .join("assets/samples")
        .join(name);
    let Ok(doc) = cshop_io::load_document(&path) else { return false };
    h.app.open_document(doc);
    h.settle(2);
    true
}

#[test]
fn the_window_says_when_the_pack_is_missing() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(Document::new("t", 64, 64, Background::White));
    h.settle(2);
    h.app.push(Action::ShowSegment);
    h.settle(1);
    let Dialog::Segment(d) = &h.app.dialog else { panic!("the window should be open") };
    if cshop_ui::vision::is_available() {
        assert!(!d.unavailable);
        assert!(d.status.contains("Click"), "it should say what to do: {}", d.status);
    } else {
        assert!(d.unavailable);
        assert!(d.status.contains("setup.sh") || d.status.contains("not installed"));
    }
}

#[test]
fn clicking_the_canvas_segments_what_was_clicked() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    if !open_sample(&mut h, "dog.jpg") {
        return;
    }
    // Smaller, so the models are quick and the test is not a benchmark.
    h.app.push(Action::ResizeImage {
        width: 600,
        height: 900,
        filter: cshop_core::resample::Resampling::Bilinear,
    });
    h.settle(2);

    h.app.push(Action::ShowSegment);
    h.settle(1);
    // Where the dog is in that crop.
    if let Dialog::Segment(d) = &mut h.app.dialog {
        d.add_hint(Vec2::new(270.0, 500.0), true);
    }
    h.app.push(Action::SegmentPreview);
    h.settle(2);

    let Dialog::Segment(d) = &h.app.dialog else { panic!("the window should still be open") };
    assert!(d.applied, "a click should have produced a selection: {}", d.status);
    let coverage = d.coverage.expect("it should report what it covered");
    assert!(
        (0.01..0.9).contains(&coverage),
        "a click on the dog should select part of the picture, not none or all of it: {coverage}"
    );

    let view = h.app.doc().expect("a document");
    let selection = view.doc.selection.as_ref().expect("the selection should be set");
    assert!(!selection.is_empty());
    assert!(selection.bounds().width() > 20, "and be a real region");
}

/// Cancelling has to leave the document as it was found.
#[test]
fn cancelling_restores_the_previous_selection() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    if !open_sample(&mut h, "dog.jpg") {
        return;
    }
    h.app.push(Action::ResizeImage {
        width: 600,
        height: 900,
        filter: cshop_core::resample::Resampling::Bilinear,
    });
    h.settle(2);
    h.app.push(Action::SelectAll);
    h.settle(1);
    let before = h.app.doc().unwrap().doc.selection.as_ref().map(|s| s.bounds());

    h.app.push(Action::ShowSegment);
    h.settle(1);
    if let Dialog::Segment(d) = &mut h.app.dialog {
        d.add_hint(Vec2::new(270.0, 500.0), true);
    }
    h.app.push(Action::SegmentPreview);
    h.settle(2);
    h.app.push(Action::SegmentCancel);
    h.settle(1);

    let after = h.app.doc().unwrap().doc.selection.as_ref().map(|s| s.bounds());
    assert_eq!(before, after, "cancel should put the selection back");
}
