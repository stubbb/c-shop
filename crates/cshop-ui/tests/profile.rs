//! The colour-profile window, driven through the real interface.

use cshop_core::color::Rgba8;
use cshop_core::document::{Background, Document};
use cshop_ui::commands::Action;
use cshop_ui::dialogs::Dialog;
use cshop_ui::input_harness::Harness;

const WIDE: &str = "/usr/share/color/icc/colord/WideGamutRGB.icc";

fn open(h: &mut Harness) {
    h.app.open_document(Document::new("t", 32, 32, Background::Color(Rgba8::new(200, 60, 60, 255))));
    h.settle(2);
}

#[test]
fn the_window_says_what_the_document_is_in() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);
    h.app.push(Action::ShowColorProfile);
    h.settle(1);
    let Dialog::ColorProfile(d) = &h.app.dialog else { panic!("it should be open") };
    assert!(d.current.contains("sRGB"), "a new document is in sRGB: {}", d.current);
    assert!(d.chosen.is_none(), "and sRGB is what is selected");
}

#[test]
fn converting_through_the_window_changes_the_pixels_and_undoes() {
    if !std::path::Path::new(WIDE).exists() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    h.app.push(Action::SetColorProfile { path: Some(WIDE.into()), convert: true });
    h.settle(2);
    let doc = &h.app.doc().unwrap().doc;
    assert!(!doc.profile.is_srgb(), "the working space should have moved");
    assert_ne!(
        doc.tree.get(id).unwrap().pixels().unwrap(),
        &before,
        "and converting should have rewritten the pixels"
    );

    h.app.push(Action::Undo);
    h.settle(2);
    let doc = &h.app.doc().unwrap().doc;
    assert!(doc.profile.is_srgb());
    assert_eq!(doc.tree.get(id).unwrap().pixels().unwrap(), &before);
}

#[test]
fn assigning_through_the_window_leaves_the_pixels_alone() {
    if !std::path::Path::new(WIDE).exists() {
        return;
    }
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);
    let id = h.app.doc().unwrap().doc.active.unwrap();
    let before = h.app.doc().unwrap().doc.tree.get(id).unwrap().pixels().unwrap().clone();

    h.app.push(Action::SetColorProfile { path: Some(WIDE.into()), convert: false });
    h.settle(2);
    let doc = &h.app.doc().unwrap().doc;
    assert!(!doc.profile.is_srgb());
    assert_eq!(doc.tree.get(id).unwrap().pixels().unwrap(), &before);
}

/// A path that is not a profile has to say so rather than take effect.
#[test]
fn a_profile_that_will_not_read_is_reported() {
    let Some(mut h) = Harness::new((1400, 820)) else { return };
    open(&mut h);
    h.app.push(Action::SetColorProfile {
        path: Some("/nowhere/at/all.icc".into()),
        convert: true,
    });
    h.settle(2);
    assert!(h.app.doc().unwrap().doc.profile.is_srgb(), "nothing should have changed");
}
