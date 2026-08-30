//! Separating a picture into layers by what things are.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
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
    h.app.open_document(Document::new("t", 64, 64, Background::Color(Rgba8::WHITE)));
    h.settle(2);
    h.app.push(Action::ShowSeparate);
    h.settle(1);
    let Dialog::Separate(d) = &h.app.dialog else { panic!("the window should be open") };
    if !cshop_ui::vision::is_available() {
        assert!(d.unavailable);
        assert!(d.status.contains("setup.sh") || d.status.contains("not installed"));
    }
}

/// The labeller runs on a thread, so the window is live while it looks — and
/// what it finds ends up as layers named after it.
#[test]
fn a_photograph_separates_into_layers_named_for_what_they_are() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    if !open_sample(&mut h, "dog.jpg") {
        return;
    }
    h.app.push(Action::ResizeImage {
        width: 400,
        height: 600,
        filter: cshop_core::resample::Resampling::Bilinear,
    });
    h.settle(2);
    let before = h.app.doc().unwrap().doc.tree.len();

    h.app.push(Action::ShowSeparate);
    h.settle(1);
    if let Dialog::Separate(d) = &h.app.dialog {
        assert!(d.busy, "it should say it is looking");
    }
    for _ in 0..600 {
        let done = matches!(&h.app.dialog, Dialog::Separate(d) if !d.busy);
        if done {
            break;
        }
        h.settle(1);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let Dialog::Separate(d) = &h.app.dialog else { panic!("still open") };
    assert!(!d.busy, "it should have finished: {}", d.status);
    assert!(!d.found.is_empty(), "a photograph has something in it: {}", d.status);
    assert_eq!(d.found.len(), d.chosen.len());
    // Sorted with most of the picture first, which is what makes the list
    // readable without anyone having to sort it by eye.
    for pair in d.found.windows(2) {
        assert!(pair[0].coverage >= pair[1].coverage, "the list should be ordered");
    }
    let names: Vec<String> = d.found.iter().map(|r| r.class.clone()).collect();
    let ticked = d.picked().len();
    assert!(ticked > 0, "something should be worth a layer: {names:?}");

    h.app.push(Action::RunSeparate);
    h.settle(2);
    let doc = &h.app.doc().unwrap().doc;
    assert_eq!(doc.tree.len(), before + ticked, "one layer for each thing ticked");
    for name in doc.tree.iter_all().into_iter().filter_map(|id| {
        doc.tree.get(id).map(|l| l.name.clone())
    }) {
        assert!(!name.is_empty());
    }
}

/// A separated layer holds its own share of the picture and nothing else.
#[test]
fn each_layer_holds_only_its_own_pixels() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    if !open_sample(&mut h, "dog.jpg") {
        return;
    }
    h.app.push(Action::ResizeImage {
        width: 300,
        height: 450,
        filter: cshop_core::resample::Resampling::Bilinear,
    });
    h.settle(2);
    h.app.push(Action::ShowSeparate);
    for _ in 0..600 {
        let done = matches!(&h.app.dialog, Dialog::Separate(d) if !d.busy);
        if done {
            break;
        }
        h.settle(1);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let expected: Vec<(String, f32)> = match &h.app.dialog {
        Dialog::Separate(d) => d.picked().iter().map(|r| (r.class.clone(), r.coverage)).collect(),
        _ => return,
    };
    if expected.is_empty() {
        return;
    }
    h.app.push(Action::RunSeparate);
    h.settle(2);

    let doc = &h.app.doc().unwrap().doc;
    for (class, coverage) in expected {
        let layer = doc
            .tree
            .iter_all()
            .into_iter()
            .filter_map(|id| doc.tree.get(id))
            .find(|l| l.name == class)
            .unwrap_or_else(|| panic!("there should be a layer called {class}"));
        let pixels = layer.pixels().expect("with pixels in it");
        let opaque = pixels.pixels().iter().filter(|p| p.a > 128).count() as f32
            / (pixels.width() * pixels.height()) as f32;
        // Feathering spreads the edge a little, so this is about the share of
        // the picture rather than an exact count.
        assert!(
            (opaque - coverage).abs() < 0.08,
            "{class} covers {:.1}% of the picture but its layer is {:.1}% opaque",
            coverage * 100.0,
            opaque * 100.0
        );
    }
}

/// A window whose button both queues an action and closes the window.
///
/// That is the ordinary shape of these windows, and it used to be broken: the
/// window was dropped as the frame ended and the action ran afterwards, so
/// every handler that read the window back for what to do found nothing and
/// returned in silence. The canvas is unchanged by a successful separate — the
/// new layers reconstruct the picture exactly — so silence looked like success.
///
/// This goes through [`CShopApp::finish_dialog_frame`], which is what the
/// frame does with a window that has asked to close. Pushing the action by
/// hand instead leaves the window open and passes either way, which is exactly
/// how the bug got in.
#[test]
fn a_button_that_closes_the_window_still_does_its_work() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    if !open_sample(&mut h, "dog.jpg") {
        return;
    }
    h.app.push(Action::ResizeImage {
        width: 300,
        height: 450,
        filter: cshop_core::resample::Resampling::Bilinear,
    });
    h.settle(2);
    let before = h.app.doc().unwrap().doc.tree.len();

    h.app.push(Action::ShowSeparate);
    for _ in 0..600 {
        if matches!(&h.app.dialog, Dialog::Separate(d) if !d.busy) {
            break;
        }
        h.settle(1);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let ticked = match &h.app.dialog {
        Dialog::Separate(d) => d.picked().len(),
        _ => return,
    };
    if ticked == 0 {
        return;
    }

    // Exactly what the frame does when the Separate button is pressed: it
    // hands back the window, the action, and "close me".
    let dialog = std::mem::replace(&mut h.app.dialog, Dialog::None);
    h.app.finish_dialog_frame(dialog, true, vec![Action::RunSeparate]);
    h.settle(3);

    assert!(!h.app.dialog.is_open(), "the window should have closed");
    assert_eq!(
        h.app.doc().unwrap().doc.tree.len(),
        before + ticked,
        "and the layers should be there: closing must not cancel the work"
    );
}
