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
    // The model runs on another thread now, so the window says it is busy
    // first and the answer arrives over the next few frames.
    h.settle(1);
    if let Dialog::Segment(d) = &h.app.dialog {
        assert!(d.busy, "it should say it is working: {}", d.status);
    }
    for _ in 0..200 {
        h.settle(1);
        let Dialog::Segment(d) = &h.app.dialog else { break };
        if !d.busy {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let Dialog::Segment(d) = &h.app.dialog else { panic!("the window should still be open") };
    assert!(!d.busy, "it should have finished: {}", d.status);
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
    for _ in 0..200 {
        h.settle(1);
        let Dialog::Segment(d) = &h.app.dialog else { break };
        if !d.busy {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    h.app.push(Action::SegmentCancel);
    h.settle(1);

    let after = h.app.doc().unwrap().doc.selection.as_ref().map(|s| s.bounds());
    assert_eq!(before, after, "cancel should put the selection back");
}

/// Expand has to make the selection bigger, and by about what it says.
#[test]
fn expand_grows_the_selection_it_was_given() {
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
    h.app.push(Action::ShowSegment);
    h.settle(1);
    if let Dialog::Segment(d) = &mut h.app.dialog {
        d.add_hint(Vec2::new(270.0, 500.0), true);
    }

    let run = |h: &mut Harness| {
        h.app.push(Action::SegmentPreview);
        for _ in 0..200 {
            h.settle(1);
            let Dialog::Segment(d) = &h.app.dialog else { break };
            if !d.busy {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        h.app.doc().unwrap().doc.selection.as_ref().map(|s| s.bounds()).expect("a selection")
    };

    let tight = run(&mut h);
    if let Dialog::Segment(d) = &mut h.app.dialog {
        d.expand = 8;
    }
    let grown = run(&mut h);

    // The second pass reuses the cached embedding, so what is being measured
    // here is the growing, not the model — it was given the same click twice.
    //
    // Every side that has room moves out by the radius asked for. A side
    // already against the picture's edge has nowhere to go, and on this
    // photograph the dog runs off the left, so three sides move and one holds.
    assert_eq!(grown.y0, tight.y0 - 8, "the top should move by the radius: {tight:?} {grown:?}");
    assert_eq!(grown.y1, tight.y1 + 8, "and the bottom: {tight:?} {grown:?}");
    assert_eq!(grown.x1, tight.x1 + 8, "and the right: {tight:?} {grown:?}");
    assert!(
        (tight.x0 - 8..=tight.x0).contains(&grown.x0),
        "and the left by no more, however much room it had: {tight:?} {grown:?}"
    );
}
