//! The Relight window, driven through the real interface.

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

fn wait_for_depth(h: &mut Harness) {
    for _ in 0..900 {
        if matches!(&h.app.dialog, Dialog::Relight(d) if !d.busy) {
            return;
        }
        h.settle(1);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn the_window_says_when_the_pack_is_missing() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    h.app.open_document(Document::new("t", 64, 64, Background::Color(Rgba8::WHITE)));
    h.settle(2);
    h.app.push(Action::ShowRelight);
    h.settle(1);
    let Dialog::Relight(d) = &h.app.dialog else { panic!("the window should be open") };
    if !cshop_ui::vision::is_available() {
        assert!(d.unavailable);
        assert!(d.status.contains("setup.sh") || d.status.contains("not installed"));
    }
}

/// The depth is worked out once, and after that moving the lamp is arithmetic
/// — which is the whole reason the window is shaped this way.
#[test]
fn the_depth_is_worked_out_once_and_the_lamp_moves_freely() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    if !open_sample(&mut h, "dog.jpg") {
        return;
    }
    h.app.push(Action::ResizeImage {
        width: 240,
        height: 360,
        filter: cshop_core::resample::Resampling::Bilinear,
    });
    h.settle(2);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    h.app.push(Action::ShowRelight);
    h.settle(1);
    let Dialog::Relight(d) = &h.app.dialog else { panic!("open") };
    assert!(d.busy, "it should say it is working the shape out");
    wait_for_depth(&mut h);

    let Dialog::Relight(d) = &h.app.dialog else { panic!("still open") };
    assert!(d.ready(), "the depth should be there: {}", d.status);
    assert!(d.showing, "and a lighting should already be on the canvas");

    // Lighting from opposite sides has to give different pictures, without
    // the model being asked anything again.
    if let Dialog::Relight(d) = &mut h.app.dialog {
        d.lamp.azimuth = 0.0;
        d.lamp.intensity = 1.2;
        d.lamp.ambient = 0.5;
    }
    h.app.push(Action::RelightPreview);
    h.settle(2);
    let left = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    if let Dialog::Relight(d) = &mut h.app.dialog {
        d.lamp.azimuth = 180.0;
    }
    h.app.push(Action::RelightPreview);
    h.settle(2);
    let right = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    assert_ne!(left.pixels(), right.pixels(), "the side the light comes from should matter");
    assert_ne!(left.pixels(), before.pixels());

    let dialog = std::mem::replace(&mut h.app.dialog, Dialog::None);
    h.app.finish_dialog_frame(dialog, true, vec![Action::RelightKeep]);
    h.settle(2);

    // A single undo has to land on the picture as it was, not on one of the
    // lightings tried along the way — which is the check that the previews
    // went onto the canvas and not into the history.
    h.app.push(Action::Undo);
    h.settle(2);
    assert_eq!(
        h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap(),
        &before,
        "one undo should put the original light back"
    );
}

/// Cancelling has to undo the preview, which was written straight into the
/// layer with no history entry to fall back on.
#[test]
fn cancelling_puts_the_original_light_back() {
    if !cshop_ui::vision::is_available() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    if !open_sample(&mut h, "dog.jpg") {
        return;
    }
    h.app.push(Action::ResizeImage {
        width: 200,
        height: 300,
        filter: cshop_core::resample::Resampling::Bilinear,
    });
    h.settle(2);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    h.app.push(Action::ShowRelight);
    wait_for_depth(&mut h);
    if let Dialog::Relight(d) = &mut h.app.dialog {
        d.lamp.intensity = 1.5;
        d.lamp.ambient = 0.3;
    }
    h.app.push(Action::RelightPreview);
    h.settle(2);
    assert_ne!(h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap(), &before);

    let dialog = std::mem::replace(&mut h.app.dialog, Dialog::None);
    h.app.finish_dialog_frame(dialog, true, vec![Action::RelightCancel]);
    h.settle(2);
    assert_eq!(
        h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap(),
        &before,
        "cancel should leave no trace"
    );
}
